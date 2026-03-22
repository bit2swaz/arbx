# arbx — Single Source of Truth

## Project Name
**arbx** — Arbitrum atomic arbitrage engine

---

## Mission Statement
A self-funded, flash-loan-powered atomic arbitrage engine targeting DEX price
inefficiencies on Arbitrum. Correctness first, latency second. Fully independent,
fully open source, fully documented.

---

## Build Status

**Last updated:** 2026-03-23

| Phase | Description | Status | Commit |
|---|---|---|---|
| 0.1 | Git, CI, linting baseline | ✅ | — |
| 0.2 | Secret scanning, dep pinning | ✅ | — |
| 1.1 | Workspace scaffold | ✅ | — |
| 1.2 | Shared types + full tests | ✅ | — |
| 1.3 | Config system + env expansion | ✅ | — |
| 1.4 | Observability: metrics + tracing | ✅ | — |
| 2.1 | Foundry project setup | ✅ | — |
| 2.2 | ArbExecutor.sol implementation | ✅ | — |
| 2.3 | Contract tests: Arbitrum fork TDD | ✅ | — |
| 2.4 | Fuzz tests and invariants | ✅ | — |
| 2.5 | Deploy script | ✅ | — |
| 3.1 | Pool state store + concurrent tests | ✅ | — |
| 3.2 | Sequencer feed + reconnection tests | ✅ | — |
| 3.3 | Block reconciler + staleness tests | ✅ | — |
| 4.1 | Two-hop path scanner + property tests | ✅ | — |
| 4.2 | Profit calculator + 2D gas tests | ✅ | — |
| 5.1 | revm fork infrastructure | ✅ | — |
| 5.2 | Full arb simulation + regression suite | ✅ | — |
| 6.1 | Transaction submitter + mock tests | ✅ | — |
| 6.2 | PnL tracker + persistence tests | ✅ | — |
| 7.1 | Full pipeline wiring | ✅ | — |
| 7.2 | Integration test suite | ✅ | — |
| 8.1 | Comprehensive property test suite | ✅ | — |
| 8.2 | Chaos tests: feed + RPC fault injection | ✅ | `ab80e6d` |
| 8.3 | Benchmarking infrastructure | ✅ | `9cd34f2` |
| 9.1 | Testnet infrastructure + smoke tests | ✅ | `743e21b` |
| 9.2 | Anvil fork validation (pre-mainnet) | ✅ | `1e2ade1` |
| 10.1 | Mainnet launch | ✅ | `a1e1df2` |

**Codebase stats (2026-03-23):**
- Rust: **10,550 lines** across 27 files
- Solidity: **1,916 lines** across 5 files
- Tests: **186 passing**, 0 failing, 0 unexpected ignores
- Benchmarks: **5 hot-path benchmarks** (Criterion 0.5)

---

## Core Invariants
These never change regardless of how the system evolves:

1. **Never hold inventory** — every transaction is atomic, opens and closes in one block
2. **Never execute without simulation** — if simulation does not profit, we do not submit
3. **Never lose more than gas** — flash loan means zero principal risk
4. **Correctness before speed** — a fast wrong bot just burns gas faster
5. **Always revert cleanly** — failed arb = full revert, never partial execution

---

## Why Arbitrum (Not Base or Mainnet)

**Mainnet:** A bloodbath. Institutional players with dedicated fiber lines and
co-located infrastructure dominate. Not viable on $60.

**Base (OP Stack):** Centralized sequencer operates as a black box. No public pending
mempool. Pure reactive arbitrage turns into statistical probability and state-spamming.
You cannot snipe what you cannot see.

**Arbitrum:** Has a sequencer feed (`wss://arb1.arbitrum.io/feed`) that broadcasts
transactions the instant they are ordered. This gives real visibility into transaction
flow. Arbitrum uses a FCFS (First-Come, First-Served) model, making backrunning
deterministic rather than probabilistic. Competition is real but less entrenched
than mainnet.

