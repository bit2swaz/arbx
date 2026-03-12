//! Integration test helpers: fixture builders and hand-coded stub
//! implementations of the GasFetcher / TransactionSender traits.
//!
//! These stubs replace the `#[cfg(test)]`-gated mockall mocks that live inside
//! each crate and are therefore not accessible at workspace integration level.

// Helpers are a shared fixture library; not every item is used by every test.
#![allow(dead_code)]

use std::sync::Arc;

use alloy::primitives::{Address, Bytes, TxHash, I256, U256};
use alloy::rpc::types::TransactionReceipt;
use async_trait::async_trait;

use arbx_common::{
    config::{
        Config, ExecutionConfig, NetworkConfig, ObservabilityConfig, PoolsConfig, StrategyConfig,
    },
    types::{ArbPath, DexKind, Opportunity, PoolState, SubmissionResult},
};
use arbx_detector::profit::GasFetcher;
use arbx_executor::submitter::TransactionSender;
use arbx_ingestion::pool_state::PoolStateStore;

/// Returns a path string for a fresh (non-existent) temporary file.
///
/// Uses a `tempfile::TempDir` to get a temp directory, then constructs a
/// path to `pnl.json` inside it.  The returned `(TempDir, String)` pair
/// keeps the directory alive for the lifetime of the test.
pub fn temp_pnl_path() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("pnl.json");
    let path_str = path.to_string_lossy().into_owned();
    (dir, path_str)
}

/// Build a deterministic Address from a single seed byte.
pub fn addr(seed: u8) -> Address {
    Address::from([seed; 20])
}

// ─── Config fixture ──────────────────────────────────────────────────────────

/// Returns a fully-populated [`Config`] that does not touch env vars or disk.
pub fn make_test_config() -> Config {
    Config {
        network: NetworkConfig {
            rpc_url: "http://127.0.0.1:8545".to_owned(),
            sequencer_feed_url: "wss://127.0.0.1:9876/feed".to_owned(),
            chain_id: 42161,
        },
        strategy: StrategyConfig {
            min_profit_floor_usd: 0.10,
            gas_buffer_multiplier: 1.1,
            max_gas_gwei: 1.0,
            flash_loan_fee_bps: 0,
        },
        pools: PoolsConfig {
            balancer_vault: format!("{}", addr(0xBB)),
            uniswap_v3_factory: format!("{}", addr(0xB1)),
            camelot_factory: format!("{}", addr(0xB2)),
            sushiswap_factory: format!("{}", addr(0xB3)),
            traderjoe_factory: format!("{}", addr(0xB4)),
            seed_from_block: 0,
            known_pools: vec![],
        },
        execution: ExecutionConfig {
            contract_address: format!("{}", addr(0xCE)),
            // A dummy 64-char hex private key (test-only)
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_owned(),
            max_concurrent_simulations: 3,
            gas_estimate_buffer: 1.25,
            node_interface_address: "0x00000000000000000000000000000000000000C8".to_owned(),
            l2_gas_units: 500_000,
            receipt_timeout_secs: 30,
            dry_run: false,
        },
        observability: ObservabilityConfig {
            log_level: "debug".to_owned(),
            metrics_port: 9090,
        },
    }
}

// ─── PoolStateStore fixture ───────────────────────────────────────────────────

/// Returns a [`PoolStateStore`] with two pools that form an exploitable two-hop
/// cycle: USDC → pool_a → ETH → pool_b → USDC.
///
/// Pool A has an inverted price (cheap ETH) so the scanner can find profit.
pub fn make_pool_state_store_with_known_pools() -> PoolStateStore {
    let usdc = addr(0x01);
    let eth = addr(0x02);
    let pool_a_addr = addr(0x10);
    let pool_b_addr = addr(0x11);

    let pool_a = PoolState {
        address: pool_a_addr,
        token0: usdc,
        token1: eth,
        // Inverted reserves: lots of ETH, little USDC → ETH is cheap here
        reserve0: U256::from(1_000_000_000_000_000_000_u128), // 1 ETH equivalent of USDC (small)
        reserve1: U256::from(100_000_000_000_000_000_000_u128), // 100 ETH (large)
        fee_tier: 3000,
        last_updated_block: 100,
        dex: DexKind::CamelotV2,
    };

    let pool_b = PoolState {
        address: pool_b_addr,
        token0: eth,
        token1: usdc,
        // Normal reserves: 1:2000 ETH/USDC
        reserve0: U256::from(10_000_000_000_000_000_000_u128), // 10 ETH
        reserve1: U256::from(20_000_000_000_000_000_000_000_u128), // 20 000 USDC
        fee_tier: 3000,
        last_updated_block: 100,
        dex: DexKind::SushiSwap,
    };

    let store = PoolStateStore::new();
    store.upsert(pool_a);
    store.upsert(pool_b);
    store
}

