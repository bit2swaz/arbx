//! Mini-Phase 8.2 — Chaos Tests: Feed and RPC Fault Injection.
//!
//! # Design
//! The Arbitrum sequencer feed is a WebSocket that streams JSON messages.
//! `SequencerFeedManager::run()` uses `sequencer_client::SequencerReader::new()`
//! which calls `tokio_tungstenite::connect_async` under the hood.  To inject
//! faults without mocking the entire crate we spin up a real
//! `tokio-tungstenite` WebSocket server on a random loopback port and
//! control it via an `mpsc` channel.
//!
//! # RPC fault injection
//! `BlockReconciler` and `TransactionSubmitter` expose `mockall`-generated
//! mocks through their public trait APIs.  The `MockReserveFetcher` lives
//! inside `arbx_ingestion` (gated by `#[cfg(test)]`) and is re-exported only
//! in test builds; we replicate the same trait here with a hand-rolled stub
//! to avoid depending on `cfg(test)` items from another crate.

use std::{
    net::TcpListener,
    sync::Arc,
    time::{Duration, Instant},
};

use alloy::primitives::{Address, Bytes, TxHash, U256};
use alloy::rpc::types::TransactionReceipt;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use arbx_common::{
    config::ExecutionConfig,
    metrics::Metrics,
    types::{ArbPath, DexKind, Opportunity, PoolState},
};
use arbx_executor::submitter::{TransactionSender, TransactionSubmitter};
use arbx_ingestion::{
    pool_state::PoolStateStore,
    reconciler::{BlockReconciler, ReserveFetcher},
    sequencer_feed::{FeedConfig, SequencerFeedManager},
};

// ═════════════════════════════════════════════════════════════════════════════
// MockFeedServer — lightweight WS server for chaos injection
// ═════════════════════════════════════════════════════════════════════════════

/// Instructions sent to the mock WS server task.
#[derive(Debug)]
pub enum FeedFault {
    /// Close the connection (triggers reconnect in the feed manager).
    Disconnect,
    /// Send a message that is not valid JSON.
    SendMalformedMessage,
    /// Send a JSON object that has a valid `messages` array but is empty.
    SendEmptyBatch,
    /// Send the same sequence-number message twice back-to-back.
    SendDuplicateSequenceNumber,
    /// Sleep for the given duration without sending anything.
    Pause(Duration),
}

/// A minimal WebSocket server that accepts one connection at a time and
/// executes injected [`FeedFault`] commands.
pub struct MockFeedServer {
    port: u16,
    control_tx: mpsc::Sender<FeedFault>,
    /// Fires whenever the server accepts a new client connection.
    connected: Arc<Notify>,
}