---

## Why Backrunning (Not Frontrunning or Sandwiching)

**Frontrunning:** Requires seeing a transaction before it executes. Arbitrum has no
public pending mempool — transactions are private until sequenced. Impossible.

**Sandwiching:** Requires predicting exact victim slippage and holding volatile token
inventory. High capital requirement. Inventory risk. Out of scope entirely.

**Atomic arbitrage via backrunning:** We see a large swap hit a pool via the sequencer
feed, instantly compute the resulting price dislocation, and fire our arb transaction
to capture the correction. Zero inventory risk. If the math does not check out, the
transaction reverts and we lose only gas.

---

## System Architecture

### Three Layers

```
Layer 1 — Eyes     (Ingestion)
Layer 2 — Brain    (Opportunity Detection + Simulation)
Layer 3 — Hands    (Execution + Submission)
```

---

## Layer 1 — Ingestion Engine

**Language:** Rust
**Purpose:** Monitor Arbitrum state in real time via the Sequencer Feed

### What It Does
- Connects to the Arbitrum Sequencer Feed (`wss://arb1.arbitrum.io/feed`)
- Uses `sequencer_client` crate (0.4+) to parse feed messages — do NOT build a
  custom parser; the feed uses compressed batched formats that are error-prone to
  decode manually
- Detects large swap transactions the instant they are sequenced
- Maintains in-memory pool state using `DashMap` — concurrent HashMap, no mutex
  needed for reads, critical when a worker pool is hammering it in parallel
- Reconciles pool state every block via RPC to catch any feed misses
- Feeds raw state changes to Layer 2

### Why sequencer_client Crate
Building a manual parser for the Arbitrum sequencer feed requires handling:
- Compressed batch formats
- L1 rollup message decoding
- Custom BroadcastMessage / BroadcastFeedMessage structs
- ParseL2Transactions logic with decompression

The `sequencer_client` crate handles all of this, parses into alloy types, and
manages reconnections automatically. Saves 3-5 days of painful debugging. Use it.

### Key Components
- `sequencer_client` crate for feed connection, parsing, and reconnection
- Exponential backoff reconnection: 1s → 2s → 4s → 8s → 16s → 32s cap
- Semaphore-controlled Tokio worker pool for parallel opportunity evaluation
- `DashMap<Address, PoolState>` for in-memory pool state (lock-free concurrent reads)
- Block listener for periodic RPC state reconciliation (ground truth)
- `BlockPoller` — local-fork-only component that polls Anvil blocks for direct
  pool swap transactions (complements the feed, which only sees top-level calls)

### Important: Sequencer Feed Delivers Top-Level Transactions Only
The live Arbitrum sequencer feed delivers the first call in each transaction's
call stack. When users swap via routers (`exactInputSingle`, `swapExactTokensForTokens`
etc.) the internal `pool.swap()` call is **not** visible in the feed. Only direct
pool calls from MEV bots appear. In production, the detection pipeline is primarily
triggered by detecting reserve changes via block reconciliation, not feed scanning.
The feed is still useful for speed — it delivers transactions before they are mined.

### Timeboost Awareness
Arbitrum's Timeboost auction gives winning searchers a 200ms express lane advantage.
On a $60 budget, we do not win Timeboost auctions. Our edge is simulation speed
within the non-express lane. Timeboost participation is a Phase 4 consideration
after the bot is profitable.

### Target DEXes
- Uniswap V3 on Arbitrum
- Camelot V2 (Arbitrum native — start here, simpler pool math than V3)
- SushiSwap on Arbitrum
- Trader Joe V1 on Arbitrum

### Target Pairs — Start Mid-Tier
USDC/ETH is the most competitive pair on Arbitrum. Dominated by well-capitalised
bots with VPS infrastructure already in place.

Start with:
- ARB/USDT
- WBTC/ETH
- Other mid-tier pairs with real liquidity but thinner competition

