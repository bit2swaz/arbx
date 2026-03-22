//! Mini-Phase 7.2 — Workspace Integration Tests.
//!
//! These tests exercise the pipeline components end-to-end (without network
//! access) by composing real crate types with hand-coded stubs.  The private
//! `detection_loop` and `execution_loop` functions in `bin/arbx.rs` are not
//! accessible here; instead the underlying generic types are wired directly.

mod helpers;
mod testnet_validation;

use std::io::Write as _;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use alloy::primitives::I256;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};

use arbx_common::{
    metrics::Metrics,
    pnl::PnlTracker,
    types::{Opportunity, SubmissionResult},
};
use arbx_detector::{opportunity::PathScanner, profit::ProfitCalculator};
use arbx_executor::submitter::{TransactionSender, TransactionSubmitter};
use arbx_simulator::revm_sim::CallDataEncoder;

use helpers::{
    addr, make_balanced_pool_state_store, make_pool_state_store_with_known_pools,
    make_reverted_submission_result, make_test_config, make_test_opportunity, temp_pnl_path,
    FixedGasFetcher, PanickingTransactionSender,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum BudgetLoopError {
    BudgetExhausted,
}

async fn budget_guarded_test_execution_loop(
    mut opportunity_rx: mpsc::Receiver<Opportunity>,
    pnl: Arc<PnlTracker>,
    gas_cost_usd: f64,
    eth_price_usd: f64,
    submission_counter: Arc<AtomicUsize>,
    warn_event_counter: Arc<AtomicUsize>,
) -> Result<(), BudgetLoopError> {
    while let Some(_opp) = opportunity_rx.recv().await {
        if pnl.is_budget_exhausted() {
            return Err(BudgetLoopError::BudgetExhausted);
        }

        let gas_wei = ((gas_cost_usd / eth_price_usd) * 1e18).round() as u128;
        let result = SubmissionResult {
            tx_hash: alloy::primitives::TxHash::ZERO,
            success: false,
            revert_reason: Some("budgeted revert".to_owned()),
            gas_used: 50_000,
            l2_gas_cost_wei: alloy::primitives::U256::from(gas_wei),
            l1_gas_cost_wei: alloy::primitives::U256::ZERO,
            net_pnl_wei: -alloy::primitives::I256::from_raw(alloy::primitives::U256::from(gas_wei)),
        };

        pnl.record_submission(&result, eth_price_usd)
            .await
            .expect("record_submission must succeed");
        submission_counter.fetch_add(1, Ordering::SeqCst);

        if pnl.is_budget_low() && !pnl.is_budget_exhausted() {
            warn_event_counter.fetch_add(1, Ordering::SeqCst);
        }

        if pnl.is_budget_exhausted() {
            return Err(BudgetLoopError::BudgetExhausted);
        }
    }

    Ok(())
}

// ─── Shared generic execution loop ───────────────────────────────────────────

/// Generic execution loop used in integration tests.
///
/// Mirrors the logic of `bin::arbx::execution_loop` but is generic over
/// `S: TransactionSender`, allowing mock/stub variants.  The loop runs until
/// the `opportunity_rx` channel is closed.
async fn test_execution_loop<S>(
    mut opportunity_rx: mpsc::Receiver<Opportunity>,
    submitter: Arc<TransactionSubmitter<S>>,
    dry_run: bool,
    metrics: Arc<Metrics>,
    semaphore: Arc<Semaphore>,
    pnl: Arc<PnlTracker>,
) where
    S: TransactionSender + Send + Sync + 'static,
{
    while let Some(opp) = opportunity_rx.recv().await {
        let sub = Arc::clone(&submitter);
        let met = Arc::clone(&metrics);
        let sema = Arc::clone(&semaphore);
        let pnl_c = Arc::clone(&pnl);

        tokio::spawn(async move {
            let _permit = match sema.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };

            // Inline simulation stub: always returns Success
            met.opportunities_cleared_simulation.inc();

            if dry_run {
                let fake = SubmissionResult {
                    tx_hash: alloy::primitives::TxHash::ZERO,
                    success: false,
                    revert_reason: Some("DRY RUN".to_owned()),
                    gas_used: 300_000,
                    l2_gas_cost_wei: opp.l2_gas_cost_wei,
                    l1_gas_cost_wei: opp.l1_gas_cost_wei,
                    net_pnl_wei: I256::ZERO,
                };
                let _ = pnl_c.record_submission(&fake, 3_000.0).await;
            } else {
                let calldata = CallDataEncoder::encode_execute_arb(&opp.path, opp.net_profit_wei);
                if let Ok(result) = sub.submit(&opp, calldata).await {
                    met.transactions_submitted.inc();
                    if result.success {
                        met.transactions_succeeded.inc();
                    }
                    let _ = pnl_c.record_submission(&result, 3_000.0).await;
                }
            }
        });
    }
}

