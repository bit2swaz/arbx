//! Arbitrum sequencer feed listener with automatic reconnection.
//!
//! # Architecture
//! - [`SequencerFeedManager`] owns the connection config and exposes a `run()` method
//!   that streams transactions from the Arbitrum sequencer feed indefinitely,
//!   with exponential-backoff reconnection on failure.
//! - Detected swaps are published to a [`tokio::sync::mpsc`] channel.
//! - [`BackoffCalculator`] implements the exponential-backoff delay schedule.
//!
//! # Testability
//! [`SequencerFeedManager::process_transaction`] is a pure, synchronous method
//! that can be exercised directly with crafted [`TxInfo`] inputs — no network
//! connection required.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, TxHash, U256};
use arbx_common::types::PoolState;
use tokio::sync::mpsc;

use crate::pool_state::PoolStateStore;

// ─── Selectors ───────────────────────────────────────────────────────────────

/// `IUniswapV3Pool.swap(address,bool,int256,uint160,bytes)` selector.
const UNISWAP_V3_SWAP: [u8; 4] = [0x12, 0x8a, 0xcb, 0x08];

/// `IUniswapV2Pair.swap(uint256,uint256,address,bytes)` selector.
const UNIV2_SWAP: [u8; 4] = [0x02, 0x2c, 0x0d, 0x9f];

/// All selectors we treat as potential swaps.
const KNOWN_SELECTORS: &[[u8; 4]] = &[UNISWAP_V3_SWAP, UNIV2_SWAP];

/// Swap is "large" when the estimated amount exceeds this many basis points
/// (0.1 %) of total pool reserves.
const LARGE_SWAP_THRESHOLD_BPS: u128 = 10;

// ─── TxInfo ──────────────────────────────────────────────────────────────────

/// Lightweight, owned representation of the fields needed for swap detection.
///
/// Built from an `ArbTxEnvelope` in the live path; constructed directly in
/// unit tests for full isolation from the network.
#[derive(Debug, Clone)]
pub struct TxInfo {
    /// Transaction hash.
    pub hash: TxHash,
    /// Call target (`None` for contract-creation transactions).
    pub to: Option<Address>,
    /// Raw calldata (ABI-encoded function call).
    pub input: Bytes,
}

// ─── DetectedSwap ────────────────────────────────────────────────────────────

/// A swap transaction detected on the Arbitrum sequencer feed.
#[derive(Debug, Clone)]
pub struct DetectedSwap {
    /// Transaction hash.
    pub tx_hash: TxHash,
    /// Pool contract the swap targets.
    pub pool_address: Address,
    /// 4-byte function selector extracted from calldata.
    pub selector: [u8; 4],
    /// Raw calldata including the selector.
    pub calldata: Bytes,
    /// Unix timestamp in milliseconds at which the sequencer message arrived.
    pub sequenced_at_ms: u64,
    /// `true` when estimated swap impact is > [`LARGE_SWAP_THRESHOLD_BPS`] bps
    /// of total pool reserves.
    pub is_large: bool,
}

// ─── FeedConfig ──────────────────────────────────────────────────────────────

/// Configuration for the sequencer feed connection and reconnection policy.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// WebSocket URL of the Arbitrum sequencer feed.
    pub feed_url: String,
    /// Initial reconnection delay in milliseconds.
    pub reconnect_base_ms: u64,
    /// Maximum reconnection delay in milliseconds (caps the exponential).
    pub reconnect_max_ms: u64,
    /// Delay multiplier applied after each consecutive failure.
    pub reconnect_multiplier: f64,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            feed_url: "wss://arb1-feed.arbitrum.io/feed".to_owned(),
            reconnect_base_ms: 1_000,
            reconnect_max_ms: 32_000,
            reconnect_multiplier: 2.0,
        }
    }
}

// ─── BackoffCalculator ───────────────────────────────────────────────────────

/// Stateful exponential-backoff schedule.
///
/// ```
/// use arbx_ingestion::sequencer_feed::BackoffCalculator;
/// let mut b = BackoffCalculator::new(1_000, 32_000, 2.0);
/// assert_eq!(b.next(), 1_000);
/// assert_eq!(b.next(), 2_000);
/// b.reset();
/// assert_eq!(b.next(), 1_000);
/// ```
#[derive(Debug, Clone)]
pub struct BackoffCalculator {
    base_ms: u64,
    max_ms: u64,
    multiplier: f64,
    current_ms: u64,
}

impl BackoffCalculator {
    /// Creates a new calculator, starting at `base_ms`.
    pub fn new(base_ms: u64, max_ms: u64, multiplier: f64) -> Self {
        Self {
            base_ms,
            max_ms,
            multiplier,
            current_ms: base_ms,
        }
    }