Add USDC/ETH in Phase 3 once the system is proven profitable on easier pairs.

### Data Tracked Per Pool
- Token0, Token1
- Reserve0, Reserve1
- Fee tier
- Last updated block

---

## Layer 2 — Opportunity Detection + Simulation

**Language:** Rust
**Purpose:** Find profitable arb paths and verify them before touching mainnet

### 2a — Opportunity Detector

**What it does:**
- Watches for reserve updates triggered by large swaps from Layer 1
- Computes post-swap spot prices on the affected pool
- Scans all two-hop paths that include the affected pool
- Flags discrepancies above the dynamic minimum profit threshold

**Path structure — two-hop only in Phase 1:**
```
Token A → Pool 1 → Token B → Pool 2 → Token A
```

Example:
```
USDC → Uniswap V3 → ETH → Camelot V2 → USDC
If output USDC > input USDC = opportunity
```

**Why two-hop only first:**
Easiest to reason about mathematically. Lowest simulation complexity. Get this
working and profitable before introducing three-hop paths in Phase 3.

**Pathfinding — Phase 1:**
Brute-force pair scanning is acceptable for two-hop paths. Simple, correct, easy
to debug. Do not over-engineer before you have a working bot.

**Pathfinding — Phase 3:**
Add `petgraph` crate for a proper token-to-pools DAG when three-hop paths are
introduced. Not needed now.

### 2b — Profit Calculator

For each flagged opportunity, compute:

```
gross_profit   = output_amount - input_amount
flash_loan_fee = 0              (Balancer V2 is always 0%)
gas_cost       = l2_execution_gas_cost + l1_calldata_gas_cost
net_profit     = gross_profit - gas_cost
```

**Critical: Arbitrum uses a 2-Dimensional Gas Model.**

A standard `eth_estimateGas` call only returns the L2 execution component. There
are two real costs to every Arbitrum transaction:

1. **L2 Execution Gas** — computational cost of running your Solidity contract.
   Usually fractions of a cent. This is what `eth_estimateGas` returns.

2. **L1 Calldata Gas** — cost of posting your transaction data back to Ethereum
   mainnet for data availability. This is the silent killer. It fluctuates wildly
   with mainnet congestion and can spike to $1.50+ on a busy mainnet day, completely
   eating your profit buffer and turning a winning simulation into an on-chain loss.

**The Fix — query Arbitrum's NodeInterface precompile:**

Before submitting, call `gasEstimateL1Component` on Arbitrum's `NodeInterface`
precompile at address `0x00000000000000000000000000000000000000C8` via alloy-rs
to get the accurate L1 calldata cost component:

```rust
// NodeInterface precompile address on Arbitrum
const NODE_INTERFACE: Address =
    address!("00000000000000000000000000000000000000C8");

// Call gasEstimateL1Component to get true L1 cost
// Returns: (gasEstimateForL1, baseFee, l1BaseFeeEstimate)
// Total true gas cost = L2 estimate + gasEstimateForL1
```

**True gas cost formula:**
```
l2_gas_cost = eth_estimateGas() * l2_gas_price
l1_gas_cost = nodeInterface.gasEstimateL1Component() * l1_base_fee
total_gas_cost = l2_gas_cost + l1_gas_cost
net_profit = gross_profit - total_gas_cost
```

**Dynamic minimum threshold — do not hardcode $2:**
```
min_profit = total_gas_cost * 1.1 + $0.50
```
- `total_gas_cost` now includes both L2 and L1 components
- `* 1.1` provides 10% buffer above true gas cost
- `+ $0.50` covers slippage variance and estimation error
- Adapts automatically to both L2 and mainnet gas price changes

Only proceed to simulation if `net_profit > min_profit`.