/// Returns a [`PoolStateStore`] with two balanced pools (no meaningful price
/// dislocation) — the estimated profit for any path will be near zero, ensuring
/// the profit filter rejects everything even with modest gas costs.
pub fn make_balanced_pool_state_store() -> PoolStateStore {
    let usdc = addr(0x01);
    let eth = addr(0x02);
    let pool_a_addr = addr(0x20);
    let pool_b_addr = addr(0x21);

    let pool_a = PoolState {
        address: pool_a_addr,
        token0: usdc,
        token1: eth,
        // Balanced: each 1 USDC ≈ 1 ETH (same reserves)
        reserve0: U256::from(1_000_000_u128),
        reserve1: U256::from(1_000_000_u128),
        fee_tier: 3000,
        last_updated_block: 100,
        dex: DexKind::CamelotV2,
    };

    let pool_b = PoolState {
        address: pool_b_addr,
        token0: eth,
        token1: usdc,
        // Balanced: same reserves
        reserve0: U256::from(1_000_000_u128),
        reserve1: U256::from(1_000_000_u128),
        fee_tier: 3000,
        last_updated_block: 100,
        dex: DexKind::SushiSwap,
    };

    let store = PoolStateStore::new();
    store.upsert(pool_a);
    store.upsert(pool_b);
    store
}

// ─── Opportunity fixture ─────────────────────────────────────────────────────

/// Returns a minimal [`Opportunity`] with a 0.1 ETH gross profit and zero gas.
pub fn make_test_opportunity() -> Opportunity {
    let usdc = addr(0x01);
    let eth = addr(0x02);
    let path = ArbPath {
        token_in: usdc,
        pool_a: addr(0x10),
        token_mid: eth,
        pool_b: addr(0x11),
        token_out: usdc,
        estimated_profit_wei: U256::from(100_000_000_000_000_000_u128), // 0.1 ETH
        flash_loan_amount_wei: U256::from(1_000_000_000_000_000_000_u128), // 1 ETH
    };
    Opportunity {
        path,
        gross_profit_wei: U256::from(100_000_000_000_000_000_u128),
        l2_gas_cost_wei: U256::ZERO,
        l1_gas_cost_wei: U256::ZERO,
        net_profit_wei: U256::from(100_000_000_000_000_000_u128),
        detected_at_ms: 0,
    }
}

// ─── SubmissionResult fixtures ───────────────────────────────────────────────

/// Returns a successful [`SubmissionResult`] with a positive net PnL.
pub fn make_profitable_submission_result() -> SubmissionResult {
    SubmissionResult {
        tx_hash: TxHash::from([0xAA; 32]),
        success: true,
        revert_reason: None,
        gas_used: 300_000,
        // Small gas cost — less than the profit
        l2_gas_cost_wei: U256::from(1_000_000_000_000_000_u128), // 0.001 ETH
        l1_gas_cost_wei: U256::from(500_000_000_000_000_u128),   // 0.0005 ETH
        net_pnl_wei: I256::try_from(U256::from(98_500_000_000_000_000_u128)).unwrap(),
    }
}

/// Returns a reverted [`SubmissionResult`] with a given revert reason.
pub fn make_reverted_submission_result(reason: &str) -> SubmissionResult {
    SubmissionResult {
        tx_hash: TxHash::from([0xBB; 32]),
        success: false,
        revert_reason: Some(reason.to_owned()),
        gas_used: 50_000,
        l2_gas_cost_wei: U256::from(200_000_000_000_000_u128), // 0.0002 ETH
        l1_gas_cost_wei: U256::from(100_000_000_000_000_u128), // 0.0001 ETH
        net_pnl_wei: I256::ZERO - I256::try_from(U256::from(300_000_000_000_000_u128)).unwrap(),
    }
}

// ─── FixedGasFetcher ─────────────────────────────────────────────────────────

/// A [`GasFetcher`] stub that returns predetermined constant values.
///
/// Used in integration tests where network access is unavailable.
pub struct FixedGasFetcher {
    /// L2 execution gas price in wei.
    pub l2_price_wei: u128,
    /// L1 base fee in wei.
    pub l1_base_fee_wei: u128,
    /// Gas units returned by estimate_l2_gas.
    pub l2_gas_units: u64,
    /// Gas units returned by estimate_l1_gas.
    pub l1_gas_units: u64,
    /// Cached ETH/USD price.
    pub eth_price: f64,
}

impl FixedGasFetcher {
    /// Cheap gas: ~$0.002 total for a typical arb tx (far below profit floors).
    pub fn cheap() -> Self {
        Self {
            l2_price_wei: 100_000_000,       // 0.1 gwei
            l1_base_fee_wei: 10_000_000_000, // 10 gwei (L1 base)
            l2_gas_units: 500_000,
            l1_gas_units: 1_000,
            eth_price: 3_000.0,
        }
    }

    /// Astronomically expensive gas: ~$10 000 per tx (guaranteed to be
    /// rejected by any sane profit filter).
    pub fn prohibitive() -> Self {
        Self {
            l2_price_wei: 1_000_000_000_000_000, // very high
            l1_base_fee_wei: 1_000_000_000_000_000,
            l2_gas_units: 500_000,
            l1_gas_units: 1_000,
            eth_price: 3_000.0,
        }
    }
}