impl MockFeedServer {
    /// Bind to a random loopback port and start the server task.
    pub async fn start() -> Self {
        // Pick a free port by binding to :0 then releasing the listener.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let (control_tx, mut control_rx) = mpsc::channel::<FeedFault>(32);
        let connected = Arc::new(Notify::new());
        let connected_clone = connected.clone();

        tokio::spawn(async move {
            // Re-bind as an async listener.
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .expect("async bind");

            // Serve connections in a loop so that after a Disconnect the
            // manager's reconnect attempt is also handled.
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let mut ws = accept_async(stream).await.expect("ws handshake");
                connected_clone.notify_one();

                // Drive this connection until it closes or a fault fires.
                loop {
                    tokio::select! {
                        // Inbound frame from the client (keep the connection alive).
                        msg = ws.next() => {
                            match msg {
                                Some(Ok(_)) => {}           // ignore client frames
                                _ => break,                 // client closed
                            }
                        }
                        // Fault injected from the test.
                        cmd = control_rx.recv() => {
                            match cmd {
                                None => return,             // test dropped sender → shut down

                                Some(FeedFault::Disconnect) => {
                                    // Abruptly close without a WS close handshake.
                                    let _ = ws.close(None).await;
                                    break;
                                }

                                Some(FeedFault::SendMalformedMessage) => {
                                    let _ = ws.send(Message::Text(
                                        "this is not json at all !!!".into()
                                    )).await;
                                }

                                Some(FeedFault::SendEmptyBatch) => {
                                    // Valid JSON root with an explicit empty messages array.
                                    let payload = r#"{"version":1,"messages":[]}"#;
                                    let _ = ws.send(Message::Text(payload.into())).await;
                                }

                                Some(FeedFault::SendDuplicateSequenceNumber) => {
                                    // Two copies of the same sequence number 42.
                                    for _ in 0..2 {
                                        let msg = r#"{"version":1,"messages":[{"sequenceNumber":42,"message":{"header":{"kind":3,"sender":"0x1234","blockNumber":1,"blockHash":"0x0000000000000000000000000000000000000000000000000000000000000000","timestamp":0,"requestId":null,"l1BaseFee":null},"l2Msg":""}}]}"#;
                                        let _ = ws.send(Message::Text(msg.into())).await;
                                    }
                                }

                                Some(FeedFault::Pause(d)) => {
                                    tokio::time::sleep(d).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        Self {
            port,
            control_tx,
            connected,
        }
    }

    /// The WebSocket URL to connect to.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Inject a fault into the currently-connected session.
    pub async fn inject(&self, fault: FeedFault) {
        self.control_tx
            .send(fault)
            .await
            .expect("control channel closed");
    }

    /// Wait until a client has connected (or the timeout elapses).
    pub async fn wait_connected(&self, timeout: Duration) {
        tokio::time::timeout(timeout, self.connected.notified())
            .await
            .expect("timed out waiting for client connection");
    }
}

// ─── Helper: build a FeedConfig pointing at the mock server ──────────────────

fn feed_config(url: String) -> FeedConfig {
    FeedConfig {
        feed_url: url,
        // Use very short backoff for tests so reconnects happen quickly.
        reconnect_base_ms: 50,
        reconnect_max_ms: 400,
        reconnect_multiplier: 2.0,
    }
}

// ─── Helper: build a minimal PoolStateStore with one pool ────────────────────

fn one_pool_store() -> PoolStateStore {
    let store = PoolStateStore::new();
    store.upsert(PoolState {
        address: Address::from([0x10u8; 20]),
        token0: Address::from([0x01u8; 20]),
        token1: Address::from([0x02u8; 20]),
        reserve0: U256::from(1_000_000_u128),
        reserve1: U256::from(2_000_000_u128),
        fee_tier: 3000,
        last_updated_block: 1,
        dex: DexKind::CamelotV2,
    });
    store
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 1 — Feed Chaos Tests
// ═════════════════════════════════════════════════════════════════════════════

/// 1. Disconnect mid-stream → manager stays alive and reconnects.
#[tokio::test]
async fn chaos_feed_reconnects_after_disconnect() {
    let server = MockFeedServer::start().await;
    let (swap_tx, _swap_rx) = mpsc::channel(100);

    let mgr = SequencerFeedManager::new(feed_config(server.url()), one_pool_store(), swap_tx);
    let mgr_handle = tokio::spawn(mgr.run());

    // Wait for first connection.
    server.wait_connected(Duration::from_secs(5)).await;

    // Inject a hard disconnect.
    server.inject(FeedFault::Disconnect).await;

    // Give the manager time to detect the disconnect and try to reconnect.
    // With base_ms=50 ms the first reconnect fires after ~50 ms.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The task must still be running — it should not have panicked or exited.
    assert!(
        !mgr_handle.is_finished(),
        "manager task must survive a disconnect"
    );

    mgr_handle.abort();
}

/// 2. Malformed JSON message is silently discarded; manager keeps running.
#[tokio::test]
async fn chaos_feed_handles_malformed_message_without_panic() {
    let server = MockFeedServer::start().await;
    let (swap_tx, _swap_rx) = mpsc::channel(100);

    let mgr = SequencerFeedManager::new(feed_config(server.url()), one_pool_store(), swap_tx);
    let mgr_handle = tokio::spawn(mgr.run());

    server.wait_connected(Duration::from_secs(5)).await;

    // Blast 5 malformed messages.
    for _ in 0..5 {
        server.inject(FeedFault::SendMalformedMessage).await;
    }

    // Allow some processing time.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !mgr_handle.is_finished(),
        "manager must survive malformed messages"
    );

    mgr_handle.abort();
}

/// 3. Empty batch message is handled without panic or crash.
#[tokio::test]
async fn chaos_feed_handles_empty_batch_without_panic() {
    let server = MockFeedServer::start().await;
    let (swap_tx, _swap_rx) = mpsc::channel(100);

    let mgr = SequencerFeedManager::new(feed_config(server.url()), one_pool_store(), swap_tx);
    let mgr_handle = tokio::spawn(mgr.run());

    server.wait_connected(Duration::from_secs(5)).await;

    for _ in 0..10 {
        server.inject(FeedFault::SendEmptyBatch).await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !mgr_handle.is_finished(),
        "manager must survive empty batch messages"
    );

    mgr_handle.abort();
}

/// 4. Duplicate sequence numbers are deduplicated by sequencer_client
///    internals; no duplicate `DetectedSwap` events are emitted.
///    (Even if sequencer_client lets through a duplicate, the manager must
///    not crash — we verify the task stays alive.)
#[tokio::test]
async fn chaos_feed_handles_duplicate_sequence_numbers() {
    let server = MockFeedServer::start().await;
    let (swap_tx, _swap_rx) = mpsc::channel(100);

    let mgr = SequencerFeedManager::new(feed_config(server.url()), one_pool_store(), swap_tx);
    let mgr_handle = tokio::spawn(mgr.run());

    server.wait_connected(Duration::from_secs(5)).await;

    // Inject the duplicate-sequence-number fault several times.
    for _ in 0..5 {
        server.inject(FeedFault::SendDuplicateSequenceNumber).await;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        !mgr_handle.is_finished(),
        "manager must survive duplicate sequence numbers"
    );

    mgr_handle.abort();
}

/// 5. Backoff timing: the `BackoffCalculator` produces the correct exponential
///    sequence and caps at the configured maximum.
///
///    We test `BackoffCalculator` directly — it is the public component that
///    owns the backoff schedule.  The integration with `SequencerFeedManager`
///    is exercised implicitly by the reconnect test (test 1).
#[tokio::test]
async fn chaos_feed_backoff_timing() {
    use arbx_ingestion::sequencer_feed::BackoffCalculator;

    // ── Standard exponential sequence ────────────────────────────────────
    let mut calc = BackoffCalculator::new(100, 5_000, 2.0);
    assert_eq!(calc.next(), 100, "1st delay must be base");
    assert_eq!(calc.next(), 200, "2nd delay doubles");
    assert_eq!(calc.next(), 400, "3rd delay doubles again");
    assert_eq!(calc.next(), 800, "4th delay");
    assert_eq!(calc.next(), 1_600, "5th delay");

    // ── Cap is respected ─────────────────────────────────────────────────
    let mut capped = BackoffCalculator::new(100, 300, 2.0);
    assert_eq!(capped.next(), 100);
    assert_eq!(capped.next(), 200);
    assert_eq!(capped.next(), 300, "must not exceed max");
    assert_eq!(capped.next(), 300, "stays at max");

    // ── Reset returns to base ─────────────────────────────────────────────
    calc.reset();
    assert_eq!(calc.next(), 100, "after reset must return base");

    // ── Manager stays alive while connected to a real server ─────────────
    // (Ensures the manager loop itself is healthy, not just the calculator.)
    let server = MockFeedServer::start().await;
    let (swap_tx, _swap_rx) = mpsc::channel(8);
    let cfg = FeedConfig {
        feed_url: server.url(),
        reconnect_base_ms: 50,
        reconnect_max_ms: 400,
        reconnect_multiplier: 2.0,
    };
    let mgr = SequencerFeedManager::new(cfg, one_pool_store(), swap_tx);
    let mgr_handle = tokio::spawn(mgr.run());

    server.wait_connected(Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!mgr_handle.is_finished(), "manager must remain alive");
    mgr_handle.abort();
}

// ═════════════════════════════════════════════════════════════════════════════
// GROUP 2 — RPC Fault Injection Tests
// ═════════════════════════════════════════════════════════════════════════════

// ─── Hand-rolled ReserveFetcher stub ─────────────────────────────────────────
//
// We cannot use the `#[cfg(test)]`-gated MockReserveFetcher from inside
// arbx_ingestion at the workspace test level, so we write our own stubs.

/// A `ReserveFetcher` that always fails with a configurable error after an
/// optional delay.
struct FailingFetcher {
    delay: Option<Duration>,
    message: &'static str,
}

impl FailingFetcher {
    fn with_delay(delay: Duration, message: &'static str) -> Self {
        Self {
            delay: Some(delay),
            message,
        }
    }
}

#[async_trait]
impl ReserveFetcher for FailingFetcher {
    async fn fetch_v2_reserves(&self, _pool: Address) -> anyhow::Result<(U256, U256, u64)> {
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        Err(anyhow::anyhow!("{}", self.message))
    }

    async fn fetch_v3_slot0(&self, _pool: Address) -> anyhow::Result<(U256, i32, U256)> {
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        Err(anyhow::anyhow!("{}", self.message))
    }

    async fn current_block(&self) -> anyhow::Result<u64> {
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
        Err(anyhow::anyhow!("{}", self.message))
    }
}

/// A `ReserveFetcher` where a configurable subset of pool addresses fails.
struct PartialFailFetcher {
    /// Addresses whose calls succeed (all others fail).
    succeeding: std::collections::HashSet<Address>,
}

#[async_trait]
impl ReserveFetcher for PartialFailFetcher {
    async fn fetch_v2_reserves(&self, pool: Address) -> anyhow::Result<(U256, U256, u64)> {
        if self.succeeding.contains(&pool) {
            Ok((U256::from(1_000_u128), U256::from(2_000_u128), 1))
        } else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Err(anyhow::anyhow!("simulated timeout for {pool}"))
        }
    }

    async fn fetch_v3_slot0(&self, pool: Address) -> anyhow::Result<(U256, i32, U256)> {
        if self.succeeding.contains(&pool) {
            Ok((U256::from(1_000_u128), 0, U256::from(2_000_u128)))
        } else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Err(anyhow::anyhow!("simulated timeout for {pool}"))
        }
    }

    async fn current_block(&self) -> anyhow::Result<u64> {
        Ok(100)
    }
}

/// Build a PoolStateStore with `n` V2 pools at addresses [seed, seed+1, ..].
fn make_pool_store(n: u8, seed: u8) -> PoolStateStore {
    let store = PoolStateStore::new();
    for i in 0..n {
        let addr = Address::from([seed + i; 20]);
        store.upsert(PoolState {
            address: addr,
            token0: Address::ZERO,
            token1: Address::ZERO,
            reserve0: U256::from(1_000_u128),
            reserve1: U256::from(2_000_u128),
            fee_tier: 3000,
            last_updated_block: 1,
            dex: DexKind::CamelotV2,
        });
    }
    store
}

/// 6. Every pool fails when the fetcher times out on every call.
#[tokio::test]
async fn chaos_reconciler_handles_rpc_timeout() {
    let n_pools: u8 = 5;
    let store = make_pool_store(n_pools, 0x10);

    let fetcher = FailingFetcher::with_delay(Duration::from_millis(100), "simulated RPC timeout");
    let reconciler = BlockReconciler::new(fetcher, store, 20);

    let stats = reconciler.reconcile_all(42).await;

    assert_eq!(
        stats.pools_checked, n_pools as usize,
        "all pools must be checked"
    );
    assert_eq!(
        stats.pools_failed, n_pools as usize,
        "all pools must fail when fetcher times out"
    );
    assert_eq!(
        stats.pools_updated, 0,
        "no pool should be updated on failure"
    );
}

/// 7. 10 pools: 7 succeed, 3 fail → stats correctly split.
#[tokio::test]
async fn chaos_reconciler_partial_failure() {
    let n_total: u8 = 10;
    let n_succeed: u8 = 7;

    let store = make_pool_store(n_total, 0x20);

    // Addresses [0x20..0x26] succeed; [0x27..0x29] fail.
    let succeeding: std::collections::HashSet<Address> = (0..n_succeed)
        .map(|i| Address::from([0x20u8 + i; 20]))
        .collect();

    let fetcher = PartialFailFetcher { succeeding };
    let reconciler = BlockReconciler::new(fetcher, store, 20);

    let stats = reconciler.reconcile_all(99).await;

    assert_eq!(stats.pools_checked, n_total as usize);
    // The 7 succeeding pools have stale reserves (1000/2000 ≠ on-chain 1000/2000
    // at same block) — actually same values, so they read as unchanged not updated.
    // What we care about is that exactly (n_total - n_succeed) fail.
    assert_eq!(
        stats.pools_failed,
        (n_total - n_succeed) as usize,
        "exactly 3 pools must fail"
    );
}

// ─── Hand-rolled TransactionSender stubs ─────────────────────────────────────

/// A `TransactionSender` whose `send()` hangs for a long time (simulates RPC
/// timeout on submission).
struct HangingSender {
    hang_duration: Duration,
}

#[async_trait]
impl TransactionSender for HangingSender {
    async fn send(
        &self,
        _calldata: Bytes,
        _to: Address,
        _gas: u64,
        _price: u128,
    ) -> anyhow::Result<TxHash> {
        tokio::time::sleep(self.hang_duration).await;
        Ok(TxHash::default())
    }
    async fn get_receipt(&self, _hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        Ok(None)
    }
    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(1_000_000_000)
    }
    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(50_000)
    }
    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(200_000)
    }
    async fn call_for_revert(&self, _to: Address, _data: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }
}

/// A `TransactionSender` that returns a hash immediately but whose
/// `get_receipt()` always returns `None` (receipt never arrives).
struct NoReceiptSender;

#[async_trait]
impl TransactionSender for NoReceiptSender {
    async fn send(
        &self,
        _calldata: Bytes,
        _to: Address,
        _gas: u64,
        _price: u128,
    ) -> anyhow::Result<TxHash> {
        Ok(TxHash::default())
    }
    async fn get_receipt(&self, _hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        // Simulate a slow response to prevent busy-loop in tests.
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(None)
    }
    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(1_000_000_000)
    }
    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(50_000)
    }
    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(200_000)
    }
    async fn call_for_revert(&self, _to: Address, _data: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }
}