**Why this matters on $60:**
If mainnet gas spikes and you are using only `eth_estimateGas`, your bot will
submit transactions where the real cost is $1.50 but you estimated $0.10. On a
$60 budget, a few of these wipe you out before you ever see a profitable execution.
The NodeInterface call is cheap — always make it.

**Why Balancer V2 over Aave V3:**
Aave V3 charges 0.09% (9 basis points) per flash loan.
Balancer V2 charges 0%. Always.

On razor-thin arb margins, eliminating 9bps changes the profitability math
entirely. Bots paying Aave fees mathematically cannot take opportunities that
Balancer-powered bots can. This is a structural edge.

Example:
```
$10,000 flash loan, $15 gross profit opportunity

With Aave:    fee = $9.00  →  net = $6.00  →  marginal, risky
With Balancer: fee = $0.00  →  net = $15.00  →  clear winner
```

### 2c — Simulation Engine

**Before submitting anything to mainnet, always simulate. No exceptions.**

Use **revm** (Rust EVM) to simulate the full transaction locally:

1. Fork current Arbitrum state via archive RPC
2. Simulate Balancer V2 flash loan borrow (fee = 0, always)
3. Simulate swap on DEX 1
4. Simulate swap on DEX 2
5. Simulate flash loan repayment (principal only)
6. Verify net positive output after gas

If simulation succeeds and profit clears threshold → pass to Layer 3.
If simulation fails → discard, log reason (slippage? stale reserves? state race?),
move on.

**Why revm:**
~10x faster than Geth-based forks. Runs entirely in-process. No external call
overhead. Perfect for a $60 budget where every failed on-chain transaction counts.

**Known edge case — Arbitrum delayed inbox:**
L1 → L2 transactions can affect Arbitrum state mid-block unexpectedly. revm
supports custom forks that handle this. Treat as a known edge case — handle in
Phase 3 when optimising simulation accuracy. Not a blocker for Phase 1-2.

**This is your correctness shield. Never skip this step.**

---

## Layer 3 — Execution Engine

**Language:** Rust + Solidity (smart contract)
**Purpose:** Execute verified opportunities on-chain

### 3a — The Smart Contract

Written in Solidity. Deployed on Arbitrum. Implements Balancer V2's
`IFlashLoanRecipient` interface — NOT Aave's `IFlashLoanReceiver`.

**What it does:**
1. Receives call from bot with encoded opportunity parameters
2. Calls Balancer V2 Vault to initiate flash loan
3. Vault calls back into `receiveFlashLoan`
4. Executes swap on DEX 1
5. Executes swap on DEX 2
6. Enforces profit: `require(output >= input + minProfitWei, "No profit")`
7. Repays flash loan principal only (feeAmounts[0] is always 0)
8. Sends profit to owner wallet
9. Reverts entirely if profit condition not met

**Critical property — double protection:**
Off-chain simulation catches most failures before they hit mainnet.
The contract's `require` catches anything that slipped through due to state
changes between simulation and execution. You never execute a losing trade.

**Balancer V2 interface:**
```solidity
import {IVault} from
    "@balancer-labs/v2-interfaces/contracts/vault/IVault.sol";
import {IFlashLoanRecipient} from
    "@balancer-labs/v2-interfaces/contracts/vault/IFlashLoanRecipient.sol";

contract ArbExecutor is IFlashLoanRecipient {
    IVault private immutable balancerVault;
    address private immutable owner;
    uint256 private immutable minProfitWei;

    constructor(address _vault, uint256 _minProfit) {
        balancerVault = IVault(_vault);
        owner = msg.sender;
        minProfitWei = _minProfit;
    }

    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts, // always 0 on Balancer V2
        bytes memory userData
    ) external override {
        require(msg.sender == address(balancerVault), "Not Balancer");
        // feeAmounts[0] == 0 always — no fee charged

        // 1. Execute swap on DEX 1
        // 2. Execute swap on DEX 2

        // 3. Enforce minimum profit before repaying
        require(
            tokens[0].balanceOf(address(this)) >= amounts[0] + minProfitWei,
            "No profit"
        );

        // 4. Repay principal only — no fee
        tokens[0].transfer(address(balancerVault), amounts[0]);

        // 5. Send profit to owner
        tokens[0].transfer(owner, tokens[0].balanceOf(address(this)));
    }
}
```

