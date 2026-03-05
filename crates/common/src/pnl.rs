//! Mini-Phase 6.2 — PnL Tracker with Persistence Tests.
//!
//! Tracks the bot's running profit-and-loss, cumulative gas spend, and the
//! $60 operating budget across restarts. All state is persisted to a JSON
//! file via an atomic write (write to `{path}.tmp` → rename) so the file is
//! never left in a partially-written state.

use std::{
    io::Write as _,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy::primitives::{I256, U256};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::types::SubmissionResult;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Convert [`I256`] to `f64` for Prometheus / USD calculations.
///
/// Uses `into_sign_and_abs()` to avoid overflow; precision loss is acceptable
/// for values in the range that arise from ETH gas / profit amounts.
fn i256_to_f64(v: I256) -> f64 {
    let (sign, abs) = v.into_sign_and_abs();
    let abs_f64 = abs.to::<u128>() as f64;
    if sign.is_negative() {
        -abs_f64
    } else {
        abs_f64
    }
}

// ─── PnlState ────────────────────────────────────────────────────────────────

/// Serialisable snapshot of the bot's cumulative PnL.
///
/// `total_gas_spent_wei` and `total_profit_wei` are stored as decimal strings
/// so that the full `U256`/`I256` range survives a JSON round-trip without
/// precision loss.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PnlState {
    /// Sum of all gas paid (L2 + L1), U256 as decimal string.
    pub total_gas_spent_wei: String,
    pub total_gas_spent_usd: f64,
    /// Cumulative net PnL across all submissions (can be negative), I256 as decimal string.
    pub total_profit_wei: String,
    pub total_profit_usd: f64,
    /// Equal to `total_profit_usd` — the signed net outcome after gas.
    pub net_pnl_usd: f64,
    pub successful_arbs: u64,
    pub reverted_arbs: u64,
    pub total_submissions: u64,
    /// Remaining operating budget in USD; decreases by gas cost per submission.
    pub budget_remaining_usd: f64,
    pub session_start_ms: u64,
    pub last_updated_ms: u64,
}

// ─── PnlTracker ──────────────────────────────────────────────────────────────

/// Thread-safe PnL tracker with atomic JSON persistence.
pub struct PnlTracker {
    state: Arc<Mutex<PnlState>>,
    file_path: String,
    initial_budget_usd: f64,
}