// ─── Test 1 ──────────────────────────────────────────────────────────────────

/// A swap on a pool that is part of a profitable two-hop cycle should produce
/// at least one opportunity on the channel.
#[tokio::test]
async fn integration_detection_loop_sends_opportunities() {
    let store = make_pool_state_store_with_known_pools();
    let scanner = PathScanner::new(store);
    let config = make_test_config();
    let calc = ProfitCalculator::new(FixedGasFetcher::cheap(), config.strategy.clone());
    let metrics = Arc::new(Metrics::new().unwrap());

    let (opportunity_tx, mut opportunity_rx) = mpsc::channel::<Opportunity>(32);

    let affected_pool = addr(0x10);
    let paths = scanner.scan(affected_pool);
    assert!(!paths.is_empty(), "expected at least one two-hop path");

    metrics.opportunities_detected.inc_by(paths.len() as u64);

    for path in &paths {
        let calldata = CallDataEncoder::encode_execute_arb(path, path.estimated_profit_wei);
        match calc.filter(path, calldata).await {
            Ok(Some(opp)) => {
                metrics.opportunities_cleared_threshold.inc();
                opportunity_tx.send(opp).await.unwrap();
            }
            Ok(None) => {}
            Err(e) => panic!("filter returned error: {e}"),
        }
    }
    drop(opportunity_tx);

    let mut received: Vec<Opportunity> = Vec::new();
    while let Some(opp) = opportunity_rx.recv().await {
        received.push(opp);
    }

    assert!(
        !received.is_empty(),
        "expected at least one opportunity to pass the profit filter"
    );
    assert!(
        metrics.opportunities_detected.get() >= 1,
        "opportunities_detected counter must be >= 1"
    );
    assert!(
        metrics.opportunities_cleared_threshold.get() >= 1,
        "opportunities_cleared_threshold must be >= 1"
    );
}

// ─── Test 2 ──────────────────────────────────────────────────────────────────

/// When gas costs exceed the profit estimate the filter should return `None`
/// for every path — no opportunity reaches the channel.
#[tokio::test]
async fn integration_detection_loop_filters_unprofitable() {
    // Use balanced pools — estimated profit ≈ 0, so even cheap gas exceeds it
    let store = make_balanced_pool_state_store();
    let scanner = PathScanner::new(store);
    let config = make_test_config();
    // Any non-zero gas cost will exceed the near-zero profit
    let calc = ProfitCalculator::new(FixedGasFetcher::cheap(), config.strategy.clone());
    let metrics = Arc::new(Metrics::new().unwrap());

    let (opportunity_tx, mut opportunity_rx) = mpsc::channel::<Opportunity>(32);

    let affected_pool = addr(0x20);
    let paths = scanner.scan(affected_pool);
    metrics.opportunities_detected.inc_by(paths.len() as u64);

    for path in &paths {
        let calldata = CallDataEncoder::encode_execute_arb(path, path.estimated_profit_wei);
        if let Ok(Some(opp)) = calc.filter(path, calldata).await {
            metrics.opportunities_cleared_threshold.inc();
            opportunity_tx.send(opp).await.unwrap();
        }
    }
    drop(opportunity_tx);

    let mut count = 0usize;
    while opportunity_rx.recv().await.is_some() {
        count += 1;
    }

    assert_eq!(
        count, 0,
        "no opportunities should pass a prohibitive gas filter"
    );
    assert_eq!(
        metrics.opportunities_cleared_threshold.get(),
        0,
        "cleared_threshold must be 0 when all paths are unprofitable"
    );
}

// ─── Test 3 ──────────────────────────────────────────────────────────────────