    /// Returns the **current** delay in milliseconds, then advances the
    /// internal state:
    ///
    /// `next_ms = min(current_ms × multiplier, max_ms)`
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        let delay = self.current_ms;
        self.current_ms = ((self.current_ms as f64 * self.multiplier) as u64).min(self.max_ms);
        delay
    }

    /// Resets the delay back to `base_ms` (call on a successful connection).
    pub fn reset(&mut self) {
        self.current_ms = self.base_ms;
    }
}

// ─── SequencerFeedManager ────────────────────────────────────────────────────

/// Drives the Arbitrum sequencer feed: connects, detects swaps, and forwards
/// them via a channel, reconnecting with exponential backoff on error.
pub struct SequencerFeedManager {
    config: FeedConfig,
    pool_store: PoolStateStore,
    swap_tx: mpsc::Sender<DetectedSwap>,
}

impl SequencerFeedManager {
    /// Constructs a new manager.
    pub fn new(
        config: FeedConfig,
        pool_store: PoolStateStore,
        swap_tx: mpsc::Sender<DetectedSwap>,
    ) -> Self {
        Self {
            config,
            pool_store,
            swap_tx,
        }
    }

    /// Runs the feed loop indefinitely.
    ///
    /// On stream error or clean close, reconnects using exponential backoff.
    /// Resets the backoff counter whenever a stream runs successfully.
    ///
    /// # Prerequisite
    /// The `rustls` TLS crypto provider must be installed **before** calling
    /// this method:
    ///
    /// ```ignore
    /// rustls::crypto::aws_lc_rs::default_provider().install_default().ok();
    /// ```
    pub async fn run(self) -> anyhow::Result<()> {
        let mut backoff = BackoffCalculator::new(
            self.config.reconnect_base_ms,
            self.config.reconnect_max_ms,
            self.config.reconnect_multiplier,
        );

        loop {
            match self.do_connect_and_stream().await {
                Ok(()) => {
                    tracing::info!("Feed stream closed cleanly; reconnecting immediately");
                    backoff.reset();
                }
                Err(e) => {
                    let wait_ms = backoff.next();
                    tracing::warn!(
                        error = %e,
                        wait_ms,
                        "Feed error; reconnecting after backoff"
                    );
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                }
            }
        }
    }

    /// Connects to the sequencer feed and processes messages until the stream
    /// ends or an error occurs.
    async fn do_connect_and_stream(&self) -> anyhow::Result<()> {
        use alloy::consensus::Transaction as _;
        use futures_util::StreamExt;

        let reader =
            sequencer_client::SequencerReader::new(&self.config.feed_url, 42161_u64, 1_u8).await;

        let mut stream = reader.into_stream();

        while let Some(result) = stream.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(e) => return Err(anyhow::anyhow!("stream error: {e}")),
            };

            for tx in &msg.txs {
                let tx_info = TxInfo {
                    hash: tx.hash(),
                    to: tx.to(),
                    input: tx.input().clone(),
                };

                if let Some(swap) = self.process_transaction(&tx_info) {
                    if self.swap_tx.send(swap).await.is_err() {
                        tracing::warn!("Swap channel closed; stopping feed");
                        return Err(anyhow::anyhow!("swap sender channel closed"));
                    }
                }
            }
        }

        Ok(())
    }

    /// Inspects a single transaction and returns a [`DetectedSwap`] when it
    /// targets a tracked pool with a known DEX selector.
    ///
    /// This is a **pure** method with no I/O, designed for unit testing.
    pub fn process_transaction(&self, tx: &TxInfo) -> Option<DetectedSwap> {
        // 1. Must be a call, not a contract creation.
        let to = tx.to?;

        // 2. Target must be a pool we are tracking.
        let pool_state = self.pool_store.get(&to)?;

        // 3. Calldata must contain at least a 4-byte selector.
        if tx.input.len() < 4 {
            return None;
        }
        let selector: [u8; 4] = tx.input[0..4].try_into().ok()?;

        // 4. Selector must correspond to a supported DEX.
        if !KNOWN_SELECTORS.contains(&selector) {
            return None;
        }

        // 5. Estimate whether this is a large swap.
        let is_large = compute_is_large(&tx.input, &pool_state, &selector);

        Some(DetectedSwap {
            tx_hash: tx.hash,
            pool_address: to,
            selector,
            calldata: tx.input.clone(),
            sequenced_at_ms: now_ms(),
            is_large,
        })
    }
}

// ─── Private Helpers ─────────────────────────────────────────────────────────

