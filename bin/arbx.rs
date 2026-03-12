//! arbx — Arbitrum MEV arbitrage engine entry point.
//!
//! # Architecture
//! Wires all five crates into a supervised Tokio pipeline:
//! ```text
//! SequencerFeedManager  →  swap_channel  →  detection_loop
//! BlockReconciler       ─────────────────────────────────▶  opportunity_channel
//!                                                                     │
//!                                                             execution_loop
//!                                                           (revm simulate → submit)
//!                                                                     ↓
//!                                                         PnlTracker + Metrics
//! ```
//!
//! # CLI
//! ```text
//! arbx --config <path>   (required) path to TOML config file
//! arbx --dry-run         simulate but never submit on-chain
//! arbx --help
//! ```

#![allow(clippy::too_many_arguments)]

use std::sync::{Arc, RwLock};

use alloy::{
    network::Ethereum,
    primitives::Address,
    providers::{ProviderBuilder, RootProvider},
    signers::local::PrivateKeySigner,
};
use anyhow::Context as _;
use clap::Parser;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use arbx_common::{
    config::Config,
    metrics::Metrics,
    pnl::PnlTracker,
    tracing_init::init_tracing,
    types::{Opportunity, SimulationResult, SubmissionResult},
};
use arbx_detector::{
    opportunity::PathScanner,
    profit::{AlloyGasFetcher, ProfitCalculator},
};
use arbx_executor::submitter::{AlloyTransactionSender, TransactionSubmitter};
use arbx_ingestion::{
    pool_seeder,
    pool_state::PoolStateStore,
    reconciler::{AlloyReserveFetcher, BlockReconciler},
    sequencer_feed::{DetectedSwap, FeedConfig, SequencerFeedManager},
};
use arbx_simulator::revm_sim::{ArbSimulator, CallDataEncoder};

// ─── CLI ─────────────────────────────────────────────────────────────────────

/// Arbitrum atomic arbitrage engine.
#[derive(Parser, Debug)]
#[command(name = "arbx", version, about = "Arbitrum atomic arbitrage engine")]
struct Cli {
    /// Path to TOML configuration file.
    #[arg(short, long)]
    config: String,

    /// Simulate paths but never submit transactions on-chain.
    #[arg(long)]
    dry_run: bool,

    /// Run a self-test that validates the detection pipeline with synthetic
    /// data, then exit.  Exits 0 on success, 1 on failure.  Useful for
    /// smoke-testing the binary without a live sequencer feed.
    #[arg(long)]
    self_test: bool,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install the rustls crypto provider required by sequencer_client's WebSocket
    // TLS handshake.  Calling this more than once is a no-op (returns Err).
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();
    let config = Config::load(&cli.config).context("failed to load config file")?;

    init_tracing(&config.observability.log_level);

    // dry_run can be set via the CLI flag --dry-run OR via `dry_run = true` in
    // the [execution] section of the config file (e.g. anvil_fork.toml).
    let dry_run = cli.dry_run || config.execution.dry_run;
    if dry_run {
        info!("DRY RUN mode enabled — transactions will NOT be submitted on-chain");
    }

    if cli.self_test {
        return self_test(&config);
    }

    if let Err(e) = run(config, dry_run).await {
        error!(error = %e, "arbx pipeline exited with error");
        std::process::exit(1);
    }

    info!("arbx shut down cleanly");
    Ok(())
}

// ─── Self-test ────────────────────────────────────────────────────────────────

