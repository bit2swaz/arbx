# arbx — ROADMAP (Full TDD, Production-Ready)

## How To Use This File

This roadmap is the complete execution plan for building arbx end-to-end — from
an empty directory to a profitable, production-grade, fully-tested MEV arbitrage
engine running on Arbitrum.

Every mini-phase contains:
- A clear goal and strict definition of done
- A complete, self-contained prompt to paste into Claude Sonnet 4.6 in your AI IDE
- The prompt includes all context the LLM needs — no additional input required

**The TDD contract:**
Every mini-phase writes tests BEFORE or ALONGSIDE implementation. No mini-phase
is complete until `cargo test` passes with zero failures and zero ignored tests
(except fork tests that require a live RPC, which are explicitly marked).

**Rules:**
1. Complete mini-phases in strict order
2. `cargo build`, `cargo clippy -- -D warnings`, and `cargo test` must all pass
   before marking any mini-phase complete
3. SSOT.md is the source of truth — if ROADMAP and SSOT conflict, SSOT wins
4. Keep both SSOT.md and ROADMAP.md in the repository root at all times
5. If a prompt produces broken code, paste the compiler error back and fix before
   moving on — never carry broken code forward

---

## Full Phase Overview

```
Phase 0  — Project Hygiene        (CI, git hooks, linting, security baseline)
Phase 1  — Foundation             (workspace, types, config, observability)
Phase 2  — Smart Contract         (ArbExecutor.sol, Foundry TDD, deploy)
Phase 3  — Ingestion Engine       (pool state, sequencer feed, reconciler)
Phase 4  — Opportunity Brain      (path scanner, 2D gas profit calculator)
Phase 5  — Simulation Engine      (revm fork, full arb sim, regression suite)
Phase 6  — Execution Engine       (NodeInterface gas, submission, PnL tracker)
Phase 7  — Integration            (full pipeline wiring, integration test suite)
Phase 8  — Property & Chaos Tests (proptest, feed chaos, RPC fault injection)
Phase 9  — Testnet Validation     (Arbitrum Sepolia, smoke tests, funnel check)
Phase 10 — Mainnet Launch         (deploy, budget tracker, kill switch)
Phase 11 — Optimisation           (flamegraphs, latency, three-hop, VPS)
Phase 12 — Expansion              (liquidations, Timeboost, more DEXes)
Phase 13 — Open Source & Grants   (docs, blog, EF ESP, Flashbots grants)
```

---

---

# PHASE 0 — Project Hygiene

**Goal:** Establish the non-negotiable production baseline before writing a single
line of business logic. CI must be green. Linting must be strict. Secrets must
never enter the repo. Security scanning must be automated. This phase costs almost
nothing to set up and saves you from catastrophic mistakes later.

---

## Mini-Phase 0.1 — Git, CI, and Linting Baseline

**Definition of done:**
- `.github/workflows/ci.yml` runs on every push and PR
- CI runs: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`,
  `cargo audit`
- Pre-commit hook prevents committing if any of the above fail locally
- `.gitignore` and `.env.example` exist — no real secrets ever enter the repo
- `cargo clippy -- -D warnings` passes on empty workspace

---

**PROMPT 0.1**

```
You are building `arbx`, a production-grade Arbitrum MEV arbitrage engine in Rust.
This is Mini-Phase 0.1: Git, CI, and Linting Baseline.

Read SSOT.md in full before writing any code.

Your task is to set up all project hygiene infrastructure. Write every file below
completely — no stubs, no TODOs.

File 1: `.github/workflows/ci.yml`
A GitHub Actions workflow that triggers on push and pull_request to main.
Jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable with components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo test --workspace
      - run: cargo audit
        (install: cargo install cargo-audit first via cache)
    env:
      ARBITRUM_RPC_URL: "https://arb1.alchemyapi.io/v2/test"
      ARBITRUM_SEPOLIA_RPC_URL: "https://arb-sepolia.g.alchemy.com/v2/test"
      ARB_EXECUTOR_ADDRESS: "0x0000000000000000000000000000000000000001"
      PRIVATE_KEY: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

File 2: `.github/workflows/security.yml`
A separate workflow that runs weekly and on push to main:
  - cargo audit (known vulnerability check)
  - cargo deny check (license compliance + duplicate dependency check)

File 3: `.gitignore`
Comprehensive gitignore for Rust + Foundry + secrets:
  /target
  /contracts/out
  /contracts/cache
  /contracts/lib
  *.env
  .env
  .env.*
  !.env.example
  logs/
  flamegraph.svg
  perf.data
  *.log

File 4: `.env.example`
Template showing every required environment variable with placeholder values:
  ARBITRUM_RPC_URL=https://arb-mainnet.g.alchemy.com/v2/YOUR_KEY_HERE
  ARBITRUM_SEPOLIA_RPC_URL=https://arb-sepolia.g.alchemy.com/v2/YOUR_KEY_HERE
  ARB_EXECUTOR_ADDRESS=0x0000000000000000000000000000000000000000
  PRIVATE_KEY=0x0000000000000000000000000000000000000000000000000000000000000000
  ARBISCAN_API_KEY=YOUR_ARBISCAN_KEY_HERE

File 5: `deny.toml`
cargo-deny config:
  [licenses]
  allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
           "BSD-3-Clause", "ISC", "Unicode-DFS-2016", "CC0-1.0", "Zlib"]
  [bans]
  multiple-versions = "warn"
  [advisories]
  vulnerability = "deny"
  unmaintained = "warn"
  yanked = "deny"

File 6: `.rustfmt.toml`
  edition = "2021"
  max_width = 100
  use_field_init_shorthand = true
  use_try_shorthand = true
  imports_granularity = "Crate"
  group_imports = "StdExternalCrate"

File 7: `scripts/pre-commit`
A shell script to install as a git pre-commit hook:
  #!/bin/sh
  set -e
  cargo fmt --check
  cargo clippy -- -D warnings
  cargo test --workspace --quiet
  echo "Pre-commit checks passed."

File 8: `scripts/install-hooks.sh`
  #!/bin/sh
  cp scripts/pre-commit .git/hooks/pre-commit
  chmod +x .git/hooks/pre-commit
  echo "Git hooks installed."

Also create an empty `Cargo.toml` at root with just:
  [workspace]
  members = []
  resolver = "2"

...so that cargo commands work from day one before the full workspace is populated.

Write every file completely. No placeholders.
```

---

## Mini-Phase 0.2 — Secret Scanning and Dependency Pinning

**Definition of done:**
- `gitleaks` config prevents accidental private key commits
- `Cargo.lock` is committed (binary — always lock deps)
- `rust-toolchain.toml` pins the exact Rust version used

---

**PROMPT 0.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.

This is Mini-Phase 0.2: Secret Scanning and Dependency Pinning.

File 1: `.gitleaks.toml`
Configure gitleaks to detect Ethereum private keys and API keys:
  title = "arbx gitleaks config"

  [[rules]]
  id = "ethereum-private-key"
  description = "Ethereum private key"
  regex = '''0x[0-9a-fA-F]{64}'''
  tags = ["key", "ethereum"]

  [[rules]]
  id = "alchemy-api-key"
  description = "Alchemy API key"
  regex = '''[a-zA-Z0-9_-]{32,}'''
  tags = ["key", "alchemy"]
  [rules.allowlist]
  paths = [".env.example", "ROADMAP.md", "SSOT.md", "docs/"]

Add gitleaks step to `.github/workflows/security.yml`:
  - uses: gitleaks/gitleaks-action@v2
    env:
      GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

File 2: `rust-toolchain.toml`
  [toolchain]
  channel = "1.88.0"
  components = ["rustfmt", "clippy", "rust-src"]
  targets = ["x86_64-unknown-linux-musl"]

File 3: Update `.github/workflows/ci.yml`
Add a job that verifies Cargo.lock is committed and up to date:
  - run: cargo update --locked
    (this fails if Cargo.lock is stale, ensuring deps are always pinned)

File 4: `SECURITY.md`
  # Security Policy
  ## Reporting Vulnerabilities
  Do not open a public GitHub issue for security vulnerabilities.
  Email: [your email]
  
  ## Known Risk Areas
  - PRIVATE_KEY env var: never log, never commit, rotate immediately if exposed
  - RPC endpoints: treat API keys as secrets
  - Smart contract: ArbExecutor.sol is not audited — use at your own risk
  - MEV competition: bot may lose gas on failed transactions — start with small budget

Write all files completely.
```

---

---

# PHASE 1 — Foundation

**Goal:** Build the complete Rust workspace with shared types, config, and
observability. Every type has tests. Config loading is tested with property-based
inputs. Observability metrics are verified to register and increment correctly.

---

## Mini-Phase 1.1 — Workspace Scaffold

**Definition of done:**
- `cargo build --workspace` passes with zero errors
- `cargo clippy --workspace -- -D warnings` passes with zero warnings
- Workspace structure exactly matches SSOT.md repository structure
- All crates compile as empty libraries

---

**PROMPT 1.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 1.1: Workspace Scaffold.

Create the complete Rust workspace. Write every file needed for
`cargo build --workspace` and `cargo clippy --workspace -- -D warnings`
to pass with zero errors and zero warnings.

Full repository structure from SSOT.md:
arbx/
├── contracts/
│   ├── src/ArbExecutor.sol        (empty placeholder)
│   └── test/ArbExecutor.t.sol     (empty placeholder)
├── crates/
│   ├── ingestion/src/
│   │   ├── lib.rs
│   │   ├── sequencer_feed.rs
│   │   ├── pool_state.rs
│   │   └── reconciler.rs
│   ├── detector/src/
│   │   ├── lib.rs
│   │   ├── opportunity.rs
│   │   ├── profit.rs
│   │   └── graph.rs
│   ├── simulator/src/
│   │   ├── lib.rs
│   │   └── revm_sim.rs
│   ├── executor/src/
│   │   ├── lib.rs
│   │   └── submitter.rs
│   └── common/src/
│       ├── lib.rs
│       ├── types.rs
│       ├── config.rs
│       ├── metrics.rs
│       └── pnl.rs
├── bin/arbx.rs
├── tests/
│   └── integration/
│       └── mod.rs                 (empty for now)
├── config/
│   ├── default.toml
│   └── sepolia.toml
├── Cargo.toml                     (workspace root)
└── README.md

Root Cargo.toml — workspace with shared deps:
  [workspace]
  members = [
    "crates/common",
    "crates/ingestion",
    "crates/detector",
    "crates/simulator",
    "crates/executor",
  ]
  resolver = "2"

  [[bin]]
  name = "arbx"
  path = "bin/arbx.rs"

  [workspace.dependencies]
  # Async runtime
  tokio = { version = "1", features = ["full"] }
  # Ethereum
  alloy = { version = "0.3", features = ["full"] }
  revm = { version = "14", features = ["std", "optional_balance_check"] }
  # Concurrency
  dashmap = "6"
  # Serialization
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  toml = "0.8"
  # Logging
  tracing = "0.1"
  tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
  # Errors
  anyhow = "1"
  thiserror = "1"
  # Networking
  futures = "0.3"
  futures-util = "0.3"
  tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
  # Metrics
  prometheus = "0.13"
  # Math
  uint = "0.9"
  # Testing
  proptest = "1"
  tokio-test = "0.4"
  mockall = "0.13"

Individual crate Cargo.toml requirements:
- Each must have [package] name, version="0.1.0", edition="2021"
- All deps use { workspace = true }
- common: serde, toml, anyhow, thiserror, tracing, prometheus, serde_json
- ingestion: tokio, alloy, dashmap, tracing, anyhow, futures-util,
  tokio-tungstenite; dep on common
- detector: alloy, tracing, anyhow, thiserror; dep on common
- simulator: revm, alloy, tracing, anyhow; dep on common
- executor: tokio, alloy, tracing, anyhow; dep on common
- bin: tokio, tracing, tracing-subscriber, anyhow; dep on all five crates

Every .rs file must compile. Use empty pub mod declarations.
Use #[allow(dead_code, unused_imports)] at crate level temporarily.
Write every single file completely. Do not skip any.
```

---

## Mini-Phase 1.2 — Shared Types with Full Test Coverage

**Definition of done:**
- All types in `common/src/types.rs` compile with correct derives
- Every type has at minimum: a construction test, a serialization round-trip test,
  and an equality test
- `cargo test -p arbx-common` passes with zero failures

---

**PROMPT 1.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 1.2: Shared Types with Full Test Coverage.

Write `crates/common/src/types.rs` with all shared types AND their complete
test suite. Tests are written in the same file in a #[cfg(test)] module.

Types to implement:

1. PoolState
   - address: Address, token0: Address, token1: Address
   - reserve0: U256, reserve1: U256
   - fee_tier: u32 (bps, e.g. 3000 = 0.3%)
   - last_updated_block: u64
   - dex: DexKind
   - Derives: Debug, Clone, PartialEq, Serialize, Deserialize

2. DexKind enum
   Variants: UniswapV3, CamelotV2, SushiSwap, TraderJoeV1
   Derives: Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize
   #[serde(rename_all = "snake_case")]

3. ArbPath
   - token_in: Address, pool_a: Address, token_mid: Address
   - pool_b: Address, token_out: Address
   - estimated_profit_wei: U256, flash_loan_amount_wei: U256
   - Derives: Debug, Clone, PartialEq, Serialize, Deserialize
   - Implement: fn is_circular(&self) -> bool { self.token_out == self.token_in }

4. Opportunity
   - path: ArbPath
   - gross_profit_wei: U256, l2_gas_cost_wei: U256
   - l1_gas_cost_wei: U256, net_profit_wei: U256
   - detected_at_ms: u64
   - Derives: Debug, Clone, PartialEq, Serialize, Deserialize
   - Implement: fn total_gas_cost_wei(&self) -> U256

5. SimulationResult enum
   - Success { net_profit_wei: U256, gas_used: u64 }
   - Failure { reason: String }
   - Derives: Debug, Clone, PartialEq
   - Implement: fn is_success(&self) -> bool, fn profit(&self) -> Option<U256>

6. SubmissionResult
   - tx_hash: TxHash, success: bool
   - revert_reason: Option<String>
   - gas_used: u64
   - l2_gas_cost_wei: U256, l1_gas_cost_wei: U256
   - net_pnl_wei: I256 (signed — can be negative after gas)
   - Derives: Debug, Clone, Serialize, Deserialize
   - Implement: fn is_profitable(&self) -> bool { self.net_pnl_wei > I256::ZERO }

7. GasEstimate
   - l2_gas_units: u64, l2_gas_price_wei: u128, l2_cost_wei: U256
   - l1_calldata_gas: u64, l1_base_fee_wei: u128, l1_cost_wei: U256
   - total_cost_wei: U256, total_cost_usd: f64
   - Derives: Debug, Clone, Serialize, Deserialize
   - Implement: fn total_cost_wei(&self) -> U256

Tests to write in #[cfg(test)] mod tests:

// PoolState tests
test_pool_state_construction — build a PoolState, assert all fields correct
test_pool_state_serde_roundtrip — serialize to JSON, deserialize back, assert eq
test_dex_kind_serde_snake_case — serialize DexKind::UniswapV3, assert "uniswap_v3"
test_dex_kind_all_variants — verify all 4 variants serialize/deserialize

// ArbPath tests
test_arb_path_circular_true — token_in == token_out, assert is_circular() true
test_arb_path_circular_false — token_in != token_out, assert is_circular() false
test_arb_path_serde_roundtrip

// Opportunity tests
test_opportunity_total_gas — l2=100, l1=50, assert total=150
test_opportunity_serde_roundtrip

// SimulationResult tests
test_simulation_success_is_success — Success variant, assert is_success() true
test_simulation_failure_is_not_success — Failure variant, assert is_success() false
test_simulation_success_profit — Success{profit: 1000}, assert profit() == Some(1000)
test_simulation_failure_profit — Failure, assert profit() == None

// SubmissionResult tests
test_submission_profitable — positive net_pnl_wei, assert is_profitable() true
test_submission_unprofitable — negative net_pnl_wei (gas > profit), assert false
test_submission_serde_roundtrip

Update common/src/lib.rs:
  pub mod types;
  pub mod config;  // stub for now
  pub mod metrics; // stub for now
  pub mod pnl;     // stub for now
  pub use types::*;

Write the complete file with all types and all tests. Every test must pass.
```

