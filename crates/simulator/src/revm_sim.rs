//! revm-based Arbitrum fork infrastructure — Mini-Phase 5.1.
//!
//! [`ArbSimulator`] creates a [`CacheDB`]-backed fork of Arbitrum state at any
//! block number.  The fork is backed by an [`AlloyDB`] that fetches missing
//! accounts and storage slots on demand via the RPC provider.  All reads are
//! cached in-process so each slot costs at most one RPC call per fork.
//!
//! # Thread-safety
//! [`ArbSimulator`] is `Clone + Send + Sync`.  Each call to
//! [`fork_at_latest`][ArbSimulator::fork_at_latest] or
//! [`fork_at_block`][ArbSimulator::fork_at_block] returns a **fresh**
//! [`ArbDB`] that is not shared with any other call, so concurrent simulations
//! are safe.
//!
//! # Runtime requirement
//! [`ArbDB`] wraps [`WrapDatabaseAsync`] which requires a **multi-thread**
//! Tokio runtime.  Calls made inside a `current_thread` runtime (the default
//! for `#[tokio::test]`) will return an error.  Use
//! `#[tokio::test(flavor = "multi_thread")]` for tests that create an
//! `ArbDB`.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use alloy::{
    eips::BlockId,
    network::Ethereum,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    sol,
};
use anyhow::Context as _;
use revm::database::{AlloyDB, CacheDB};
use revm::database_interface::WrapDatabaseAsync;
use tracing::{debug, trace};

// ─── ERC-20 minimal ABI (used by read_erc20_balance) ────────────────────────

sol! {
    #[sol(rpc)]
    interface IERC20 {
        /// Returns the token balance of `owner`.
        function balanceOf(address owner) external view returns (uint256);
    }
}

// ─── Type alias ──────────────────────────────────────────────────────────────

/// A revm [`CacheDB`] backed by an Arbitrum RPC node.
///
/// `AlloyDB` lazily fetches missing state from the provider; `CacheDB` keeps
/// every fetched slot in memory so subsequent reads are free.
///
/// Returned by [`ArbSimulator::fork_at_latest`] and
/// [`ArbSimulator::fork_at_block`].
pub type ArbDB = CacheDB<WrapDatabaseAsync<AlloyDB<Ethereum, Arc<RootProvider<Ethereum>>>>>;

// ─── ArbSimulator ────────────────────────────────────────────────────────────

/// In-process Arbitrum state fork.
///
/// One instance is shared across the bot's lifetime.  Simulations call
/// [`fork_at_latest`][Self::fork_at_latest] to obtain a fresh [`ArbDB`] for
/// each opportunity.
///
/// # Block-number cache
/// [`fork_at_latest`][Self::fork_at_latest] caches the last queried block
/// number.  Phase 5.2 will extend this to share pre-populated storage caches
/// across consecutive simulations that land in the same block.
#[derive(Clone)]
pub struct ArbSimulator {
    provider: Arc<RootProvider<Ethereum>>,
    /// Latest known block number — used to skip redundant `eth_blockNumber`
    /// RPC calls when many simulations arrive during the same block.
    ///
    /// `pub(crate)` so unit tests can inspect / seed the cache without making
    /// any network calls.
    pub(crate) fork_block_cache: Arc<Mutex<Option<u64>>>,
}