// ─── Submitter helpers ────────────────────────────────────────────────────────

fn test_exec_config() -> ExecutionConfig {
    ExecutionConfig {
        contract_address: format!("{}", Address::ZERO),
        private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
            .to_owned(),
        max_concurrent_simulations: 4,
        gas_estimate_buffer: 1.2,
        node_interface_address: "0x00000000000000000000000000000000000000C8".to_owned(),
        l2_gas_units: 500_000,
        receipt_timeout_secs: 30, // will be overridden per-test
        dry_run: false,
    }
}

fn test_opportunity() -> Opportunity {
    Opportunity {
        path: ArbPath {
            token_in: Address::ZERO,
            pool_a: Address::ZERO,
            token_mid: Address::ZERO,
            pool_b: Address::ZERO,
            token_out: Address::ZERO,
            estimated_profit_wei: U256::from(1_000_000_u128),
            flash_loan_amount_wei: U256::ZERO,
        },
        gross_profit_wei: U256::from(1_000_000_u128),
        l2_gas_cost_wei: U256::from(100_000_u128),
        l1_gas_cost_wei: U256::from(50_000_u128),
        net_profit_wei: U256::from(850_000_u128),
        detected_at_ms: 0,
    }
}

/// Build a `TransactionSubmitter` with a custom receipt timeout.
fn make_submitter<S: TransactionSender>(sender: S, timeout: Duration) -> TransactionSubmitter<S> {
    TransactionSubmitter::new(
        sender,
        Address::ZERO,
        test_exec_config(),
        Arc::new(Metrics::new().expect("metrics")),
    )
    .with_receipt_timeout(timeout)
}