---

## Mini-Phase 1.3 — Config System with Env Var Expansion Tests

**Definition of done:**
- Config loads correctly from TOML + env var expansion
- Missing required env var returns a clear error
- Invalid TOML returns a clear error
- All edge cases tested
- `cargo test -p arbx-common` passes

---

**PROMPT 1.3**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 1.3: Config System with Env Var Expansion Tests.

Write `crates/common/src/config.rs` and `config/default.toml` with full tests.

Config structs (must exactly mirror SSOT.md TOML structure):

  #[derive(Debug, Clone, Deserialize)]
  pub struct Config {
      pub network: NetworkConfig,
      pub strategy: StrategyConfig,
      pub pools: PoolsConfig,
      pub execution: ExecutionConfig,
      pub observability: ObservabilityConfig,
  }

  NetworkConfig: rpc_url: String, sequencer_feed_url: String, chain_id: u64
  StrategyConfig: min_profit_floor_usd: f64, gas_buffer_multiplier: f64,
                  max_gas_gwei: f64, flash_loan_fee_bps: u64
  PoolsConfig: balancer_vault: String, uniswap_v3_factory: String,
               camelot_factory: String, sushiswap_factory: String,
               traderjoe_factory: String
  ExecutionConfig: contract_address: String, private_key: String,
                   max_concurrent_simulations: usize, gas_estimate_buffer: f64,
                   node_interface_address: String
  ObservabilityConfig: log_level: String, metrics_port: u16

Implement:
  impl Config {
      pub fn load(path: &str) -> anyhow::Result<Self>
      // Reads TOML, expands ${VAR_NAME} patterns with env vars
      // Returns Err if file not found, invalid TOML, or missing env var

      pub fn load_str(toml_str: &str) -> anyhow::Result<Self>
      // Same but from a string — used in tests to avoid filesystem

      fn expand_env_vars(input: &str) -> anyhow::Result<String>
      // Replaces all ${VAR_NAME} with env var values
      // Returns Err("Missing env var: VAR_NAME") if any var is unset
  }

Write `config/default.toml` exactly as specified in SSOT.md with all verified
contract addresses and comments.

Write `config/sepolia.toml`:
  chain_id = 421614
  sequencer_feed_url = "wss://sepolia-rollup.arbitrum.io/feed"
  rpc_url = "${ARBITRUM_SEPOLIA_RPC_URL}"
  min_profit_floor_usd = 0.01
  max_gas_gwei = 1.0
  All other fields same as default.toml

Tests in #[cfg(test)] mod tests:

test_load_str_valid_config — inline TOML with env vars pre-substituted,
  assert chain_id = 42161, flash_loan_fee_bps = 0

test_expand_env_vars_substitutes_correctly — set env var TEST_VAR=hello,
  input "${TEST_VAR}", assert output "hello"

test_expand_env_vars_multiple — two vars in one string, both substituted

test_expand_env_vars_missing_var — unset var, assert Err contains var name

test_expand_env_vars_no_vars — plain string, returned unchanged

test_load_str_invalid_toml — garbage input, assert Err

test_balancer_vault_address — load default config, assert balancer_vault ==
  "0xBA12222222228d8Ba445958a75a0704d566BF2C8"

test_node_interface_address — assert node_interface ==
  "0x00000000000000000000000000000000000000C8"

test_flash_loan_fee_is_zero — assert flash_loan_fee_bps == 0

test_chain_id_arbitrum — assert chain_id == 42161

Write all files completely. Every test must pass.
```

---

## Mini-Phase 1.4 — Observability: Metrics and Tracing

**Definition of done:**
- All eight Prometheus metrics from SSOT register without panicking
- Metrics increment correctly
- Tracing subscriber initialises with correct log level from config
- `cargo test -p arbx-common` passes

---

**PROMPT 1.4**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 1.4: Observability: Metrics and Tracing.

Write `crates/common/src/metrics.rs` with complete tests.

Implement the Metrics struct tracking all eight SSOT funnel metrics:

  pub struct Metrics {
      pub opportunities_detected: IntCounter,
      pub opportunities_cleared_threshold: IntCounter,
      pub opportunities_cleared_simulation: IntCounter,
      pub transactions_submitted: IntCounter,
      pub transactions_succeeded: IntCounter,
      pub transactions_reverted: IntCounterVec,  // with "reason" label
      pub net_pnl_wei: Gauge,
      pub gas_spent_wei: Counter,
  }

  impl Metrics {
      pub fn new() -> anyhow::Result<Self>
      // Register all metrics with a fresh Registry (not the default global one)
      // Use Registry::new() so tests can create independent instances

      pub fn registry(&self) -> &Registry

      pub fn render(&self) -> String
      // Returns prometheus text format string of all metrics

      pub async fn start_server(registry: Registry, port: u16) -> anyhow::Result<()>
      // Starts tokio TCP listener on 0.0.0.0:port
      // GET /metrics returns render() output
      // Any other path returns 404
  }

Also write `crates/common/src/tracing_init.rs`:
  pub fn init_tracing(log_level: &str)
  // Initialises tracing-subscriber with env-filter
  // Uses JSON format in release, pretty format in debug
  // Safe to call multiple times (no-op if already initialised)

Tests in #[cfg(test)]:

test_metrics_new_registers_all — create Metrics::new(), assert render() contains
  all eight metric names

test_counter_increments — increment opportunities_detected 3 times, render,
  assert value = 3

test_revert_counter_with_label — increment transactions_reverted with
  reason="No profit" twice, assert rendered value with that label = 2

test_gauge_set_and_read — set net_pnl_wei to 1_000_000.0, render, assert present

test_render_valid_prometheus_format — render() output must start with "# HELP"
  and contain "# TYPE" for each metric

test_independent_registries — two Metrics::new() instances do not share state
  (incrementing one does not affect the other)

test_metrics_server_responds — start server on port 19090, send HTTP GET /metrics,
  assert 200 response with body containing metric names
  (use tokio::test and reqwest or raw TCP)

Write all files completely. All tests must pass.
```

---

---

# PHASE 2 — Smart Contract

**Goal:** Write, test with full Foundry TDD, and deploy `ArbExecutor.sol`.
Every execution path through the contract has a corresponding test on a real
Arbitrum mainnet fork. No path is untested.

---

## Mini-Phase 2.1 — Foundry Project Setup

**Definition of done:**
- `forge build` passes with zero errors
- `forge test` passes with the placeholder test
- foundry.toml correctly configured for Arbitrum fork testing

---

**PROMPT 2.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 2.1: Foundry Project Setup.

Set up the complete Foundry project inside contracts/.

File 1: contracts/foundry.toml
  [profile.default]
  src = "src"
  test = "test"
  script = "script"
  out = "out"
  libs = ["lib"]
  solc = "0.8.24"
  optimizer = true
  optimizer_runs = 200
  via_ir = false

  [profile.ci]
  fuzz = { runs = 1000 }
  invariant = { runs = 256 }

  [rpc_endpoints]
  arbitrum = "${ARBITRUM_RPC_URL}"
  arbitrum_sepolia = "${ARBITRUM_SEPOLIA_RPC_URL}"

  [etherscan]
  arbitrum = { key = "${ARBISCAN_API_KEY}", url = "https://api.arbiscan.io/api" }

File 2: contracts/src/ArbExecutor.sol
Minimal stub that compiles:
  // SPDX-License-Identifier: MIT
  pragma solidity ^0.8.24;
  contract ArbExecutor {
      address public immutable owner;
      constructor() { owner = msg.sender; }
  }

File 3: contracts/test/ArbExecutor.t.sol
  pragma solidity ^0.8.24;
  import "forge-std/Test.sol";
  import "../src/ArbExecutor.sol";
  contract ArbExecutorTest is Test {
      ArbExecutor executor;
      function setUp() public {
          executor = new ArbExecutor();
      }
      function test_owner_is_deployer() public {
          assertEq(executor.owner(), address(this));
      }
  }

File 4: contracts/.gitignore
  out/
  cache/
  lib/
  broadcast/

File 5: contracts/README.md
  # ArbExecutor Contract
  ## Install deps
  forge install OpenZeppelin/openzeppelin-contracts --no-commit
  forge install balancer-labs/balancer-v2-monorepo --no-commit
  ## Test
  forge test -vvv
  ## Deploy
  See script/Deploy.s.sol

Write all files. forge build and forge test must pass.
```

---

## Mini-Phase 2.2 — ArbExecutor.sol Full Implementation

**Definition of done:**
- Contract implements all swap routes for all four DEX kinds
- `forge build` passes with zero errors and zero warnings
- All interfaces are correctly defined

---

**PROMPT 2.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 2.2: ArbExecutor.sol Full Implementation.

Write the complete `contracts/src/ArbExecutor.sol`.

Requirements from SSOT.md:
- Implements Balancer V2 IFlashLoanRecipient (NOT Aave)
- Balancer V2 Vault: 0xBA12222222228d8Ba445958a75a0704d566BF2C8
- feeAmounts[0] is ALWAYS 0 on Balancer V2
- Enforces: require(output >= input + minProfitWei, "No profit")
- Supports all four DEX swap kinds: UniswapV3, CamelotV2, SushiSwap, TraderJoeV1

ArbParams struct (passed as userData through flash loan):
  struct ArbParams {
      address tokenIn;
      address poolA;
      address tokenMid;
      address poolB;
      uint256 flashLoanAmount;
      uint256 minProfit;
      uint8 poolAKind;   // 0=UniswapV3, 1=CamelotV2, 2=SushiSwap, 3=TraderJoe
      uint8 poolBKind;
  }

External interfaces needed (define inline):
  - IVault: flashLoan function
  - IFlashLoanRecipient: receiveFlashLoan function
  - IUniswapV3Pool: swap function with callback
  - IUniswapV3SwapCallback: uniswapV3SwapCallback
  - IUniswapV2Pair: swap function (used by CamelotV2, SushiSwap, TraderJoe)

Contract functions:
  constructor(address _vault, uint256 _minProfitWei)
  setMinProfit(uint256) external onlyOwner
  executeArb(IERC20[] calldata, uint256[] calldata, ArbParams calldata) external onlyOwner
  receiveFlashLoan(IERC20[], uint256[], uint256[], bytes) external onlyVault
  recoverTokens(address, uint256) external onlyOwner
  receive() external payable

_executeSwap internal:
  For UniswapV3: call pool.swap() with zeroForOne based on token ordering,
    implement uniswapV3SwapCallback to transfer tokenIn to pool
  For CamelotV2/SushiSwap/TraderJoe: compute amounts out using getReserves,
    call pair.swap(amount0Out, amount1Out, address(this), "")

Safety requirements:
  - onlyOwner modifier on executeArb and setMinProfit and recoverTokens
  - onlyVault modifier on receiveFlashLoan
  - Reentrancy guard on receiveFlashLoan (use a simple bool _executing lock)
  - require(balanceAfter >= amounts[0] + params.minProfit, "No profit")
    checked BEFORE repaying flash loan
  - Use SafeERC20 for all token transfers

Write the complete contract. No stubs. Every swap variant fully implemented.
Include detailed NatSpec comments on every function.
```

---

## Mini-Phase 2.3 — Contract Tests: Full TDD on Arbitrum Fork

**Definition of done:**
- `forge test -vvv --fork-url $ARBITRUM_RPC_URL` passes ALL tests
- Access control: 4 tests
- Flash loan flow: 3 tests
- Swap execution: 2 tests per DEX kind (8 total)
- Profit enforcement: 4 tests
- Edge cases: 4 tests
- Total: minimum 23 tests, all passing

---

**PROMPT 2.3**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 2.3: Contract Tests — Full TDD on Arbitrum Fork.

Write `contracts/test/ArbExecutor.t.sol` with comprehensive fork tests.

Setup:
  contract ArbExecutorTest is Test {
      ArbExecutor executor;
      address constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
      address constant USDC = 0xFF970A61A04b1cA14834A43f5dE4533eBDDB5CC8;
      address constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
      address constant ARB  = 0x912CE59144191C1204E64559FE8253a0e49E6548;
      address constant WBTC = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0F;

      // Uniswap V3 USDC/WETH pool on Arbitrum
      address constant UNIV3_USDC_WETH = 0xC31E54c7a869B9FcBEcc14363CF510d1c41fa443;
      // Camelot V2 WETH/ARB pool
      address constant CAMELOT_WETH_ARB = 0xa6c5C7D189fA4eB5Af8ba34E63dCDD3a635D433;

      function setUp() public {
          vm.createSelectFork(vm.envString("ARBITRUM_RPC_URL"));
          executor = new ArbExecutor(BALANCER_VAULT, 1e15);
          // Fund executor with 0.01 ETH for gas in tests
          vm.deal(address(executor), 0.01 ether);
      }
  }

Write ALL of these tests:

ACCESS CONTROL (4 tests):
  test_only_owner_execute_arb — non-owner calls executeArb, expect revert "Not owner"
  test_only_owner_set_min_profit — non-owner calls setMinProfit, revert "Not owner"
  test_only_owner_recover_tokens — non-owner calls recoverTokens, revert "Not owner"
  test_only_vault_receive_flash_loan — direct call to receiveFlashLoan, revert "Not Balancer"

FLASH LOAN FLOW (3 tests):
  test_flash_loan_fee_is_zero — execute a real Balancer flash loan of 1000 USDC,
    verify feeAmounts[0] == 0 in the callback (use vm.expectCall or logging)
  test_flash_loan_repays_principal — after a successful arb, vault balance unchanged
  test_flash_loan_reverts_if_not_repaid — mock swap to return 0, expect revert

PROFIT ENFORCEMENT (4 tests):
  test_profit_require_triggers — swap returns exactly input amount (0 profit),
    minProfit = 1, expect revert "No profit"
  test_profit_require_passes — swap returns input + 2*minProfit, no revert
  test_set_min_profit_updates — owner sets new value, assert minProfitWei updated
  test_profit_prevents_loss — simulate state where arb would lose money,
    verify contract reverts before repaying (protecting principal)

SWAP EXECUTION — UniswapV3 (2 tests):
  test_univ3_swap_usdc_to_weth — swap 1000 USDC through real UniswapV3 pool,
    assert WETH received > 0
  test_univ3_swap_weth_to_usdc — reverse direction, assert USDC received > 0

SWAP EXECUTION — CamelotV2 (2 tests):
  test_camelot_swap_weth_to_arb — swap through real Camelot pool, assert ARB > 0
  test_camelot_swap_arb_to_weth — reverse direction