/// When `dry_run = true` the `PanickingTransactionSender` must never be called
/// (otherwise the test panics).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_execution_loop_dry_run_never_submits() {
    let config = make_test_config();
    let metrics = Arc::new(Metrics::new().unwrap());
    let semaphore = Arc::new(Semaphore::new(4));
    let (_tmp_dir, pnl_path) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::new(pnl_path, 60.0).unwrap());
    let contract_address = config.execution.contract_address.parse().unwrap();
    let submitter = Arc::new(TransactionSubmitter::new(
        PanickingTransactionSender,
        contract_address,
        config.execution.clone(),
        Arc::clone(&metrics),
    ));

    let (opportunity_tx, opportunity_rx) = mpsc::channel::<Opportunity>(32);

    let loop_handle = tokio::spawn(test_execution_loop(
        opportunity_rx,
        submitter,
        true, // dry_run — must never touch PanickingTransactionSender
        Arc::clone(&metrics),
        semaphore,
        pnl,
    ));

    for _ in 0..3 {
        opportunity_tx.send(make_test_opportunity()).await.unwrap();
    }
    drop(opportunity_tx);

    timeout(Duration::from_secs(5), loop_handle)
        .await
        .expect("execution loop timed out")
        .expect("execution loop panicked");

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        metrics.opportunities_cleared_simulation.get(),
        3,
        "all 3 opportunities should have been (mock-)simulated"
    );
    assert_eq!(
        metrics.transactions_submitted.get(),
        0,
        "dry-run must not increment transactions_submitted"
    );
}

// ─── Test 4 ──────────────────────────────────────────────────────────────────

/// After enough losing submissions the PnL tracker must report budget exhaustion.
#[tokio::test]
async fn integration_pnl_tracker_budget_triggers_shutdown() {
    let (_tmp_dir, pnl_path) = temp_pnl_path();
    // Start with $0.50 budget. One reverted tx costs 0.0003 ETH * $3 000 = $0.90 USD.
    let pnl = Arc::new(PnlTracker::new(pnl_path, 0.50).unwrap());

    assert!(
        !pnl.is_budget_exhausted(),
        "budget must not be exhausted before any submissions"
    );

    let result = make_reverted_submission_result("INSUFFICIENT_PROFIT");
    pnl.record_submission(&result, 3_000.0).await.unwrap();

    assert!(
        pnl.is_budget_exhausted(),
        "budget should be exhausted after a single high-gas reverted submission"
    );

    let snap = pnl.state_snapshot();
    assert_eq!(snap.reverted_arbs, 1);
    assert_eq!(snap.total_submissions, 1);
}

// ─── Test 5 ──────────────────────────────────────────────────────────────────

/// Full pipeline trace (no network): store → scan → filter → (mock sim) →
/// dry_run execution → metrics assertions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_full_pipeline_mock_discovery_to_skip() {
    let store = make_pool_state_store_with_known_pools();
    let scanner = PathScanner::new(store);
    let config = make_test_config();
    let calc = ProfitCalculator::new(FixedGasFetcher::cheap(), config.strategy.clone());
    let metrics = Arc::new(Metrics::new().unwrap());
    let semaphore = Arc::new(Semaphore::new(3));
    let (_tmp_dir5, pnl_path5) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::new(pnl_path5, 60.0).unwrap());
    let contract_address = config.execution.contract_address.parse().unwrap();
    let submitter = Arc::new(TransactionSubmitter::new(
        PanickingTransactionSender, // safe — dry_run=true
        contract_address,
        config.execution.clone(),
        Arc::clone(&metrics),
    ));

    let (opportunity_tx, opportunity_rx) = mpsc::channel::<Opportunity>(32);

    let exec_handle = tokio::spawn(test_execution_loop(
        opportunity_rx,
        submitter,
        true,
        Arc::clone(&metrics),
        semaphore,
        pnl,
    ));

    // Detection phase
    let affected_pool = addr(0x10);
    let paths = scanner.scan(affected_pool);
    assert!(!paths.is_empty());
    metrics.opportunities_detected.inc_by(paths.len() as u64);

    let mut forwarded = 0u64;
    for path in &paths {
        let calldata = CallDataEncoder::encode_execute_arb(path, path.estimated_profit_wei);
        if let Ok(Some(opp)) = calc.filter(path, calldata).await {
            metrics.opportunities_cleared_threshold.inc();
            opportunity_tx.send(opp).await.unwrap();
            forwarded += 1;
        }
    }
    drop(opportunity_tx);

    timeout(Duration::from_secs(5), exec_handle)
        .await
        .expect("pipeline timed out")
        .expect("execution loop panicked");

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(metrics.opportunities_detected.get() >= 1);
    assert_eq!(metrics.opportunities_cleared_threshold.get(), forwarded);
    assert_eq!(metrics.opportunities_cleared_simulation.get(), forwarded);
    assert_eq!(
        metrics.transactions_submitted.get(),
        0,
        "dry-run must not submit"
    );
}