**Constructor parameters:**
- `_vault`: Balancer V2 Vault address on Arbitrum
- `_minProfit`: Minimum profit in wei (set conservatively, update via owner function)

### 3b — Transaction Submitter

**What it does:**
- Takes verified opportunity from Layer 2
- Encodes calldata via alloy-rs
- Estimates gas with buffer: `simulation_gas * 1.2`
- Submits directly to Arbitrum sequencer endpoint for minimum latency
- Logs result with full revert reason if failed

**Submission targets:**
- Arbitrum sequencer RPC directly (lowest latency)
- Alchemy / QuickNode RPC as fallback

**Note:** Flashbots is not used here. Flashbots MEV-Boost is an Ethereum mainnet
architecture. Arbitrum has its own sequencer — submit directly to it.

**Revert reason logging:**
Revert reasons are your primary debugging signal. Log them always:
- `"No profit"` → state changed between simulation and execution (race condition)
- Slippage error → reserve model was stale, check reconciliation timing
- Gas error → increase buffer multiplier from 1.2

---

## Data Flow End to End

```
Arbitrum Sequencer Feed broadcasts sequenced swap tx
        ↓
sequencer_client parses feed message into alloy tx
        ↓
Worker pool identifies large DEX swap, updates DashMap pool state
        ↓
Opportunity detector scans two-hop paths on affected pool
        ↓
Profit calculator: net_profit > gas_estimate * 1.1 + $0.50?
        ↓
revm forks Arbitrum state, simulates full arb tx end-to-end
        ↓
Simulation profitable? → Submitter encodes calldata + fires backrun to sequencer
        ↓
Result logged: success / revert reason / gas cost
        ↓
PnL tracker updated
```

---

## Observability — Full Funnel

Every component logs. Reading the funnel tells you exactly where the system leaks:

| Metric | If Low or Wrong, Check |
|---|---|
| Opportunities detected / min | Is sequencer feed connected? Is pool state updating? |
| Cleared profit threshold | Is dynamic threshold too high? Is gas spiking? |
| Cleared simulation | Is reserve model stale? Is revm fork accurate? |
| Transactions submitted | Is submitter firing? Is RPC responding? |
| Transactions succeeded | On-chain success rate |
| Transactions reverted + reason | Race? Slippage? Gas? Each has a different fix |
| Net PnL running total | Are you making money? |
| Gas spent total | Budget tracking against $60 |

**Funnel diagnosis:**
- High detections, low threshold clears → profit floor too high or gas too expensive
- High threshold clears, low sim clears → reserve model stale or wrong
- High sim clears, high revert rate → state races, reduce submission latency
- Low revert rate, negative PnL → gas eating profit, need larger opportunities

---

## Repository Structure

```
arbx/
├── contracts/
│   ├── src/
│   │   └── ArbExecutor.sol           # IFlashLoanRecipient, Balancer V2
│   └── test/
│       └── ArbExecutor.t.sol         # Foundry tests
├── crates/
│   ├── ingestion/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── sequencer_feed.rs     # sequencer_client wrapper + reconnection
│   │       ├── pool_state.rs         # DashMap<Address, PoolState> + RPC reconcile
│   │       └── block_poller.rs       # Anvil block watcher (local fork only)
│   ├── detector/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── opportunity.rs        # Two-hop path scanner
│   │       └── profit.rs             # Dynamic threshold calculator
│   ├── simulator/
│   │   └── src/
│   │       ├── lib.rs
│   │       └── revm_sim.rs           # Fork + simulate full arb tx
│   ├── executor/
│   │   └── src/
│   │       ├── lib.rs
│   │       └── submitter.rs          # Calldata encoding + sequencer submission
│   └── common/
│       └── src/
│           ├── lib.rs
│           ├── types.rs              # PoolState, Opportunity, ArbPath
│           └── config.rs             # Config loading from TOML + env
├── bin/
│   └── arbx.rs                       # Entry point, wires all crates
├── config/
│   └── default.toml
├── Cargo.toml
└── README.md
```