SWAP EXECUTION — SushiSwap (2 tests):
  test_sushi_swap_executes — swap through real SushiSwap pool, assert output > 0
  test_sushi_swap_reverse

SWAP EXECUTION — TraderJoe (2 tests):
  test_traderjoe_swap_executes
  test_traderjoe_swap_reverse

EDGE CASES (4 tests):
  test_reentrancy_guard_blocks — attempt reentrancy during callback, expect revert
  test_recover_tokens_works — send USDC to contract, recover via recoverTokens
  test_receive_eth — send ETH to contract, verify balance increases
  test_full_arb_end_to_end — execute a complete two-hop arb using a real
    historical price dislocation (use vm.rollFork to go to a specific block
    where arb existed), assert profit > 0 after fees

Write every test completely. Use vm.expectRevert, vm.prank, deal, and fork
utilities correctly. Every assertion must be specific — no assert(true).
```

---

## Mini-Phase 2.4 — Fuzz Tests and Invariants

**Definition of done:**
- Fuzz tests run 1000 iterations without finding violations
- Invariants hold across all states
- `forge test --fuzz-runs 1000` passes

---

**PROMPT 2.4**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 2.4: Fuzz Tests and Invariants.

Write `contracts/test/ArbExecutorFuzz.t.sol` with fuzz and invariant tests.

Fuzz tests:

  // Test that minProfit enforcement always holds regardless of amounts
  function testFuzz_profit_requirement_always_enforced(
      uint256 flashLoanAmount,
      uint256 swapOutput,
      uint256 minProfit
  ) public {
      // Bound inputs to realistic ranges
      flashLoanAmount = bound(flashLoanAmount, 1e6, 1e24); // 1 USDC to 1B USDC
      minProfit = bound(minProfit, 1, 1e18);
      // If swapOutput < flashLoanAmount + minProfit, must revert
      // If swapOutput >= flashLoanAmount + minProfit, must succeed
      // Use vm.mockCall to control swap output
      // Assert the require behaves correctly in both branches
  }

  // Test that owner can always recover any amount of any token
  function testFuzz_recover_tokens_always_works(
      address token,
      uint256 amount
  ) public {
      vm.assume(token != address(0));
      vm.assume(amount > 0 && amount < type(uint128).max);
      // Deploy mock ERC20, mint to executor, recover, verify balance
  }

  // Test that setMinProfit always updates regardless of value
  function testFuzz_set_min_profit(uint256 newMinProfit) public {
      executor.setMinProfit(newMinProfit);
      assertEq(executor.minProfitWei(), newMinProfit);
  }

Invariant test (stateful):

  contract ArbExecutorInvariantTest is Test {
      // Invariant: owner never changes
      function invariant_owner_never_changes() public {
          assertEq(executor.owner(), deployerAddress);
      }
      // Invariant: contract never holds tokens after successful arb
      // (all profit sent to owner, all principal repaid)
      function invariant_no_residual_balance_after_arb() public {
          // If no arb is in progress, USDC balance should be 0
          // (unless explicitly sent via recoverTokens scenario)
      }
  }

Write the complete fuzz test file. Include all bounds with vm.assume and bound().
```

---

## Mini-Phase 2.5 — Deploy Script

**Definition of done:**
- `forge script` runs without error against Arbitrum Sepolia
- Deployed address is logged and written to `deployments/<chainid>.json`

---

**PROMPT 2.5**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 2.5: Deploy Script.

Write the complete deploy infrastructure for ArbExecutor.

File 1: contracts/script/Deploy.s.sol
  pragma solidity ^0.8.24;
  import "forge-std/Script.sol";
  import "../src/ArbExecutor.sol";

  contract Deploy is Script {
      function run() external {
          uint256 privateKey = vm.envUint("PRIVATE_KEY");
          address vault = vm.envAddress("BALANCER_VAULT");
          uint256 minProfit = vm.envOr("MIN_PROFIT_WEI", uint256(1e15));

          vm.startBroadcast(privateKey);
          ArbExecutor executor = new ArbExecutor(vault, minProfit);
          vm.stopBroadcast();

          console.log("ArbExecutor deployed at:", address(executor));
          console.log("Owner:", executor.owner());
          console.log("MinProfit:", executor.minProfitWei());

          // Write deployment to JSON
          string memory chainId = vm.toString(block.chainid);
          vm.writeJson(
              string.concat('{"address":"', vm.toString(address(executor)), '","block":', vm.toString(block.number), '}'),
              string.concat("deployments/", chainId, ".json")
          );
      }
  }

File 2: contracts/script/Verify.s.sol
  Script that reads deployments/<chainid>.json and verifies on Arbiscan

File 3: scripts/deploy-sepolia.sh
  #!/bin/bash
  set -euo pipefail
  source .env
  forge script contracts/script/Deploy.s.sol \
    --rpc-url "$ARBITRUM_SEPOLIA_RPC_URL" \
    --broadcast \
    --verify \
    --etherscan-api-key "$ARBISCAN_API_KEY" \
    -vvvv
  echo "Deployment complete. Address in deployments/421614.json"

File 4: scripts/deploy-mainnet.sh
  Same but with mainnet RPC, add confirmation prompt before broadcasting:
  echo "WARNING: Deploying to Arbitrum MAINNET. Continue? (yes/no)"
  read confirm
  if [ "$confirm" != "yes" ]; then exit 1; fi

File 5: deployments/.gitkeep
  (empty, ensures deployments/ directory exists in repo)

Write all files completely.
```

---

---

# PHASE 3 — Ingestion Engine

**Goal:** Build Layer 1. Every component has unit tests. The pool state store is
tested for concurrent access correctness. The sequencer feed connection has chaos
tests for reconnection behaviour. The block reconciler has tests verifying
staleness recovery.

---

## Mini-Phase 3.1 — Pool State Store with Concurrent Tests

**Definition of done:**
- `PoolStateStore` operations are all tested
- Concurrent access test verifies no data races
- `cargo test -p arbx-ingestion` passes

---

**PROMPT 3.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 3.1: Pool State Store with Concurrent Tests.

Write `crates/ingestion/src/pool_state.rs` with full tests including concurrency.

Implement PoolStateStore:

  use dashmap::DashMap;
  use alloy::primitives::{Address, U256};
  use std::sync::Arc;
  use arbx_common::types::{PoolState, DexKind};

  #[derive(Clone, Debug)]
  pub struct PoolStateStore {
      inner: Arc<DashMap<Address, PoolState>>,
  }

  impl PoolStateStore {
      pub fn new() -> Self
      pub fn upsert(&self, state: PoolState)
      pub fn get(&self, address: &Address) -> Option<PoolState>
      pub fn all_addresses(&self) -> Vec<Address>
      pub fn by_dex(&self, dex: DexKind) -> Vec<PoolState>
      pub fn update_reserves(
          &self, address: &Address,
          reserve0: U256, reserve1: U256, block: u64
      ) -> bool  // false if pool not known
      pub fn len(&self) -> usize
      pub fn is_empty(&self) -> bool
      // Get pools where token is either token0 or token1
      pub fn pools_containing_token(&self, token: &Address) -> Vec<PoolState>
  }

Tests in #[cfg(test)]:

BASIC OPERATIONS:
  test_new_store_is_empty
  test_upsert_and_get_returns_correct_state
  test_upsert_twice_overwrites
  test_get_missing_returns_none
  test_update_reserves_success
  test_update_reserves_updates_block_number
  test_update_reserves_missing_pool_returns_false
  test_len_correct_after_insertions
  test_all_addresses_correct_count
  test_by_dex_filters_correctly
  test_pools_containing_token_both_positions

CONCURRENCY TEST (this is the critical one):
  #[tokio::test]
  async fn test_concurrent_upserts_no_data_race() {
      // Spawn 100 Tokio tasks, each upsert a different pool
      // Wait for all to complete
      // Assert store.len() == 100
      // Assert every pool is retrievable with correct data
      let store = PoolStateStore::new();
      let mut handles = vec![];
      for i in 0u64..100 {
          let store = store.clone();
          handles.push(tokio::spawn(async move {
              let pool = make_pool_state(i); // helper that creates unique pool
              store.upsert(pool);
          }));
      }
      for h in handles { h.await.unwrap(); }
      assert_eq!(store.len(), 100);
  }

  #[tokio::test]
  async fn test_concurrent_reads_while_writing() {
      // Writer task: continuously upserts pools every 1ms
      // Reader tasks: 10 tasks continuously read, assert no panics
      // Run for 100ms total
      // Assert reader tasks never panicked
  }

Write helper functions in the test module:
  fn make_pool_state(seed: u64) -> PoolState — creates a deterministic PoolState
  fn make_address(seed: u64) -> Address — creates Address from seed

Write the complete file. All tests must pass.
```

---

## Mini-Phase 3.2 — Sequencer Feed with Reconnection Tests

**Definition of done:**
- `SequencerFeedManager` connects, parses swap transactions, detects DEX calls
- Reconnection with exponential backoff is tested with a mock WebSocket server
- `cargo test -p arbx-ingestion` passes

---

**PROMPT 3.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 3.2: Sequencer Feed with Reconnection Tests.

Write `crates/ingestion/src/sequencer_feed.rs` with reconnection logic and tests.

Add to ingestion Cargo.toml:
  sequencer_client = "0.4"
  tokio-test = { workspace = true }

Key function selectors for swap detection:
  const UNISWAP_V3_SWAP: [u8;4] = [0x12, 0x8a, 0xcb, 0x08]; // IUniswapV3Pool.swap
  const UNIV2_SWAP: [u8;4] = [0x02, 0x2c, 0x0d, 0x9f];       // IUniswapV2Pair.swap

  pub struct DetectedSwap {
      pub tx_hash: TxHash,
      pub pool_address: Address,
      pub selector: [u8; 4],
      pub calldata: Bytes,
      pub sequenced_at_ms: u64,
      pub is_large: bool,  // true if estimated impact > 0.1% of reserves
  }

  pub struct FeedConfig {
      pub feed_url: String,
      pub reconnect_base_ms: u64,      // default 1000
      pub reconnect_max_ms: u64,       // default 32000
      pub reconnect_multiplier: f64,   // default 2.0
  }

  pub struct SequencerFeedManager {
      config: FeedConfig,
      pool_store: PoolStateStore,
      swap_tx: mpsc::Sender<DetectedSwap>,
  }

  impl SequencerFeedManager {
      pub fn new(config: FeedConfig, pool_store: PoolStateStore,
                 swap_tx: mpsc::Sender<DetectedSwap>) -> Self
      pub async fn run(self) -> anyhow::Result<()>

      // Exposed for testing — process a single transaction
      pub fn process_transaction(
          &self,
          tx: &alloy::rpc::types::Transaction,
      ) -> Option<DetectedSwap>
  }

Reconnection logic inside run():
  let mut backoff_ms = config.reconnect_base_ms;
  loop {
      match connect_and_stream(&config.feed_url).await {
          Ok(stream) => {
              backoff_ms = config.reconnect_base_ms; // reset on success
              // process stream
          }
          Err(e) => {
              tracing::warn!("Feed disconnected: {e}. Reconnecting in {backoff_ms}ms");
              tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
              backoff_ms = (backoff_ms as f64 * config.reconnect_multiplier) as u64;
              backoff_ms = backoff_ms.min(config.reconnect_max_ms);
          }
      }
  }

Tests in #[cfg(test)]:

SWAP DETECTION (no network required):
  test_process_tx_detects_univ3_swap — craft a transaction with to=known_pool
    and calldata starting with UNISWAP_V3_SWAP selector, assert Some(DetectedSwap)
  test_process_tx_detects_univ2_swap — same for UNIV2_SWAP selector
  test_process_tx_ignores_non_pool_address — tx to random address, assert None
  test_process_tx_ignores_unknown_selector — tx to known pool but unknown selector
  test_process_tx_sets_correct_timestamp — assert sequenced_at_ms > 0
  test_process_tx_large_swap_flag — mock reserves, verify is_large correctly set

RECONNECTION LOGIC:
  test_backoff_doubles_each_failure — simulate 5 consecutive failures,
    assert backoff sequence: 1000, 2000, 4000, 8000, 16000ms
  test_backoff_caps_at_max — simulate 10 failures, assert never exceeds 32000ms
  test_backoff_resets_on_success — fail twice (backoff=4000), succeed,
    fail once more, assert backoff resets to 1000ms

Write a BackoffCalculator struct with its own unit tests to make the logic
independently testable:
  pub struct BackoffCalculator {
      base_ms: u64, max_ms: u64, multiplier: f64, current_ms: u64
  }
  impl BackoffCalculator {
      pub fn next(&mut self) -> u64
      pub fn reset(&mut self)
  }

Write the complete file with all tests.
```

---

## Mini-Phase 3.3 — Block Reconciler with Staleness Tests

**Definition of done:**
- Reconciler correctly fetches reserves from all four DEX kinds
- Staleness recovery is tested: feed misses are detected and corrected
- `cargo test -p arbx-ingestion` passes

---

**PROMPT 3.3**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 3.3: Block Reconciler with Staleness Tests.

Write `crates/ingestion/src/reconciler.rs` with mockable provider and full tests.

Add to ingestion Cargo.toml:
  mockall = { workspace = true }

Design the reconciler to use a trait for the provider so it can be mocked:

  #[cfg_attr(test, mockall::automock)]
  #[async_trait::async_trait]
  pub trait ReserveFetcher: Send + Sync {
      // For UniswapV2-style pools: returns (reserve0, reserve1, block_timestamp)
      async fn fetch_v2_reserves(&self, pool: Address) -> anyhow::Result<(U256, U256, u64)>;
      // For UniswapV3 pools: returns (sqrtPriceX96, tick, liquidity)
      async fn fetch_v3_slot0(&self, pool: Address) -> anyhow::Result<(U256, i32, U256)>;
      // Returns current block number
      async fn current_block(&self) -> anyhow::Result<u64>;
  }

  pub struct AlloyReserveFetcher {
      provider: Arc<dyn Provider>,
  }
  // Implement ReserveFetcher for AlloyReserveFetcher using real ABI calls

  pub struct BlockReconciler<F: ReserveFetcher> {
      fetcher: F,
      pool_store: PoolStateStore,
      concurrency_limit: usize,  // default 20
  }

  impl<F: ReserveFetcher> BlockReconciler<F> {
      pub fn new(fetcher: F, pool_store: PoolStateStore, concurrency_limit: usize) -> Self
      pub async fn run(self) -> anyhow::Result<()>
      pub async fn reconcile_all(&self, block: u64) -> ReconcileStats
      async fn reconcile_pool(&self, pool: &PoolState, block: u64) -> anyhow::Result<bool>
      // returns true if reserves changed (staleness detected and corrected)
  }

  pub struct ReconcileStats {
      pub pools_checked: usize,
      pub pools_updated: usize,
      pub pools_failed: usize,
      pub block: u64,
  }

Tests in #[cfg(test)] using MockReserveFetcher:

  test_reconcile_updates_stale_pool — mock returns different reserves than store has,
    assert pool_store updated after reconcile_pool()

  test_reconcile_skips_fresh_pool — mock returns same reserves as store,
    assert update_count == 0

  test_reconcile_all_returns_correct_stats — 3 pools: 2 stale, 1 fresh,
    mock fetcher accordingly, assert stats.pools_updated == 2

  test_reconcile_all_handles_fetch_failure — one pool's fetcher returns Err,
    assert stats.pools_failed == 1, other pools still reconciled

  test_reconcile_all_concurrency — insert 50 pools, mock fetcher with 5ms delay,
    assert reconcile_all completes in <500ms (proves concurrency working,
    not sequential)

  test_reconcile_detects_staleness — upsert pool with block=100, mock returns
    same reserves but at block=200, verify last_updated_block updated to 200

Write the complete file. All tests must pass.
```

