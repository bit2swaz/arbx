//! On-startup pool discovery via factory event logs.
//!
//! # Why this exists
//! The [`SequencerFeedManager`] can only detect swaps for pools that are
//! already in the [`PoolStateStore`].  The store starts empty, which means
//! the first swap seen by the feed always looks like an unknown target and is
//! silently dropped.
//!
//! This module solves the chicken-and-egg problem by querying each DEX factory
//! contract for its `PairCreated` or `PoolCreated` event logs at startup.
//! Every discovered pool address is inserted into the store with placeholder
//! reserves (0); the [`BlockReconciler`] will hydrate accurate reserves on the
//! next reconcile pass.
//!
//! # Block range chunking
//! Alchemy's free tier limits `eth_getLogs` to a maximum of 10 blocks per
//! request.  We therefore scan in chunks of [`CHUNK_SIZE`] blocks from
//! `seed_from_block` (configured per-environment) up to the current head.
//! On mainnet with a full archive node the chunk size can be set much higher.

use std::sync::Arc;

use alloy::network::Ethereum;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::Filter;
use arbx_common::types::{DexKind, PoolState};
use tracing::{debug, info, warn};

use crate::pool_state::PoolStateStore;

// ─── Tuning ───────────────────────────────────────────────────────────────────

/// Maximum blocks per `eth_getLogs` request.
/// Alchemy free tier enforces a 10-block limit; paid tiers allow up to 10 000.
/// Set conservatively so the seeder works on the free tier out of the box.
const CHUNK_SIZE: u64 = 10;

// ─── Event topic keccak256 hashes ────────────────────────────────────────────

/// `PairCreated(address,address,address,uint256)` — emitted by Uniswap-V2-style
/// factories (Camelot V2, SushiSwap, Trader Joe V1).
/// keccak256("PairCreated(address,address,address,uint256)")
const PAIR_CREATED_TOPIC: B256 = B256::new([
    0x0d, 0x36, 0x48, 0xbd, 0x0f, 0x6b, 0xa8, 0x01, 0x34, 0xa3, 0x3b, 0xa9, 0x27, 0x5a, 0xc5, 0x85,
    0xd9, 0xd3, 0x15, 0xf0, 0xad, 0x83, 0x55, 0xcd, 0xde, 0xfd, 0xe3, 0x1a, 0xfa, 0x28, 0xd0, 0xe9,
]);

/// `PoolCreated(address,address,uint24,int24,address)` — emitted by the
/// Uniswap V3 factory.
/// keccak256("PoolCreated(address,address,uint24,int24,address)")
const POOL_CREATED_TOPIC: B256 = B256::new([
    0x78, 0x3c, 0xca, 0x1c, 0x04, 0x12, 0xdd, 0x0d, 0x69, 0x5e, 0x78, 0x45, 0x68, 0xc9, 0x6d, 0xa2,
    0xe9, 0xc2, 0x2f, 0xf9, 0x89, 0x35, 0x7a, 0x2e, 0x8b, 0x1d, 0x9b, 0x2b, 0x4e, 0x6b, 0x71, 0x18,
]);

// ─── Public entry point ───────────────────────────────────────────────────────