---

## Tech Stack

| Component | Technology |
|---|---|
| Core engine | Rust |
| Ethereum interaction | alloy-rs |
| EVM simulation | revm |
| Async runtime | Tokio |
| Smart contracts | Solidity + Foundry |
| Flash loans | Balancer V2 Vault (0% fee) |
| Data feed | Arbitrum Sequencer Feed |
| Feed parsing | sequencer_client crate (0.4+) |
| Pool state store | DashMap (concurrent, lock-free reads) |
| Pathfinding (Phase 3+) | petgraph |
| RPC provider | Alchemy / QuickNode (Arbitrum) |
| Config | TOML + env vars |
| Logging | tracing + tracing-subscriber |
| Metrics | Prometheus client |

---

## Contract Addresses (Verified On-Chain)

| Contract | Address |
|---|---|
| Balancer V2 Vault (Arbitrum) | `0xBA12222222228d8Ba445958a75a0704d566BF2C8` |
| Uniswap V3 Factory (Arbitrum) | `0x1F98431c8aD98523631AE4a59f267346ea31F984` |
| Camelot V2 Factory | `0x6EcCab422D763aC031210895C81787E87B43A652` |
| SushiSwap Factory (Arbitrum) | `0xc35DADB65012eC5796536bD9864eD8773aBc74C4` |
| Trader Joe V1 Factory | `0x9Ad6C38BE94206cA50bb0d90783181662f0CfA10` |

---

## Configuration (default.toml)

```toml
[network]
rpc_url            = "${ARBITRUM_RPC_URL}"
sequencer_feed_url = "wss://arb1.arbitrum.io/feed"
chain_id           = 42161

[strategy]
# Dynamic threshold: gas_estimate * gas_buffer_multiplier + min_profit_floor_usd
# Do not use a hardcoded min_profit_usd — it does not adapt to gas volatility
min_profit_floor_usd   = 0.50
gas_buffer_multiplier  = 1.1
max_gas_gwei           = 0.1
flash_loan_fee_bps     = 0    # Balancer V2 is always 0

[pools]
balancer_vault     = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
uniswap_v3_factory = "0x1F98431c8aD98523631AE4a59f267346ea31F984"
camelot_factory    = "0x6EcCab422D763aC031210895C81787E87B43A652"
sushiswap_factory  = "0xc35DADB65012eC5796536bD9864eD8773aBc74C4"
traderjoe_factory  = "0x9Ad6C38BE94206cA50bb0d90783181662f0CfA10"

[execution]
contract_address           = "${ARB_EXECUTOR_ADDRESS}"
private_key                = "${PRIVATE_KEY}"
max_concurrent_simulations = 10
gas_estimate_buffer        = 1.2  # simulation_gas * 1.2 for on-chain submission

# Arbitrum 2D gas model
# Always query NodeInterface for true total gas cost before submitting
# eth_estimateGas alone only returns L2 execution cost — misses L1 calldata cost
node_interface_address     = "0x00000000000000000000000000000000000000C8"

[observability]
log_level    = "info"
metrics_port = 9090
```

---

## Development Phases

### Phases 0–8 — Complete ✅

All engineering phases through Phase 8.3 (Benchmarking Infrastructure) are complete
and committed. The full Rust engine — ingestion, detection, simulation, execution,
property tests, chaos tests, and benchmarks — is built and green.

**Codebase:** 6,680 lines of Rust · 1,079 lines of Solidity · 172 tests passing

---

