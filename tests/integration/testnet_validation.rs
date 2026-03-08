//! Phase 9.1 — Testnet smoke test.
//!
//! This test runs the full ingestion + detection pipeline against Arbitrum
//! Sepolia and asserts that the top-of-funnel metrics increment within 2
//! minutes, confirming the sequencer feed is live and the opportunity detector
//! is operating correctly.
//!
//! # Running
//!
//! ```bash
//! # Ensure .env is populated with Sepolia credentials first.
//! cargo test testnet_full_pipeline_smoke_test -- --ignored --nocapture
//! ```
//!
//! # What is validated
//! - Feed connects and stays connected for 2 minutes without panicking
//! - `opportunities_detected` > 0 (feed is alive, swaps are arriving)
//! - `opportunities_cleared_threshold` >= 0 (profit math is running, not panicking)
//! - No supervised task exits unexpectedly
//! - PnL budget is not exhausted (no on-chain submissions happen in this test)
//!
//! # What is NOT validated
//! - Simulation correctness (requires fork RPC, Phase 5 regression suite)
//! - On-chain execution (Phase 10)
//! - Profitable arb (no real liquidity on Sepolia)

#![allow(dead_code)]

use std::sync::{Arc, RwLock};
use std::time::Duration;

use alloy::{
    network::Ethereum,
    providers::{ProviderBuilder, RootProvider},
};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::info;

use arbx_common::{config::Config, metrics::Metrics, pnl::PnlTracker};
use arbx_detector::{
    opportunity::PathScanner,
    profit::{AlloyGasFetcher, ProfitCalculator},
};
use arbx_ingestion::{
    pool_state::PoolStateStore,
    sequencer_feed::{DetectedSwap, FeedConfig, SequencerFeedManager},
};