#[async_trait]
impl GasFetcher for FixedGasFetcher {
    async fn l2_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(self.l2_price_wei)
    }

    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(self.l2_gas_units)
    }

    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(self.l1_gas_units)
    }

    async fn l1_base_fee_wei(&self) -> anyhow::Result<u128> {
        Ok(self.l1_base_fee_wei)
    }

    fn eth_price_usd(&self) -> f64 {
        self.eth_price
    }
}

// ─── PanickingTransactionSender ──────────────────────────────────────────────

/// A [`TransactionSender`] that panics if any method is called.
///
/// Used in dry-run tests to prove the submitter is never touched.
pub struct PanickingTransactionSender;

#[async_trait]
impl TransactionSender for PanickingTransactionSender {
    async fn send(
        &self,
        _calldata: Bytes,
        _to: Address,
        _gas_limit: u64,
        _gas_price_wei: u128,
    ) -> anyhow::Result<TxHash> {
        panic!("PanickingTransactionSender::send must not be called in dry-run mode")
    }

    async fn get_receipt(&self, _tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        panic!("PanickingTransactionSender::get_receipt must not be called in dry-run mode")
    }

    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        panic!(
            "PanickingTransactionSender::current_gas_price_wei must not be called in dry-run mode"
        )
    }

    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        panic!("PanickingTransactionSender::estimate_l1_gas must not be called in dry-run mode")
    }

    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        panic!("PanickingTransactionSender::estimate_l2_gas must not be called in dry-run mode")
    }

    async fn call_for_revert(&self, _to: Address, _data: Bytes) -> anyhow::Result<Bytes> {
        panic!("PanickingTransactionSender::call_for_revert must not be called in dry-run mode")
    }
}

// ─── AlwaysSucceedingSender ───────────────────────────────────────────────────

/// A [`TransactionSender`] that succeeds all calls with preset values.
pub struct AlwaysSucceedingSender {
    pub tx_hash: TxHash,
}

impl AlwaysSucceedingSender {
    pub fn new() -> Self {
        Self {
            tx_hash: TxHash::from([0xCC; 32]),
        }
    }
}

#[async_trait]
impl TransactionSender for AlwaysSucceedingSender {
    async fn send(
        &self,
        _calldata: Bytes,
        _to: Address,
        _gas_limit: u64,
        _gas_price_wei: u128,
    ) -> anyhow::Result<TxHash> {
        Ok(self.tx_hash)
    }

    async fn get_receipt(&self, _tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        use alloy::primitives::Bloom;
        use alloy::rpc::types::TransactionReceipt;

        let receipt = TransactionReceipt {
            inner: alloy::consensus::ReceiptEnvelope::Legacy(alloy::consensus::ReceiptWithBloom {
                receipt: alloy::consensus::Receipt {
                    status: alloy::consensus::Eip658Value::Eip658(true),
                    cumulative_gas_used: 500_000,
                    logs: vec![],
                },
                logs_bloom: Bloom::ZERO,
            }),
            transaction_hash: self.tx_hash,
            transaction_index: Some(0),
            block_hash: None,
            block_number: Some(1),
            gas_used: 500_000,
            effective_gas_price: 1_000_000_000,
            blob_gas_used: None,
            blob_gas_price: None,
            from: Address::ZERO,
            to: None,
            contract_address: None,
        };
        Ok(Some(receipt))
    }

    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(1_000_000_000) // 1 gwei
    }

    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(1_000)
    }

    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(500_000)
    }

    async fn call_for_revert(&self, _to: Address, _data: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }
}

// ─── CountingTransactionSender ────────────────────────────────────────────────

/// A [`TransactionSender`] that counts concurrent `send` calls.
///
/// Used to verify the concurrency semaphore actually caps parallel submissions.
pub struct CountingTransactionSender {
    pub concurrent: Arc<std::sync::atomic::AtomicUsize>,
    pub max_observed: Arc<std::sync::atomic::AtomicUsize>,
    pub delay_ms: u64,
}

impl CountingTransactionSender {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            concurrent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            max_observed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            delay_ms,
        }
    }
}

#[async_trait]
impl TransactionSender for CountingTransactionSender {
    async fn send(
        &self,
        _calldata: Bytes,
        _to: Address,
        _gas_limit: u64,
        _gas_price_wei: u128,
    ) -> anyhow::Result<TxHash> {
        use std::sync::atomic::Ordering;
        let cur = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        // Track the high-water mark of concurrent executions
        let _ = self
            .max_observed
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |prev| {
                if cur > prev {
                    Some(cur)
                } else {
                    None
                }
            });
        tokio::time::sleep(tokio::time::Duration::from_millis(self.delay_ms)).await;
        self.concurrent.fetch_sub(1, Ordering::SeqCst);
        Ok(TxHash::from([0xDD; 32]))
    }

    async fn get_receipt(&self, _tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>> {
        Ok(None)
    }

    async fn current_gas_price_wei(&self) -> anyhow::Result<u128> {
        Ok(1_000_000_000)
    }

    async fn estimate_l1_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(1_000)
    }

    async fn estimate_l2_gas(&self, _to: Address, _data: Bytes) -> anyhow::Result<u64> {
        Ok(500_000)
    }

    async fn call_for_revert(&self, _to: Address, _data: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }
}
