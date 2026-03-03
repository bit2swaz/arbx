//! Prometheus metrics for the arbx observability funnel.
//!
//! All eight SSOT funnel metrics are registered against an isolated
//! [`Registry`] (not the default global one) so test instances are
//! completely independent.

use anyhow::Context;
use prometheus::{Counter, Encoder, Gauge, IntCounter, IntCounterVec, Opts, Registry, TextEncoder};

// ── Metrics struct ───────────────────────────────────────────────────────────

/// The eight SSOT observability funnel metrics.
pub struct Metrics {
    registry: Registry,

    /// Total arb opportunities detected from the sequencer feed.
    pub opportunities_detected: IntCounter,
    /// Opportunities that cleared the dynamic minimum profit threshold.
    pub opportunities_cleared_threshold: IntCounter,
    /// Opportunities that passed revm full simulation.
    pub opportunities_cleared_simulation: IntCounter,
    /// Arb transactions submitted to the sequencer.
    pub transactions_submitted: IntCounter,
    /// On-chain arb transactions that succeeded.
    pub transactions_succeeded: IntCounter,
    /// On-chain arb transactions that reverted, labelled by `reason`.
    pub transactions_reverted: IntCounterVec,
    /// Running net PnL in wei; goes negative when gas exceeds profit.
    pub net_pnl_wei: Gauge,
    /// Total gas spent in wei (monotonically increasing).
    pub gas_spent_wei: Counter,
}

impl Metrics {
    /// Create all eight metrics and register them with a fresh [`Registry`].
    ///
    /// Uses `Registry::new()` — **not** the default global registry — so
    /// multiple `Metrics` instances in tests have no shared state.
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();

        // Helper: create and register a metric in one step.
        macro_rules! reg {
            ($expr:expr) => {{
                let m = $expr.context("metric creation failed")?;
                registry
                    .register(Box::new(m.clone()))
                    .context("metric registration failed")?;
                m
            }};
        }

        let opportunities_detected = reg!(IntCounter::new(
            "opportunities_detected",
            "Total arb opportunities detected from the sequencer feed",
        ));
        let opportunities_cleared_threshold = reg!(IntCounter::new(
            "opportunities_cleared_threshold",
            "Opportunities that cleared the minimum profit threshold",
        ));
        let opportunities_cleared_simulation = reg!(IntCounter::new(
            "opportunities_cleared_simulation",
            "Opportunities that passed revm simulation",
        ));
        let transactions_submitted = reg!(IntCounter::new(
            "transactions_submitted",
            "Arb transactions submitted to the sequencer",
        ));
        let transactions_succeeded = reg!(IntCounter::new(
            "transactions_succeeded",
            "On-chain arb transactions that succeeded",
        ));
        let transactions_reverted = reg!(IntCounterVec::new(
            Opts::new(
                "transactions_reverted",
                "On-chain arb transactions that reverted, labelled by revert reason",
            ),
            &["reason"],
        ));
        let net_pnl_wei = reg!(Gauge::new(
            "net_pnl_wei",
            "Running net PnL in wei (negative when gas exceeds profit)",
        ));
        let gas_spent_wei = reg!(Counter::new(
            "gas_spent_wei",
            "Total gas spent in wei across all arb submissions",
        ));

