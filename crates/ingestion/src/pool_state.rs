//! In-memory pool state store backed by `DashMap` for lock-free concurrent reads.
//!
//! The pool state store is the source of truth for all cached pool reserves,
//! tokens, and metadata. It is updated by the sequencer feed listener and
//! periodically reconciled against on-chain state via RPC.

use alloy::primitives::{Address, U256};
use arbx_common::types::{DexKind, PoolState};
use dashmap::DashMap;
use std::sync::Arc;

// ─── PoolStateStore ──────────────────────────────────────────────────────────

/// A concurrent, lock-free store of all known liquidity pools.
///
/// Uses `DashMap` internally to provide fast reads without blocking,
/// and safe concurrent writes via internal RwLocks on each entry.
#[derive(Clone, Debug)]
pub struct PoolStateStore {
    inner: Arc<DashMap<Address, PoolState>>,
}

impl PoolStateStore {
    /// Creates a new empty pool state store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
        }
    }

    /// Inserts or updates a pool in the store. If the pool already exists,
    /// it is completely replaced.
    pub fn upsert(&self, state: PoolState) {
        self.inner.insert(state.address, state);
    }

    /// Retrieves a pool by address, returning a clone of the state.
    ///
    /// Returns `None` if the pool is not in the store.
    pub fn get(&self, address: &Address) -> Option<PoolState> {
        self.inner.get(address).map(|entry| entry.clone())
    }

    /// Returns the number of pools currently in the store.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns all pool addresses currently in the store, in arbitrary order.
    pub fn all_addresses(&self) -> Vec<Address> {
        self.inner
            .iter()
            .map(|entry| entry.key().to_owned())
            .collect()
    }

    /// Returns all pools of a specific DEX kind.
    pub fn by_dex(&self, dex: DexKind) -> Vec<PoolState> {
        self.inner
            .iter()
            .filter(|entry| entry.dex == dex)
            .map(|entry| entry.clone())
            .collect()
    }

    /// Returns all pools where the given token appears as either token0 or token1.
    pub fn pools_containing_token(&self, token: &Address) -> Vec<PoolState> {
        self.inner
            .iter()
            .filter(|entry| entry.token0 == *token || entry.token1 == *token)
            .map(|entry| entry.clone())
            .collect()
    }

    /// Updates the reserves and block number for an existing pool.
    ///
    /// Returns `true` if the pool was found and updated, `false` if not found.
    pub fn update_reserves(
        &self,
        address: &Address,
        reserve0: U256,
        reserve1: U256,
        block: u64,
    ) -> bool {
        if let Some(mut entry) = self.inner.get_mut(address) {
            entry.reserve0 = reserve0;
            entry.reserve1 = reserve1;
            entry.last_updated_block = block;
            true
        } else {
            false
        }
    }
}