/// Returns `true` when the estimated swap size exceeds
/// [`LARGE_SWAP_THRESHOLD_BPS`] of total pool reserves.
///
/// * **UniswapV3** — `amountSpecified` (int256) is at calldata offset 68.
/// * **UniswapV2** — largest of `amount0Out` / `amount1Out` at offsets 4 / 36.
fn compute_is_large(input: &Bytes, pool: &PoolState, selector: &[u8; 4]) -> bool {
    let total_reserves = pool.reserve0.saturating_add(pool.reserve1);
    if total_reserves.is_zero() {
        return false;
    }

    let amount: U256 = if selector == &UNISWAP_V3_SWAP {
        // calldata layout: selector(4) | recipient(32) | zeroForOne(32) | amountSpecified(32) | …
        if input.len() < 100 {
            return false;
        }
        let raw_bytes: [u8; 32] = input[68..100].try_into().unwrap();
        let raw = U256::from_be_bytes(raw_bytes);
        // amountSpecified is int256; if the MSB is set it is negative (exact-output).
        // Take two's-complement absolute value so we still get a magnitude.
        if raw_bytes[0] & 0x80 != 0 {
            U256::MAX - raw + U256::from(1u64)
        } else {
            raw
        }
    } else {
        // V2 layout: selector(4) | amount0Out(32) | amount1Out(32) | …
        if input.len() < 68 {
            return false;
        }
        let b0: [u8; 32] = input[4..36].try_into().unwrap();
        let b1: [u8; 32] = input[36..68].try_into().unwrap();
        U256::from_be_bytes(b0).max(U256::from_be_bytes(b1))
    };

    // is_large ⟺ amount / total_reserves > threshold_bps / 10_000
    //           ⟺ amount × 10_000 > total_reserves × threshold_bps
    amount * U256::from(10_000u128) > total_reserves * U256::from(LARGE_SWAP_THRESHOLD_BPS)
}