/// Validates the detection pipeline with two synthetic pools and one injected
/// swap.  Exits 0 on success, 1 on failure.
///
/// This is the canonical Phase 9.1 smoke-test path for testnets where the
/// sequencer feed carries zero DEX swap transactions.
fn self_test(config: &Config) -> anyhow::Result<()> {
    use alloy::primitives::{address, U256};
    use arbx_common::{
        metrics::Metrics,
        types::{DexKind, PoolState},
    };
    use arbx_detector::opportunity::PathScanner;
    use arbx_ingestion::pool_state::PoolStateStore;

    info!("--- self-test: validating detection pipeline ---");

    // ── 1. Synthetic tokens ───────────────────────────────────────────────
    // WETH-like and USDC-like — arbitrary but deterministic addresses.
    let token_a = address!("0000000000000000000000000000000000000001");
    let token_b = address!("0000000000000000000000000000000000000002");
    let pool_addr_1 = address!("1111111111111111111111111111111111111111");
    let pool_addr_2 = address!("2222222222222222222222222222222222222222");

    let reserve = U256::from(1_000_000_000_000u128); // 1 trillion units each side

    // Pool A: token_a / token_b  (price slightly off to create arb opportunity)
    let pool_a = PoolState {
        address: pool_addr_1,
        token0: token_a,
        token1: token_b,
        reserve0: reserve,
        reserve1: reserve * U256::from(2u32), // token_b is 2× cheaper here
        fee_tier: 3000,
        last_updated_block: 0,
        dex: DexKind::CamelotV2,
    };

    // Pool B: token_b / token_a  (token_b is 1× cheaper here — arb exists)
    let pool_b = PoolState {
        address: pool_addr_2,
        token0: token_b,
        token1: token_a,
        reserve0: reserve,
        reserve1: reserve,
        fee_tier: 3000,
        last_updated_block: 0,
        dex: DexKind::SushiSwap,
    };

    // ── 2. Seed store ─────────────────────────────────────────────────────
    let store = PoolStateStore::new();
    store.upsert(pool_a.clone());
    store.upsert(pool_b);

    info!(
        pool_a = %pool_addr_1,
        pool_b = %pool_addr_2,
        "self-test: seeded 2 synthetic pools",
    );

    // ── 3. Scan for two-hop paths ─────────────────────────────────────────
    let scanner = PathScanner::new(store);
    let paths = scanner.scan(pool_addr_1);

    if paths.is_empty() {
        error!("self-test FAIL: PathScanner found 0 two-hop paths — detection pipeline broken");
        std::process::exit(1);
    }

    info!(
        count = paths.len(),
        "self-test: PathScanner found {} two-hop path(s) ✓",
        paths.len(),
    );

    // ── 4. Increment metrics ──────────────────────────────────────────────
    let metrics = Metrics::new().context("self-test: failed to init metrics")?;
    metrics.opportunities_detected.inc_by(paths.len() as u64);

    info!(
        opportunities_detected = metrics.opportunities_detected.get(),
        "self-test: metrics.opportunities_detected incremented ✓",
    );

    // ── 5. Print summary ──────────────────────────────────────────────────
    info!("--- self-test PASSED — detection pipeline is healthy ---");
    info!("config_chain = {}", config.network.chain_id,);
    Ok(())
}

// ─── Pipeline wiring ─────────────────────────────────────────────────────────