/// Full pipeline smoke test against Arbitrum Sepolia.
///
/// Tagged `#[ignore]` so it never runs in CI.  Run manually once the contract
/// is deployed and `.env` is populated with Sepolia credentials.
///
/// Definition of done (Phase 9.1):
///   - Runs for 2 minutes without any task panicking
///   - `opportunities_detected` > 0 within 2 minutes
///   - `opportunities_cleared_threshold` is a non-negative integer
///   - PnL budget is not exhausted (bot spent nothing)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ARBITRUM_SEPOLIA_RPC_URL, ARB_EXECUTOR_ADDRESS and PRIVATE_KEY set in .env"]
async fn testnet_full_pipeline_smoke_test() {
    // ── Load .env if present (best-effort) ────────────────────────────────
    if std::path::Path::new(".env").exists() {
        for line in std::fs::read_to_string(".env").unwrap_or_default().lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim_matches('"').trim_matches('\'');
                // Only set if not already in the environment.
                if std::env::var(k).is_err() {
                    std::env::set_var(k, v);
                }
            }
        }
    }

    // ── Load Sepolia config ───────────────────────────────────────────────
    let config = Config::load("config/sepolia.toml")
        .expect("config/sepolia.toml must exist and env vars must be set");

    arbx_common::tracing_init::init_tracing(&config.observability.log_level);

    info!(
        chain_id = config.network.chain_id,
        feed_url = %config.network.sequencer_feed_url,
        "testnet_full_pipeline_smoke_test starting"
    );

    // ── Build alloy HTTP provider ─────────────────────────────────────────
    let provider: Arc<RootProvider<Ethereum>> = Arc::new(
        ProviderBuilder::default()
            .connect_http(config.network.rpc_url.parse().expect("invalid rpc_url")),
    );

    // ── Bootstrap shared state ────────────────────────────────────────────
    let pool_store = PoolStateStore::new();
    let metrics = Arc::new(Metrics::new().expect("Metrics::new must not fail"));

    // Testnet PnL tracker — use a temp file so we don't pollute mainnet state.
    let pnl_path = format!(
        "/tmp/arbx_testnet_pnl_{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    let pnl =
        Arc::new(PnlTracker::new(pnl_path.clone(), 0.0).expect("PnlTracker::new must not fail"));

    // ── Channels ──────────────────────────────────────────────────────────
    let (swap_tx, mut swap_rx) = mpsc::channel::<DetectedSwap>(1_024);

    // ── Sequencer feed task ───────────────────────────────────────────────
    let feed_config = FeedConfig {
        feed_url: config.network.sequencer_feed_url.clone(),
        reconnect_base_ms: 1_000,
        reconnect_max_ms: 32_000,
        reconnect_multiplier: 2.0,
    };
    let feed_mgr = SequencerFeedManager::new(feed_config, pool_store.clone(), swap_tx.clone());

    let feed_handle = tokio::spawn(async move { feed_mgr.run().await });

    // ── Detection task ────────────────────────────────────────────────────
    let eth_price_usd: Arc<RwLock<f64>> = Arc::new(RwLock::new(3_000.0));
    let node_interface: alloy::primitives::Address = config
        .execution
        .node_interface_address
        .parse()
        .expect("invalid node_interface_address");

    let gas_fetcher = AlloyGasFetcher::new(
        Arc::clone(&provider),
        node_interface,
        Arc::clone(&eth_price_usd),
    );
    let profit_calc = Arc::new(ProfitCalculator::new(gas_fetcher, config.strategy.clone()));
    let path_scanner = Arc::new(PathScanner::new(pool_store.clone()));
    let metrics_det = Arc::clone(&metrics);

    let detection_handle = tokio::spawn(async move {
        use arbx_simulator::revm_sim::CallDataEncoder;

        while let Some(swap) = swap_rx.recv().await {
            let paths = path_scanner.scan(swap.pool_address);
            if paths.is_empty() {
                continue;
            }

            metrics_det
                .opportunities_detected
                .inc_by(paths.len() as u64);

            for path in &paths {
                let calldata = CallDataEncoder::encode_execute_arb(path, path.estimated_profit_wei);
                match profit_calc.filter(path, calldata).await {
                    Ok(Some(_opp)) => {
                        metrics_det.opportunities_cleared_threshold.inc();
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "profit_calc.filter error");
                    }
                }
            }
        }
    });

    // ── Run for 2 minutes ─────────────────────────────────────────────────
    info!("smoke test running for 2 minutes...");
    sleep(Duration::from_secs(120)).await;
    info!("2 minutes elapsed — asserting funnel metrics");

    // ── Assertions ────────────────────────────────────────────────────────

    // Feed and detection tasks must still be alive.
    assert!(
        !feed_handle.is_finished(),
        "sequencer feed task exited unexpectedly — check ARBITRUM_SEPOLIA_RPC_URL"
    );
    assert!(
        !detection_handle.is_finished(),
        "detection task exited unexpectedly"
    );

    let detected = metrics.opportunities_detected.get();
    let cleared = metrics.opportunities_cleared_threshold.get();

    info!(detected, cleared, "funnel metrics after 2 min");

    assert!(
        detected > 0,
        "opportunities_detected should be > 0 after 2 minutes on Arbitrum Sepolia. \
         Got 0 — check that the sequencer feed is streaming swaps. \
         Feed URL: {}",
        config.network.sequencer_feed_url
    );

    // cleared_threshold may be 0 on testnet (no real liquidity) — that's fine.
    // Just assert the counter is a valid value (not somehow negative via overflow).
    // IntCounter is always non-negative by type, so this is a smoke check.
    let _ = cleared; // acknowledged; no assertion needed

    // PnL budget must not be exhausted — this test never submits transactions.
    let exhausted = pnl.is_budget_exhausted();
    assert!(
        !exhausted,
        "PnL budget should not be exhausted in a read-only smoke test"
    );

    info!("testnet_full_pipeline_smoke_test PASSED");

    // Clean up temp PnL file.
    let _ = std::fs::remove_file(&pnl_path);
}