/// Current Unix time in milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use arbx_common::types::{DexKind, PoolState};
    use tokio::sync::mpsc;

    // ── test helpers ─────────────────────────────────────────────────────────

    fn pool_addr() -> Address {
        Address::from([0x42u8; 20])
    }

    fn make_pool(address: Address, reserve0: u128, reserve1: u128) -> PoolState {
        PoolState {
            address,
            token0: Address::ZERO,
            token1: Address::ZERO,
            reserve0: U256::from(reserve0),
            reserve1: U256::from(reserve1),
            fee_tier: 3000,
            last_updated_block: 0,
            dex: DexKind::UniswapV3,
        }
    }

    fn make_store(address: Address, reserve0: u128, reserve1: u128) -> PoolStateStore {
        let store = PoolStateStore::new();
        store.upsert(make_pool(address, reserve0, reserve1));
        store
    }

    fn make_manager(store: PoolStateStore) -> (SequencerFeedManager, mpsc::Receiver<DetectedSwap>) {
        let (tx, rx) = mpsc::channel(64);
        let mgr = SequencerFeedManager::new(FeedConfig::default(), store, tx);
        (mgr, rx)
    }

    fn make_tx(to: Option<Address>, input: Vec<u8>) -> TxInfo {
        TxInfo {
            hash: alloy::primitives::B256::ZERO,
            to,
            input: Bytes::from(input),
        }
    }

    /// Build UniswapV3 calldata with `amount_specified` placed at offset 68.
    fn v3_calldata(amount_specified: u128) -> Vec<u8> {
        let mut data = Vec::with_capacity(164);
        data.extend_from_slice(&UNISWAP_V3_SWAP); // selector      [0..4)
        data.extend_from_slice(&[0u8; 32]); //       recipient    [4..36)
        data.extend_from_slice(&[0u8; 32]); //       zeroForOne   [36..68)
        let mut slot = [0u8; 32]; //                 amountSpec   [68..100)
        slot[16..].copy_from_slice(&amount_specified.to_be_bytes());
        data.extend_from_slice(&slot);
        data.extend_from_slice(&[0u8; 64]); //       remainder
        data
    }

    /// Build UniswapV2 calldata with `amount0Out` at offset 4 and
    /// `amount1Out` at offset 36.
    fn v2_calldata(amount0_out: u128, amount1_out: u128) -> Vec<u8> {
        let mut data = Vec::with_capacity(132);
        data.extend_from_slice(&UNIV2_SWAP); // selector       [0..4)
        let mut b0 = [0u8; 32]; //             amount0Out     [4..36)
        b0[16..].copy_from_slice(&amount0_out.to_be_bytes());
        data.extend_from_slice(&b0);
        let mut b1 = [0u8; 32]; //             amount1Out     [36..68)
        b1[16..].copy_from_slice(&amount1_out.to_be_bytes());
        data.extend_from_slice(&b1);
        data.extend_from_slice(&[0u8; 64]); // remainder
        data
    }

    // ── swap-detection tests ──────────────────────────────────────────────────

    #[test]
    fn test_process_tx_detects_univ3_swap() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        let result = mgr.process_transaction(&make_tx(Some(addr), v3_calldata(100)));

        assert!(result.is_some(), "expected Some(DetectedSwap) for V3 swap");
        assert_eq!(result.unwrap().selector, UNISWAP_V3_SWAP);
    }

    #[test]
    fn test_process_tx_detects_univ2_swap() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        let result = mgr.process_transaction(&make_tx(Some(addr), v2_calldata(100, 0)));

        assert!(result.is_some(), "expected Some(DetectedSwap) for V2 swap");
        assert_eq!(result.unwrap().selector, UNIV2_SWAP);
    }

    #[test]
    fn test_process_tx_ignores_non_pool_address() {
        let addr = pool_addr();
        let other = Address::from([0x99u8; 20]);
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        assert!(
            mgr.process_transaction(&make_tx(Some(other), v3_calldata(100)))
                .is_none(),
            "should ignore address not in pool store"
        );
    }

    #[test]
    fn test_process_tx_ignores_unknown_selector() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        let mut data = vec![0xde, 0xad, 0xbe, 0xef];
        data.extend_from_slice(&[0u8; 96]);

        assert!(
            mgr.process_transaction(&make_tx(Some(addr), data))
                .is_none(),
            "should ignore unknown selector"
        );
    }

    #[test]
    fn test_process_tx_ignores_contract_creation() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        // `to = None` signals a contract-creation transaction.
        assert!(
            mgr.process_transaction(&make_tx(None, v3_calldata(100)))
                .is_none(),
            "should ignore contract creation (to = None)"
        );
    }

    #[test]
    fn test_process_tx_ignores_short_calldata() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        // Only 2 bytes — not enough for a 4-byte selector.
        assert!(
            mgr.process_transaction(&make_tx(Some(addr), vec![0x12, 0x8a]))
                .is_none(),
            "should ignore calldata shorter than 4 bytes"
        );
    }

    #[test]
    fn test_process_tx_sets_correct_timestamp() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        let swap = mgr
            .process_transaction(&make_tx(Some(addr), v3_calldata(100)))
            .expect("should detect swap");

        assert!(swap.sequenced_at_ms > 0, "timestamp must be non-zero");
    }

    #[test]
    fn test_process_tx_records_correct_fields() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000_000, 1_000_000));

        let calldata = v2_calldata(50, 0);
        let swap = mgr
            .process_transaction(&make_tx(Some(addr), calldata.clone()))
            .expect("should detect swap");

        assert_eq!(swap.pool_address, addr);
        assert_eq!(swap.selector, UNIV2_SWAP);
        assert_eq!(swap.calldata.as_ref(), calldata.as_slice());
        assert_eq!(swap.tx_hash, alloy::primitives::B256::ZERO);
    }

    #[test]
    fn test_process_tx_large_swap_detected() {
        let addr = pool_addr();
        // total reserves = 1_000; 1% of 1_000 = 10 → is_large
        let (mgr, _rx) = make_manager(make_store(addr, 1_000, 0));

        let swap = mgr
            .process_transaction(&make_tx(Some(addr), v2_calldata(10, 0)))
            .expect("should detect swap");

        assert!(
            swap.is_large,
            "amount=10 against reserves=1000 (1%) should be large"
        );
    }

    #[test]
    fn test_process_tx_small_swap_not_large() {
        let addr = pool_addr();
        let (mgr, _rx) = make_manager(make_store(addr, 1_000, 0));

        let swap = mgr
            .process_transaction(&make_tx(Some(addr), v2_calldata(0, 0)))
            .expect("should detect swap");

        assert!(!swap.is_large, "zero-amount swap should not be large");
    }

    // ── backoff tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_backoff_doubles_each_failure() {
        let mut b = BackoffCalculator::new(1_000, 32_000, 2.0);
        assert_eq!(b.next(), 1_000);
        assert_eq!(b.next(), 2_000);
        assert_eq!(b.next(), 4_000);
        assert_eq!(b.next(), 8_000);
        assert_eq!(b.next(), 16_000);
    }

    #[test]
    fn test_backoff_caps_at_max() {
        let mut b = BackoffCalculator::new(1_000, 32_000, 2.0);
        for i in 0..10 {
            let delay = b.next();
            assert!(
                delay <= 32_000,
                "iteration {i}: delay {delay} exceeded max 32_000"
            );
        }
    }

    #[test]
    fn test_backoff_resets_on_success() {
        let mut b = BackoffCalculator::new(1_000, 32_000, 2.0);
        b.next(); // 1_000 → current becomes 2_000
        b.next(); // 2_000 → current becomes 4_000
        b.reset();
        assert_eq!(b.next(), 1_000, "after reset, next() must return base_ms");
    }
}