async fn run(config: Config, dry_run: bool) -> anyhow::Result<()> {
    // ── 1. Build alloy HTTP provider ──────────────────────────────────────
    let provider: Arc<RootProvider<Ethereum>> = Arc::new(
        ProviderBuilder::default().connect_http(
            config
                .network
                .rpc_url
                .parse()
                .context("invalid [network].rpc_url")?,
        ),
    );
    info!(rpc = %config.network.rpc_url, "connected to Arbitrum RPC");

    // ── 2. Initialise Metrics and start /metrics HTTP server ──────────────
    let metrics = Arc::new(Metrics::new().context("failed to create Prometheus metrics")?);
    {
        let registry = metrics.registry().clone();
        let port = config.observability.metrics_port;
        tokio::spawn(async move {
            if let Err(e) = Metrics::start_server(registry, port).await {
                error!(error = %e, "metrics HTTP server crashed");
            }
        });
        info!(port, "Prometheus /metrics server started");
    }

    // ── 3. Bootstrap PoolStateStore ───────────────────────────────────────
    // (a) Seed from factory event logs (may yield 0 on testnets with no pools)
    let pool_store = PoolStateStore::new();
    {
        let seeded = pool_seeder::seed_pools_from_factories(
            Arc::clone(&provider),
            &pool_store,
            &config.pools.uniswap_v3_factory,
            &config.pools.camelot_factory,
            &config.pools.sushiswap_factory,
            &config.pools.traderjoe_factory,
            config.pools.seed_from_block,
        )
        .await;
        info!(seeded, "PoolStateStore bootstrapped from factory logs");
    }

    // (b) Seed any explicitly-configured known_pools (testnet / manual override)
    {
        use arbx_common::types::{DexKind, PoolState};
        let mut known_seeded = 0usize;
        for addr_str in &config.pools.known_pools {
            match addr_str.parse::<alloy::primitives::Address>() {
                Ok(addr) => {
                    pool_store.upsert(PoolState {
                        address: addr,
                        token0: alloy::primitives::Address::ZERO,
                        token1: alloy::primitives::Address::ZERO,
                        reserve0: alloy::primitives::U256::ZERO,
                        reserve1: alloy::primitives::U256::ZERO,
                        fee_tier: 300,
                        last_updated_block: 0,
                        dex: DexKind::CamelotV2,
                    });
                    known_seeded += 1;
                }
                Err(e) => {
                    tracing::warn!(address = %addr_str, error = %e, "invalid known_pool address — skipping");
                }
            }
        }
        if known_seeded > 0 {
            info!(known_seeded, "seeded known_pools from config");
        }
    }

    // ── 4. Create PnlTracker ──────────────────────────────────────────────
    let pnl_file =
        std::env::var("ARBX_PNL_FILE").unwrap_or_else(|_| "arbx_pnl_state.json".to_owned());
    let initial_budget_usd: f64 = std::env::var("ARBX_BUDGET_USD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60.0);
    let pnl = Arc::new(
        PnlTracker::new(pnl_file.clone(), initial_budget_usd)
            .context("failed to initialise PnlTracker")?,
    );
    info!(pnl_file, initial_budget_usd, "PnlTracker initialised");

    // ── 5. Channels ───────────────────────────────────────────────────────
    let (swap_tx, swap_rx) = mpsc::channel::<DetectedSwap>(1_024);
    let (opportunity_tx, opportunity_rx) = mpsc::channel::<Opportunity>(256);

    // ── 6. ETH/USD price for gas-cost calculations ─────────────────────────
    // Phase 7.1: hardcoded at $3 000.  Phase 3 adds a live CoinGecko feed.
    let eth_price_usd: Arc<RwLock<f64>> = Arc::new(RwLock::new(3_000.0));
    warn!(
        "ETH/USD price is hard-coded at $3 000 — \
         add a live price feed before Phase 3 mainnet deployment"
    );

    // ── 7. Construct all components ───────────────────────────────────────
    let node_interface: Address = config
        .execution
        .node_interface_address
        .parse()
        .context("invalid [execution].node_interface_address")?;

    let gas_fetcher = AlloyGasFetcher::new(
        Arc::clone(&provider),
        node_interface,
        Arc::clone(&eth_price_usd),
    );
    let profit_calc = Arc::new(ProfitCalculator::new(gas_fetcher, config.strategy.clone()));
    let path_scanner = Arc::new(PathScanner::new(pool_store.clone()));
    let simulator = Arc::new(ArbSimulator::new(Arc::clone(&provider)));

    let contract_address: Address = config
        .execution
        .contract_address
        .parse()
        .context("invalid [execution].contract_address")?;

    let signer: PrivateKeySigner = config
        .execution
        .private_key
        .parse()
        .context("invalid [execution].private_key")?;

    let tx_sender = AlloyTransactionSender::new(Arc::clone(&provider), signer, node_interface);
    let submitter = Arc::new(TransactionSubmitter::new(
        tx_sender,
        contract_address,
        config.execution.clone(),
        Arc::clone(&metrics),
    ));

    let semaphore = Arc::new(Semaphore::new(config.execution.max_concurrent_simulations));

    // ── 8. Spawn supervised tasks ─────────────────────────────────────────

    // (a) Sequencer feed — streams DetectedSwap messages into swap_tx
    let feed_config = FeedConfig {
        feed_url: config.network.sequencer_feed_url.clone(),
        ..FeedConfig::default()
    };
    let feed_mgr = SequencerFeedManager::new(feed_config, pool_store.clone(), swap_tx)
        .with_provider(Arc::clone(&provider));
    let handle_feed: JoinHandle<anyhow::Result<()>> =
        tokio::spawn(async move { feed_mgr.run().await });

    // (b) Block reconciler — keeps PoolStateStore in sync with on-chain state
    let reconciler = BlockReconciler::new(
        AlloyReserveFetcher::new(Arc::clone(&provider)),
        pool_store.clone(),
        20,
    );
    let handle_reconciler: JoinHandle<anyhow::Result<()>> =
        tokio::spawn(async move { reconciler.run().await });

    // (c) Detection loop — scan paths + filter profit → opportunity_tx
    let handle_detection: JoinHandle<anyhow::Result<()>> = {
        let scanner = Arc::clone(&path_scanner);
        let calc = Arc::clone(&profit_calc);
        let metrics_d = Arc::clone(&metrics);
        let opp_tx = opportunity_tx;
        tokio::spawn(detection_loop(swap_rx, scanner, calc, opp_tx, metrics_d))
    };

    // (d) Execution loop — simulate → submit → PnL track
    let handle_execution: JoinHandle<anyhow::Result<()>> = {
        let sim = Arc::clone(&simulator);
        let sub = Arc::clone(&submitter);
        let pnl_e = Arc::clone(&pnl);
        let metrics_e = Arc::clone(&metrics);
        let eth_e = Arc::clone(&eth_price_usd);
        let sema = Arc::clone(&semaphore);
        tokio::spawn(execution_loop(
            opportunity_rx,
            sim,
            sub,
            contract_address,
            pnl_e,
            dry_run,
            metrics_e,
            eth_e,
            sema,
        ))
    };

    // (e) Budget watchdog — checks every 60 s; returns Err when exhausted
    let handle_watchdog: JoinHandle<anyhow::Result<()>> = {
        let pnl_w = Arc::clone(&pnl);
        tokio::spawn(budget_watchdog(pnl_w))
    };

    info!("arbx pipeline fully running — 5 supervised tasks active");

    // ── 9. Stash abort handles before moving JoinHandles into select! ─────
    let abort_handles = [
        handle_feed.abort_handle(),
        handle_reconciler.abort_handle(),
        handle_detection.abort_handle(),
        handle_execution.abort_handle(),
        handle_watchdog.abort_handle(),
    ];

    // ── 10. Wait for the first exit or an OS signal ───────────────────────
    let shutdown_reason = tokio::select! {
        res = handle_feed       => format!("sequencer feed task exited: {res:?}"),
        res = handle_reconciler => format!("block reconciler task exited: {res:?}"),
        res = handle_detection  => format!("detection loop task exited: {res:?}"),
        res = handle_execution  => format!("execution loop task exited: {res:?}"),
        res = handle_watchdog   => format!("budget watchdog triggered: {res:?}"),
        _   = shutdown_signal() => "received SIGTERM or SIGINT".to_owned(),
    };

    warn!(reason = %shutdown_reason, "graceful shutdown initiated");

    // ── 11. Abort all remaining tasks ─────────────────────────────────────
    for h in &abort_handles {
        h.abort();
    }

    // ── 12. Persist PnL before exit ───────────────────────────────────────
    if let Err(e) = pnl.save().await {
        error!(error = %e, "failed to persist PnL state on shutdown");
    } else {
        info!(summary = %pnl.summary(), "PnL state persisted on shutdown");
    }

    Ok(())
}

// ─── detection_loop ──────────────────────────────────────────────────────────

/// Reads [`DetectedSwap`]s, scans two-hop paths, applies the profit filter,
/// and forwards cleared [`Opportunity`]s to `opportunity_tx`.
async fn detection_loop(
    mut swap_rx: mpsc::Receiver<DetectedSwap>,
    scanner: Arc<PathScanner>,
    calc: Arc<ProfitCalculator<AlloyGasFetcher>>,
    opportunity_tx: mpsc::Sender<Opportunity>,
    metrics: Arc<Metrics>,
) -> anyhow::Result<()> {
    info!("detection loop started");

    while let Some(swap) = swap_rx.recv().await {
        let paths = scanner.scan(swap.pool_address);
        if paths.is_empty() {
            tracing::debug!(pool = %swap.pool_address, "no two-hop paths for pool — skipping");
            continue;
        }

        metrics.opportunities_detected.inc_by(paths.len() as u64);
        tracing::debug!(
            pool  = %swap.pool_address,
            count = paths.len(),
            "scanning {} candidate two-hop path(s)",
            paths.len(),
        );

        for path in &paths {
            // Encode calldata (needed for L1/L2 gas estimation inside filter)
            let calldata = CallDataEncoder::encode_execute_arb(path, path.estimated_profit_wei);

            match calc.filter(path, calldata).await {
                Ok(Some(opportunity)) => {
                    metrics.opportunities_cleared_threshold.inc();
                    tracing::debug!(
                        pool_a = %path.pool_a,
                        profit = %opportunity.net_profit_wei,
                        "opportunity cleared profit threshold",
                    );
                    if opportunity_tx.send(opportunity).await.is_err() {
                        warn!("opportunity channel closed — detection loop exiting");
                        return Ok(());
                    }
                }
                Ok(None) => {
                    tracing::debug!(pool_a = %path.pool_a, "path below profit threshold");
                }
                Err(e) => {
                    warn!(error = %e, pool_a = %path.pool_a, "profit filter error");
                }
            }
        }
    }

    info!("swap channel closed — detection loop exiting cleanly");
    Ok(())
}

// ─── execution_loop ──────────────────────────────────────────────────────────

/// Receives [`Opportunity`]s, runs revm simulation, and (in live mode) submits
/// the arb transaction.  Tracks every outcome in the [`PnlTracker`].
///
/// Parallelism is capped by `semaphore` (= `max_concurrent_simulations` from
/// config).  Each opportunity runs in its own `tokio::spawn` subtask so
/// independent opportunities never block each other.
async fn execution_loop(
    mut opportunity_rx: mpsc::Receiver<Opportunity>,
    simulator: Arc<ArbSimulator>,
    submitter: Arc<TransactionSubmitter<AlloyTransactionSender>>,
    contract_address: Address,
    pnl: Arc<PnlTracker>,
    dry_run: bool,
    metrics: Arc<Metrics>,
    eth_price_usd: Arc<RwLock<f64>>,
    semaphore: Arc<Semaphore>,
) -> anyhow::Result<()> {
    info!("execution loop started");

    while let Some(opp) = opportunity_rx.recv().await {
        let sim = Arc::clone(&simulator);
        let sub = Arc::clone(&submitter);
        let pnl_c = Arc::clone(&pnl);
        let met = Arc::clone(&metrics);
        let eth = Arc::clone(&eth_price_usd);
        let sema = Arc::clone(&semaphore);
        let contract = contract_address;

        tokio::spawn(async move {
            // Acquire a simulation concurrency slot.
            let _permit = match sema.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return, // semaphore closed → pipeline shutting down
            };

            match sim.simulate(&opp, contract, contract).await {
                SimulationResult::Success {
                    net_profit_wei,
                    gas_used,
                } => {
                    met.opportunities_cleared_simulation.inc();
                    tracing::debug!(profit = %net_profit_wei, gas_used, "simulation succeeded");

                    if dry_run {
                        info!(profit = %net_profit_wei, "DRY RUN — would submit arb transaction");
                        // Record a synthetic result so the PnL snapshot shows potential costs.
                        let fake = make_dry_run_result(&opp, gas_used);
                        let price = read_eth_price(&eth);
                        if let Err(e) = pnl_c.record_submission(&fake, price).await {
                            warn!(error = %e, "dry-run PnL record failed");
                        }
                    } else {
                        let calldata =
                            CallDataEncoder::encode_execute_arb(&opp.path, opp.net_profit_wei);

                        match sub.submit(&opp, calldata).await {
                            Ok(result) => {
                                met.transactions_submitted.inc();
                                if result.success {
                                    met.transactions_succeeded.inc();
                                    info!(
                                        tx    = %result.tx_hash,
                                        pnl   = %result.net_pnl_wei,
                                        "arb transaction succeeded",
                                    );
                                } else {
                                    let reason =
                                        result.revert_reason.as_deref().unwrap_or("unknown");
                                    met.transactions_reverted.with_label_values(&[reason]).inc();
                                    warn!(
                                        tx     = %result.tx_hash,
                                        reason,
                                        "arb transaction reverted",
                                    );
                                }
                                let price = read_eth_price(&eth);
                                if let Err(e) = pnl_c.record_submission(&result, price).await {
                                    warn!(error = %e, "PnL record failed after submission");
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "transaction submission failed");
                            }
                        }
                    }
                }
                SimulationResult::Failure { reason } => {
                    tracing::debug!(reason, "simulation failed — discarding opportunity");
                }
            }
        });
    }

    info!("opportunity channel closed — execution loop exiting cleanly");
    Ok(())
}