        Ok(Self {
            registry,
            opportunities_detected,
            opportunities_cleared_threshold,
            opportunities_cleared_simulation,
            transactions_submitted,
            transactions_succeeded,
            transactions_reverted,
            net_pnl_wei,
            gas_spent_wei,
        })
    }

    /// Return a reference to the underlying [`Registry`].
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Render all registered metrics in Prometheus text format.
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        encoder
            .encode(&self.registry.gather(), &mut buf)
            .expect("prometheus text encoding is infallible");
        String::from_utf8(buf).expect("prometheus output is always valid UTF-8")
    }

    /// Start a minimal HTTP server on `0.0.0.0:port`.
    ///
    /// - `GET /metrics` → `200 OK` with the Prometheus text output.
    /// - Any other path → `404 Not Found`.
    ///
    /// Runs forever; call with `tokio::spawn`.
    pub async fn start_server(registry: Registry, port: u16) -> anyhow::Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
            .await
            .with_context(|| format!("failed to bind metrics server on port {port}"))?;

        loop {
            let (mut stream, _) = listener.accept().await?;
            let registry = registry.clone();

            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");

                if first_line.starts_with("GET /metrics") {
                    let encoder = TextEncoder::new();
                    let mut body_bytes = Vec::new();
                    let _ = encoder.encode(&registry.gather(), &mut body_bytes);
                    let body = String::from_utf8_lossy(&body_bytes).into_owned();
                    let response = format!(
                        "HTTP/1.0 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
                // stream is dropped here → TCP FIN sent → client read_to_string returns
            });
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::Metrics;

    // ── Registration ────────────────────────────────────────────────────────

    #[test]
    fn test_metrics_new_registers_all() {
        let m = Metrics::new().expect("Metrics::new must not fail");
        // IntCounterVec only appears in gather() after at least one label combination
        // has been observed.  Observe once with a sentinel value.
        m.transactions_reverted.with_label_values(&["_check"]).inc();
        let rendered = m.render();
        for name in &[
            "opportunities_detected",
            "opportunities_cleared_threshold",
            "opportunities_cleared_simulation",
            "transactions_submitted",
            "transactions_succeeded",
            "transactions_reverted",
            "net_pnl_wei",
            "gas_spent_wei",
        ] {
            assert!(
                rendered.contains(name),
                "render() missing metric '{name}'\n{rendered}"
            );
        }
    }

    // ── Increment / value ───────────────────────────────────────────────────

    #[test]
    fn test_counter_increments() {
        let m = Metrics::new().unwrap();
        m.opportunities_detected.inc();
        m.opportunities_detected.inc();
        m.opportunities_detected.inc();
        let rendered = m.render();
        // Match the metric line (not a comment) that ends with the value 3.
        let has_three = rendered
            .lines()
            .filter(|l| !l.starts_with('#'))
            .any(|l| l.starts_with("opportunities_detected") && l.ends_with(" 3"));
        assert!(has_three, "Expected opportunities_detected 3\n{rendered}");
    }

    #[test]
    fn test_revert_counter_with_label() {
        let m = Metrics::new().unwrap();
        m.transactions_reverted
            .with_label_values(&["No profit"])
            .inc();
        m.transactions_reverted
            .with_label_values(&["No profit"])
            .inc();
        let rendered = m.render();
        let has_two = rendered.lines().filter(|l| !l.starts_with('#')).any(|l| {
            l.contains("transactions_reverted")
                && l.contains(r#"reason="No profit""#)
                && l.ends_with(" 2")
        });
        assert!(
            has_two,
            "Expected transactions_reverted{{reason=\"No profit\"}} 2\n{rendered}"
        );
    }

    #[test]
    fn test_gauge_set_and_read() {
        let m = Metrics::new().unwrap();
        m.net_pnl_wei.set(1_000_000.0);
        let rendered = m.render();
        let present = rendered
            .lines()
            .filter(|l| !l.starts_with('#'))
            .any(|l| l.starts_with("net_pnl_wei"));
        assert!(present, "net_pnl_wei should appear in render()\n{rendered}");
    }

    // ── Format ──────────────────────────────────────────────────────────────

    #[test]
    fn test_render_valid_prometheus_format() {
        let m = Metrics::new().unwrap();
        // Observe transactions_reverted so it appears in gather() output.
        m.transactions_reverted.with_label_values(&["_check"]).inc();
        let rendered = m.render();
        assert!(
            rendered.starts_with("# HELP"),
            "render() must start with '# HELP'\n{rendered}"
        );
        for name in &[
            "opportunities_detected",
            "opportunities_cleared_threshold",
            "opportunities_cleared_simulation",
            "transactions_submitted",
            "transactions_succeeded",
            "transactions_reverted",
            "net_pnl_wei",
            "gas_spent_wei",
        ] {
            assert!(
                rendered.contains(&format!("# TYPE {name}")),
                "render() missing '# TYPE {name}'\n{rendered}"
            );
        }
    }

    // ── Isolation ───────────────────────────────────────────────────────────

    #[test]
    fn test_independent_registries() {
        let m1 = Metrics::new().unwrap();
        let m2 = Metrics::new().unwrap();

        m1.opportunities_detected.inc_by(5);

        let r1 = m1.render();
        let r2 = m2.render();

        let m1_has_five = r1
            .lines()
            .filter(|l| !l.starts_with('#'))
            .any(|l| l.starts_with("opportunities_detected") && l.ends_with(" 5"));
        assert!(m1_has_five, "m1 should have opportunities_detected 5\n{r1}");

        let m2_has_five = r2
            .lines()
            .filter(|l| !l.starts_with('#'))
            .any(|l| l.starts_with("opportunities_detected") && l.ends_with(" 5"));
        assert!(!m2_has_five, "m2 must not share state with m1\n{r2}");
    }

    // ── HTTP server ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_server_responds() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let m = Metrics::new().unwrap();
        let registry = m.registry().clone();

        tokio::spawn(Metrics::start_server(registry, 19090));

        // Give the listener time to bind.
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        let mut stream = tokio::net::TcpStream::connect("127.0.0.1:19090")
            .await
            .expect("should connect to metrics server on 19090");

        stream
            .write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        assert!(response.contains("200 OK"), "expected 200 OK\n{response}");
        assert!(
            response.contains("opportunities_detected"),
            "response body should contain metric names\n{response}"
        );
    }
}