---

---

# PHASE 4 — Opportunity Brain

**Goal:** Build the opportunity detector and profit calculator. Every formula
is tested with both handpicked values and property-based fuzz tests. The 2D gas
model calculation has regression tests against known Arbitrum transactions.

---

## Mini-Phase 4.1 — Two-Hop Path Scanner with Property Tests

**Definition of done:**
- Path scanner finds all valid two-hop cycles through an affected pool
- AMM output formulas are verified mathematically correct
- Property tests verify: output always ≤ input for zero-fee pools (conservation)
- `cargo test -p arbx-detector` passes

---

**PROMPT 4.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 4.1: Two-Hop Path Scanner with Property Tests.

Write `crates/detector/src/opportunity.rs` with full tests including proptest.

Add to detector Cargo.toml:
  proptest = { workspace = true }

Implement:

  pub struct PathScanner {
      pool_store: PoolStateStore,
  }

  impl PathScanner {
      pub fn new(pool_store: PoolStateStore) -> Self
      pub fn scan(&self, affected_pool: Address) -> Vec<ArbPath>

      // UniswapV2-style constant product formula
      // amount_out = (amount_in * (10000 - fee_bps) * reserve_out)
      //            / (reserve_in * 10000 + amount_in * (10000 - fee_bps))
      pub fn compute_output_v2(
          &self, pool: &PoolState,
          token_in: Address, amount_in: U256
      ) -> Option<U256>

      // UniswapV3: simplified sqrt price based approximation
      // (mark with TODO: replace with full tick math in Phase 10)
      pub fn compute_output_v3(
          &self, pool: &PoolState,
          token_in: Address, amount_in: U256
      ) -> Option<U256>

      // Compute best flash loan amount for maximum profit
      // Uses binary search over [1e6, 1e24] range
      pub fn optimal_flash_loan_amount(
          &self, path: &PartialPath
      ) -> U256
  }

  pub struct PartialPath {
      pub token_in: Address,
      pub pool_a: PoolState,
      pub token_mid: Address,
      pub pool_b: PoolState,
  }

Tests:

UNIT TESTS (deterministic):
  test_scan_finds_two_hop_cycle — 2 pools sharing a token, verify ArbPath returned
  test_scan_no_path_no_shared_token — 2 pools, no shared token, verify empty
  test_scan_finds_multiple_paths — 3 pools with complex topology, verify all paths
  test_scan_only_considers_known_pools — affected pool not in store, verify empty
  test_arb_path_is_circular — all returned paths must satisfy is_circular()
  test_compute_output_v2_known_values:
    reserve0=1_000_000 USDC, reserve1=400 WETH, fee=3000bps
    amount_in=10_000 USDC
    expected_out = (10000 * 9970 * 400) / (1000000 * 10000 + 10000 * 9970)
    assert within 1 wei of expected
  test_compute_output_v2_zero_input — assert None
  test_compute_output_v2_exceeds_reserves — amount_in > reserve_in, assert None
  test_compute_output_v3_directional_correct — higher price means more output

PROPERTY TESTS (with proptest):
  proptest! {
      // Conservation: output < input when reserves are equal (no arb possible)
      #[test]
      fn prop_v2_output_less_than_input_equal_reserves(
          amount_in in 1u64..1_000_000_000u64,
          reserve in 1_000_000u64..1_000_000_000_000u64,
      ) {
          let reserve = U256::from(reserve);
          let amount = U256::from(amount_in);
          // When reserve0 == reserve1, output < input (fee removes value)
          let output = compute_v2(amount, reserve, reserve, 3000);
          prop_assert!(output < amount);
      }

      // Monotonicity: higher amount_in => higher output (up to reserve limits)
      #[test]
      fn prop_v2_output_monotone_in_amount(
          amount_a in 1u64..1_000_000u64,
          amount_b in 1u64..1_000_000u64,
          reserve_in in 10_000_000u64..1_000_000_000u64,
          reserve_out in 10_000_000u64..1_000_000_000u64,
      ) {
          let (a, b) = if amount_a < amount_b {
              (amount_a, amount_b)
          } else {
              (amount_b, amount_a)
          };
          let out_a = compute_v2(U256::from(a), U256::from(reserve_in), U256::from(reserve_out), 3000);
          let out_b = compute_v2(U256::from(b), U256::from(reserve_in), U256::from(reserve_out), 3000);
          prop_assert!(out_a <= out_b);
      }

      // No free money: two-hop through same reserves must lose to fees
      #[test]
      fn prop_no_arb_through_identical_pools(
          amount in 1u64..1_000_000u64,
          reserve in 10_000_000u64..1_000_000_000u64,
      ) {
          // Pool A and Pool B have identical reserves
          // Two-hop A→B must return less than amount (fees)
          let mid = compute_v2(U256::from(amount), U256::from(reserve), U256::from(reserve), 3000);
          let out = compute_v2(mid, U256::from(reserve), U256::from(reserve), 3000);
          prop_assert!(out < U256::from(amount));
      }
  }

Write the complete file. All tests including property tests must pass.
```

---

## Mini-Phase 4.2 — Profit Calculator with 2D Gas Tests

**Definition of done:**
- Dynamic threshold correctly includes both L2 and L1 components
- NodeInterface query is mockable and tested
- Gas spike scenario is tested (L1 gas jumps 10x, threshold adapts)
- `cargo test -p arbx-detector` passes

---

**PROMPT 4.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 4.2: Profit Calculator with 2D Gas Tests.

Write `crates/detector/src/profit.rs` with mockable gas fetching and full tests.

Add to detector Cargo.toml:
  mockall = { workspace = true }
  async-trait = "0.1"

Design with a mockable interface for gas fetching:

  #[cfg_attr(test, mockall::automock)]
  #[async_trait]
  pub trait GasFetcher: Send + Sync {
      async fn l2_gas_price_wei(&self) -> anyhow::Result<u128>;
      async fn estimate_l2_gas(&self, to: Address, data: &Bytes) -> anyhow::Result<u64>;
      // Calls NodeInterface.gasEstimateL1Component
      async fn estimate_l1_gas(&self, to: Address, data: &Bytes) -> anyhow::Result<u64>;
      async fn l1_base_fee_wei(&self) -> anyhow::Result<u128>;
      async fn eth_price_usd(&self) -> f64; // cached, not async
  }

  pub struct AlloyGasFetcher {
      provider: Arc<dyn Provider>,
      node_interface: Address, // 0x00000000000000000000000000000000000000C8
      eth_price_usd: Arc<RwLock<f64>>, // updated periodically
  }
  // Implement GasFetcher for AlloyGasFetcher

  pub struct ProfitCalculator<G: GasFetcher> {
      fetcher: G,
      strategy: StrategyConfig,
  }

  impl<G: GasFetcher> ProfitCalculator<G> {
      pub fn new(fetcher: G, strategy: StrategyConfig) -> Self

      pub async fn estimate_gas(
          &self, to: Address, calldata: &Bytes
      ) -> anyhow::Result<GasEstimate>
      // Returns full GasEstimate with both L2 and L1 components

      pub fn compute_min_profit_wei(
          &self, gas: &GasEstimate
      ) -> U256
      // min_profit = total_gas_cost_wei * gas_buffer_multiplier + floor_usd_in_wei
      // floor_usd_in_wei = min_profit_floor_usd * eth_price_usd * 1e18

      pub async fn filter(
          &self,
          path: &ArbPath,
          calldata: &Bytes,
      ) -> anyhow::Result<Option<Opportunity>>
      // Returns Some(Opportunity) only if estimated_profit > min_profit + total_gas
  }

Tests using MockGasFetcher:

UNIT TESTS:
  test_gas_estimate_sums_l2_and_l1:
    mock: l2_gas=200_000 units, l2_price=100_gwei, l1_gas=5_000 units, l1_base=20_gwei
    expected l2_cost = 200_000 * 100e9 = 20_000_000_000_000_000 wei
    expected l1_cost = 5_000 * 20e9 = 100_000_000_000_000 wei
    expected total = l2_cost + l1_cost
    assert within rounding tolerance

  test_min_profit_includes_floor:
    strategy: floor=0.50 USD, buffer=1.1, eth_price=3000 USD
    floor_in_wei = 0.50 / 3000 * 1e18 = 166_666_666_666_666 wei
    gas_cost = 1_000_000_000_000_000 wei
    expected_min = 1_000_000 * 1.1 + 166_666... (in wei)
    assert computed min == expected

  test_profitable_path_passes_filter:
    path.estimated_profit_wei = min_profit * 2
    assert filter() returns Some(Opportunity)

  test_unprofitable_path_filtered:
    path.estimated_profit_wei = min_profit / 2
    assert filter() returns None

  test_gas_spike_raises_threshold:
    baseline: l1_gas=1_000 units, l1_price=5_gwei => cheap
    spike: l1_gas=100_000 units, l1_price=50_gwei => 200x more expensive
    assert threshold_spike > threshold_baseline * 100
    (verifies the 2D model actually adapts to mainnet gas spikes)

  test_zero_profit_path_filtered:
    path.estimated_profit_wei = 0
    assert filter() returns None

  test_exact_threshold_boundary:
    profit == min_profit exactly => None (not strictly profitable)
    profit == min_profit + 1 => Some (strictly profitable)

PROPERTY TESTS:
  proptest! {
      fn prop_threshold_always_positive(...) {
          // Any gas estimate always produces a positive min_profit
      }
      fn prop_higher_gas_higher_threshold(...) {
          // Doubling gas cost always doubles threshold (linear relationship)
      }
  }

Write the complete file. All tests must pass.
```

---

---

# PHASE 5 — Simulation Engine

**Goal:** Build the revm simulation layer with a regression test suite using real
historical Arbitrum blocks. Every simulation path is tested. The golden test
library ensures the bot correctly identifies real historical arb opportunities.

---

## Mini-Phase 5.1 — revm Fork Infrastructure

**Definition of done:**
- revm can fork Arbitrum state at any block
- Fork correctly reflects on-chain balances and storage
- `cargo test -p arbx-simulator` passes

---

**PROMPT 5.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 5.1: revm Fork Infrastructure.

Write `crates/simulator/src/revm_sim.rs` base infrastructure with tests.

Implement:

  pub struct ArbSimulator {
      provider: Arc<dyn Provider>,
      fork_block_cache: Arc<Mutex<Option<(u64, CacheDB<AlloyDB<...>>)>>>,
      // Cache the most recent block's fork to avoid re-forking on every sim
  }

  impl ArbSimulator {
      pub fn new(provider: Arc<dyn Provider>) -> Self

      pub async fn fork_at_latest(&self) -> anyhow::Result<CacheDB<AlloyDB<...>>>
      // Creates a new CacheDB backed by AlloyDB at the latest block
      // Reuses cached fork if current block hasn't changed

      pub async fn fork_at_block(&self, block: u64) -> anyhow::Result<CacheDB<AlloyDB<...>>>
      // Creates fork at specific block (for testing with historical blocks)

      pub async fn read_balance(
          &self, address: Address, block: Option<u64>
      ) -> anyhow::Result<U256>
      // Helper: reads ETH balance from forked state

      pub async fn read_erc20_balance(
          &self, token: Address, account: Address, block: Option<u64>
      ) -> anyhow::Result<U256>
      // Helper: reads ERC20 balance from forked state
  }

Tests:

UNIT (no network required):
  test_fork_is_created — verify ArbSimulator::new() succeeds
  test_cache_key_construction — verify fork_at_latest caches by block number