impl Default for PoolStateStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: creates a deterministic PoolState from a seed.
    fn make_pool_state(seed: u64) -> PoolState {
        PoolState {
            address: make_address(seed),
            token0: make_address(seed.wrapping_add(1000)),
            token1: make_address(seed.wrapping_add(2000)),
            reserve0: U256::from(seed.wrapping_mul(1_000_000_000_000_000_000)),
            reserve1: U256::from(seed.wrapping_add(1).wrapping_mul(1_000_000_000_000_000_000)),
            fee_tier: 3000,
            last_updated_block: seed,
            dex: match seed % 4 {
                0 => DexKind::UniswapV3,
                1 => DexKind::CamelotV2,
                2 => DexKind::SushiSwap,
                _ => DexKind::TraderJoeV1,
            },
        }
    }

    /// Helper: creates a deterministic Address from a seed.
    fn make_address(seed: u64) -> Address {
        let mut bytes = [0u8; 20];
        bytes[0..8].copy_from_slice(&seed.to_le_bytes());
        Address::new(bytes)
    }

    // ─── BASIC OPERATIONS ─────────────────────────────────────────────────────

    #[test]
    fn test_new_store_is_empty() {
        let store = PoolStateStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_upsert_and_get_returns_correct_state() {
        let store = PoolStateStore::new();
        let pool = make_pool_state(42);

        store.upsert(pool.clone());

        let retrieved = store.get(&pool.address).expect("Pool should be found");
        assert_eq!(retrieved, pool);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_upsert_twice_overwrites() {
        let store = PoolStateStore::new();
        let pool1 = make_pool_state(42);
        let mut pool2 = make_pool_state(42);
        pool2.reserve0 = U256::from(999);

        store.upsert(pool1.clone());
        assert_eq!(store.get(&pool1.address).unwrap().reserve0, pool1.reserve0);

        store.upsert(pool2.clone());
        assert_eq!(store.get(&pool2.address).unwrap().reserve0, pool2.reserve0);
        assert_eq!(store.len(), 1); // Still one pool
    }

    #[test]
    fn test_get_missing_returns_none() {
        let store = PoolStateStore::new();
        let addr = make_address(999);
        assert_eq!(store.get(&addr), None);
    }

    #[test]
    fn test_update_reserves_success() {
        let store = PoolStateStore::new();
        let pool = make_pool_state(42);
        store.upsert(pool.clone());

        let new_reserve0 = U256::from(123_456);
        let new_reserve1 = U256::from(654_321);
        let new_block = 999;

        let updated = store.update_reserves(&pool.address, new_reserve0, new_reserve1, new_block);
        assert!(updated);

        let retrieved = store.get(&pool.address).expect("Pool should exist");
        assert_eq!(retrieved.reserve0, new_reserve0);
        assert_eq!(retrieved.reserve1, new_reserve1);
        assert_eq!(retrieved.last_updated_block, new_block);
    }

    #[test]
    fn test_update_reserves_updates_block_number() {
        let store = PoolStateStore::new();
        let pool = make_pool_state(50);
        store.upsert(pool.clone());

        let original_block = pool.last_updated_block;
        let new_block = original_block + 100;

        store.update_reserves(&pool.address, pool.reserve0, pool.reserve1, new_block);
        let retrieved = store.get(&pool.address).unwrap();
        assert_eq!(retrieved.last_updated_block, new_block);
    }

    #[test]
    fn test_update_reserves_missing_pool_returns_false() {
        let store = PoolStateStore::new();
        let missing_addr = make_address(9999);

        let updated = store.update_reserves(&missing_addr, U256::from(100), U256::from(200), 10);
        assert!(!updated);
    }

    #[test]
    fn test_len_correct_after_insertions() {
        let store = PoolStateStore::new();
        assert_eq!(store.len(), 0);

        for i in 0..10 {
            store.upsert(make_pool_state(i));
        }
        assert_eq!(store.len(), 10);

        // Upsert a duplicate (should not increase len)
        store.upsert(make_pool_state(0));
        assert_eq!(store.len(), 10);
    }

    #[test]
    fn test_all_addresses_correct_count() {
        let store = PoolStateStore::new();
        for i in 0..5 {
            store.upsert(make_pool_state(i));
        }

        let addrs = store.all_addresses();
        assert_eq!(addrs.len(), 5);
        // All addresses should be unique (we used unique seeds)
        assert_eq!(
            addrs.iter().collect::<std::collections::HashSet<_>>().len(),
            5
        );
    }

    #[test]
    fn test_by_dex_filters_correctly() {
        let store = PoolStateStore::new();
        let mut univ3_count = 0;
        let mut camelot_count = 0;

        for i in 0..20 {
            let pool = make_pool_state(i);
            if pool.dex == DexKind::UniswapV3 {
                univ3_count += 1;
            }
            if pool.dex == DexKind::CamelotV2 {
                camelot_count += 1;
            }
            store.upsert(pool);
        }

        let univ3_pools = store.by_dex(DexKind::UniswapV3);
        let camelot_pools = store.by_dex(DexKind::CamelotV2);

        assert_eq!(univ3_pools.len(), univ3_count);
        assert_eq!(camelot_pools.len(), camelot_count);
        assert!(univ3_pools.iter().all(|p| p.dex == DexKind::UniswapV3));
        assert!(camelot_pools.iter().all(|p| p.dex == DexKind::CamelotV2));
    }

    #[test]
    fn test_pools_containing_token_both_positions() {
        let store = PoolStateStore::new();

        // Pool 1: token A <-> token B
        let pool1 = {
            let mut p = make_pool_state(1);
            p.token0 = make_address(100);
            p.token1 = make_address(200);
            p
        };

        // Pool 2: token A <-> token C (A in token0)
        let pool2 = {
            let mut p = make_pool_state(2);
            p.token0 = make_address(100);
            p.token1 = make_address(300);
            p
        };

        // Pool 3: token B <-> token C (B in token1)
        let pool3 = {
            let mut p = make_pool_state(3);
            p.token0 = make_address(300);
            p.token1 = make_address(200);
            p
        };

        store.upsert(pool1);
        store.upsert(pool2);
        store.upsert(pool3);

        let token_a = make_address(100);
        let token_b = make_address(200);
        let token_c = make_address(300);

        // Token A should be in 2 pools
        let pools_with_a = store.pools_containing_token(&token_a);
        assert_eq!(pools_with_a.len(), 2);

        // Token B should be in 2 pools
        let pools_with_b = store.pools_containing_token(&token_b);
        assert_eq!(pools_with_b.len(), 2);

        // Token C should be in 2 pools
        let pools_with_c = store.pools_containing_token(&token_c);
        assert_eq!(pools_with_c.len(), 2);
    }

    // ─── CONCURRENCY TESTS ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_concurrent_upserts_no_data_race() {
        let store = PoolStateStore::new();
        let mut handles = vec![];

        // Spawn 100 concurrent tasks, each upserts a unique pool
        for i in 0u64..100 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let pool = make_pool_state(i);
                store.upsert(pool);
            }));
        }

        // Wait for all tasks to complete
        for h in handles {
            h.await.expect("Task should complete without panic");
        }

        // Verify all pools were inserted
        assert_eq!(store.len(), 100);

        // Verify each pool is retrievable with correct data
        for i in 0..100 {
            let pool = make_pool_state(i);
            let retrieved = store.get(&pool.address).expect("Pool should be found");
            assert_eq!(retrieved, pool);
        }
    }

    #[tokio::test]
    async fn test_concurrent_reads_while_writing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let store = PoolStateStore::new();
        let panic_flag = Arc::new(AtomicBool::new(false));

        // Pre-populate with some pools
        for i in 0..50 {
            store.upsert(make_pool_state(i));
        }

        let should_stop = Arc::new(AtomicBool::new(false));

        // Writer task: continuously upserts new pools
        let store_writer = store.clone();
        let should_stop_writer = should_stop.clone();
        let writer_handle = tokio::spawn(async move {
            let mut counter = 50;
            while !should_stop_writer.load(Ordering::Relaxed) {
                store_writer.upsert(make_pool_state(counter));
                counter += 1;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });

        // 10 reader tasks: continuously read and verify no panics
        let mut reader_handles = vec![];
        for _ in 0..10 {
            let store_reader = store.clone();
            let should_stop_reader = should_stop.clone();
            let _panic_flag_clone = panic_flag.clone();

            reader_handles.push(tokio::spawn(async move {
                while !should_stop_reader.load(Ordering::Relaxed) {
                    // Perform various reads
                    let _len = store_reader.len();
                    let _addrs = store_reader.all_addresses();
                    let _by_dex = store_reader.by_dex(DexKind::UniswapV3);

                    // Try to get a pool if any exist
                    if let Some(addr) = _addrs.first() {
                        let _pool = store_reader.get(addr);
                    }

                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }));
        }

        // Let readers and writers run for 100ms
        tokio::time::sleep(Duration::from_millis(100)).await;
        should_stop.store(true, Ordering::Relaxed);

        // Wait for writer
        writer_handle.await.expect("Writer should complete");

        // Wait for all readers
        for h in reader_handles {
            h.await.expect("Reader should complete without panic");
        }

        // Verify no panic occurred
        assert!(!panic_flag.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_concurrent_reads_same_pool() {
        let store = PoolStateStore::new();
        let pool = make_pool_state(42);
        store.upsert(pool.clone());

        let mut handles = vec![];

        // Spawn 50 readers, all trying to read the same pool concurrently
        for _ in 0..50 {
            let store = store.clone();
            let expected_pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let retrieved = store
                    .get(&expected_pool.address)
                    .expect("Pool should be found");
                assert_eq!(retrieved, expected_pool);
            }));
        }

        // Wait for all readers
        for h in handles {
            h.await.expect("Reader should complete");
        }
    }

    #[tokio::test]
    async fn test_concurrent_updates_same_pool() {
        let store = PoolStateStore::new();
        let pool = make_pool_state(42);
        store.upsert(pool.clone());

        let mut handles = vec![];

        // Spawn 50 concurrent update tasks on the same pool
        for i in 0..50 {
            let store = store.clone();
            let addr = pool.address;
            handles.push(tokio::spawn(async move {
                let new_reserve0 = U256::from(i * 1000);
                let new_reserve1 = U256::from(i * 2000);
                let updated = store.update_reserves(&addr, new_reserve0, new_reserve1, i);
                assert!(updated);
            }));
        }

        // Wait for all updates
        for h in handles {
            h.await.expect("Update should complete");
        }

        // Verify the pool exists and has been updated by at least one task
        let final_pool = store.get(&pool.address).expect("Pool should still exist");
        assert_eq!(final_pool.address, pool.address);
    }

    #[test]
    fn test_clone_shares_state() {
        let store1 = PoolStateStore::new();
        let pool = make_pool_state(123);

        store1.upsert(pool.clone());

        // Clone the store
        let store2 = store1.clone();

        // Both stores should see the same pool
        assert_eq!(store1.len(), 1);
        assert_eq!(store2.len(), 1);
        assert_eq!(store1.get(&pool.address), store2.get(&pool.address));

        // Insert via store2, should be visible in store1
        let pool2 = make_pool_state(456);
        store2.upsert(pool2.clone());

        assert_eq!(store1.len(), 2);
        assert_eq!(store2.len(), 2);
        assert_eq!(store1.get(&pool2.address), Some(pool2));
    }
}