impl PnlTracker {
    /// Create a new tracker.
    ///
    /// If `file_path` already exists, the previous session state is loaded
    /// from it. Otherwise a fresh state is initialised with `budget_usd` as
    /// the starting budget.
    pub fn new(file_path: String, budget_usd: f64) -> anyhow::Result<Self> {
        let state = if std::path::Path::new(&file_path).exists() {
            let json = std::fs::read_to_string(&file_path)
                .with_context(|| format!("read PnL state from {file_path}"))?;
            serde_json::from_str::<PnlState>(&json).context("deserialise PnlState")?
        } else {
            let ms = now_ms();
            PnlState {
                total_gas_spent_wei: U256::ZERO.to_string(),
                total_profit_wei: I256::ZERO.to_string(),
                budget_remaining_usd: budget_usd,
                session_start_ms: ms,
                last_updated_ms: ms,
                ..Default::default()
            }
        };
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            file_path,
            initial_budget_usd: budget_usd,
        })
    }

    /// Record the outcome of one on-chain submission.
    ///
    /// Updates all counters, recomputes `net_pnl_usd` and
    /// `budget_remaining_usd`, then atomically saves state to disk.
    pub async fn record_submission(
        &self,
        result: &SubmissionResult,
        eth_price_usd: f64,
    ) -> anyhow::Result<()> {
        // ── Gas cost in USD ───────────────────────────────────────────────
        let gas_cost_wei = result.l2_gas_cost_wei + result.l1_gas_cost_wei;
        let gas_cost_eth = gas_cost_wei.to::<u128>() as f64 / 1e18;
        let gas_cost_usd = gas_cost_eth * eth_price_usd;

        // ── Net PnL delta in USD ──────────────────────────────────────────
        let net_pnl_usd_delta = i256_to_f64(result.net_pnl_wei) / 1e18 * eth_price_usd;

        {
            let mut st = self.state.lock().expect("PnlState mutex poisoned");

            // Gas tracking
            let prev_gas: U256 = st.total_gas_spent_wei.parse().unwrap_or(U256::ZERO);
            st.total_gas_spent_wei = (prev_gas + gas_cost_wei).to_string();
            st.total_gas_spent_usd += gas_cost_usd;

            // Net PnL accumulation
            let prev_pnl: I256 = st.total_profit_wei.parse().unwrap_or(I256::ZERO);
            let new_pnl = prev_pnl + result.net_pnl_wei;
            st.total_profit_wei = new_pnl.to_string();
            st.total_profit_usd += net_pnl_usd_delta;

            // Counters
            st.total_submissions += 1;
            if result.success {
                st.successful_arbs += 1;
            } else {
                st.reverted_arbs += 1;
            }

            // Derived fields
            st.net_pnl_usd = st.total_profit_usd;
            st.budget_remaining_usd = self.initial_budget_usd - st.total_gas_spent_usd;
            st.last_updated_ms = now_ms();
        }

        info!(
            "PnL updated: submissions={} budget_remaining=${:.4}",
            self.state.lock().unwrap().total_submissions,
            self.state.lock().unwrap().budget_remaining_usd,
        );

        self.save().await
    }

    /// Returns `true` when `budget_remaining_usd` has fallen to $0.10 or below.
    ///
    /// A $0.10 safety margin is kept so the bot always has enough gas to
    /// cleanly shut down.  The threshold is `< 0.101` rather than `<= 0.10`
    /// to absorb the floating-point imprecision from wei-to-USD conversions
    /// (e.g. `60.0 - 59.9 ≈ 0.10000000000000142` in IEEE 754).
    pub fn is_budget_exhausted(&self) -> bool {
        self.state
            .lock()
            .expect("PnlState mutex poisoned")
            .budget_remaining_usd
            < 0.101
    }

    /// Returns a human-readable one-line summary for logging.
    pub fn summary(&self) -> String {
        let st = self.state.lock().expect("PnlState mutex poisoned");
        format!(
            "PnL Summary | submissions={} ok={} rev={} | \
             net_pnl={:.6} USD | budget_remaining={:.4} USD | \
             gas_spent={:.6} USD",
            st.total_submissions,
            st.successful_arbs,
            st.reverted_arbs,
            st.net_pnl_usd,
            st.budget_remaining_usd,
            st.total_gas_spent_usd,
        )
    }

    /// Returns a clone of the current [`PnlState`].
    pub fn state_snapshot(&self) -> PnlState {
        self.state.lock().expect("PnlState mutex poisoned").clone()
    }

    /// Write [`PnlState`] as pretty-printed JSON to `file_path` atomically.
    ///
    /// Implementation: serialise to `{file_path}.tmp`, `sync_all`, then
    /// `std::fs::rename` to the target. On any failure before the rename,
    /// the original file is untouched.
    pub async fn save(&self) -> anyhow::Result<()> {
        let json = {
            let st = self.state.lock().expect("PnlState mutex poisoned");
            serde_json::to_string_pretty(&*st).context("serialise PnlState")?
        };
        let file_path = self.file_path.clone();

        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let target = std::path::Path::new(&file_path);
            let tmp_str = format!("{file_path}.tmp");
            let tmp = std::path::Path::new(&tmp_str);

            // Ensure parent directory exists.
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create parent dir for {file_path}"))?;
                }
            }

            // Write to the .tmp file.
            {
                let mut f = std::fs::File::create(tmp).context("create .tmp for atomic write")?;
                f.write_all(json.as_bytes()).context("write JSON to .tmp")?;
                f.sync_all().context("sync .tmp to disk")?;
            }

            // Atomic rename: if this succeeds, the target always has complete data.
            std::fs::rename(tmp, target).context("atomic rename .tmp → target")?;

            Ok(())
        })
        .await
        .context("spawn_blocking panicked")??;

        debug!("PnL state saved to {}", self.file_path);
        Ok(())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use alloy::primitives::{TxHash, I256, U256};

    use crate::types::SubmissionResult;

    use super::{PnlState, PnlTracker};

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a successful [`SubmissionResult`] with the given gross profit and gas cost (in wei).
    fn make_success(gross_profit_wei: u128, gas_wei: u128) -> SubmissionResult {
        let gas = U256::from(gas_wei);
        let gross = I256::from_raw(U256::from(gross_profit_wei));
        let cost = I256::from_raw(gas);
        SubmissionResult {
            tx_hash: TxHash::ZERO,
            success: true,
            revert_reason: None,
            gas_used: 21_000,
            l2_gas_cost_wei: gas,
            l1_gas_cost_wei: U256::ZERO,
            net_pnl_wei: gross - cost,
        }
    }

    /// Build a reverted [`SubmissionResult`] that lost `gas_wei` wei to gas.
    fn make_revert(gas_wei: u128) -> SubmissionResult {
        let gas = U256::from(gas_wei);
        SubmissionResult {
            tx_hash: TxHash::ZERO,
            success: false,
            revert_reason: Some("reverted".to_string()),
            gas_used: 21_000,
            l2_gas_cost_wei: gas,
            l1_gas_cost_wei: U256::ZERO,
            net_pnl_wei: -I256::from_raw(gas),
        }
    }

    fn temp_path() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pnl.json").to_str().unwrap().to_string();
        (dir, path)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_new_creates_fresh_state() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path, 60.0).unwrap();
        let st = tracker.state_snapshot();

        assert_eq!(st.total_submissions, 0);
        assert_eq!(st.successful_arbs, 0);
        assert_eq!(st.reverted_arbs, 0);
        assert!((st.budget_remaining_usd - 60.0).abs() < 1e-9);
        assert_eq!(st.total_gas_spent_wei, "0");
    }

    #[tokio::test]
    async fn test_new_loads_existing_state() {
        let (_dir, path) = temp_path();

        // Write a known state JSON to disk manually.
        let existing = PnlState {
            total_gas_spent_wei: "1000000000000000".to_string(),
            total_gas_spent_usd: 3.0,
            total_profit_wei: "5000000000000000".to_string(),
            total_profit_usd: 15.0,
            net_pnl_usd: 12.0,
            successful_arbs: 5,
            reverted_arbs: 2,
            total_submissions: 7,
            budget_remaining_usd: 57.0,
            session_start_ms: 1_000,
            last_updated_ms: 2_000,
        };
        std::fs::write(&path, serde_json::to_string(&existing).unwrap()).unwrap();

        let tracker = PnlTracker::new(path, 60.0).unwrap();
        let st = tracker.state_snapshot();

        assert_eq!(st.total_submissions, 7);
        assert_eq!(st.successful_arbs, 5);
        assert_eq!(st.reverted_arbs, 2);
        assert!((st.budget_remaining_usd - 57.0).abs() < 1e-9);
        assert_eq!(st.total_gas_spent_wei, "1000000000000000");
    }

    #[tokio::test]
    async fn test_record_successful_arb() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path, 60.0).unwrap();

        // gross = 0.01 ETH, gas = 0.001 ETH → net_pnl_wei = 0.009 ETH
        let r = make_success(
            10_000_000_000_000_000, // 0.01 ETH gross
            1_000_000_000_000_000,  // 0.001 ETH gas
        );
        tracker.record_submission(&r, 3_000.0).await.unwrap();

        let st = tracker.state_snapshot();
        assert_eq!(st.successful_arbs, 1);
        assert_eq!(st.total_submissions, 1);
        assert!(
            st.net_pnl_usd > 0.0,
            "expected positive PnL after profitable arb"
        );
        // Gas cost = 0.001 ETH * $3000 = $3.00; budget should drop by $3
        assert!((st.budget_remaining_usd - 57.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_record_reverted_arb() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path.clone(), 60.0).unwrap();

        // Gas = 21_000 * 1e9 (gwei) = 21_000_000_000_000 wei
        let gas_wei: u128 = 21_000_000_000_000;
        let r = make_revert(gas_wei);
        tracker.record_submission(&r, 3_000.0).await.unwrap();

        let st = tracker.state_snapshot();
        assert_eq!(st.reverted_arbs, 1);
        assert_eq!(st.total_submissions, 1);
        assert!(st.net_pnl_usd < 0.0, "expected negative PnL after revert");

        let gas_usd = gas_wei as f64 / 1e18 * 3_000.0;
        assert!(
            (st.budget_remaining_usd - (60.0 - gas_usd)).abs() < 1e-6,
            "budget should decrease by gas cost"
        );
    }

    #[tokio::test]
    async fn test_budget_not_exhausted() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path, 60.0).unwrap();

        // Spend $1 in gas (eth_price=$1 → gas = 1 ETH worth)
        let one_eth_wei: u128 = 1_000_000_000_000_000_000;
        tracker
            .record_submission(&make_revert(one_eth_wei), 1.0)
            .await
            .unwrap();

        assert!(
            !tracker.is_budget_exhausted(),
            "should not be exhausted after spending $1"
        );
    }

    #[tokio::test]
    async fn test_budget_exhausted_at_limit() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path, 60.0).unwrap();

        // Spend $59.95 in gas: with eth_price=$1, gas = 59.95 ETH = 59_950_000_000_000_000_000 wei
        let gas_wei: u128 = 59_950_000_000_000_000_000;
        tracker
            .record_submission(&make_revert(gas_wei), 1.0)
            .await
            .unwrap();

        // budget_remaining ≈ 60.0 - 59.95 = 0.05 ≤ 0.10 → exhausted
        assert!(
            tracker.is_budget_exhausted(),
            "should be exhausted when remaining ≤ $0.10"
        );
    }

    #[tokio::test]
    async fn test_budget_safety_margin() {
        // At exactly $0.10 remaining → exhausted.
        {
            let (_dir, path) = temp_path();
            let tracker = PnlTracker::new(path, 60.0).unwrap();
            // Spend $59.90 → remaining ≈ $0.10
            let gas_wei: u128 = 59_900_000_000_000_000_000;
            tracker
                .record_submission(&make_revert(gas_wei), 1.0)
                .await
                .unwrap();
            assert!(
                tracker.is_budget_exhausted(),
                "should be exhausted at $0.10 remaining (remaining={:.6})",
                tracker.state_snapshot().budget_remaining_usd,
            );
        }

        // At $0.11 remaining → not exhausted.
        {
            let (_dir, path) = temp_path();
            let tracker = PnlTracker::new(path, 60.0).unwrap();
            // Spend $59.89 → remaining ≈ $0.11
            let gas_wei: u128 = 59_890_000_000_000_000_000;
            tracker
                .record_submission(&make_revert(gas_wei), 1.0)
                .await
                .unwrap();
            assert!(
                !tracker.is_budget_exhausted(),
                "should NOT be exhausted at $0.11 remaining (remaining={:.6})",
                tracker.state_snapshot().budget_remaining_usd,
            );
        }
    }

    #[tokio::test]
    async fn test_persistence_survives_restart() {
        let (_dir, path) = temp_path();

        // Phase 1: create, record 3 submissions, drop.
        let snap_before = {
            let tracker = PnlTracker::new(path.clone(), 60.0).unwrap();
            for _ in 0..3 {
                tracker
                    .record_submission(
                        &make_success(1_000_000_000_000_000, 100_000_000_000_000),
                        3_000.0,
                    )
                    .await
                    .unwrap();
            }
            tracker.state_snapshot()
        }; // tracker dropped here

        // Phase 2: reload from the same path.
        let tracker2 = PnlTracker::new(path, 60.0).unwrap();
        let snap_after = tracker2.state_snapshot();

        assert_eq!(snap_after.total_submissions, snap_before.total_submissions);
        assert_eq!(snap_after.successful_arbs, snap_before.successful_arbs);
        assert_eq!(snap_after.reverted_arbs, snap_before.reverted_arbs);
        assert_eq!(
            snap_after.total_gas_spent_wei,
            snap_before.total_gas_spent_wei
        );
        assert!((snap_after.budget_remaining_usd - snap_before.budget_remaining_usd).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_atomic_write() {
        let (_dir, path) = temp_path();
        let tmp_path = format!("{path}.tmp");

        // Pre-create a "corrupted" leftover .tmp from a hypothetical previous crash.
        std::fs::write(&tmp_path, b"CORRUPTED_PARTIAL_WRITE").expect("write sentinel .tmp");

        // Create tracker and save (no record_submission needed — save() is enough).
        let tracker = PnlTracker::new(path.clone(), 60.0).unwrap();
        tracker.save().await.unwrap();

        // Target file must contain valid JSON.
        let contents = std::fs::read_to_string(&path).expect("target file must exist");
        let _: PnlState = serde_json::from_str(&contents)
            .expect("target file must contain valid PnlState JSON after atomic write");

        // .tmp file must not linger — it was renamed to target.
        assert!(
            !std::path::Path::new(&tmp_path).exists(),
            ".tmp file should be gone after successful atomic write"
        );
    }

    #[tokio::test]
    async fn test_summary_contains_key_fields() {
        let (_dir, path) = temp_path();
        let tracker = PnlTracker::new(path, 60.0).unwrap();
        tracker
            .record_submission(
                &make_success(5_000_000_000_000_000, 500_000_000_000_000),
                2_000.0,
            )
            .await
            .unwrap();

        let summary = tracker.summary();
        assert!(summary.contains("PnL"), "summary must mention 'PnL'");
        assert!(
            summary.contains('1'),
            "summary must include submission count (1)"
        );
        // Check budget_remaining appears (60 minus a small amount → starts with "5")
        assert!(
            summary.contains("budget_remaining") || summary.to_lowercase().contains("budget"),
            "summary must mention budget remaining"
        );
    }
}