// ─── Test 6 ──────────────────────────────────────────────────────────────────

/// Sending 100 opportunities into a capacity-1 channel must complete without
/// deadlock within a generous timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_channel_backpressure() {
    let metrics = Arc::new(Metrics::new().unwrap());
    let semaphore = Arc::new(Semaphore::new(4));
    let (_tmp_dir6, pnl_path6) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::new(pnl_path6, 60.0).unwrap());
    let config = make_test_config();
    let contract_address = config.execution.contract_address.parse().unwrap();
    let submitter = Arc::new(TransactionSubmitter::new(
        PanickingTransactionSender,
        contract_address,
        config.execution.clone(),
        Arc::clone(&metrics),
    ));

    // Capacity = 1 to exercise backpressure
    let (opportunity_tx, opportunity_rx) = mpsc::channel::<Opportunity>(1);

    let loop_handle = tokio::spawn(test_execution_loop(
        opportunity_rx,
        submitter,
        true,
        Arc::clone(&metrics),
        semaphore,
        pnl,
    ));

    // Send 100 items — will block on backpressure until the receiver drains
    let sender_handle = tokio::spawn(async move {
        for _ in 0..100u64 {
            opportunity_tx.send(make_test_opportunity()).await.unwrap();
        }
    });

    timeout(Duration::from_secs(10), sender_handle)
        .await
        .expect("sender timed out")
        .expect("sender panicked");

    timeout(Duration::from_secs(10), loop_handle)
        .await
        .expect("execution loop timed out")
        .expect("execution loop panicked");
}

#[tokio::test]
async fn test_kill_switch_halts_at_budget_exhaustion() {
    let (_tmp_dir, pnl_path) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::with_thresholds(pnl_path.clone(), 5.0, 0.5, 1.0).unwrap());
    let submission_counter = Arc::new(AtomicUsize::new(0));
    let warn_event_counter = Arc::new(AtomicUsize::new(0));
    let (opportunity_tx, opportunity_rx) = mpsc::channel(8);

    for _ in 0..6 {
        opportunity_tx.send(make_test_opportunity()).await.unwrap();
    }
    drop(opportunity_tx);

    let result = budget_guarded_test_execution_loop(
        opportunity_rx,
        Arc::clone(&pnl),
        0.80,
        1.0,
        Arc::clone(&submission_counter),
        Arc::clone(&warn_event_counter),
    )
    .await;

    assert_eq!(result, Err(BudgetLoopError::BudgetExhausted));
    assert!(pnl.is_budget_exhausted());
    assert_eq!(submission_counter.load(Ordering::SeqCst), 5);
    assert_eq!(warn_event_counter.load(Ordering::SeqCst), 0);

    let snap = pnl.state_snapshot();
    assert!((snap.total_gas_spent_usd - 4.0).abs() < 0.0001);
    assert!((snap.budget_remaining_usd - 1.0).abs() < 0.0001);

    let json = std::fs::read_to_string(&pnl_path).expect("pnl state file must exist");
    let persisted: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    let total_gas_spent = persisted
        .get("total_gas_spent_usd")
        .and_then(|v| v.as_f64())
        .expect("persisted gas spend");
    assert!((total_gas_spent - 4.0).abs() < 0.0001);
}

