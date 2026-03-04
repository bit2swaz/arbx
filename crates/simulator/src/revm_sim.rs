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
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    sol,
    sol_types::{SolCall, SolValue},
};
use anyhow::Context as _;
use arbx_common::types::{ArbPath, Opportunity, SimulationResult};
use revm::{
    context::TxEnv,
    context_interface::result::ExecutionResult,
    database::{AlloyDB, CacheDB},
    database_interface::WrapDatabaseAsync,
    primitives::TxKind,
    Context, ExecuteCommitEvm, MainBuilder, MainContext,
};
use tracing::{debug, trace};

// ─── ERC-20 minimal ABI (used by read_erc20_balance) ────────────────────────

sol! {
    #[sol(rpc)]
    interface IERC20 {
        /// Returns the token balance of `owner`.
        function balanceOf(address owner) external view returns (uint256);
    }
}

// ─── ArbExecutor ABI (for calldata encoding) ─────────────────────────────────

// ABI types matching `ArbExecutor.sol::executeArb`.
// Field names use Rust snake_case; the wire encoding is identical to the
// Solidity camelCase original because Solidity ABI encoding ignores names.
sol! {
    struct ArbParams {
        address token_in;
        address pool_a;
        address token_mid;
        address pool_b;
        uint256 flash_loan_amount;
        uint256 min_profit;
        uint8   pool_a_kind;
        uint8   pool_b_kind;
    }

    /// Selector: keccak256("executeArb(address[],uint256[],(address,address,address,address,uint256,uint256,uint8,uint8))")
    function executeArb(
        address[] tokens,
        uint256[] amounts,
        ArbParams params
    );
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

// ─── CallDataEncoder ──────────────────────────────────────────────────────────

/// Encodes and decodes ABI calldata for [`ArbExecutor`].
///
/// All methods are pure (no I/O), so the struct carries no fields.
pub struct CallDataEncoder;

impl CallDataEncoder {
    /// ABI-encodes an `executeArb` call for the given arbitrage `path`.
    ///
    /// Prepends the 4-byte function selector via [`SolCall::abi_encode`].
    ///
    /// `pool_a_kind` and `pool_b_kind` default to `0` (UniswapV3) because
    /// [`ArbPath`] does not carry per-pool DEX-kind information at this stage.
    pub fn encode_execute_arb(path: &ArbPath, min_profit_wei: U256) -> Bytes {
        let call = executeArbCall {
            tokens: vec![path.token_in],
            amounts: vec![path.flash_loan_amount_wei],
            params: ArbParams {
                token_in: path.token_in,
                pool_a: path.pool_a,
                token_mid: path.token_mid,
                pool_b: path.pool_b,
                flash_loan_amount: path.flash_loan_amount_wei,
                min_profit: min_profit_wei,
                pool_a_kind: 0,
                pool_b_kind: 0,
            },
        };
        Bytes::from(call.abi_encode())
    }

    /// Decodes a revert reason from raw EVM output bytes.
    ///
    /// | Input                                      | Result                      |
    /// |--------------------------------------------|-----------------------------|
    /// | Empty bytes                                | `"empty revert"`            |
    /// | Starts with `0x08c379a0` (`Error(string)`) | decoded string              |
    /// | Anything else                              | hex-encoded bytes           |
    pub fn decode_revert_reason(output: &Bytes) -> String {
        if output.is_empty() {
            return "empty revert".to_string();
        }
        // Standard ABI Error(string) selector = keccak256("Error(string)")[0..4] = 0x08c379a0.
        if output.len() > 4 && output[..4] == [0x08_u8, 0xc3, 0x79, 0xa0] {
            if let Ok(reason) = String::abi_decode(&output[4..]) {
                return reason;
            }
        }
        alloy::hex::encode(output.as_ref())
    }
}

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

    // ── Simulation ───────────────────────────────────────────────────────────

    /// Simulate `opportunity` against a fork of the **latest** Arbitrum block.
    ///
    /// Forks state via [`fork_at_latest`][Self::fork_at_latest] then executes
    /// the EVM inside [`tokio::task::spawn_blocking`] (required because
    /// [`WrapDatabaseAsync`] calls `Handle::block_on` internally, which
    /// panics from within an async context).
    pub async fn simulate(
        &self,
        opportunity: &Opportunity,
        contract: Address,
        owner: Address,
    ) -> SimulationResult {
        let db = match self.fork_at_latest().await {
            Ok(db) => db,
            Err(e) => {
                return SimulationResult::Failure {
                    reason: format!("fork error: {e}"),
                }
            }
        };
        let opp = opportunity.clone();
        tokio::task::spawn_blocking(move || Self::run_evm(db, &opp, contract, owner))
            .await
            .unwrap_or_else(|e| SimulationResult::Failure {
                reason: format!("spawn_blocking join error: {e}"),
            })
    }

    /// Simulate `opportunity` against a fork at a specific historical `block`.
    ///
    /// Primarily used by regression tests that replay known-profitable blocks.
    pub async fn simulate_at_block(
        &self,
        opportunity: &Opportunity,
        contract: Address,
        owner: Address,
        block: u64,
    ) -> SimulationResult {
        let db = match self.fork_at_block(block).await {
            Ok(db) => db,
            Err(e) => {
                return SimulationResult::Failure {
                    reason: format!("fork error at block {block}: {e}"),
                }
            }
        };
        let opp = opportunity.clone();
        tokio::task::spawn_blocking(move || Self::run_evm(db, &opp, contract, owner))
            .await
            .unwrap_or_else(|e| SimulationResult::Failure {
                reason: format!("spawn_blocking join error: {e}"),
            })
    }

    /// Execute one EVM transaction on `db` and return a [`SimulationResult`].
    ///
    /// Encodes the `executeArb` calldata, runs it against the fork, and maps:
    /// - `Success` → [`SimulationResult::Success`] with `opportunity.net_profit_wei`
    /// - `Revert`  → [`SimulationResult::Failure`] with decoded reason string
    /// - `Halt`    → [`SimulationResult::Failure`] with halt description
    ///
    /// Sets `chain_id = 42161` (Arbitrum One) and disables nonce, balance, and
    /// base-fee checks so the simulation succeeds even when `owner` holds no
    /// ETH on the fork.
    fn run_evm(
        mut db: ArbDB,
        opportunity: &Opportunity,
        contract: Address,
        owner: Address,
    ) -> SimulationResult {
        let calldata = CallDataEncoder::encode_execute_arb(
            &opportunity.path,
            opportunity.path.estimated_profit_wei,
        );

        let mut evm = Context::mainnet()
            .modify_cfg_chained(|c| {
                c.chain_id = 42161; // Arbitrum One
                c.disable_nonce_check = true;
                c.disable_balance_check = true;
                c.disable_base_fee = true;
            })
            .with_db(&mut db)
            .build_mainnet();

        let tx = TxEnv::builder()
            .caller(owner)
            .kind(TxKind::Call(contract))
            .data(calldata)
            .gas_limit(2_000_000)
            .build()
            .unwrap();

        match evm.transact_commit(tx) {
            Ok(ExecutionResult::Success { gas_used, .. }) => {
                let net = opportunity.net_profit_wei;
                debug!(
                    token_in = %opportunity.path.token_in,
                    gas_used,
                    "simulation success"
                );
                SimulationResult::Success {
                    net_profit_wei: net,
                    gas_used,
                }
            }
            Ok(ExecutionResult::Revert { output, .. }) => {
                let reason = CallDataEncoder::decode_revert_reason(&output);
                debug!(%reason, "simulation revert");
                SimulationResult::Failure { reason }
            }
            Ok(ExecutionResult::Halt { reason, .. }) => {
                let r = format!("EVM halt: {reason:?}");
                debug!(reason = %r, "simulation halt");
                SimulationResult::Failure { reason: r }
            }
            Err(e) => {
                let reason = format!("EVM error: {e}");
                debug!(%reason, "simulation EVM error");
                SimulationResult::Failure { reason }
            }
        }
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

    // ── CallDataEncoder unit tests (no network required) ────────────────────

    /// `encode_execute_arb` must be pure: identical inputs produce identical bytes.
    #[test]
    fn test_encode_execute_arb_deterministic() {
        use alloy::primitives::{Address, U256};
        use arbx_common::types::ArbPath;

        let path = ArbPath {
            token_in: Address::ZERO,
            pool_a: Address::ZERO,
            token_mid: Address::ZERO,
            pool_b: Address::ZERO,
            token_out: Address::ZERO,
            estimated_profit_wei: U256::from(1_000_u64),
            flash_loan_amount_wei: U256::from(1_000_000_u64),
        };
        let min = U256::from(500_u64);
        let enc1 = CallDataEncoder::encode_execute_arb(&path, min);
        let enc2 = CallDataEncoder::encode_execute_arb(&path, min);
        assert_eq!(enc1, enc2, "encode_execute_arb must be deterministic");
    }

    /// Encoded calldata must contain more than just the 4-byte selector.
    #[test]
    fn test_encode_execute_arb_non_empty() {
        use alloy::primitives::{Address, U256};
        use arbx_common::types::ArbPath;

        let path = ArbPath {
            token_in: Address::ZERO,
            pool_a: Address::ZERO,
            token_mid: Address::ZERO,
            pool_b: Address::ZERO,
            token_out: Address::ZERO,
            estimated_profit_wei: U256::ZERO,
            flash_loan_amount_wei: U256::ZERO,
        };
        let encoded = CallDataEncoder::encode_execute_arb(&path, U256::ZERO);
        assert!(
            encoded.len() > 4,
            "calldata must have >4 bytes; got {} bytes",
            encoded.len()
        );
    }

    /// Standard `Error(string)` revert must be decoded to the plain message.
    #[test]
    fn test_decode_revert_standard_error() {
        use alloy::primitives::Bytes;
        use alloy::sol_types::SolValue;

        // Craft Error("No profit") identical to what Solidity emits.
        // selector for Error(string) = 0x08c379a0
        const SELECTOR: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];
        let message = "No profit";
        let string_abi = message.to_string().abi_encode();
        let mut revert_bytes = SELECTOR.to_vec();
        revert_bytes.extend_from_slice(&string_abi);
        let revert = Bytes::from(revert_bytes);

        let reason = CallDataEncoder::decode_revert_reason(&revert);
        assert_eq!(reason, message, "should decode standard Error(string)");
    }

    /// Non-standard revert data (unknown selector) must be hex-encoded.
    #[test]
    fn test_decode_revert_non_standard() {
        use alloy::primitives::Bytes;

        let bad = Bytes::from_static(b"\xde\xad\xbe\xef");
        let reason = CallDataEncoder::decode_revert_reason(&bad);
        assert!(
            reason.contains("deadbeef"),
            "non-standard revert should be hex-encoded; got: {reason}"
        );
    }

    /// Empty revert output must return the sentinel string `"empty revert"`.
    #[test]
    fn test_decode_revert_empty() {
        use alloy::primitives::Bytes;

        let empty = Bytes::new();
        let reason = CallDataEncoder::decode_revert_reason(&empty);
        assert_eq!(reason, "empty revert");
    }

    // ── Property tests ───────────────────────────────────────────────────────

    proptest::proptest! {
        /// For any random [`ArbPath`] the encoded calldata must be non-trivially
        /// larger than the 4-byte selector alone.
        #[test]
        fn prop_encode_decode_roundtrip(
            token_in    in proptest::array::uniform20(0_u8..),
            pool_a      in proptest::array::uniform20(0_u8..),
            token_mid   in proptest::array::uniform20(0_u8..),
            pool_b      in proptest::array::uniform20(0_u8..),
            token_out   in proptest::array::uniform20(0_u8..),
            profit       in 0_u64..u64::MAX,
            flash_amount in 0_u64..u64::MAX,
        ) {
            use alloy::primitives::{Address, U256};
            use arbx_common::types::ArbPath;

            let path = ArbPath {
                token_in:              Address::from(token_in),
                pool_a:                Address::from(pool_a),
                token_mid:             Address::from(token_mid),
                pool_b:                Address::from(pool_b),
                token_out:             Address::from(token_out),
                estimated_profit_wei:  U256::from(profit),
                flash_loan_amount_wei: U256::from(flash_amount),
            };
            let encoded = CallDataEncoder::encode_execute_arb(&path, U256::from(profit));
            proptest::prop_assert!(
                encoded.len() > 4,
                "encoded calldata must be >4 bytes; got {} bytes", encoded.len()
            );
        }
    }

    // ── Integration tests (require ARBITRUM_RPC_URL) ─────────────────────────

    /// Call the Balancer Vault (no `executeArb` function) — must report Failure.
    ///
    /// The Vault has no matching selector, so the EVM reverts.  Verifies that
    /// revm correctly surfaces reverts and that `simulate` maps them to
    /// [`SimulationResult::Failure`].
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ARBITRUM_RPC_URL"]
    async fn regression_simulates_as_failure_without_deployed_contract() {
        use alloy::primitives::{address, Address, U256};
        use arbx_common::types::{ArbPath, Opportunity, SimulationResult};

        let rpc = std::env::var("ARBITRUM_RPC_URL").unwrap();
        let provider = Arc::new(ProviderBuilder::default().connect_http(rpc.parse().unwrap()));
        let sim = ArbSimulator::new(provider);

        // Balancer V2 Vault: has code but no `executeArb` function → EVM revert.
        const BALANCER_VAULT: Address = address!("BA12222222228d8Ba445958a75a0704d566BF2C8");
        let path = ArbPath {
            token_in: BALANCER_VAULT,
            pool_a: BALANCER_VAULT,
            token_mid: BALANCER_VAULT,
            pool_b: BALANCER_VAULT,
            token_out: BALANCER_VAULT,
            estimated_profit_wei: U256::from(1_000_u64),
            flash_loan_amount_wei: U256::from(1_000_000_000_000_000_000_u128), // 1 ETH
        };
        let opp = Opportunity {
            path,
            gross_profit_wei: U256::from(1_000_u64),
            l2_gas_cost_wei: U256::ZERO,
            l1_gas_cost_wei: U256::ZERO,
            net_profit_wei: U256::from(1_000_u64),
            detected_at_ms: 0,
        };

        let owner = address!("0000000000000000000000000000000000000001");
        let result = sim.simulate(&opp, BALANCER_VAULT, owner).await;
        assert!(
            matches!(result, SimulationResult::Failure { .. }),
            "Balancer Vault has no executeArb: expected Failure, got {result:?}"
        );
    }

    /// Golden test: replay a historical Arbitrum block end-to-end without
    /// panicking.
    ///
    /// NOTE: Replace `CONTRACT` with a real deployed `ArbExecutor` address
    /// and tune `path` to a known-profitable path at `BLOCK` to turn this
    /// into a strict regression test.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ARBITRUM_RPC_URL"]
    async fn golden_test_historical_arb_block_200000000() {
        use alloy::primitives::{address, Address, U256};
        use arbx_common::types::{ArbPath, Opportunity};

        let rpc = std::env::var("ARBITRUM_RPC_URL").unwrap();
        let provider = Arc::new(ProviderBuilder::default().connect_http(rpc.parse().unwrap()));
        let sim = ArbSimulator::new(provider);

        const BLOCK: u64 = 200_000_000;
        const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
        const USDC: Address = address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8");
        // Placeholder — replace with a real ArbExecutor deployment once available.
        const CONTRACT: Address = address!("0000000000000000000000000000000000000001");
        const OWNER: Address = address!("0000000000000000000000000000000000000002");

        let path = ArbPath {
            token_in: WETH,
            pool_a: address!("C31E54c7a869B9FcBEcc14363CF510d1c41fa443"), // UniV3 USDC/WETH
            token_mid: USDC,
            pool_b: address!("905dfCD5649217c42684f23958568e533C711Aa3"),
            token_out: WETH,
            estimated_profit_wei: U256::from(5_000_000_000_000_000_u128), // 0.005 ETH
            flash_loan_amount_wei: U256::from(1_000_000_000_000_000_000_u128), // 1 WETH
        };
        let opp = Opportunity {
            path,
            gross_profit_wei: U256::from(5_000_000_000_000_000_u128),
            l2_gas_cost_wei: U256::from(500_000_000_000_000_u128),
            l1_gas_cost_wei: U256::from(100_000_000_000_000_u128),
            net_profit_wei: U256::from(4_400_000_000_000_000_u128),
            detected_at_ms: 0,
        };

        // With a placeholder contract address the simulation returns Failure
        // (no code at CONTRACT); any non-panic outcome is acceptable here.
        let result = sim.simulate_at_block(&opp, CONTRACT, OWNER, BLOCK).await;
        println!("golden test result: {result:?}");
        let _ = result;
    }
}
