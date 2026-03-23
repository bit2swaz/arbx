# arbx Testnet Validation

## Overview

The original Phase 9 plan was to validate the full pipeline on Arbitrum Sepolia. In practice, Sepolia is a ghost chain for MEV work. The network is alive, but there is almost no useful DEX liquidity and almost no real swap flow worth backrunning.

Because of that, Phase 9 split into two useful validation tracks:

1. **Arbitrum Sepolia infrastructure validation**, to prove the bot could connect, seed state, and expose metrics.
2. **Anvil mainnet fork validation**, to prove the real detection pipeline worked against mainnet pool state without spending real money.

This document reflects what actually happened, not what the original plan hoped would happen.

## What Was Validated

| Component | Validated | Method |
|---|---|---|
| Sequencer feed connection | Yes | Live Arbitrum Sepolia |
| Pool state seeding (640 pools) | Yes | Real factory scan |
| Block reconciler | Yes | Live blocks |
| Prometheus metrics | Yes | Smoke test script |
| Swap detection | Yes | Synthetic injection via cast send |
| Opportunity detection | Yes | 4 paths found on synthetic swap |
| Profit simulation | No | No profitable opportunity at fork block |
| Transaction submission | No | Dry run mode |

The key result is that the pipeline from detection input to simulated decision-making was exercised under realistic conditions, even though no real profitable opportunity appeared during the validation window.

## What Was Not Validated

The full live cycle, detect a real profitable swap, simulate it successfully, submit it on-chain, and see it land profitably, has **not** been observed on real live market data yet.

That sounds like a big gap, but it is acceptable at this stage for three reasons:

1. The codebase already has strong unit, integration, property, fuzz, chaos, and fork coverage.
2. The Anvil fork setup proved that the live detection path can feed the rest of the runtime using real mainnet state.
3. Phase 10 added the missing operational safety rails, especially the budget tracker and kill switch, before any real-money run.

In other words, Phase 9 did not prove profitability. It proved infrastructure readiness and detection-path correctness.

## Running Testnet Validation

The most useful validation path is the Anvil mainnet fork flow.

### 1. Set up environment variables

```bash
cp .env.example .env
```

Fill in at least these values:

```text
ARBITRUM_RPC_URL=https://arb-mainnet.g.alchemy.com/v2/YOUR_KEY
ARBITRUM_SEPOLIA_RPC_URL=https://arb-sepolia.g.alchemy.com/v2/YOUR_KEY
ARB_EXECUTOR_ADDRESS=0xYOUR_DEPLOYED_CONTRACT
PRIVATE_KEY=0xYOUR_TEST_KEY
```

### 2. Build the binary

```bash
cargo build --release --bin arbx
```

### 3. Start the fork validation run

```bash
./scripts/run_anvil_fork.sh
```

This script starts a local Anvil fork, runs the bot in dry-run mode, and injects a synthetic direct pool swap with `cast send` so the detection pipeline is guaranteed to fire during the session.

### 4. Check the smoke-test metrics

In a second terminal:

```bash
./scripts/anvil_smoke_test.sh
```

You should see the funnel move, including non-zero detections. In the recorded Phase 9.2 run, the smoke test passed and `opportunities_detected = 4`.

### 5. Review the logs

```bash
tail -f logs/anvil_fork_*.log
```

Look for these messages:

- `PoolStateStore bootstrapped from factory logs`
- `sequencer feed connected`
- `scanning ... candidate two-hop path(s)`
- `DRY RUN, would submit arb transaction`
- `simulation failed`, if the path was not profitable at that block state

### 6. Shut down cleanly

Press `Ctrl+C` in the bot terminal. The runtime persists PnL state and exits cleanly.

## Running on Arbitrum Sepolia

Arbitrum Sepolia is still useful for checking that the bot can start, connect, and expose metrics, but it is not a realistic MEV proving ground.

You can still run it:

```bash
./scripts/run_sepolia.sh
./scripts/smoke_test.sh
```

What this proves:

- the sequencer feed connection works
- the runtime starts cleanly
- metrics are exposed
- pool discovery and reconciliation code run

What it does **not** reliably prove:

- profitable opportunities exist
- simulation will clear on real live opportunities
- submission logic will be exercised meaningfully

To get real Sepolia validation, you would need to create the conditions yourself. In practice that means:

1. Deploy your own test pools.
2. Seed them with enough liquidity to create price movement.
3. Inject your own swaps.
4. Force a measurable cross-pool price difference.

Without that setup, Sepolia mostly validates plumbing, not trading behavior.

## Why the Anvil Fork Was the Right Move

The mainnet fork gives you something Sepolia cannot:

- real pool contracts
- real liquidity layouts
- real reserve data
- realistic swap math
- no real-money risk because the run stays in dry-run mode

That made it the correct bridge between pure tests and eventual mainnet operation.

## Bottom Line

Phase 9 proved that `arbx` can:

- connect to live Arbitrum infrastructure
- seed and reconcile real pool state
- detect swaps through the live pipeline
- discover two-hop candidate paths
- run safely in dry-run mode against a mainnet fork

Phase 9 did **not** prove that the bot has already captured a live profitable arbitrage. That proof belongs to mainnet execution, which is why Phase 10 focused on explicit confirmations, budget controls, and clean shutdown behavior before any real-money run.