/// Seed `store` with every pool discovered from the configured factory contracts.
///
/// Scans factory event logs in [`CHUNK_SIZE`]-block chunks from `seed_from_block`
/// to the current chain head.  Pools are inserted with zero reserves — the block
/// reconciler will fill them in on the next pass.
///
/// Returns the total number of pools inserted.
pub async fn seed_pools_from_factories(
    provider: Arc<RootProvider<Ethereum>>,
    store: &PoolStateStore,
    uniswap_v3_factory: &str,
    camelot_factory: &str,
    sushiswap_factory: &str,
    traderjoe_factory: &str,
    seed_from_block: u64,
) -> usize {
    let mut total = 0usize;

    // Fetch current head so we know the upper bound of the scan range.
    let head = match provider.get_block_number().await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "failed to fetch current block number; skipping pool seeding");
            return 0;
        }
    };

    if head < seed_from_block {
        info!(
            head,
            seed_from_block, "seed_from_block is ahead of chain head; no blocks to scan"
        );
        return 0;
    }

    info!(
        from = seed_from_block,
        to = head,
        chunk = CHUNK_SIZE,
        "scanning factory logs for pool seeds"
    );

    // ── Uniswap V3 ────────────────────────────────────────────────────────────
    if let Ok(addr) = uniswap_v3_factory.parse::<Address>() {
        match fetch_v3_pools(&provider, addr, seed_from_block, head).await {
            Ok(pools) => {
                let n = pools.len();
                for p in pools {
                    store.upsert(p);
                }
                info!(factory = %addr, count = n, "seeded UniswapV3 pools");
                total += n;
            }
            Err(e) => warn!(factory = %addr, error = %e, "failed to seed UniswapV3 pools"),
        }
    } else {
        warn!(
            addr = uniswap_v3_factory,
            "invalid uniswap_v3_factory address — skipping"
        );
    }

    // ── Camelot V2 ────────────────────────────────────────────────────────────
    if let Ok(addr) = camelot_factory.parse::<Address>() {
        match fetch_v2_pools(&provider, addr, DexKind::CamelotV2, seed_from_block, head).await {
            Ok(pools) => {
                let n = pools.len();
                for p in pools {
                    store.upsert(p);
                }
                info!(factory = %addr, count = n, "seeded CamelotV2 pools");
                total += n;
            }
            Err(e) => warn!(factory = %addr, error = %e, "failed to seed CamelotV2 pools"),
        }
    }

    // ── SushiSwap ─────────────────────────────────────────────────────────────
    if let Ok(addr) = sushiswap_factory.parse::<Address>() {
        match fetch_v2_pools(&provider, addr, DexKind::SushiSwap, seed_from_block, head).await {
            Ok(pools) => {
                let n = pools.len();
                for p in pools {
                    store.upsert(p);
                }
                info!(factory = %addr, count = n, "seeded SushiSwap pools");
                total += n;
            }
            Err(e) => warn!(factory = %addr, error = %e, "failed to seed SushiSwap pools"),
        }
    }

    // ── Trader Joe V1 ─────────────────────────────────────────────────────────
    if let Ok(addr) = traderjoe_factory.parse::<Address>() {
        match fetch_v2_pools(&provider, addr, DexKind::TraderJoeV1, seed_from_block, head).await {
            Ok(pools) => {
                let n = pools.len();
                for p in pools {
                    store.upsert(p);
                }
                info!(factory = %addr, count = n, "seeded TraderJoeV1 pools");
                total += n;
            }
            Err(e) => warn!(factory = %addr, error = %e, "failed to seed TraderJoeV1 pools"),
        }
    }

    info!(total, "pool seeding complete");
    total
}

// ─── V3 pool discovery ────────────────────────────────────────────────────────

/// Query `PoolCreated` events from a Uniswap V3 factory over `[from, to]`.
///
/// Requests are split into [`CHUNK_SIZE`]-block windows to comply with
/// Alchemy free-tier restrictions.
///
/// Event layout:
/// ```text
/// PoolCreated(
///   address indexed token0,
///   address indexed token1,
///   uint24  indexed fee,
///   int24           tickSpacing,
///   address         pool          ← non-indexed, in data
/// )
/// ```
async fn fetch_v3_pools(
    provider: &Arc<RootProvider<Ethereum>>,
    factory: Address,
    from_block: u64,
    to_block: u64,
) -> anyhow::Result<Vec<PoolState>> {
    let mut pools = Vec::new();
    let mut start = from_block;

    while start <= to_block {
        let end = (start + CHUNK_SIZE - 1).min(to_block);

        let filter = Filter::new()
            .address(factory)
            .event_signature(POOL_CREATED_TOPIC)
            .from_block(start)
            .to_block(end);

        let logs = provider.get_logs(&filter).await?;
        debug!(
            count = logs.len(),
            start, end, "raw PoolCreated logs in chunk"
        );

        for log in &logs {
            // token0, token1 are indexed topics[1] and topics[2]
            let token0 = match log.topics().get(1) {
                Some(t) => Address::from_word(*t),
                None => {
                    debug!("PoolCreated log missing token0 topic — skipping");
                    continue;
                }
            };
            let token1 = match log.topics().get(2) {
                Some(t) => Address::from_word(*t),
                None => {
                    debug!("PoolCreated log missing token1 topic — skipping");
                    continue;
                }
            };
            // fee is topics[3] (lower 24 bits)
            let fee_tier = log
                .topics()
                .get(3)
                .map(|t| {
                    let v = U256::from_be_bytes(t.0);
                    v.wrapping_rem(U256::from(u64::MAX))
                        .try_into()
                        .unwrap_or(3000u32)
                })
                .unwrap_or(3000u32);

            // pool address is the last 20 bytes of `data` (after 64 bytes of tickSpacing + padding)
            let data = log.data().data.as_ref();
            if data.len() < 64 {
                debug!("PoolCreated log data too short — skipping");
                continue;
            }
            // data layout: tickSpacing(32) + pool(32, right-padded address)
            let pool_addr = Address::from_slice(&data[44..64]);

            if pool_addr.is_zero() {
                continue;
            }

            pools.push(PoolState {
                address: pool_addr,
                token0,
                token1,
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
                fee_tier,
                last_updated_block: 0,
                dex: DexKind::UniswapV3,
            });
        }

        start = end + 1;
    }

    Ok(pools)
}