INTEGRATION (require ARBITRUM_RPC_URL, marked #[ignore]):
  #[tokio::test]
  #[ignore = "requires ARBITRUM_RPC_URL"]
  async fn test_fork_reads_balancer_vault_balance() {
      // Fork latest block
      // Read USDC balance of Balancer Vault
      // Assert balance > 1_000_000 * 1e6 (vault holds billions)
  }

  #[tokio::test]
  #[ignore = "requires ARBITRUM_RPC_URL"]
  async fn test_fork_reads_weth_balance() {
      // Read WETH balance of Uniswap V3 USDC/WETH pool
      // Assert reasonable balance
  }

  #[tokio::test]
  #[ignore = "requires ARBITRUM_RPC_URL"]
  async fn test_fork_block_caching() {
      // Fork twice at same block
      // Second call must be faster (from cache)
      // Measure timing to verify
  }

Write the complete file. All non-ignored tests must pass.
```

---

## Mini-Phase 5.2 — Full Arb Simulation with Regression Suite

**Definition of done:**
- Full simulation encodes and executes the complete flash loan + swap + repay flow
- Regression tests verify the bot would have caught real historical arb opportunities
- Success and failure cases both work correctly
- `cargo test -p arbx-simulator` passes (non-ignored tests)

---

**PROMPT 5.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 5.2: Full Arb Simulation with Regression Suite.

Extend `crates/simulator/src/revm_sim.rs` with full simulation and regression tests.

Add to simulator's Cargo.toml:
  proptest = { workspace = true }

Implement:

  pub struct CallDataEncoder;

  impl CallDataEncoder {
      // Encode ArbExecutor.executeArb() calldata for an ArbPath
      pub fn encode_execute_arb(path: &ArbPath, min_profit_wei: U256) -> Bytes
      // Must produce exactly the same encoding as Solidity would
      // Use alloy's ABI encoding

      // Decode revert reason from EVM output
      pub fn decode_revert_reason(output: &Bytes) -> String
      // Try to ABI-decode Error(string) first
      // Fall back to hex string if not standard revert
  }

  impl ArbSimulator {
      pub async fn simulate(
          &self,
          opportunity: &Opportunity,
          contract: Address,
          owner: Address,
      ) -> SimulationResult
      // 1. Encode calldata via CallDataEncoder
      // 2. Fork latest state
      // 3. Set up TxEnv: caller=owner, to=contract, data=calldata, gas=2_000_000
      // 4. Execute EVM
      // 5. Parse result:
      //    - Success: extract gas_used and compute net_profit
      //    - Revert: decode reason, return Failure
      // 6. Log at debug level: opportunity path, result, gas_used

      pub async fn simulate_at_block(
          &self,
          opportunity: &Opportunity,
          contract: Address,
          owner: Address,
          block: u64,
      ) -> SimulationResult
      // Same but forks at specific block (for regression tests)
  }

Tests:

UNIT TESTS (no network):
  test_encode_execute_arb_deterministic — same input always produces same bytes
  test_encode_execute_arb_non_empty — encoded calldata is non-empty
  test_decode_revert_standard_error — bytes encoding Error("No profit"),
    assert decoded == "No profit"
  test_decode_revert_non_standard — random bytes, assert returns hex string
  test_decode_revert_empty — empty bytes, assert returns "empty revert"

INTEGRATION TESTS (#[ignore = "requires ARBITRUM_RPC_URL"]):

  #[tokio::test]
  #[ignore = "requires ARBITRUM_RPC_URL"]
  async fn regression_simulates_as_failure_without_deployed_contract() {
      // Without a real deployed ArbExecutor, simulation reverts
      // This tests that revm correctly simulates EVM execution
      // Use address(0) as contract, expect SimulationResult::Failure
  }

  // Golden test: known historical arb opportunity
  // Block 200_000_000 on Arbitrum had a USDC/WETH/ARB price dislocation
  // (replace with a real block number you research)
  #[tokio::test]
  #[ignore = "requires ARBITRUM_RPC_URL"]
  async fn golden_test_historical_arb_block_200000000() {
      // Fork at block 200_000_000
      // Construct the known arb path at that block
      // Simulate against forked state
      // Assert SimulationResult::Success with profit > 0
      // This is your regression anchor — if this ever fails, simulation is broken
  }

PROPERTY TESTS (no network):
  proptest! {
      fn prop_encode_decode_roundtrip(
          token_in: [u8; 20], pool_a: [u8; 20], token_mid: [u8; 20],
          pool_b: [u8; 20], token_out: [u8; 20],
          profit: u64, flash_amount: u64,
      ) {
          let path = ArbPath { ... from inputs ... };
          let encoded = CallDataEncoder::encode_execute_arb(&path, U256::from(profit));
          // Encoded must be non-empty and have correct length
          prop_assert!(encoded.len() > 4); // at least selector + some data
      }
  }

Write the complete file. All non-ignored tests must pass.
```

---

---

# PHASE 6 — Execution Engine

**Goal:** Build Layer 3. The submitter has tests for both successful and failed
submissions. The PnL tracker has persistence tests. The kill switch is tested
to correctly halt the bot when the budget is exhausted.

---

## Mini-Phase 6.1 — Transaction Submitter with Mock Provider Tests

**Definition of done:**
- Submitter correctly computes 2D gas cost before every submission
- Mock provider tests verify submission behaviour without spending real gas
- Revert reason decoding is tested for all known revert formats
- `cargo test -p arbx-executor` passes

---

**PROMPT 6.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 6.1: Transaction Submitter with Mock Provider Tests.

Write `crates/executor/src/submitter.rs` with mockable provider and full tests.

Add to executor Cargo.toml:
  mockall = { workspace = true }
  async-trait = "0.1"

Design with mock-friendly interface:

  #[cfg_attr(test, mockall::automock)]
  #[async_trait]
  pub trait TransactionSender: Send + Sync {
      async fn send(
          &self,
          calldata: Bytes,
          to: Address,
          gas_limit: u64,
          gas_price_wei: u128,
      ) -> anyhow::Result<TxHash>;

      async fn get_receipt(&self, tx_hash: TxHash) -> anyhow::Result<Option<TransactionReceipt>>;
      async fn current_gas_price_wei(&self) -> anyhow::Result<u128>;
      async fn estimate_l1_gas(&self, to: Address, data: &Bytes) -> anyhow::Result<u64>;
  }

  pub struct AlloyTransactionSender {
      provider: Arc<dyn Provider>,
      signer: PrivateKeySigner,
      node_interface: Address,
  }
  // Implement TransactionSender for AlloyTransactionSender

  pub struct TransactionSubmitter<S: TransactionSender> {
      sender: S,
      contract: Address,
      config: ExecutionConfig,
      metrics: Metrics,
  }

  impl<S: TransactionSender> TransactionSubmitter<S> {
      pub fn new(sender: S, contract: Address, config: ExecutionConfig, metrics: Metrics) -> Self

      pub async fn submit(
          &self,
          opportunity: &Opportunity,
          calldata: Bytes,
      ) -> anyhow::Result<SubmissionResult>
      // 1. estimate_l1_gas() for L1 component
      // 2. current_gas_price_wei() for L2 price
      // 3. compute gas_limit = (l2_gas + l1_gas) * gas_estimate_buffer
      // 4. send() the transaction
      // 5. poll for receipt with 30s timeout
      // 6. parse receipt into SubmissionResult
      // 7. update all metrics
      // 8. log appropriately
  }

  pub fn decode_revert_reason(output: &[u8]) -> String
  // ABI-decode Error(string) if possible, else hex

Tests using MockTransactionSender:

  test_submit_successful_tx — mock send() returns valid hash, mock receipt returns
    success=true, assert SubmissionResult.success == true

  test_submit_failed_tx_no_profit — mock receipt returns success=false with
    revert data encoding Error("No profit"),
    assert SubmissionResult.revert_reason == Some("No profit")

  test_submit_failed_tx_unknown_revert — mock receipt returns success=false
    with non-standard revert data, assert revert_reason contains hex

  test_gas_limit_includes_l1_component — mock l1_gas=50_000, l2_gas=200_000,
    buffer=1.2, assert gas_limit sent == (250_000 * 1.2) rounded up

  test_metrics_incremented_on_success — after successful submit,
    assert transactions_submitted == 1 and transactions_succeeded == 1

  test_metrics_incremented_on_revert — after reverted submit,
    assert transactions_submitted == 1 and transactions_reverted{reason="No profit"} == 1

  test_receipt_timeout_returns_error — mock get_receipt() always returns None,
    assert submit() returns Err after timeout

  test_net_pnl_positive_on_success — profitable opportunity, assert
    SubmissionResult.net_pnl_wei > I256::ZERO

  test_net_pnl_negative_on_revert — reverted tx (lost gas), assert
    SubmissionResult.net_pnl_wei < I256::ZERO

  test_decode_revert_standard_error
  test_decode_revert_empty_returns_empty
  test_decode_revert_non_abi_returns_hex

Write the complete file. All tests must pass.
```

---

## Mini-Phase 6.2 — PnL Tracker with Persistence Tests

**Definition of done:**
- PnL state persists to JSON and loads correctly on restart
- Budget exhaustion triggers kill switch
- All accounting is tested to be correct
- `cargo test -p arbx-common` passes

---

**PROMPT 6.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 6.2: PnL Tracker with Persistence Tests.

Write `crates/common/src/pnl.rs` with complete persistence and accounting tests.

  #[derive(Debug, Clone, Serialize, Deserialize, Default)]
  pub struct PnlState {
      pub total_gas_spent_wei: String,  // U256 as string for JSON
      pub total_gas_spent_usd: f64,
      pub total_profit_wei: String,
      pub total_profit_usd: f64,
      pub net_pnl_usd: f64,
      pub successful_arbs: u64,
      pub reverted_arbs: u64,
      pub total_submissions: u64,
      pub budget_remaining_usd: f64,
      pub session_start_ms: u64,
      pub last_updated_ms: u64,
  }

  pub struct PnlTracker {
      state: Arc<Mutex<PnlState>>,
      file_path: String,
      initial_budget_usd: f64,
  }

  impl PnlTracker {
      pub fn new(file_path: String, budget_usd: f64) -> anyhow::Result<Self>
      // Loads existing state from file if it exists, else creates fresh

      pub async fn record_submission(
          &self,
          result: &SubmissionResult,
          eth_price_usd: f64,
      ) -> anyhow::Result<()>
      // Updates all counters, recalculates net_pnl, saves to file

      pub fn is_budget_exhausted(&self) -> bool
      // Returns true if budget_remaining_usd < 0.10 (keep $0.10 safety margin)

      pub fn summary(&self) -> String
      // Returns formatted summary string for logging

      pub fn state_snapshot(&self) -> PnlState
      // Returns clone of current state

      pub async fn save(&self) -> anyhow::Result<()>
      // Write JSON to file_path atomically (write to .tmp then rename)
  }

Tests (use tempfile::NamedTempFile for test isolation):

  test_new_creates_fresh_state — new tracker with fresh path,
    assert total_submissions == 0, budget_remaining == initial

  test_new_loads_existing_state — write JSON to temp file,
    create new tracker pointing to it, assert state loaded correctly

  test_record_successful_arb — submit a profitable SubmissionResult,
    assert successful_arbs == 1, total_submissions == 1,
    net_pnl_usd > 0, gas correctly deducted from budget

  test_record_reverted_arb — submit a failed SubmissionResult (lost gas),
    assert reverted_arbs == 1, budget_remaining decreased by gas cost,
    net_pnl_usd < 0

  test_budget_not_exhausted — start with $60, spend $1, assert not exhausted

  test_budget_exhausted_at_limit — start with $60, record submissions totaling
    $59.95 in gas costs, assert is_budget_exhausted() == true

  test_budget_safety_margin — exhausted at $0.10 remaining, not at $0.11

  test_persistence_survives_restart — record 3 submissions, drop tracker,
    create new tracker from same file, assert state equals original

  test_atomic_write — simulate write failure (disk full) midway,
    verify original file not corrupted
    (test via mocking or by writing to a very small tempfs)

  test_summary_contains_key_fields — summary string contains "PnL", submission
    count, and budget remaining

Write the complete file with all tests. Require tempfile in dev-dependencies.
```

---

---

# PHASE 7 — Integration

**Goal:** Wire all five crates into the complete pipeline in `bin/arbx.rs`. Write
workspace-level integration tests that run the full pipeline against a mock
environment. Every channel handoff is tested. Graceful shutdown is tested.

---

## Mini-Phase 7.1 — Pipeline Wiring

**Definition of done:**
- `bin/arbx.rs` wires all layers correctly with supervised Tokio tasks
- CLI argument parsing works
- `cargo build --workspace` passes

---

**PROMPT 7.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 7.1: Pipeline Wiring.

Write the complete `bin/arbx.rs` — the entry point that wires all crates.

The complete data flow from SSOT.md:
  Feed → PoolStateStore → PathScanner + ProfitCalculator →
  ArbSimulator → TransactionSubmitter → Metrics + PnlTracker

CLI interface:
  arbx --config <path>     (required)
  arbx --dry-run           (simulate but never submit — for testing)
  arbx --help

Implement run(config, dry_run):
  1. Build alloy provider (WebSocket)
  2. Initialise Metrics, start /metrics HTTP server
  3. Bootstrap PoolStateStore from factory addresses
  4. Create PnlTracker
  5. Create channels: swap_tx/rx, opportunity_tx/rx
  6. Spawn supervised tasks (each in tokio::spawn, errors logged + propagated):
     a. SequencerFeedManager::run()
     b. BlockReconciler::run()
     c. detection_loop(swap_rx, pool_store, profit_calc, opportunity_tx, metrics)
     d. execution_loop(opportunity_rx, simulator, submitter, pnl, dry_run, metrics)
  7. Spawn budget watchdog: every 60s check pnl.is_budget_exhausted(),
     if true log critical + initiate shutdown
  8. tokio::select! on all task handles — any exit = log error + shutdown all

detection_loop:
  - Receive DetectedSwap from swap_rx
  - Run PathScanner.scan()
  - For each ArbPath, run ProfitCalculator.filter()
  - If Some(Opportunity), send to opportunity_tx
  - Update metrics at each filter stage
  - Log at debug level throughout

execution_loop:
  - Receive Opportunity from opportunity_rx
  - Use Semaphore(max_concurrent_simulations) to cap parallelism
  - Spawn subtask per opportunity:
    a. ArbSimulator.simulate()
    b. If Success AND not dry_run: TransactionSubmitter.submit()
    c. If dry_run: log "DRY RUN — would submit" and skip
    d. Record in PnlTracker
    e. Update metrics

Shutdown handling:
  - On SIGTERM or SIGINT: cancel all tasks cleanly, save PnlTracker state, exit 0
  - On task crash: log error, attempt graceful shutdown, exit 1

Write the complete file. No stubs. Every component connected.
```

---

## Mini-Phase 7.2 — Workspace Integration Tests

**Definition of done:**
- `tests/integration/` contains end-to-end pipeline tests with mocked dependencies
- Channel handoffs between all layers are tested
- Dry-run mode is tested end-to-end
- `cargo test --workspace` passes

---

**PROMPT 7.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 7.2: Workspace Integration Tests.

Write `tests/integration/mod.rs` and supporting test helpers.

Add to root Cargo.toml [dev-dependencies]:
  tokio-test = { workspace = true }
  mockall = { workspace = true }
  tempfile = "3"

Create test helpers:

  // tests/integration/helpers.rs
  pub fn make_test_config() -> Config — returns a Config with test values
  pub fn make_pool_state_store_with_known_pools() -> PoolStateStore
    — populate with 4 real Arbitrum pool addresses and initial reserves
  pub fn make_test_opportunity() -> Opportunity
    — create a realistic Opportunity struct
  pub fn make_profitable_submission_result() -> SubmissionResult
  pub fn make_reverted_submission_result(reason: &str) -> SubmissionResult

Integration tests:

  #[tokio::test]
  async fn integration_detection_loop_sends_opportunities() {
      // Create store with 2 pools sharing a token
      // Set reserves so arb exists
      // Create a DetectedSwap for one of the pools
      // Feed into detection_loop via channel
      // Assert opportunity arrives in output channel
      // Assert metrics incremented correctly
  }

  #[tokio::test]
  async fn integration_detection_loop_filters_unprofitable() {
      // Create pools with identical reserves (no arb possible)
      // Feed DetectedSwap
      // Assert no opportunity arrives in output channel
      // Assert opportunities_detected > 0 but opportunities_cleared_threshold == 0
  }

  #[tokio::test]
  async fn integration_execution_loop_dry_run_never_submits() {
      // Create a mock submitter that panics if called
      // Set dry_run = true
      // Feed an Opportunity into execution loop
      // Assert submitter was never called (no panic)
      // Assert PnlTracker shows 0 submissions
  }

  #[tokio::test]
  async fn integration_pnl_tracker_budget_triggers_shutdown() {
      // Create PnlTracker with $1 budget
      // Simulate 10 reverted submissions at $0.15 each
      // After 7th submission, is_budget_exhausted() must return true
      // Verify budget watchdog correctly detects this
  }

  #[tokio::test]
  async fn integration_full_pipeline_mock_discovery_to_skip() {
      // Wire full detection_loop with mock profit calculator that always says yes
      // Wire mock execution_loop with dry_run=true
      // Send 5 DetectedSwaps
      // Assert 5 opportunities detected, 5 filtered, 5 simulated (mocked), 0 submitted
      // Assert all metrics correct
  }

  #[tokio::test]
  async fn integration_channel_backpressure() {
      // Create channels with capacity=1
      // Send 100 DetectedSwaps rapidly
      // Assert no deadlock (tasks should apply backpressure gracefully)
      // Assert eventually all processed (may take time)
  }

  #[tokio::test]
  async fn integration_concurrent_simulations_capped() {
      // Set max_concurrent_simulations = 3
      // Send 10 opportunities simultaneously
      // Use a mock simulator with 100ms delay
      // Assert never more than 3 concurrent simulations (via counter)
  }

Write all test files completely. All tests must pass.
```

---

---

# PHASE 8 — Property and Chaos Tests

**Goal:** Harden the system against adversarial inputs and infrastructure failures.
Property-based tests fuzz every formula. Chaos tests verify the system stays
correct when the WebSocket dies, the RPC returns errors, or transactions arrive
out of order.

---

## Mini-Phase 8.1 — Comprehensive Property Test Suite

**Definition of done:**
- All critical formulas are property-tested with 10,000 iterations
- No invariant violations found
- `cargo test --workspace` passes (proptest runs as part of normal test suite)

---

**PROMPT 8.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 8.1: Comprehensive Property Test Suite.

Write `tests/property/mod.rs` with a comprehensive proptest suite covering all
critical formulas in arbx.

Add to root Cargo.toml [dev-dependencies]:
  proptest = { workspace = true }

Configure proptest for 10,000 cases in CI:
  // proptest.toml (or .proptest-regressions/ directory is auto-created)

Write these property test groups:

GROUP 1 — AMM Math Invariants:
  prop_v2_output_always_positive_for_valid_input
  prop_v2_output_always_less_than_reserve_out
  prop_v2_fee_always_deducted (output < theoretical_zero_fee_output)
  prop_v2_output_increases_with_reserve_out
  prop_v2_output_decreases_with_reserve_in
  prop_v2_different_fees_different_output (0bps > 30bps > 100bps output)
  prop_two_hop_same_pool_always_loses_to_fees (covered in Phase 4 but repeat here)
  prop_output_never_overflows_u256

GROUP 2 — Gas Model Invariants:
  prop_min_profit_always_exceeds_gas_cost
  prop_min_profit_scales_linearly_with_gas_price
  prop_l1_gas_spike_10x_raises_threshold_at_least_5x
    (conservative: L1 should dominate at high mainnet gas)
  prop_zero_gas_price_impossible
    (assert estimate always returns > 0 even with mocked zero price)

GROUP 3 — Type Safety Invariants:
  prop_arb_path_serializes_deserializes_losslessly
  prop_opportunity_total_gas_equals_l2_plus_l1
  prop_submission_result_pnl_correct
    (net_pnl = profit - gas, verify arithmetic)
  prop_pool_state_reserve_non_zero_after_upsert

GROUP 4 — PnL Accounting Invariants:
  prop_budget_remaining_never_negative_reporting
    (budget_remaining may go negative in accounting but is_budget_exhausted
     triggers before it does)
  prop_total_submissions_equals_successful_plus_reverted
  prop_net_pnl_equals_profit_minus_gas
  prop_pnl_state_serializes_losslessly

GROUP 5 — Backoff Invariants:
  prop_backoff_never_exceeds_max
  prop_backoff_always_increases_until_max
  prop_backoff_resets_to_base_after_reset

For each group, use proptest strategies:
  - U256 values: use_random_u256() strategy sampling from [1, 2^128)
  - Addresses: use_random_address() strategy
  - f64 values: use reasonable ranges (gas prices: 0.01..1000 gwei)

Write the complete file. All 30+ property tests must pass with 10,000 iterations.
```

---

## Mini-Phase 8.2 — Chaos Tests: Feed and RPC Fault Injection

**Definition of done:**
- Feed manager correctly handles: sudden disconnect, malformed messages,
  empty batches, extremely large batches, duplicate sequence numbers
- RPC fault injection: timeout, 500 error, rate limit, stale response
- All chaos scenarios result in clean recovery, never panic
- `cargo test --workspace` passes

---

**PROMPT 8.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 8.2: Chaos Tests — Feed and RPC Fault Injection.

Write `tests/chaos/mod.rs` — fault injection and chaos tests.

Add to root Cargo.toml [dev-dependencies]:
  tokio-test = { workspace = true }

FEED CHAOS TESTS:

Write a MockFeedServer that simulates the Arbitrum sequencer feed WebSocket:
  pub struct MockFeedServer {
      port: u16,
      // Control channel to inject faults
      control_tx: mpsc::Sender<FeedFault>,
  }

  pub enum FeedFault {
      Disconnect,
      SendMalformedMessage,
      SendEmptyBatch,
      SendDuplicateSequenceNumber,
      Pause(Duration),
  }

  impl MockFeedServer {
      pub async fn start() -> Self  // binds to random port
      pub fn url(&self) -> String
      pub async fn inject(&self, fault: FeedFault)
  }

Tests:

  #[tokio::test]
  async fn chaos_feed_reconnects_after_disconnect() {
      let server = MockFeedServer::start().await;
      let (swap_tx, mut swap_rx) = mpsc::channel(100);
      let store = PoolStateStore::new();
      let mgr = SequencerFeedManager::new(config_with_url(server.url()), store, swap_tx);
      let mgr_handle = tokio::spawn(mgr.run());

      // Let it connect
      tokio::time::sleep(Duration::from_millis(100)).await;
      // Inject disconnect
      server.inject(FeedFault::Disconnect).await;
      // Wait for reconnect
      tokio::time::sleep(Duration::from_millis(2000)).await;
      // Assert manager task is still alive (not panicked/exited)
      assert!(!mgr_handle.is_finished());
  }

  #[tokio::test]
  async fn chaos_feed_handles_malformed_message_without_panic() {
      // Inject malformed JSON message
      // Assert manager continues running, no panic
  }

  #[tokio::test]
  async fn chaos_feed_handles_empty_batch_without_panic() {
      // Inject empty batch
      // Assert no panic, no crash
  }

  #[tokio::test]
  async fn chaos_feed_handles_duplicate_sequence_numbers() {
      // Inject same sequence number twice
      // Assert no duplicate DetectedSwap sent
  }

  #[tokio::test]
  async fn chaos_feed_backoff_timing() {
      // Disconnect 3 times rapidly
      // Measure actual reconnect delays
      // Assert they follow exponential pattern (1s, 2s, 4s with ±10% tolerance)
  }

RPC FAULT INJECTION TESTS:

  #[tokio::test]
  async fn chaos_reconciler_handles_rpc_timeout() {
      // Mock fetcher that times out after 100ms
      // Run reconcile_all
      // Assert pools_failed == total_pools
      // Assert reconciler does not crash
  }

  #[tokio::test]
  async fn chaos_reconciler_partial_failure() {
      // 10 pools: 7 succeed, 3 timeout
      // Assert pools_updated == 7, pools_failed == 3
  }

  #[tokio::test]
  async fn chaos_submitter_handles_rpc_timeout() {
      // Mock sender: send() hangs for 60s
      // Submit with 30s timeout
      // Assert returns Err within ~31s
  }

  #[tokio::test]
  async fn chaos_submitter_handles_receipt_never_arriving() {
      // send() returns hash immediately
      // get_receipt() always returns None
      // Assert returns Err with timeout message
  }

Write the complete file. All chaos tests must pass.
```

---

## Mini-Phase 8.3 — Benchmarking Infrastructure ✅ COMPLETE

**Commit:** `9cd34f2` — 2026-03-08

**Definition of done:**
- `benches/hot_paths.rs` exists with exactly 5 Criterion benchmarks
- `cargo bench --bench hot_paths` runs without errors
- All 5 benchmarks produce output with mean ± std dev
- Baseline numbers filled into the comment block after first run
- `scripts/flamegraph.sh` exists and is executable

**Baseline results (recorded 2026-03-08):**

| Benchmark | Mean | Notes |
|---|---|---|
| `path_scan_100_pools` | 467 µs | PathScanner over 100-pool store |
| `v2_compute/1k–1M` | ~17 ns | O(1) — flat across all input sizes |
| `profit_threshold` | 21 ns | Pure function, no I/O |
| `calldata_encode` | 125 ns | ABI encoding via alloy |
| `pool_state_lookup` | 51 ns | DashMap concurrent read |

**Files created:**
- `benches/hot_paths.rs` — 5 Criterion benchmarks
- `scripts/flamegraph.sh` — CPU flamegraph helper (executable)
- `Cargo.toml` — added `criterion = { version = "0.5", features = ["async_tokio"] }`,
  `[[bench]] name = "hot_paths" harness = false`, and `[profile.profiling]`

---

---

# PHASE 9 — Testnet Validation

**Goal:** Run the complete bot on Arbitrum Sepolia. Validate the full funnel works
end-to-end. All five observability metrics must be incrementing correctly before
any mainnet deployment.

---

## Mini-Phase 9.1 — Testnet Infrastructure and Smoke Tests

**Definition of done:**
- Bot runs stably on Arbitrum Sepolia for 10 minutes
- All funnel metrics increment (opportunities detected > 0 within 5 min)
- Smoke test script verifies all metrics are non-zero
- `scripts/run_sepolia.sh` exits 0

---

**PROMPT 9.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 9.1: Testnet Infrastructure and Smoke Tests.

Create the complete testnet validation infrastructure.

File 1: config/sepolia.toml
Full config for Arbitrum Sepolia:
  chain_id = 421614
  sequencer_feed_url = "wss://sepolia-rollup.arbitrum.io/feed"
  rpc_url = "${ARBITRUM_SEPOLIA_RPC_URL}"
  balancer_vault = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"
  uniswap_v3_factory = "0x1F98431c8aD98523631AE4a59f267346ea31F984"
  (other factories same as mainnet — they are deployed on Sepolia)
  min_profit_floor_usd = 0.001
  gas_buffer_multiplier = 1.3
  max_gas_gwei = 2.0
  max_concurrent_simulations = 3
  log_level = "debug"
  metrics_port = 9090

File 2: scripts/run_sepolia.sh
  #!/bin/bash
  set -euo pipefail
  source .env
  required_vars="ARBITRUM_SEPOLIA_RPC_URL ARB_EXECUTOR_ADDRESS PRIVATE_KEY"
  for var in $required_vars; do
    [ -z "${!var}" ] && echo "ERROR: $var not set" && exit 1
  done
  mkdir -p logs
  LOG_FILE="logs/sepolia_$(date +%Y%m%d_%H%M%S).log"
  echo "Starting arbx on Arbitrum Sepolia. Logging to $LOG_FILE"
  cargo run --release -- --config config/sepolia.toml 2>&1 | tee "$LOG_FILE"

File 3: scripts/smoke_test.sh
  #!/bin/bash
  # Run after bot has been running for 5+ minutes
  # Checks all metrics are incrementing as expected
  set -euo pipefail
  METRICS_URL="http://localhost:9090/metrics"
  
  check_metric() {
    local name="$1"
    local value=$(curl -sf "$METRICS_URL" | grep "^${name} " | awk '{print $2}')
    if [ -z "$value" ] || [ "$value" = "0" ]; then
      echo "FAIL: $name is 0 or missing (got: '$value')"
      return 1
    fi
    echo "PASS: $name = $value"
  }
  
  echo "=== arbx Smoke Test ==="
  check_metric "arbx_opportunities_detected_total"
  check_metric "arbx_opportunities_cleared_threshold_total"
  echo "Note: simulation/submission metrics may be 0 if no profitable opps found"
  echo "=== Smoke Test Complete ==="

File 4: docs/TESTNET_VALIDATION.md
Step-by-step guide:
  1. Deploy ArbExecutor to Arbitrum Sepolia (scripts/deploy-sepolia.sh)
  2. Update .env with deployed address
  3. Get Sepolia ETH from faucet: https://faucet.triangleplatform.com/arbitrum/sepolia
  4. Run: ./scripts/run_sepolia.sh
  5. In another terminal: watch -n 30 ./scripts/smoke_test.sh
  6. Definition of done: smoke_test.sh shows PASS for opportunities_detected after 5 min
  7. Run for 10 minutes total with zero panics or unhandled errors

File 5: tests/integration/testnet_validation.rs
  #[tokio::test]
  #[ignore = "requires live Arbitrum Sepolia RPC and deployed contract"]
  async fn testnet_full_pipeline_smoke_test() {
      // Load config from sepolia.toml
      // Start the full pipeline
      // Wait 2 minutes
      // Assert metrics: opportunities_detected > 0
      // Assert: no task panicked
      // Assert: PnlTracker budget not exhausted
      // Shutdown cleanly
  }

Write all files completely.
```

---

---

# PHASE 10 — Mainnet Launch

**Goal:** Deploy to Arbitrum mainnet. Strict budget management. Kill switch tested.
First profitable execution validated. Nothing happens without explicit confirmation.

---

## Mini-Phase 10.1 — Mainnet Deploy, Budget Tracker, Kill Switch

**Definition of done:**
- Deploy script requires explicit "yes" confirmation
- Budget tracker file persists correctly across restarts
- Kill switch halts bot cleanly when budget exhausted
- `cargo test --workspace` passes with kill switch tests

---

**PROMPT 10.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 10.1: Mainnet Deploy, Budget Tracker, Kill Switch.

Create the complete mainnet launch infrastructure.

File 1: scripts/deploy-mainnet.sh
  #!/bin/bash
  set -euo pipefail
  echo "============================================"
  echo "WARNING: Deploying ArbExecutor to ARBITRUM MAINNET"
  echo "This will spend real ETH."
  echo "============================================"
  echo "Type 'deploy mainnet' to confirm:"
  read confirm
  [ "$confirm" != "deploy mainnet" ] && echo "Aborted." && exit 1
  source .env
  forge script contracts/script/Deploy.s.sol \
    --rpc-url "$ARBITRUM_RPC_URL" \
    --broadcast --verify \
    --etherscan-api-key "$ARBISCAN_API_KEY" \
    -vvvv
  echo "Deployment complete. Check deployments/42161.json for address."

File 2: scripts/run-mainnet.sh
  #!/bin/bash
  set -euo pipefail
  echo "============================================"
  echo "WARNING: arbx MAINNET MODE"
  echo "Real funds at risk."
  echo "Gas budget: $60 USD"
  echo "Kill switch: bot auto-halts at $0.10 remaining"
  echo "============================================"
  echo "Type 'run mainnet' to confirm:"
  read confirm
  [ "$confirm" != "run mainnet" ] && echo "Aborted." && exit 1
  source .env
  mkdir -p logs
  LOG_FILE="logs/mainnet_$(date +%Y%m%d_%H%M%S).log"
  echo "Starting. Log: $LOG_FILE"
  cargo run --release -- --config config/default.toml 2>&1 | tee "$LOG_FILE"

File 3: scripts/pnl_report.sh
  #!/bin/bash
  # Print current PnL from the JSON file
  PNL_FILE="${PNL_FILE:-pnl_state.json}"
  if [ ! -f "$PNL_FILE" ]; then
    echo "No PnL file found at $PNL_FILE"
    exit 1
  fi
  echo "=== arbx PnL Report ==="
  cat "$PNL_FILE" | python3 -m json.tool
  NET=$(cat "$PNL_FILE" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['net_pnl_usd'])")
  echo "NET PnL: \$$NET USD"

File 4: docs/MAINNET_LAUNCH.md
Pre-launch checklist:
  SYSTEM CHECKS
  [ ] Testnet validation passed (10 min run, smoke test green)
  [ ] cargo test --workspace passes with zero failures
  [ ] cargo audit passes with no known vulnerabilities
  [ ] Deployed contract address verified on Arbiscan

  CONFIGURATION
  [ ] config/default.toml: min_profit_floor_usd = 0.50
  [ ] config/default.toml: max_concurrent_simulations = 5
  [ ] config/default.toml: max_gas_gwei = 0.1
  [ ] Target pairs: ARB/USDT and WBTC/ETH only (NOT USDC/ETH)
  [ ] PnL budget set to 60.0 USD in code

  WALLET
  [ ] Funded with exactly 0.025 ETH (~$60 at $2400/ETH)
  [ ] Private key in .env (never in any file that touches git)
  [ ] Backup of private key stored securely offline

  MONITORING
  [ ] Grafana dashboard accessible at VPS_IP:3000
  [ ] /metrics endpoint responding: curl localhost:9090/metrics
  [ ] PnL file path writable: touch pnl_state.json

  DURING RUN
  [ ] Check pnl_report.sh every 30 minutes
  [ ] Watch for revert_reason patterns in logs
  [ ] If budget drops below $10, review revert reasons before continuing

File 5: Write kill switch unit test in tests/integration/mod.rs:
  #[tokio::test]
  async fn test_kill_switch_halts_execution_loop() {
      // Create PnlTracker with $1.00 budget
      // Create execution_loop with mock submitter costing $0.20/revert
      // Feed 6 reverted opportunities
      // After 5th ($1.00 spent), kill switch fires
      // Assert execution_loop exits cleanly
      // Assert 6th opportunity was NOT submitted
  }

Write all files completely.
```

---

---

# PHASE 11 — Optimisation

**Goal:** Profile with flamegraphs. Reduce simulation latency to <5ms. Add
three-hop paths with petgraph. Co-locate on Hetzner Frankfurt VPS.

---

## Mini-Phase 11.1 — Profiling Infrastructure and Benchmarks

**Definition of done:**
- `cargo bench` runs without error
- Benchmarks cover all hot paths: simulation, path scanning, profit calculation
- Baseline performance documented

---

**PROMPT 11.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 11.1: Profiling Infrastructure and Benchmarks.

Set up complete benchmarking infrastructure.

Add to root Cargo.toml:
  [dev-dependencies]
  criterion = { version = "0.5", features = ["async_tokio"] }

  [[bench]]
  name = "hot_paths"
  harness = false

Write benches/hot_paths.rs with benchmarks for every critical hot path:

  use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

  // Benchmark 1: Path scanning
  fn bench_path_scan(c: &mut Criterion) {
      // Create store with 100 pools (realistic production size)
      // Benchmark scan() on a single affected pool
      // Target: < 1ms per scan
      c.bench_function("path_scan_100_pools", |b| {
          b.iter(|| scanner.scan(affected_pool))
      });
  }

  // Benchmark 2: AMM output calculation
  fn bench_v2_output(c: &mut Criterion) {
      // Benchmark compute_output_v2 with realistic values
      // Target: < 1 microsecond per calculation
      let mut group = c.benchmark_group("amm_output");
      for amount in [1000u64, 10_000, 100_000, 1_000_000].iter() {
          group.bench_with_input(
              BenchmarkId::new("v2_compute", amount),
              amount,
              |b, &amount| b.iter(|| compute_v2(U256::from(amount), ...)
          );
      }
  }

  // Benchmark 3: Profit threshold calculation
  fn bench_profit_threshold(c: &mut Criterion) {
      // Benchmark compute_min_profit_wei
      // Target: < 10 microseconds (includes mock gas fetch)
  }

  // Benchmark 4: Calldata encoding
  fn bench_calldata_encode(c: &mut Criterion) {
      // Benchmark CallDataEncoder::encode_execute_arb
      // Target: < 100 microseconds
  }

  // Benchmark 5: Pool state DashMap lookup
  fn bench_pool_state_lookup(c: &mut Criterion) {
      // Create store with 1000 pools
      // Benchmark get() with concurrent readers
      // Target: < 100 nanoseconds per read
  }

  criterion_group!(benches, bench_path_scan, bench_v2_output,
                   bench_profit_threshold, bench_calldata_encode,
                   bench_pool_state_lookup);
  criterion_main!(benches);

Also write scripts/flamegraph.sh:
  #!/bin/bash
  set -euo pipefail
  cargo install flamegraph --quiet 2>/dev/null || true
  echo "Running flamegraph profiling for 30 seconds..."
  echo "Make sure arbx is running with real traffic first."
  sudo cargo flamegraph --profile profiling \
    --bin arbx -- --config config/default.toml &
  sleep 30
  kill %1 2>/dev/null
  echo "Flamegraph saved to flamegraph.svg"
  xdg-open flamegraph.svg 2>/dev/null || open flamegraph.svg 2>/dev/null || true

Update root Cargo.toml:
  [profile.profiling]
  inherits = "release"
  debug = 1
  strip = false

Write the complete bench file. cargo bench must run without errors.
```

---

## Mini-Phase 11.2 — Three-Hop Paths with petgraph

**Definition of done:**
- TokenGraph correctly models all token-to-pool relationships
- Three-hop cycles are found correctly
- Property tests verify no false positives or missed paths
- `cargo test -p arbx-detector` passes

---

**PROMPT 11.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 11.2: Three-Hop Paths with petgraph.

Add three-hop path support via petgraph.

Add to detector Cargo.toml:
  petgraph = "0.6"

Write `crates/detector/src/graph.rs`:

  use petgraph::graph::{DiGraph, NodeIndex};
  use std::collections::HashMap;

  pub struct PoolEdge {
      pub pool_address: Address,
      pub dex: DexKind,
      pub fee_tier: u32,
  }

  pub struct TokenGraph {
      graph: DiGraph<Address, PoolEdge>,
      token_to_node: HashMap<Address, NodeIndex>,
      pool_to_edges: HashMap<Address, (NodeIndex, NodeIndex)>, // for updates
  }

  impl TokenGraph {
      pub fn new() -> Self
      pub fn build(pool_store: &PoolStateStore) -> Self
      pub fn update_pool(&mut self, pool: &PoolState)
      pub fn remove_pool(&mut self, pool_address: &Address)
      pub fn two_hop_paths(&self, token: Address) -> Vec<TwoHopPath>
      pub fn three_hop_paths(&self, token: Address) -> Vec<ThreeHopPath>
      pub fn pool_count(&self) -> usize
      pub fn token_count(&self) -> usize
  }

  pub struct TwoHopPath {
      pub token_in: Address,
      pub pool_a: Address, pub token_mid: Address,
      pub pool_b: Address, pub token_out: Address,
  }
  impl TwoHopPath {
      pub fn is_circular(&self) -> bool { self.token_out == self.token_in }
  }

  pub struct ThreeHopPath {
      pub token_in: Address,
      pub pool_a: Address, pub token_mid1: Address,
      pub pool_b: Address, pub token_mid2: Address,
      pub pool_c: Address, pub token_out: Address,
  }
  impl ThreeHopPath {
      pub fn is_circular(&self) -> bool { self.token_out == self.token_in }
  }

Add ThreeHopPath to common/src/types.rs with all required fields.

Tests in #[cfg(test)]:

UNIT TESTS:
  test_empty_graph — new graph has 0 nodes 0 edges
  test_build_from_single_pool — 1 pool, 2 nodes, 2 edges (bidirectional)
  test_build_from_two_pools_shared_token — 2 pools sharing token, 3 nodes, 4 edges
  test_two_hop_finds_cycle — 2 pools: A-B, B-A, assert circular path found
  test_two_hop_no_cycle_no_shared_token — no shared tokens, assert empty
  test_three_hop_finds_cycle — 3 pools forming A→B→C→A cycle, assert found
  test_three_hop_no_false_positive — 3 pools but no circular 3-hop, assert empty
  test_update_pool_changes_edges — change reserve via update_pool, verify graph updated
  test_remove_pool_cleans_graph — remove pool, verify edges gone
  test_all_returned_two_hop_paths_are_circular — property: all have token_in==token_out
  test_all_returned_three_hop_paths_are_circular

PROPERTY TESTS:
  proptest! {
      fn prop_path_count_bounded_by_topology(pool_count in 2usize..20) {
          // n pools can produce at most n*(n-1) two-hop paths
          // Assert path count never exceeds theoretical maximum
      }
      fn prop_no_path_uses_same_pool_twice(pool_count in 2usize..10) {
          // No valid arbitrage path should use the same pool twice
          // (would be circular in a bad way)
      }
  }

Write the complete file. All tests must pass.
```

---

## Mini-Phase 11.3 — VPS Deployment with Docker and Monitoring

**Definition of done:**
- Docker image builds successfully
- `docker-compose up` starts arbx + Prometheus + Grafana
- Grafana dashboard shows all eight metrics with correct visualizations
- Health check endpoint responds correctly

---

**PROMPT 11.3**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 11.3: VPS Deployment with Docker and Monitoring.

Create complete containerized deployment infrastructure.

File 1: Dockerfile
  # Stage 1: Builder — statically linked binary
  FROM rust:1.88.0 AS builder
  RUN apt-get update && apt-get install -y musl-tools musl-dev
  RUN rustup target add x86_64-unknown-linux-musl
  WORKDIR /app
  COPY . .
  RUN RUSTFLAGS="-C target-feature=+crt-static" \
      cargo build --release --target x86_64-unknown-linux-musl \
      --bin arbx
  
  # Stage 2: Runtime — scratch (nothing but the binary)
  FROM scratch
  COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/arbx /arbx
  COPY --from=builder /app/config /config
  EXPOSE 9090
  HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD wget -qO- http://localhost:9090/metrics || exit 1
  ENTRYPOINT ["/arbx", "--config", "/config/default.toml"]

File 2: docker-compose.yml
  version: "3.9"
  services:
    arbx:
      build: .
      restart: unless-stopped
      env_file: .env
      volumes:
        - ./config:/config:ro
        - ./logs:/logs
        - ./pnl_state.json:/pnl_state.json
      ports:
        - "9090:9090"
      depends_on:
        - prometheus
      healthcheck:
        test: ["CMD", "wget", "-qO-", "http://localhost:9090/metrics"]
        interval: 30s
        timeout: 10s
        retries: 3

    prometheus:
      image: prom/prometheus:v2.47.0
      restart: unless-stopped
      volumes:
        - ./monitoring/prometheus.yml:/etc/prometheus/prometheus.yml:ro
        - prometheus_data:/prometheus
      ports:
        - "9091:9090"
      command:
        - --config.file=/etc/prometheus/prometheus.yml
        - --storage.tsdb.retention.time=30d

    grafana:
      image: grafana/grafana:10.2.0
      restart: unless-stopped
      environment:
        GF_SECURITY_ADMIN_PASSWORD: ${GRAFANA_PASSWORD:-admin}
        GF_USERS_ALLOW_SIGN_UP: "false"
      volumes:
        - grafana_data:/var/lib/grafana
        - ./monitoring/grafana/provisioning:/etc/grafana/provisioning:ro
        - ./monitoring/grafana/dashboards:/var/lib/grafana/dashboards:ro
      ports:
        - "3000:3000"
      depends_on:
        - prometheus

  volumes:
    prometheus_data:
    grafana_data:

File 3: monitoring/prometheus.yml
  global:
    scrape_interval: 15s
    evaluation_interval: 15s
  scrape_configs:
    - job_name: arbx
      static_configs:
        - targets: ["arbx:9090"]
      metrics_path: /metrics

File 4: monitoring/grafana/provisioning/datasources/prometheus.yml
  apiVersion: 1
  datasources:
    - name: Prometheus
      type: prometheus
      url: http://prometheus:9090
      isDefault: true

File 5: monitoring/grafana/provisioning/dashboards/arbx.yml
  apiVersion: 1
  providers:
    - name: arbx
      folder: arbx
      type: file
      options:
        path: /var/lib/grafana/dashboards

File 6: monitoring/grafana/dashboards/arbx.json
  Complete Grafana dashboard JSON with these panels:
  - Opportunities detected per minute (rate graph, 1h window)
  - Funnel conversion rates (threshold/sim/submit as % of detected)
  - Transaction success rate (% gauge)
  - Net PnL over time (line graph, USD)
  - Gas spent total (stat panel)
  - Budget remaining (gauge with red zone at < $10)
  - Revert reasons breakdown (pie chart by label)
  - Feed reconnection rate (rate graph)

File 7: docs/VPS_DEPLOYMENT.md
  Complete Hetzner setup guide:
  1. Create account at hetzner.com
  2. Create CX21 server (2 vCPU, 4GB RAM, €5/month)
     Region: Nuremberg or Falkenstein (EU, close to Arbitrum sequencer)
     OS: Ubuntu 22.04
  3. SSH in: ssh root@<IP>
  4. Install Docker:
     curl -fsSL https://get.docker.com | sh
     usermod -aG docker $USER
  5. Clone repo and configure:
     git clone https://github.com/yourname/arbx
     cd arbx
     cp .env.example .env
     nano .env  # fill in all values
  6. Start services:
     docker-compose up -d
  7. Check health:
     docker-compose ps
     curl localhost:9090/metrics
  8. Access Grafana: http://<VPS_IP>:3000

Write all files completely.
```

---

---

# PHASE 12 — Expansion

**Goal:** Add liquidation strategies. Add Timeboost awareness. Integrate Camelot V3.

---

## Mini-Phase 12.1 — Liquidation Detector with Full Test Coverage

**Definition of done:**
- Aave V3 health factor monitoring is implemented
- Liquidation opportunities detected, filtered, simulated
- All liquidation paths tested with mock Aave responses
- `cargo test -p arbx-detector` passes

---

**PROMPT 12.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 12.1: Liquidation Detector with Full Test Coverage.

Add Aave V3 liquidation detection to the detector crate.

Aave V3 on Arbitrum: 0x794a61358D6845594F94dc1DB02A252b5b4814aD

Write `crates/detector/src/liquidation.rs`:

  pub struct LiquidationOpportunity {
      pub collateral_asset: Address,
      pub debt_asset: Address,
      pub user: Address,
      pub debt_to_cover: U256,
      pub collateral_to_receive: U256,
      pub liquidation_bonus_bps: u16,
      pub estimated_profit_wei: U256,
      pub estimated_gas_cost_wei: U256,
      pub net_profit_wei: U256,
  }

  #[cfg_attr(test, mockall::automock)]
  #[async_trait]
  pub trait AaveFetcher: Send + Sync {
      async fn get_user_account_data(&self, user: Address) -> anyhow::Result<UserAccountData>;
      async fn get_reserve_data(&self, asset: Address) -> anyhow::Result<ReserveData>;
      async fn get_user_configuration(&self, user: Address) -> anyhow::Result<UserConfiguration>;
  }

  pub struct UserAccountData {
      pub total_collateral_usd: U256,
      pub total_debt_usd: U256,
      pub health_factor: U256,  // 1e18 = 1.0. Below 1.0 = liquidatable
  }

  pub struct LiquidationDetector<A: AaveFetcher, G: GasFetcher> {
      aave: A,
      gas: G,
      strategy: StrategyConfig,
      at_risk_users: Arc<DashMap<Address, u64>>, // user => last_check_block
  }

  impl<A: AaveFetcher, G: GasFetcher> LiquidationDetector<A, G> {
      pub fn new(aave: A, gas: G, strategy: StrategyConfig) -> Self

      pub async fn run(
          self,
          liquidation_tx: mpsc::Sender<LiquidationOpportunity>
      ) -> anyhow::Result<()>
      // Subscribes to Aave events, polls at-risk users every block

      pub async fn check_user(
          &self, user: Address
      ) -> anyhow::Result<Option<LiquidationOpportunity>>
      // Returns Some if health_factor < 1.0 AND net_profit > threshold
  }

Tests using MockAaveFetcher and MockGasFetcher:

  test_unhealthy_position_detected — health_factor = 0.95e18, assert Some returned
  test_healthy_position_not_detected — health_factor = 1.05e18, assert None
  test_liquidation_at_exactly_1_not_detected — health_factor = 1e18, assert None
  test_unprofitable_liquidation_filtered — profit < gas_cost, assert None
  test_profitable_liquidation_correct_fields — verify all fields computed correctly
  test_liquidation_bonus_calculation — 5% bonus, verify collateral_to_receive correct
  test_at_risk_user_tracked — checked user added to at_risk_users map
  test_healthy_user_removed_from_at_risk — position healed, removed from map

Write the complete file. All tests must pass.
```

---

## Mini-Phase 12.2 — Timeboost Research and Documentation

**Definition of done:**
- Timeboost economics are documented with break-even analysis
- Configuration support for optional Timeboost participation
- `cargo build --workspace` passes

---

**PROMPT 12.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 12.2: Timeboost Research and Documentation.

Add Timeboost awareness to arbx.

Background: Arbitrum's Timeboost auction gives the winning bidder a 200ms
"express lane" advantage. Non-express transactions are delayed by 200ms.
For single-block arbs, this is a structural disadvantage. The question is:
at what profit level does buying the express lane pay off?

File 1: Add to StrategyConfig in config.rs:
  pub timeboost_enabled: bool,               // default false
  pub timeboost_max_bid_per_minute_usd: f64, // default 0.0
  pub timeboost_express_lane_endpoint: Option<String>,

File 2: Create `crates/executor/src/timeboost.rs`:

  pub struct TimeboostClient {
      express_lane_url: String,
      max_bid_usd: f64,
  }

  impl TimeboostClient {
      pub fn new(url: String, max_bid_usd: f64) -> Self

      // Submit transaction via express lane instead of standard sequencer
      pub async fn submit_express(
          &self,
          calldata: Bytes,
          to: Address,
          gas_limit: u64,
      ) -> anyhow::Result<TxHash>

      // Calculate if Timeboost ROI makes sense given recent PnL
      pub fn should_participate(
          &self,
          recent_pnl_per_minute_usd: f64,
          current_bid_usd: f64,
      ) -> bool
      // Returns true only if expected_gain > current_bid_cost
  }

File 3: docs/TIMEBOOST_ANALYSIS.md
  # Timeboost Break-Even Analysis

  ## What is Timeboost?
  [Explain FCFS vs express lane, 200ms advantage]

  ## The Economics
  If we win X arbs per hour at $Y average profit each:
  - Hourly revenue = X * Y
  - Express lane cost = Z USD per hour (auction price)
  - Break-even: X * Y > Z

  ## Current Reality (Phase 2, no Timeboost)
  - Without express lane, we lose every single-block race to express lane holders
  - Mitigation: target multi-block price dislocations (persist for 2+ blocks)
  - These are less contested because they don't require express lane advantage

  ## When to Enable (Phase 12+ calculation)
  [Show the formula: if hourly_pnl > 2 * express_lane_cost, enable Timeboost]

  ## Implementation Plan
  1. Track: how many arbs are we losing to express lane holders?
     (detect via revert reason pattern: "No profit" within 200ms of detection)
  2. Estimate: what would our win rate have been with express lane?
  3. Calculate: break-even bid amount
  4. Enable when: break-even < max_bid_per_minute_usd from config

Tests for should_participate():
  test_participate_when_roi_positive — pnl=10/min, bid=5/min, assert true
  test_no_participate_when_roi_negative — pnl=2/min, bid=5/min, assert false
  test_no_participate_when_disabled — timeboost_enabled=false, always false

Write all files completely. All tests must pass.
```

---

---

# PHASE 13 — Open Source and Grants

**Goal:** Make arbx genuinely impressive to open source. Write documentation
that makes Sigma Prime, Paradigm, and Flashbots want to read more. Apply for grants.

---

## Mini-Phase 13.1 — Production Documentation

**Definition of done:**
- README.md is world-class
- ARCHITECTURE.md explains every design decision with rationale
- CONTRIBUTING.md enables external contributors
- All docs render correctly in GitHub

---

**PROMPT 13.1**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 13.1: Production Documentation.

Write all documentation to make arbx genuinely impressive as an open source project.

File 1: README.md (complete, production quality)
Structure:
  - Header: arbx logo/badge area with CI badge, license badge, Rust version badge
  - One-line summary: "A production-grade, flash-loan-powered MEV arbitrage engine
    for Arbitrum, written in Rust from scratch with a $60 gas budget."
  - Why it exists (2 paragraphs: the problem, the approach)
  - ASCII architecture diagram showing three-layer design
  - Features list (key technical properties)
  - Performance characteristics (from SSOT success metrics)
  - Quick start (5 commands: clone, install foundry, build, deploy to sepolia, run)
  - Configuration reference (table: field, type, default, description)
  - Observability: metrics funnel diagram, how to read it
  - How to run tests (unit, integration, property, chaos, fork)
  - Deployment guide (pointer to docs/)
  - Architecture (pointer to ARCHITECTURE.md)
  - Contributing (pointer to CONTRIBUTING.md)
  - License: MIT

File 2: docs/ARCHITECTURE.md (deep technical)
  - Full data flow diagram (ASCII, detailed)
  - Layer 1 Ingestion: why sequencer feed, why sequencer_client crate,
    why DashMap, reconciler design rationale
  - Layer 2 Brain: path scanning algorithm, why two-hop first,
    AMM math derivation, 2D gas model explanation with example numbers,
    why Balancer V2 over Aave (9bps savings calculation)
  - Layer 3 Execution: why backrunning not frontrunning, NodeInterface precompile,
    why direct sequencer submission
  - Testing strategy: unit / integration / property / chaos / fork layers
  - Known limitations and future work (Timeboost, three-hop, liquidations)
  - Economic model: expected PnL per opportunity, capital efficiency

File 3: CONTRIBUTING.md
  - How to set up dev environment
  - Running all test suites
  - Code style (rustfmt + clippy -D warnings must pass)
  - How to add a new DEX
  - How to add a new strategy
  - PR requirements (all tests pass, no clippy warnings, description of changes)

File 4: CHANGELOG.md
  # Changelog
  ## [Unreleased]
  ### Added
  - Initial implementation of three-layer MEV arbitrage engine
  - Arbitrum sequencer feed integration
  - Balancer V2 flash loan integration (0% fee)
  - Uniswap V3 and UniswapV2-style DEX support
  - Full property-based test suite with proptest
  - Chaos tests for WebSocket fault injection
  - Docker + Prometheus + Grafana deployment stack

File 5: LICENSE
  MIT License
  Copyright (c) 2024 [Your Name]
  [Full MIT license text]

Write all files completely and to the highest quality.
Make ARCHITECTURE.md genuinely educational — this is what gets you noticed.
```

---

## Mini-Phase 13.2 — Blog Series Outline and Grant Applications

**Definition of done:**
- Blog series outline covers 6 posts
- EF ESP and Flashbots grant applications are complete
- All docs ready for submission

---

**PROMPT 13.2**

```
You are building `arbx`. Read SSOT.md in full before writing any code.
This is Mini-Phase 13.2: Blog Series and Grant Applications.

Write the complete blog and grant materials.

File 1: docs/BLOG_SERIES.md
Outline for a 6-part blog series. Each post should be publishable on Mirror,
Substack, or a personal site. This is the writing that gets you noticed by
Flashbots, Paradigm, and Sigma Prime.

Post 1: "Building a Profitable MEV Bot with $60"
  - The challenge: institutional players dominate mainnet
  - Why Arbitrum: FCFS model, sequencer feed visibility, lower competition
  - The architecture overview: three layers
  - First profitable transaction (tell the story)
  Target audience: developers curious about MEV, non-technical readers welcome

Post 2: "Arbitrum's Sequencer Feed: The Only MEV Edge That Matters on L2"
  - Deep dive on wss://arb1.arbitrum.io/feed
  - Why Base and other OP Stack chains can't do this
  - How to parse the feed correctly (sequencer_client crate)
  - What transactions look like at the sequencer level
  Target audience: Ethereum infrastructure engineers

Post 3: "Balancer V2 Flash Loans: The 9 Basis Point Advantage"
  - Why Aave costs 9bps and why that kills thin arbs
  - Balancer V2: always 0% fee, same API
  - The math: at 10 trades/day, Balancer saves you $90/day in fees
  - Implementation: IFlashLoanRecipient vs IFlashLoanReceiver
  Target audience: DeFi developers building on flash loans

Post 4: "Arbitrum's Hidden Gas Tax: The 2D Gas Model"
  - What eth_estimateGas doesn't tell you
  - L1 calldata gas: the silent killer on mainnet gas spikes
  - NodeInterface precompile: how to query the real cost
  - The $1.50 transaction that looked like a $0.10 transaction
  Target audience: Arbitrum developers

Post 5: "Production MEV Infrastructure: How We Test for Correctness"
  - Why property-based testing matters for financial software
  - AMM math invariants: what proptest found that unit tests missed
  - Chaos testing: feed disconnects, RPC timeouts, duplicate events
  - Historical regression tests: fork testing on real Arbitrum blocks
  Target audience: Rust systems engineers, MEV researchers

Post 6: "From $0 to Profitable: The Arbitrum MEV Bot PnL Report"
  - Complete transparent PnL breakdown
  - What worked, what didn't
  - The USDC/ETH vs mid-tier pair comparison
  - Timeboost: when does buying the express lane pay off?
  - What I'd do differently
  Target audience: everyone in DeFi

File 2: docs/GRANTS.md
Complete grant applications for two programs:

=== ETHEREUM FOUNDATION ESP APPLICATION ===

Project name: arbx
Category: Developer Tooling / Infrastructure
Stage: Deployed and generating revenue

Summary (100 words):
arbx is an open-source, production-grade MEV arbitrage engine for Arbitrum
written in Rust. Built by a solo developer with a $60 gas budget, arbx
demonstrates that well-engineered software can compete with institutional MEV
infrastructure. The project includes comprehensive documentation of Arbitrum's
sequencer feed mechanics, 2D gas model, and Balancer V2 flash loan integration —
educational resources that benefit the entire Arbitrum developer ecosystem.
All source code, tests, and a 6-part blog series are fully public.

Problem statement:
MEV infrastructure knowledge is concentrated at well-capitalized firms.
Solo developers lack accessible, production-quality reference implementations.
The Arbitrum sequencer feed is powerful but poorly documented.

Solution:
A fully open-source, thoroughly documented, thoroughly tested MEV engine
that serves as both a working system and an educational resource.

Technical approach:
[Summarize three-layer architecture, key design decisions from SSOT]

Public goods value:
- First open-source Arbitrum sequencer feed integration in Rust
- Documentation of 2D gas model with worked examples
- Property-tested AMM math (prevents the class of bugs that caused $X in losses)
- 6-part blog series reaching [N] developers

Team:
[Your details: first-year CS student, CometBFT PR contributor, etc.]

Budget requested: $5,000 USD
Breakdown:
- Hetzner Frankfurt VPS: $60/year
- Alchemy/QuickNode API: $200/year
- Development time for expansion phases: $4,740

Success metrics:
- GitHub stars: 500+ within 6 months
- Blog series total reads: 10,000+
- External PRs from community: 5+
- Grant outcome: documented and published

=== FLASHBOTS GRANT APPLICATION ===

Project name: arbx — Open Source Arbitrum MEV Engine
Category: MEV Tooling

What are you building:
An open-source MEV searcher for Arbitrum that:
1. Integrates with Arbitrum's sequencer feed for real-time opportunity detection
2. Uses Balancer V2 for 0% flash loans (lower barrier than Aave)
3. Implements full simulation with revm before any on-chain submission
4. Documents the complete MEV supply chain for the Arbitrum ecosystem

Why does this matter to Flashbots' mission:
Democratizing MEV access requires open tooling. arbx lowers the barrier
for solo developers to understand and participate in MEV. It documents
Arbitrum-specific mechanics (sequencer feed, 2D gas, FCFS ordering) that
are not well understood outside institutional teams.

Open source contribution:
- Complete source: github.com/[yourname]/arbx
- Documentation: [link to blog series]
- All tests public: property tests, chaos tests, fork tests

Budget requested: $3,000 USD
Intended use: infrastructure costs + documentation time

Write all files completely. Make the grant applications compelling.
This is your shot at funding — write it like it matters.
```

---

---

## Phase Completion Checklist

```
Phase 0  — Project Hygiene        [x] 0.1 [x] 0.2
Phase 1  — Foundation             [x] 1.1 [x] 1.2 [x] 1.3 [x] 1.4
Phase 2  — Smart Contract         [x] 2.1 [x] 2.2 [x] 2.3 [x] 2.4 [x] 2.5
Phase 3  — Ingestion Engine       [x] 3.1 [x] 3.2 [x] 3.3
Phase 4  — Opportunity Brain      [x] 4.1 [x] 4.2
Phase 5  — Simulation Engine      [x] 5.1 [x] 5.2
Phase 6  — Execution Engine       [x] 6.1 [x] 6.2
Phase 7  — Integration            [x] 7.1 [x] 7.2
Phase 8  — Property & Chaos       [x] 8.1 [x] 8.2 [x] 8.3
Phase 9  — Testnet Validation     [x] 9.1 [x] 9.2
Phase 10 — Mainnet Launch         [ ] 10.1
Phase 11 — Optimisation           [ ] 11.1 [ ] 11.2 [ ] 11.3
Phase 12 — Expansion              [ ] 12.1 [ ] 12.2
Phase 13 — Open Source & Grants   [ ] 13.1 [ ] 13.2
```

Total: 13 phases, 28 mini-phases, 28 self-contained prompts.

**Last completed:** 9.2 — Anvil fork validation (commit `1e2ade1`, 2026-03-22)
**Next up:** 10.1 — Mainnet Launch

---

## Test Coverage Map

Every layer has multiple test types:

| Layer | Unit | Property | Integration | Chaos | Fork |
|---|---|---|---|---|---|
| Types (common) | ✓ | ✓ | — | — | — |
| Config | ✓ | — | — | — | — |
| Metrics | ✓ | — | ✓ | — | — |
| Smart Contract | ✓ | ✓ (fuzz) | — | — | ✓ |
| Pool State Store | ✓ | — | ✓ (concurrent) | — | — |
| Sequencer Feed | ✓ | — | — | ✓ | — |
| Block Reconciler | ✓ | — | — | ✓ | — |
| Path Scanner | ✓ | ✓ | — | — | — |
| Profit Calculator | ✓ | ✓ | — | — | — |
| Simulation | ✓ | ✓ | — | — | ✓ (regression) |
| Submitter | ✓ | — | ✓ | ✓ | — |
| PnL Tracker | ✓ | ✓ | — | — | — |
| Full Pipeline | — | — | ✓ | — | ✓ (testnet) |
| Liquidations | ✓ | — | — | — | — |
| Timeboost | ✓ | — | — | — | — |

---

## Rules For Using The Prompts

1. **Always include both SSOT.md and ROADMAP.md in context.** The LLM needs both.

2. **Run prompts in strict phase order.** Dependencies are real — skipping phases
   will produce type errors and missing imports in downstream phases.

3. **The definition of done is a hard gate.** Do not proceed until:
   - `cargo build --workspace` passes
   - `cargo clippy --workspace -- -D warnings` passes
   - `cargo test --workspace` passes with zero failures
   - (For contract phases) `forge test` passes

4. **When a prompt produces broken code:** paste the exact compiler error back
   to the LLM as a follow-up: "Fix this error: [error]". Do not move on until fixed.

5. **Fork tests tagged #[ignore]:** These require a live ARBITRUM_RPC_URL.
   Run them manually before deploying: `cargo test --workspace -- --ignored`

6. **Keep SSOT.md updated** if architectural decisions change during implementation.
   The SSOT is not a historical document — it is the current truth.

7. **The blog series is not optional.** Writing about what you build is what
   makes people at Sigma Prime read your 6-part MPT series before an interview.
   Do Phase 13 even if you think it's not worth it. It is.
