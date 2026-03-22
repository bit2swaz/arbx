//! Block-by-block transaction poller for local fork environments (Anvil).
//!
//! # Why this exists
//! The [`SequencerFeedManager`] only detects swaps that arrive via the live
//! Arbitrum sequencer WebSocket feed.  In a local Anvil fork, synthetic test
//! transactions sent via `cast send` or `cast rpc eth_sendTransaction` are
//! never broadcast to the live feed — they only appear in the Anvil block.
//!
//! [`BlockPoller`] subscribes to the local fork RPC for new block numbers and,
//! for every new block, fetches each transaction, runs it through the same
//! detection logic as [`SequencerFeedManager::process_transaction`], and
//! forwards any recognised swap to the `swap_tx` channel.
//!
//! Typically enabled only when `network.rpc_url` points to a local Anvil node
//! (i.e. contains `127.0.0.1`).  The production path (remote RPC + live feed)
//! never enables this component.

use std::sync::Arc;

use alloy::network::Ethereum;
use alloy::primitives::Bytes;
use alloy::providers::{Provider, RootProvider};
use tracing::{debug, info, warn};

use crate::pool_state::PoolStateStore;
use crate::sequencer_feed::DetectedSwap;

/// Polls a local fork RPC for new blocks and routes swap transactions into the
/// detection pipeline.
pub struct BlockPoller {
    provider: Arc<RootProvider<Ethereum>>,
    pool_store: PoolStateStore,
    swap_tx: tokio::sync::mpsc::Sender<DetectedSwap>,
    /// 4-byte selectors to recognise as swap calls.
    selectors: Vec<[u8; 4]>,
}

/// `IUniswapV3Pool.swap(address,bool,int256,uint160,bytes)` selector.
const UNISWAP_V3_SWAP: [u8; 4] = [0x12, 0x8a, 0xcb, 0x08];

/// `IUniswapV2Pair.swap(uint256,uint256,address,bytes)` selector.
const UNIV2_SWAP: [u8; 4] = [0x02, 0x2c, 0x0d, 0x9f];

impl BlockPoller {
    /// Create a new `BlockPoller`.
    ///
    /// `pool_store` is used to confirm that `tx.to` is a tracked pool before
    /// emitting a [`DetectedSwap`].
    pub fn new(
        provider: Arc<RootProvider<Ethereum>>,
        pool_store: PoolStateStore,
        swap_tx: tokio::sync::mpsc::Sender<DetectedSwap>,
    ) -> Self {
        Self {
            provider,
            pool_store,
            swap_tx,
            selectors: vec![UNISWAP_V3_SWAP, UNIV2_SWAP],
        }
    }

    /// Runs the polling loop indefinitely.
    ///
    /// Polls the fork RPC for new block numbers every 250 ms, fetches each
    /// new block's transactions, and forwards recognised swaps.
    pub async fn run(self) -> anyhow::Result<()> {
        use alloy::providers::Provider as _;

        let mut last_block: u64 = self.provider.get_block_number().await.unwrap_or(0);

        info!(last_block, "BlockPoller started");

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

            let head = match self.provider.get_block_number().await {
                Ok(n) => n,
                Err(e) => {
                    warn!(error = %e, "BlockPoller: failed to fetch block number; retrying");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            if head <= last_block {
                continue;
            }

            // Process each new block in order.
            for block_num in (last_block + 1)..=head {
                self.process_block(block_num).await;
            }

            last_block = head;
        }
    }

    /// Fetches `block_num` and processes every transaction in it.
    async fn process_block(&self, block_num: u64) {
        use alloy::consensus::Transaction as TxTrait;
        use alloy::providers::Provider as _;

        let block = match self
            .provider
            .get_block_by_number(block_num.into())
            .full()
            .await
        {
            Ok(Some(b)) => b,
            Ok(None) => {
                debug!(block = block_num, "BlockPoller: block not found");
                return;
            }
            Err(e) => {
                warn!(block = block_num, error = %e, "BlockPoller: failed to fetch block");
                return;
            }
        };

        debug!(
            block = block_num,
            tx_count = block.transactions.len(),
            "BlockPoller: processing block",
        );

        for tx in block.transactions.txns() {
            let inner = &tx.inner;
            let to = match inner.to() {
                Some(a) => a,
                None => continue,
            };

            let input = inner.input().clone();
            if input.len() < 4 {
                continue;
            }
            let sel: [u8; 4] = match input[0..4].try_into() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if !self.selectors.contains(&sel) {
                continue;
            }

            // Must target a tracked pool.
            if self.pool_store.get(&to).is_none() {
                continue;
            }

            let tx_hash = *inner.tx_hash();
            let pool_state = match self.pool_store.get(&to) {
                Some(p) => p,
                None => continue,
            };

            let total = pool_state.reserve0.saturating_add(pool_state.reserve1);
            let is_large = if total.is_zero() {
                false
            } else {
                use alloy::primitives::U256;
                let amount = if sel == UNISWAP_V3_SWAP {
                    if input.len() >= 100 {
                        let raw: [u8; 32] = input[68..100].try_into().unwrap();
                        let v = U256::from_be_bytes(raw);
                        if raw[0] & 0x80 != 0 {
                            U256::MAX - v + U256::from(1u64)
                        } else {
                            v
                        }
                    } else {
                        U256::ZERO
                    }
                } else if input.len() >= 68 {
                    let b0: [u8; 32] = input[4..36].try_into().unwrap();
                    let b1: [u8; 32] = input[36..68].try_into().unwrap();
                    U256::from_be_bytes(b0).max(U256::from_be_bytes(b1))
                } else {
                    U256::ZERO
                };
                amount * U256::from(10_000u128) > total * U256::from(10u128)
            };

            let swap = DetectedSwap {
                tx_hash,
                pool_address: to,
                selector: sel,
                calldata: input,
                sequenced_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                is_large,
            };

            info!(
                block = block_num,
                pool  = %to,
                hash  = %tx_hash,
                "BlockPoller: detected swap in Anvil block",
            );

            if self.swap_tx.send(swap).await.is_err() {
                warn!("BlockPoller: swap channel closed; stopping");
                return;
            }
        }
    }
}
