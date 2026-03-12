# Anvil Fork Validation

Pre-mainnet sanity check.  Runs the full bot against an Arbitrum mainnet
fork (Anvil) with `dry_run = true` — no real ETH is ever spent.

The fork provides **real pool state** (mainnet reserves at a pinned block).
The live sequencer feed provides **real swap events** happening right now.
Together they give the closest approximation to mainnet without financial risk.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Anvil (local)                                              │
│  Arbitrum mainnet fork at block 105_949_098                 │
│  http://127.0.0.1:8545                                      │
│                                                             │
│  ← all eth_call / getLogs / state queries go here          │
└─────────────────────────────────────────────────────────────┘
             ↑                              ↑
         pool seeder                  revm simulation
         block reconciler             (uses fork RPC)
             ↑
┌─────────────────────────────────────────────────────────────┐
│  Live Arbitrum Sequencer Feed                               │
│  wss://arb1.arbitrum.io/feed                                │
│                                                             │
│  ← real mainnet swap events stream in                      │
└─────────────────────────────────────────────────────────────┘
             ↑
         swap detection
         path scanner
         profit filter
             ↓
         [simulation] ← fork state (may be slightly stale — acceptable)
             ↓
         DRY RUN: log "would submit" — no tx broadcast
```

---

## Prerequisites

| Requirement | Check |
|---|---|
| [Foundry](https://getfoundry.sh) installed | `anvil --version && cast --version` |
| `ARBITRUM_RPC_URL` in `.env` | Alchemy / QuickNode Arbitrum mainnet URL |
| `ARB_EXECUTOR_ADDRESS` in `.env` | Deployed contract (Sepolia deploy is fine) |
| `PRIVATE_KEY` in `.env` | Any key — no real tx is sent |
| arbx binary built | `cargo build --release --bin arbx` |

---

## Run

```bash
# One-time setup
chmod +x scripts/run_anvil_fork.sh scripts/anvil_smoke_test.sh

# Terminal 1 — start Anvil fork and run arbx for 10 minutes
./scripts/run_anvil_fork.sh

# Terminal 2 — check metrics after ~2 minutes
./scripts/anvil_smoke_test.sh
```

Override options:

```bash
# Use a different fork block
FORK_BLOCK_NUMBER=106000000 ./scripts/run_anvil_fork.sh

# Run for 20 minutes instead of 10
RUN_DURATION=1200 ./scripts/run_anvil_fork.sh

# Check metrics on a non-default port
./scripts/anvil_smoke_test.sh --url http://localhost:19090
```

---

## What to look for in logs

Look at `logs/anvil_fork_<timestamp>.log` while the run is active:

| Log pattern | Meaning |
|---|---|
| `PoolStateStore bootstrapped ... seeded=N` | Factory scan worked; N pools found |
| `sequencer feed connected` | Feed handshake succeeded |
| `scanning N candidate two-hop path(s)` | Path scanner firing on real swaps |
| `opportunity cleared profit threshold` | Profit filter passed |
| `simulation succeeded` | **The real signal** — full arb logic validated |
| `DRY RUN — would submit arb transaction` | Simulation passed, skipping broadcast |
| `below profit threshold` | Opportunity found but not profitable enough |
| `simulation failed` | Stale reserves or path no longer valid |

---

## Definition of done

The Anvil fork validation is **complete** when at least one of the following
appears in the logs or metrics within a 10-minute run:

| Signal | Where |
|---|---|
| `simulation succeeded` in logs | `logs/anvil_fork_*.log` |
| `arbx_opportunities_cleared_simulation_total > 0` in metrics | `./scripts/anvil_smoke_test.sh` |

If **neither** appears after 10 minutes, the pipeline is still valid — it just
means no profitable opportunity occurred at the fork block state during the run
window.  Check the triage steps below before concluding there is a bug.

---

## Triage: simulation count stays 0

Work through these checks in order:

### 1. Is the pool store populated?

```bash
grep 'bootstrapped\|seeded' logs/anvil_fork_*.log
```

Expected: `PoolStateStore bootstrapped ... seeded=N` with `N > 0`.

If `N = 0`: the factory scan found no pools in the window
`seed_from_block` → fork block.  Try lowering `seed_from_block` in
`config/anvil_fork.toml` (e.g. `105000000` to scan more history).

### 2. Are swaps being detected?

```bash
grep 'scanning\|DetectedSwap\|swap.*sel\|detected' logs/anvil_fork_*.log | head -20
```

Expected: several lines per minute as mainnet swap activity flows in.

If silent: the sequencer feed may have disconnected.  Look for
`reconnecting` or `feed error` log lines.

### 3. Are paths being found?

```bash
grep 'two-hop\|candidate path\|no two-hop' logs/anvil_fork_*.log | head -10
```

If `no two-hop paths for pool` dominates: the swapped pools are not in
the store.  The reconciler should fill them in over time via feed-first
discovery — wait a few more minutes.

### 4. Is the profit threshold too high?

Check `config/anvil_fork.toml`:
```toml
min_profit_floor_usd = 0.01   # already very low
max_gas_gwei         = 0.5    # Anvil inherits mainnet base fee
```

If the fork block has high base fee, real gas cost may exceed the
$0.01 floor even with `max_gas_gwei = 0.5`.  Try `max_gas_gwei = 1.0`
and `min_profit_floor_usd = 0.001` for maximum permissiveness.

### 5. Fork block is stale

The sequencer feed delivers current mainnet swaps, but revm simulates
against fork block state.  If a pool was created or had its reserves
dramatically changed after the fork block, simulation may reject it.
This is expected — it proves the simulation is working correctly, not
that it is broken.

---

## Transitioning to Phase 10

Once `simulation succeeded` appears at least once:

1. Remove `dry_run = true` from `config/anvil_fork.toml` (or create
   `config/mainnet.toml` based on `config/default.toml`)
2. Confirm `ARB_EXECUTOR_ADDRESS` is deployed to Arbitrum mainnet
3. Fund the executor with a small ETH amount for gas (~$5)
4. Run with `--config config/mainnet.toml` (no `--dry-run`)
5. Watch `arbx_transactions_succeeded_total` and `arbx_net_pnl_wei`

See `docs/ROADMAP.md` Phase 10 for the full mainnet launch checklist.

---

## Files created in this phase

| File | Purpose |
|---|---|
| `config/anvil_fork.toml` | Bot config pointing at local Anvil RPC |
| `scripts/run_anvil_fork.sh` | Starts Anvil fork + funds executor + runs arbx |
| `scripts/anvil_smoke_test.sh` | Checks Prometheus metrics during/after run |
| `docs/ANVIL_FORK_VALIDATION.md` | This runbook |