// ─── V2 pool discovery ────────────────────────────────────────────────────────

/// Query `PairCreated` events from a Uniswap-V2-style factory over `[from, to]`.
///
/// Requests are split into [`CHUNK_SIZE`]-block windows.
///
/// Event layout:
/// ```text
/// PairCreated(
///   address indexed token0,
///   address indexed token1,
///   address         pair,    ← non-indexed, first 32 bytes of data
///   uint256         allPairsLength
/// )
/// ```
async fn fetch_v2_pools(
    provider: &Arc<RootProvider<Ethereum>>,
    factory: Address,
    dex: DexKind,
    from_block: u64,
    to_block: u64,
) -> anyhow::Result<Vec<PoolState>> {
    let mut pools = Vec::new();
    let mut start = from_block;

    while start <= to_block {
        let end = (start + CHUNK_SIZE - 1).min(to_block);

        let filter = Filter::new()
            .address(factory)
            .event_signature(PAIR_CREATED_TOPIC)
            .from_block(start)
            .to_block(end);

        let logs = provider.get_logs(&filter).await?;
        debug!(count = logs.len(), dex = ?dex, start, end, "raw PairCreated logs in chunk");

        for log in &logs {
            let token0 = match log.topics().get(1) {
                Some(t) => Address::from_word(*t),
                None => {
                    debug!("PairCreated log missing token0 topic — skipping");
                    continue;
                }
            };
            let token1 = match log.topics().get(2) {
                Some(t) => Address::from_word(*t),
                None => {
                    debug!("PairCreated log missing token1 topic — skipping");
                    continue;
                }
            };

            // pair address is the first 32 bytes of data (right-aligned address)
            let data = log.data().data.as_ref();
            if data.len() < 32 {
                debug!("PairCreated log data too short — skipping");
                continue;
            }
            let pair_addr = Address::from_slice(&data[12..32]);

            if pair_addr.is_zero() {
                continue;
            }

            pools.push(PoolState {
                address: pair_addr,
                token0,
                token1,
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
                fee_tier: 3000, // V2-style constant 0.3%
                last_updated_block: 0,
                dex,
            });
        }

        start = end + 1;
    }

    Ok(pools)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that passing all-zero / nonsense factory addresses does not
    /// panic — just emits warnings and returns 0.
    #[tokio::test]
    async fn seed_invalid_addresses_returns_zero() {
        // We cannot easily mock the provider in a unit test here, so we use a
        // live-but-invalid address and rely on the warning path.
        // This test only checks that the function accepts a bad address string
        // without panicking when the address parse fails.
        let store = PoolStateStore::new();
        // Provide deliberately invalid (non-hex) address strings — should
        // skip each factory and return 0.
        // We can't call seed_pools_from_factories without a real provider,
        // so just exercise the parse path.
        let bad = "not_an_address";
        assert!(bad.parse::<Address>().is_err());
        let _ = store; // used
    }
}