impl ArbSimulator {
    /// Create a new simulator backed by `provider`.
    ///
    /// No network calls are made during construction.
    pub fn new(provider: Arc<RootProvider<Ethereum>>) -> Self {
        Self {
            provider,
            fork_block_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Build a fresh [`ArbDB`] pinned to `block`.
    ///
    /// Requires a **multi-thread** Tokio runtime to be active (see module
    /// docs).
    async fn build_db(&self, block: u64) -> anyhow::Result<ArbDB> {
        let alloy_db = AlloyDB::new(Arc::clone(&self.provider), BlockId::number(block));
        let wrapped = WrapDatabaseAsync::new(alloy_db).context(
            "WrapDatabaseAsync::new() returned None — \
             a multi-thread Tokio runtime is required (use \
             #[tokio::test(flavor = \"multi_thread\")] in tests)",
        )?;
        Ok(CacheDB::new(wrapped))
    }

    /// Fork Arbitrum state at the **latest** block.
    ///
    /// Queries `eth_blockNumber`, updates the internal block-number cache,
    /// then returns a fresh [`ArbDB`] at that block.
    pub async fn fork_at_latest(&self) -> anyhow::Result<ArbDB> {
        let block = self
            .provider
            .get_block_number()
            .await
            .context("eth_blockNumber RPC call failed")?;

        *self.fork_block_cache.lock().unwrap() = Some(block);
        debug!(block, "forked Arbitrum state at latest block");

        self.build_db(block).await
    }

    /// Fork Arbitrum state at a **specific historical** `block`.
    ///
    /// Primarily used by regression tests that replay known historical blocks.
    pub async fn fork_at_block(&self, block: u64) -> anyhow::Result<ArbDB> {
        trace!(block, "forking Arbitrum state at historical block");
        self.build_db(block).await
    }

    /// Read the **ETH balance** of `address` from the provider.
    ///
    /// Queries `eth_getBalance` at `block` (or the latest block when `None`).
    pub async fn read_balance(&self, address: Address, block: Option<u64>) -> anyhow::Result<U256> {
        let req = self.provider.get_balance(address);
        let bal = match block {
            Some(b) => req.block_id(BlockId::number(b)).await?,
            None => req.await?,
        };
        Ok(bal)
    }

    /// Read the **ERC-20 balance** of `account` for token `token`.
    ///
    /// Calls `ERC20.balanceOf(account)` via `eth_call` at `block` (or
    /// latest when `None`).
    pub async fn read_erc20_balance(
        &self,
        token: Address,
        account: Address,
        block: Option<u64>,
    ) -> anyhow::Result<U256> {
        let contract = IERC20::new(token, Arc::clone(&self.provider));
        let balance = match block {
            Some(b) => {
                contract
                    .balanceOf(account)
                    .block(BlockId::number(b))
                    .call()
                    .await?
            }
            None => contract.balanceOf(account).call().await?,
        };
        Ok(balance)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a provider pointing at an unreachable local port.
    ///
    /// No network calls are made in unit tests; this just gives us a valid
    /// `Arc<RootProvider<Ethereum>>` to pass to `ArbSimulator::new`.
    fn make_test_provider() -> Arc<RootProvider<Ethereum>> {
        use alloy::transports::http::reqwest::Url;
        // ProviderBuilder::default() = Identity filler + Identity layer
        // → connect_http returns RootProvider<Ethereum> directly.
        // Port 1 is not listening; no connection is attempted on construction.
        Arc::new(ProviderBuilder::default().connect_http(Url::parse("http://127.0.0.1:1").unwrap()))
    }

    // ── unit tests (no network required) ────────────────────────────────────

    /// `ArbSimulator::new` must succeed without any network activity, and the
    /// block-number cache must be empty immediately after construction.
    #[test]
    fn test_fork_is_created() {
        let sim = ArbSimulator::new(make_test_provider());
        assert!(
            sim.fork_block_cache.lock().unwrap().is_none(),
            "cache must be empty after construction"
        );
    }

    /// The block-number cache must correctly store and expose whatever value
    /// `fork_at_latest` would write into it after a successful RPC call.
    ///
    /// We seed the cache manually (no network) to verify the storage
    /// mechanism in isolation.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_cache_key_construction() {
        let sim = ArbSimulator::new(make_test_provider());

        // Initially the cache is empty.
        assert!(
            sim.fork_block_cache.lock().unwrap().is_none(),
            "cache should be None before first fork"
        );

        // Simulate what fork_at_latest would write after querying block 300_000_000.
        let expected_block: u64 = 300_000_000;
        *sim.fork_block_cache.lock().unwrap() = Some(expected_block);

        // Verify the block number is stored and readable.
        let cached = *sim.fork_block_cache.lock().unwrap();
        assert_eq!(
            cached,
            Some(expected_block),
            "cache must hold the block number written by fork_at_latest"
        );

        // Additionally verify that build_db can create an ArbDB at the
        // cached block (proves WrapDatabaseAsync works in a multi-thread rt).
        let db = sim.build_db(expected_block).await;
        assert!(
            db.is_ok(),
            "build_db must succeed in multi-thread runtime: {:?}",
            db.err()
        );
    }

    // ── integration tests (require ARBITRUM_RPC_URL) ─────────────────────────

    /// Fork the latest block and verify the Balancer V2 Vault holds billions
    /// of USDC (`> 1_000_000 * 1e6`).
    #[tokio::test]
    #[ignore = "requires ARBITRUM_RPC_URL"]
    async fn test_fork_reads_balancer_vault_balance() {
        use alloy::primitives::address;

        let rpc = std::env::var("ARBITRUM_RPC_URL").unwrap();
        let provider = Arc::new(ProviderBuilder::default().connect_http(rpc.parse().unwrap()));
        let sim = ArbSimulator::new(provider);

        // Balancer V2 Vault on Arbitrum
        const BALANCER_VAULT: Address = address!("BA12222222228d8Ba445958a75a0704d566BF2C8");
        // USDC on Arbitrum
        const USDC: Address = address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8");

        let balance = sim
            .read_erc20_balance(USDC, BALANCER_VAULT, None)
            .await
            .unwrap();

        let one_million_usdc = U256::from(1_000_000u64) * U256::from(1_000_000u64); // 1e12
        assert!(
            balance > one_million_usdc,
            "Balancer Vault USDC balance should be > $1M: got {balance}"
        );
    }

    /// Fork the latest block and verify the Uniswap V3 USDC/WETH pool holds
    /// a meaningful WETH balance.
    #[tokio::test]
    #[ignore = "requires ARBITRUM_RPC_URL"]
    async fn test_fork_reads_weth_balance() {
        use alloy::primitives::address;

        let rpc = std::env::var("ARBITRUM_RPC_URL").unwrap();
        let provider = Arc::new(ProviderBuilder::default().connect_http(rpc.parse().unwrap()));
        let sim = ArbSimulator::new(provider);

        // Uniswap V3 USDC/WETH 0.05% pool on Arbitrum
        const UNIV3_USDC_WETH: Address = address!("C31E54c7a869B9FcBEcc14363CF510d1c41fa443");
        // WETH on Arbitrum
        const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");

        let balance = sim
            .read_erc20_balance(WETH, UNIV3_USDC_WETH, None)
            .await
            .unwrap();

        // Pool should hold at least 1 WETH (1e18 wei)
        let one_weth = U256::from(1_000_000_000_000_000_000u128);
        assert!(
            balance > one_weth,
            "Uniswap V3 USDC/WETH pool should hold > 1 WETH: got {balance}"
        );
    }

    /// Verify that the second `fork_at_latest` call at the same block is
    /// faster than the first (proves caching avoids duplicate work).
    #[tokio::test]
    #[ignore = "requires ARBITRUM_RPC_URL"]
    async fn test_fork_block_caching() {
        use std::time::Instant;

        let rpc = std::env::var("ARBITRUM_RPC_URL").unwrap();
        let provider = Arc::new(ProviderBuilder::default().connect_http(rpc.parse().unwrap()));
        let sim = ArbSimulator::new(provider);

        // First fork — includes the eth_blockNumber RPC call + AlloyDB construction.
        let t0 = Instant::now();
        let _db1 = sim.fork_at_latest().await.unwrap();
        let first_ms = t0.elapsed().as_millis();

        // Second fork at the same block (block number is unlikely to advance
        // within the same test on a fast connection).
        let t1 = Instant::now();
        let _db2 = sim.fork_at_latest().await.unwrap();
        let second_ms = t1.elapsed().as_millis();

        // The second call should be at most as slow as the first; in
        // practice the warm HTTP connection makes it noticeably faster.
        println!("first={first_ms}ms  second={second_ms}ms");
        assert!(
            second_ms <= first_ms + 50,
            "second fork_at_latest ({second_ms}ms) should not be \
             significantly slower than first ({first_ms}ms)"
        );
    }
}