#[tokio::test]
async fn test_kill_switch_warn_threshold_logged() {
    let (_tmp_dir, pnl_path) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::with_thresholds(pnl_path, 10.0, 5.0, 1.0).unwrap());
    let submission_counter = Arc::new(AtomicUsize::new(0));
    let warn_event_counter = Arc::new(AtomicUsize::new(0));
    let (opportunity_tx, opportunity_rx) = mpsc::channel(8);

    for _ in 0..5 {
        opportunity_tx.send(make_test_opportunity()).await.unwrap();
    }
    drop(opportunity_tx);

    let result = budget_guarded_test_execution_loop(
        opportunity_rx,
        Arc::clone(&pnl),
        1.10,
        1.0,
        Arc::clone(&submission_counter),
        Arc::clone(&warn_event_counter),
    )
    .await;

    assert_eq!(result, Ok(()));
    assert_eq!(submission_counter.load(Ordering::SeqCst), 5);
    assert!(warn_event_counter.load(Ordering::SeqCst) >= 1);
    assert!(pnl.is_budget_low());
    assert!(!pnl.is_budget_exhausted());
}

#[tokio::test]
async fn test_pnl_state_persists_across_restart() {
    let (_tmp_dir, pnl_path) = temp_pnl_path();
    let json = r#"{
  "total_gas_spent_wei": "1000000000000000000",
  "total_gas_spent_usd": 4.0,
  "total_profit_wei": "500000000000000000",
  "total_profit_usd": 1.5,
  "net_pnl_usd": 1.5,
  "successful_arbs": 1,
  "reverted_arbs": 3,
  "total_submissions": 4,
  "budget_remaining_usd": 23.0,
  "session_start_ms": 1,
  "last_updated_ms": 2
}"#;
    let mut file = std::fs::File::create(&pnl_path).unwrap();
    file.write_all(json.as_bytes()).unwrap();
    file.sync_all().unwrap();

    let tracker = PnlTracker::with_thresholds(pnl_path, 27.0, 5.0, 2.0).unwrap();
    let snap = tracker.state_snapshot();

    assert_eq!(snap.total_gas_spent_wei, "1000000000000000000");
    assert_eq!(snap.successful_arbs, 1);
    assert_eq!(snap.reverted_arbs, 3);
    assert_eq!(snap.total_submissions, 4);
    assert!((snap.budget_remaining_usd - 23.0).abs() < 0.0001);
    assert!((tracker.budget_remaining_usd() - 23.0).abs() < 0.0001);
}

// ─── Test 7 ──────────────────────────────────────────────────────────────────

/// The semaphore must cap concurrent simulations at `max_concurrent`.
///
/// A `CountingTransactionSender` with a 50 ms delay measures the peak
/// concurrency.  With semaphore(3) and 10 opportunities, the high-water mark
/// must be ≤ 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn integration_concurrent_simulations_capped() {
    use helpers::CountingTransactionSender;

    let max_concurrent = 3;
    let counter = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let sender = CountingTransactionSender {
        concurrent: Arc::clone(&counter),
        max_observed: Arc::clone(&max_observed),
        delay_ms: 50,
    };

    let metrics = Arc::new(Metrics::new().unwrap());
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let (_tmp_dir7, pnl_path7) = temp_pnl_path();
    let pnl = Arc::new(PnlTracker::new(pnl_path7, 60.0).unwrap());
    let config = make_test_config();
    let contract_address = config.execution.contract_address.parse().unwrap();
    let submitter = Arc::new(TransactionSubmitter::new(
        sender,
        contract_address,
        config.execution.clone(),
        Arc::clone(&metrics),
    ));

    let (opportunity_tx, opportunity_rx) = mpsc::channel::<Opportunity>(32);

    let loop_handle = tokio::spawn(test_execution_loop(
        opportunity_rx,
        submitter,
        false, // live mode — calls CountingTransactionSender::send()
        Arc::clone(&metrics),
        semaphore,
        pnl,
    ));

    for _ in 0..10u64 {
        opportunity_tx.send(make_test_opportunity()).await.unwrap();
    }
    drop(opportunity_tx);

    timeout(Duration::from_secs(10), loop_handle)
        .await
        .expect("execution loop timed out")
        .expect("execution loop panicked");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let observed = max_observed.load(Ordering::SeqCst);
    assert!(
        observed <= max_concurrent,
        "peak concurrent simulations ({observed}) must not exceed cap ({max_concurrent})"
    );
}