### Phase 9 — Testnet Validation ✅ + Anvil Fork Validation ✅

#### 9.1 — Arbitrum Sepolia smoke test (complete, commit `743e21b`)

Arbitrum Sepolia is a ghost chain with zero DEX liquidity.  The feed was
alive and pool-discovery infrastructure was validated.  A `--self-test` flag
was added to the binary to prove the detect pipeline with synthetic data when
no real swaps exist on the testnet.

**Outcome:** infrastructure validated, synthetic smoke test passes, 179 tests
green.  Real end-to-end arb logic (detect → simulate → submit) was NOT
exercised because no swap activity exists on Arbitrum Sepolia.

#### 9.2 — Anvil mainnet fork validation ✅ (commit `1e2ade1`, 2026-03-22)

Ran the full bot against a pinned Arbitrum mainnet fork (Anvil) with real
pool state and the live sequencer feed. `dry_run = true` — no real ETH spent.

**What was built / fixed in this phase:**
- `probe_pool()` public function in `sequencer_feed.rs` — auto-detects V2 vs V3
  ABI for each known_pool address at startup, replacing hardcoded
  `DexKind::CamelotV2 + Address::ZERO` seeds that were breaking the reconciler
  and producing empty token0/token1 for all pools.
- `BlockPoller` (`crates/ingestion/src/block_poller.rs`) — new ingestion component
  that polls the local Anvil fork every 250ms for new blocks, extracts direct
  pool swap transactions (by selector + pool_store membership), and feeds them
  into the same detection channel as the sequencer feed. Only enabled when
  `rpc_url` contains `127.0.0.1`. Required because the live Arbitrum sequencer
  feed delivers only top-level L2 transactions (router calls), never internal
  `pool.swap()` calls.
- `run_anvil_fork.sh` updated: `--block-time 2` added so Anvil mines blocks;
  `cast send` synthetic V3 swap injected 12s after startup to guarantee
  BlockPoller detection fires on every test run.

**Root cause discovered:** The Arbitrum sequencer feed (`wss://arb1.arbitrum.io/feed`)
delivers only top-level L2 transactions. User DEX swaps go via routers
(`Router.exactInputSingle()` → `Pool.swap()` internally) — only the router call
appears in the feed. Direct `pool.swap()` calls are rare and only come from
advanced MEV bots bypassing routers. The `BlockPoller` solves this for the
local fork validation context.

**Outcome:** `smoke test PASS`, `opportunities_detected = 4`. All 179 tests green.

See `docs/ANVIL_FORK_VALIDATION.md` for the complete runbook.

---

### Phase 10 — Mainnet Launch Infrastructure ✅

#### 10.1 — Mainnet deploy, budget tracker, kill switch (complete, commit `a1e1df2`, 2026-03-23)

Built the infrastructure required to attempt the first real-money mainnet run
without operating blind.

**What was added in this phase:**
- `config/mainnet.toml` — dedicated mainnet config with mid-tier pools only
  (`ARB/USDT`, `WBTC/ETH`), `dry_run = false`, and a strict execution budget.
- Runtime budget support in `crates/common/src/config.rs`,
  `crates/common/src/pnl.rs`, and `bin/arbx.rs` — explicit `[budget]` section,
  threshold-aware PnL tracking, warn threshold, and kill-switch shutdown.
- `scripts/deploy-mainnet.sh` and `scripts/run-mainnet.sh` — explicit typed
  confirmations before deployment and live execution.
- `scripts/pnl_report.sh` and `docs/MAINNET_LAUNCH.md` — operator tooling and
  runbook for low-budget mainnet operation.
- Integration coverage for budget exhaustion, warning-threshold behavior, and
  persisted PnL reload across restart.

**Budget model implemented:**
- Total real-money budget: **$31 USD** (~3000 INR)
- Estimated deploy cost: **~$4 USD**
- Execution budget: **$27 USD**
- Warning threshold: **$5 USD remaining**
- Kill threshold: **$2 USD remaining**