// ─── budget_watchdog ─────────────────────────────────────────────────────────

/// Checks the operating budget every 60 seconds.  Returns `Err` when the
/// budget is exhausted, which causes the `tokio::select!` in `run()` to
/// trigger a graceful shutdown.
async fn budget_watchdog(pnl: Arc<PnlTracker>) -> anyhow::Result<()> {
    let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(60));
    ticker.tick().await; // skip the immediate first tick

    loop {
        ticker.tick().await;
        if pnl.is_budget_exhausted() {
            let snap = pnl.state_snapshot();
            error!(
                budget_remaining_usd = snap.budget_remaining_usd,
                summary = %pnl.summary(),
                "BUDGET EXHAUSTED — initiating shutdown",
            );
            return Err(anyhow::anyhow!(
                "operating budget exhausted (${:.4} remaining)",
                snap.budget_remaining_usd
            ));
        }
        info!(summary = %pnl.summary(), "budget watchdog: OK");
    }
}

// ─── shutdown_signal ─────────────────────────────────────────────────────────

/// Resolves when SIGINT (Ctrl-C) **or** SIGTERM is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C / SIGINT handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c  => {}
        _ = sigterm => {}
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a synthetic [`SubmissionResult`] for dry-run PnL accounting.
///
/// Gas costs are sourced from the [`Opportunity`] estimates; `net_pnl_wei` is
/// zero (no real transaction was submitted).
fn make_dry_run_result(opp: &Opportunity, gas_used: u64) -> SubmissionResult {
    use alloy::primitives::{TxHash, I256};
    SubmissionResult {
        tx_hash: TxHash::ZERO,
        success: false,
        revert_reason: Some("DRY RUN — no on-chain submission".to_owned()),
        gas_used,
        l2_gas_cost_wei: opp.l2_gas_cost_wei,
        l1_gas_cost_wei: opp.l1_gas_cost_wei,
        net_pnl_wei: I256::ZERO,
    }
}

/// Read the current ETH/USD price, returning the last good value on lock
/// poisoning (extremely unlikely in practice).
#[inline]
fn read_eth_price(price: &Arc<RwLock<f64>>) -> f64 {
    *price.read().unwrap_or_else(|e| e.into_inner())
}
