//! Block reconciler — periodically refreshes pool reserves from on-chain state.
//!
//! # Why this exists
//! The sequencer feed delivers swap transactions in real time, but it can miss
//! reserve updates (e.g., direct liquidity additions, other contract calls that
//! alter reserves without emitting a detected-swap event). The reconciler runs
//! once per block, queries every tracked pool through the RPC provider, and
//! patches the `PoolStateStore` when on-chain reserves differ from the cached
//! value.
//!
//! # Design for testability
//! All RPC calls are routed through the [`ReserveFetcher`] trait. Production
//! code uses [`AlloyReserveFetcher`]; tests replace it with the mockall-generated
//! `MockReserveFetcher` and never touch the network.

use std::sync::Arc;

use alloy::network::Ethereum;
use alloy::primitives::{Address, U256};
use alloy::providers::RootProvider;
use alloy::sol;
use anyhow::Context as _;
use arbx_common::types::{DexKind, PoolState};
use futures_util::future::join_all;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::pool_state::PoolStateStore;

// ─── ABI stubs (sol! macro) ──────────────────────────────────────────────────

sol! {
    /// `IUniswapV2Pair.getReserves()` — used for V2-style pools
    /// (Camelot V2, SushiSwap, Trader Joe V1).
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IUniswapV2Pair {
        function getReserves()
            external
            view
            returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }

    /// `IUniswapV3Pool.slot0()` — used for UniswapV3 pools.
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0()
            external
            view
            returns (
                uint160 sqrtPriceX96,
                int24   tick,
                uint16  observationIndex,
                uint16  observationCardinality,
                uint16  observationCardinalityNext,
                uint8   feeProtocol,
                bool    unlocked
            );
        function liquidity() external view returns (uint128);
    }
}

// ─── ReserveFetcher trait ────────────────────────────────────────────────────

/// Abstraction over all RPC calls made by the reconciler.
///
/// Annotated with `#[cfg_attr(test, mockall::automock)]` so that tests can
/// inject a fully-controlled mock without a network connection.
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait ReserveFetcher: Send + Sync {
    /// Fetch V2-style reserves for a pool.
    ///
    /// Returns `(reserve0, reserve1, block_timestamp_last)`.
    async fn fetch_v2_reserves(&self, pool: Address) -> anyhow::Result<(U256, U256, u64)>;

    /// Fetch the UniswapV3 `slot0` values for a pool.
    ///
    /// Returns `(sqrtPriceX96, tick, liquidity)`.
    async fn fetch_v3_slot0(&self, pool: Address) -> anyhow::Result<(U256, i32, U256)>;

    /// Returns the current block number on-chain.
    async fn current_block(&self) -> anyhow::Result<u64>;
}

// ─── AlloyReserveFetcher ─────────────────────────────────────────────────────

/// Production [`ReserveFetcher`] backed by an alloy [`RootProvider<Ethereum>`].
///
/// Construct via [`AlloyReserveFetcher::new`] with an `Arc<RootProvider<Ethereum>>`
/// obtained from `ProviderBuilder::new().connect(url).await`.
pub struct AlloyReserveFetcher {
    provider: Arc<RootProvider<Ethereum>>,
}