**Outcome:** `cargo build --release` passes, `cargo test --workspace` passes,
mainnet launch scripts and kill-switch infrastructure are in place.

See `docs/MAINNET_LAUNCH.md` for the operator runbook.

---

### Phase 11 — Optimisation ← CURRENT

Goals:
- Profile hot paths with `cargo-flamegraph`, eliminate bottlenecks
- Reduce simulation latency — revm fork time is the likely bottleneck
- Add three-hop paths using `petgraph` DAG
- Co-locate on **Hetzner Frankfurt VPS** (€5/month, CX21: 2 vCPU, 4GB RAM)
- Lower minimum profit floor as system confidence grows
- Add USDC/ETH and other high-volume competitive pairs
- Handle Arbitrum delayed inbox edge case in revm fork
- Research Timeboost express lane participation cost vs benefit

**Why Frankfurt VPS:**
Physical distance to the sequencer is a direct latency cost. A request from
India to EU sequencer adds ~200ms round-trip. In MEV, 200ms is a lifetime.
Frankfurt cuts this to under 5ms.

---

### Phase 12 — Expansion

Goals:
- Add liquidation detection (Aave V3 on Arbitrum has large positions)
- Add Camelot V3 and Trader Joe V2 DEX integrations
- Timeboost express lane participation if economics justify it
- Write full blog series documenting the entire build end-to-end
- Open source with complete documentation
- Grant applications: Ethereum Foundation ESP, Flashbots grants

---

## Known Risks and Edge Cases

**Arbitrum 2D gas model — L1 calldata cost:**
Every Arbitrum transaction pays two costs: L2 execution gas and L1 calldata gas
(posting transaction data back to Ethereum mainnet). Standard `eth_estimateGas`
only returns the L2 component. During mainnet gas spikes, L1 calldata cost can
jump to $1.50+, silently turning profitable simulations into losing on-chain
transactions. Always query the NodeInterface precompile at
`0x00000000000000000000000000000000000000C8` via `gasEstimateL1Component` to
get the true total cost before submitting. This is non-negotiable on a $60 budget.

**State races:**
Between simulation and on-chain execution, another bot may have taken the same
opportunity. The contract's profit `require` catches this — you lose only the gas
cost of the revert. High revert rate with `"No profit"` reason = you are being
raced. Fix: reduce simulation-to-submission latency.

**Reserve staleness:**
The sequencer feed is reliable but not infallible. Block-level RPC reconciliation
is your ground truth. If simulation is passing but revert rate is high, your
reserve model may be stale between reconciliation windows.

**Arbitrum delayed inbox:**
L1 → L2 transactions can affect Arbitrum state mid-block unexpectedly. Known edge
case. Handle in Phase 3. Not a blocker for Phase 1-2.

**Timeboost structural disadvantage:**
Bots with express lane access have a 200ms structural head start in Phase 1-2.
Unavoidable on $60 budget. Mitigation: focus on price dislocations that persist
for multiple blocks rather than single-block races. Mid-tier pairs have this
characteristic — USDC/ETH does not.

**USDC/ETH competition:**
Most competitive pair on Arbitrum. Explicitly avoided until Phase 3. Losses on
this pair before the system is optimised will drain the $60 budget rapidly.

---

## Success Metrics

**End of Phase 2, answer all of these:**
- How many opportunities are detected per hour?
- What percentage clear the dynamic profit threshold?
- What percentage clear simulation?
- What is the on-chain success rate?
- What is actual net PnL after gas?
- What is the most common revert reason?

**Target by end of Phase 3:**
- Positive cumulative PnL
- Greater than 50% on-chain success rate (simulation pass → execution success)
- Less than 5ms simulation latency per opportunity
- Bot running stably on Frankfurt VPS without manual intervention
- At least one mid-tier pair generating consistent returns
