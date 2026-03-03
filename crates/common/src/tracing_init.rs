//! Tracing subscriber initialisation.
//!
//! [`init_tracing`] configures a global `tracing` subscriber:
//! - **Debug builds** (tests, local dev): human-readable `pretty` output.
//! - **Release builds** (production): structured `json` output.
//!
//! The function is idempotent — calling it more than once is a no-op.

use std::sync::OnceLock;

static TRACING: OnceLock<()> = OnceLock::new();

/// Initialise the global tracing subscriber with the given log level.
///
/// Safe to call multiple times; only the first call takes effect.
///
/// # Arguments
/// * `log_level` — a `tracing`-compatible filter string, e.g. `"info"`,
///   `"arbx=debug,warn"`.  Falls back to `"info"` if the string cannot be
///   parsed.
pub fn init_tracing(log_level: &str) {
    let level = log_level.to_owned();
    TRACING.get_or_init(|| {
        let filter = tracing_subscriber::EnvFilter::try_new(&level)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

        #[cfg(debug_assertions)]
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .pretty()
                .try_init();
        }

        #[cfg(not(debug_assertions))]
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .try_init();
        }
    });
}