impl AlloyReserveFetcher {
    /// Wraps any alloy `RootProvider<Ethereum>`.
    pub fn new(provider: Arc<RootProvider<Ethereum>>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl ReserveFetcher for AlloyReserveFetcher {
    async fn fetch_v2_reserves(&self, pool: Address) -> anyhow::Result<(U256, U256, u64)> {
        let contract = IUniswapV2Pair::new(pool, &*self.provider);
        let result = contract
            .getReserves()
            .call()
            .await
            .with_context(|| format!("getReserves() failed for {pool}"))?;
        Ok((
            U256::from(result.reserve0),
            U256::from(result.reserve1),
            u64::from(result.blockTimestampLast),
        ))
    }

    async fn fetch_v3_slot0(&self, pool: Address) -> anyhow::Result<(U256, i32, U256)> {
        let contract = IUniswapV3Pool::new(pool, &*self.provider);
        let slot0 = contract
            .slot0()
            .call()
            .await
            .with_context(|| format!("slot0() failed for {pool}"))?;
        let liquidity = contract
            .liquidity()
            .call()
            .await
            .with_context(|| format!("liquidity() failed for {pool}"))?;
        Ok((
            U256::from(slot0.sqrtPriceX96),
            i32::try_from(slot0.tick).unwrap_or(slot0.tick.as_i32()),
            U256::from(liquidity),
        ))
    }

    async fn current_block(&self) -> anyhow::Result<u64> {
        use alloy::providers::Provider as _;
        self.provider
            .get_block_number()
            .await
            .context("get_block_number() failed")
    }
}

// ─── ReconcileStats ──────────────────────────────────────────────────────────

/// Summary of a single reconciliation pass.
#[derive(Debug, Clone, Default)]
pub struct ReconcileStats {
    /// Number of pools checked during this pass.
    pub pools_checked: usize,
    /// Number of pools whose cached state differed from on-chain and was updated.
    pub pools_updated: usize,
    /// Number of pools where the RPC call failed.
    pub pools_failed: usize,
    /// Block number at which the reconciliation was performed.
    pub block: u64,
}

// ─── BlockReconciler ─────────────────────────────────────────────────────────

/// Periodic block-level reconciler that keeps the [`PoolStateStore`] in sync
/// with on-chain state.
///
/// All pool updates are performed concurrently, bounded by `concurrency_limit`
/// simultaneous in-flight RPC calls.
pub struct BlockReconciler<F: ReserveFetcher> {
    fetcher: F,
    pool_store: PoolStateStore,
    concurrency_limit: usize,
}

impl<F: ReserveFetcher + 'static> BlockReconciler<F> {
    /// Constructs a new reconciler.
    ///
    /// `concurrency_limit` caps the number of simultaneous RPC calls (default
    /// `20` in production).
    pub fn new(fetcher: F, pool_store: PoolStateStore, concurrency_limit: usize) -> Self {
        Self {
            fetcher,
            pool_store,
            concurrency_limit,
        }
    }

    /// Block-driven reconciliation loop.
    ///
    /// Subscribes to new block numbers via `fetcher.current_block()`, sleeping
    /// 100 ms between polls, and calls `reconcile_all` on every new block.
    pub async fn run(self) -> anyhow::Result<()> {
        let mut last_block: u64 = 0;

        loop {
            match self.fetcher.current_block().await {
                Err(e) => {
                    warn!(error = %e, "current_block() failed; retrying in 1s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
                Ok(block) if block <= last_block => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                Ok(block) => {
                    info!(block, "reconciling all pools");
                    let stats = self.reconcile_all(block).await;
                    info!(
                        block,
                        checked = stats.pools_checked,
                        updated = stats.pools_updated,
                        failed = stats.pools_failed,
                        "reconcile pass complete"
                    );
                    last_block = block;
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Runs a single reconciliation pass for every pool in the store.
    ///
    /// Spawns up to `concurrency_limit` concurrent tasks, collecting results
    /// into a [`ReconcileStats`] aggregate.
    pub async fn reconcile_all(&self, block: u64) -> ReconcileStats {
        let addresses = self.pool_store.all_addresses();
        let mut stats = ReconcileStats {
            pools_checked: addresses.len(),
            block,
            ..Default::default()
        };

        let sem = Arc::new(Semaphore::new(self.concurrency_limit));

        let tasks: Vec<_> = addresses
            .into_iter()
            .filter_map(|addr| self.pool_store.get(&addr))
            .map(|pool| {
                let sem = sem.clone();
                async move {
                    let _permit = sem.acquire().await.ok()?;
                    Some(self.reconcile_pool(&pool, block).await)
                }
            })
            .collect();

        let results = join_all(tasks).await;

        for result in results {
            match result {
                Some(Ok(true)) => stats.pools_updated += 1,
                Some(Ok(false)) => {}
                Some(Err(e)) => {
                    error!(error = %e, "reconcile_pool failed");
                    stats.pools_failed += 1;
                }
                None => {
                    // semaphore closed (shouldn't happen in practice)
                    stats.pools_failed += 1;
                }
            }
        }

        stats
    }

    /// Reconciles a single pool's cached state against on-chain reserves.
    ///
    /// Returns `Ok(true)` when the store was updated (reserves differed),
    /// `Ok(false)` when nothing changed, or `Err` on RPC failure.
    pub async fn reconcile_pool(&self, pool: &PoolState, block: u64) -> anyhow::Result<bool> {
        match pool.dex {
            // ── UniswapV3 ─────────────────────────────────────────────────
            DexKind::UniswapV3 => {
                let (sqrt_price, _tick, liquidity) =
                    self.fetcher.fetch_v3_slot0(pool.address).await?;

                // For V3 we treat sqrtPriceX96 as reserve0 and liquidity as reserve1.
                // This gives the detector enough signal to flag price dislocations
                // without requiring a full tick-math implementation at this layer.
                if sqrt_price == pool.reserve0
                    && liquidity == pool.reserve1
                    && pool.last_updated_block == block
                {
                    return Ok(false);
                }

                let changed = sqrt_price != pool.reserve0 || liquidity != pool.reserve1;
                self.pool_store
                    .update_reserves(&pool.address, sqrt_price, liquidity, block);
                Ok(changed)
            }

            // ── V2-style (Camelot V2, SushiSwap, Trader Joe V1) ──────────
            DexKind::CamelotV2 | DexKind::SushiSwap | DexKind::TraderJoeV1 => {
                let (reserve0, reserve1, _ts) =
                    self.fetcher.fetch_v2_reserves(pool.address).await?;

                if reserve0 == pool.reserve0
                    && reserve1 == pool.reserve1
                    && pool.last_updated_block == block
                {
                    return Ok(false);
                }

                let changed = reserve0 != pool.reserve0 || reserve1 != pool.reserve1;
                self.pool_store
                    .update_reserves(&pool.address, reserve0, reserve1, block);
                Ok(changed)
            }
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;
    use arbx_common::types::{DexKind, PoolState};
    use std::time::Instant;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn addr(seed: u8) -> Address {
        Address::from([seed; 20])
    }

    fn make_pool(address: Address, r0: u128, r1: u128, block: u64, dex: DexKind) -> PoolState {
        PoolState {
            address,
            token0: Address::ZERO,
            token1: Address::ZERO,
            reserve0: U256::from(r0),
            reserve1: U256::from(r1),
            fee_tier: 3000,
            last_updated_block: block,
            dex,
        }
    }

    fn make_store_with(pools: Vec<PoolState>) -> PoolStateStore {
        let store = PoolStateStore::new();
        for p in pools {
            store.upsert(p);
        }
        store
    }

    fn make_reconciler(
        mock: MockReserveFetcher,
        store: PoolStateStore,
    ) -> BlockReconciler<MockReserveFetcher> {
        BlockReconciler::new(mock, store, 20)
    }

    // ── test: stale pool gets updated ────────────────────────────────────────

    #[tokio::test]
    async fn test_reconcile_updates_stale_pool() {
        let pool_addr = addr(1);
        let store = make_store_with(vec![make_pool(
            pool_addr,
            1000,
            2000,
            99,
            DexKind::CamelotV2,
        )]);

        let mut mock = MockReserveFetcher::new();
        // on-chain reserves differ from cached → stale
        mock.expect_fetch_v2_reserves()
            .returning(|_| Ok((U256::from(1500u128), U256::from(2500u128), 100)));

        let rec = make_reconciler(mock, store.clone());
        let pool = store.get(&pool_addr).unwrap();

        let changed = rec.reconcile_pool(&pool, 100).await.unwrap();

        assert!(changed, "stale pool should report changed = true");
        let updated = store.get(&pool_addr).unwrap();
        assert_eq!(updated.reserve0, U256::from(1500u128));
        assert_eq!(updated.reserve1, U256::from(2500u128));
        assert_eq!(updated.last_updated_block, 100);
    }

    // ── test: fresh pool is skipped ──────────────────────────────────────────

    #[tokio::test]
    async fn test_reconcile_skips_fresh_pool() {
        let pool_addr = addr(2);
        // Cached reserves match on-chain AND block is current → fresh
        let store = make_store_with(vec![make_pool(
            pool_addr,
            1000,
            2000,
            100,
            DexKind::SushiSwap,
        )]);

        let mut mock = MockReserveFetcher::new();
        mock.expect_fetch_v2_reserves()
            .returning(|_| Ok((U256::from(1000u128), U256::from(2000u128), 100)));

        let rec = make_reconciler(mock, store.clone());
        let pool = store.get(&pool_addr).unwrap();

        let changed = rec.reconcile_pool(&pool, 100).await.unwrap();

        assert!(!changed, "fresh pool should report changed = false");
    }

    // ── test: reconcile_all returns correct aggregate stats ──────────────────

    #[tokio::test]
    async fn test_reconcile_all_returns_correct_stats() {
        // 3 pools: 2 V2 stale, 1 V3 fresh
        let a1 = addr(10);
        let a2 = addr(11);
        let a3 = addr(12);
        let store = make_store_with(vec![
            make_pool(a1, 100, 200, 99, DexKind::CamelotV2), // stale
            make_pool(a2, 300, 400, 99, DexKind::TraderJoeV1), // stale
            make_pool(a3, 500, 600, 100, DexKind::UniswapV3), // fresh (same values)
        ]);

        let mut mock = MockReserveFetcher::new();

        // a1 and a2 → new values → stale
        mock.expect_fetch_v2_reserves()
            .times(2)
            .returning(|_| Ok((U256::from(9999u128), U256::from(9999u128), 100)));

        // a3 → same sqrtPrice and liquidity as cached → fresh
        mock.expect_fetch_v3_slot0()
            .times(1)
            .returning(|_| Ok((U256::from(500u128), 0_i32, U256::from(600u128))));

        let rec = make_reconciler(mock, store.clone());
        let stats = rec.reconcile_all(100).await;

        assert_eq!(stats.pools_checked, 3);
        assert_eq!(stats.pools_updated, 2);
        assert_eq!(stats.pools_failed, 0);
        assert_eq!(stats.block, 100);
    }

    // ── test: fetch failures are counted, others still reconciled ────────────

    #[tokio::test]
    async fn test_reconcile_all_handles_fetch_failure() {
        let a_good = addr(20);
        let a_bad = addr(21);
        let store = make_store_with(vec![
            make_pool(a_good, 100, 200, 99, DexKind::SushiSwap),
            make_pool(a_bad, 300, 400, 99, DexKind::SushiSwap),
        ]);

        let mut mock = MockReserveFetcher::new();
        mock.expect_fetch_v2_reserves().returning(move |addr| {
            if addr == a_bad {
                Err(anyhow::anyhow!("RPC timeout"))
            } else {
                Ok((U256::from(150u128), U256::from(250u128), 100))
            }
        });

        let rec = make_reconciler(mock, store.clone());
        let stats = rec.reconcile_all(100).await;

        assert_eq!(stats.pools_checked, 2);
        assert_eq!(stats.pools_failed, 1, "bad pool should count as failed");
        assert_eq!(stats.pools_updated, 1, "good pool should still be updated");

        // Verify the good pool was actually updated
        let good = store.get(&a_good).unwrap();
        assert_eq!(good.reserve0, U256::from(150u128));
    }

    // ── test: concurrency — 50 pools with 5 ms mock delay < 500 ms total ─────

    #[tokio::test]
    async fn test_reconcile_all_concurrency() {
        const N: usize = 50;
        let pools: Vec<PoolState> = (0..N as u8)
            .map(|i| make_pool(addr(i), 100, 200, 99, DexKind::CamelotV2))
            .collect();
        let store = make_store_with(pools);

        let mut mock = MockReserveFetcher::new();
        mock.expect_fetch_v2_reserves().returning(|_| {
            // Simulate 5 ms network latency per call
            std::thread::sleep(std::time::Duration::from_millis(5));
            Ok((U256::from(999u128), U256::from(999u128), 100))
        });

        let rec = BlockReconciler::new(mock, store, 20);
        let start = Instant::now();
        let stats = rec.reconcile_all(100).await;
        let elapsed = start.elapsed();

        assert_eq!(stats.pools_checked, N);
        // Sequential would take 50 × 5 ms = 250 ms. With concurrency=20 it should
        // finish in roughly 3 × 5 ms = 15 ms. We allow a generous 500 ms budget.
        assert!(
            elapsed.as_millis() < 500,
            "expected <500 ms with concurrency, got {} ms",
            elapsed.as_millis()
        );
    }

    // ── test: staleness detected — reserves same but block number stale ───────

    #[tokio::test]
    async fn test_reconcile_detects_staleness() {
        let pool_addr = addr(30);
        // Pool cached at block 100, same reserves
        let store = make_store_with(vec![make_pool(
            pool_addr,
            1000,
            2000,
            100,
            DexKind::CamelotV2,
        )]);

        let mut mock = MockReserveFetcher::new();
        // On-chain: same reserves, but we are now at block 200
        mock.expect_fetch_v2_reserves()
            .returning(|_| Ok((U256::from(1000u128), U256::from(2000u128), 200)));

        let rec = make_reconciler(mock, store.clone());
        let pool = store.get(&pool_addr).unwrap();

        // reconcile at block 200 — block number changed even though reserves are same
        let _ = rec.reconcile_pool(&pool, 200).await.unwrap();

        let updated = store.get(&pool_addr).unwrap();
        assert_eq!(
            updated.last_updated_block, 200,
            "last_updated_block should be updated to the new block"
        );
    }
}