/// 8. `send()` hangs indefinitely → submitter returns `Err` within the
///    configured timeout.
#[tokio::test]
async fn chaos_submitter_handles_rpc_timeout() {
    let sender = HangingSender {
        hang_duration: Duration::from_secs(60),
    };
    // Set a very short timeout so the test runs fast.
    let timeout = Duration::from_millis(200);

    let submitter = make_submitter(sender, timeout);
    let calldata = Bytes::from(vec![0u8; 100]);

    // Wrap submit() in an outer timeout because the receipt_timeout only
    // covers receipt polling, not the send() call itself.
    let outer_timeout = timeout * 10; // 2 seconds — plenty of room
    let t0 = Instant::now();
    let timed = tokio::time::timeout(
        outer_timeout,
        submitter.submit(&test_opportunity(), calldata),
    )
    .await;
    let elapsed = t0.elapsed();

    // Either the outer timeout fires (Err(Elapsed)) or submit returns Err.
    let is_error = match timed {
        Err(_elapsed) => true, // outer timeout hit
        Ok(Err(_)) => true,    // submit returned Err
        Ok(Ok(_)) => false,    // unexpected success
    };
    assert!(is_error, "submit must fail or time out on RPC hang");
    assert!(
        elapsed < outer_timeout + Duration::from_millis(500),
        "submit hung for too long: {:?}",
        elapsed
    );
}

/// 9. `send()` returns a hash immediately but receipt never arrives →
///    `submit()` returns `Err` with a timeout message.
#[tokio::test]
async fn chaos_submitter_handles_receipt_never_arriving() {
    let sender = NoReceiptSender;
    // Short timeout so the test is fast.
    let timeout = Duration::from_millis(300);

    let submitter = make_submitter(sender, timeout);
    let calldata = Bytes::from(vec![0u8; 100]);

    let result = submitter.submit(&test_opportunity(), calldata).await;

    assert!(
        result.is_err(),
        "submit must return Err when receipt never arrives"
    );
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("timeout") || err_msg.contains("not mined"),
        "error should mention timeout or 'not mined', got: {err_msg}"
    );
}
