# arbx Mainnet Launch Runbook

## Overview

This runbook covers the first real-money launch flow for `arbx` on Arbitrum mainnet.

The operating model in Phase 10.1 is intentionally small and strict:

- total budget: **$31 USD**
- estimated deploy cost: **about $4 USD**
- execution budget: **about $27 USD**
- warning threshold: **$5 USD remaining**
- kill threshold: **$2 USD remaining**

The goal is not aggressive scaling. The goal is safe first contact with mainnet conditions.

## Pre-Launch Checklist

### System readiness

- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release` passes
- [ ] `cargo audit` passes
- [ ] the Anvil fork validation path has been run successfully

### Wallet readiness

- [ ] the deployer wallet is funded with enough ETH for deployment and a small execution budget
- [ ] `PRIVATE_KEY` exists only in `.env` or another local secret store
- [ ] the private key has been backed up securely offline
- [ ] `ARB_EXECUTOR_ADDRESS` will be updated after deployment

### Contract readiness

- [ ] `scripts/deploy-mainnet.sh` has been reviewed before use
- [ ] the deployed address will be written to `deployments/42161.json`
- [ ] `cast call "$ARB_EXECUTOR_ADDRESS" "owner()(address)"` returns the expected owner

### Config readiness

- [ ] `config/mainnet.toml` is the intended launch config
- [ ] `dry_run = false` is confirmed before live execution
- [ ] only the intended mid-tier pools are enabled
- [ ] `budget.warn_at_usd = 5.0` is confirmed
- [ ] `budget.kill_at_usd = 2.0` is confirmed

### Monitoring readiness

- [ ] `curl http://localhost:9090/metrics` responds
- [ ] the PnL state file path is writable
- [ ] a second terminal is ready for `./scripts/pnl_report.sh`
- [ ] the log directory is writable

## Deployment

Deploy only after the checklist is complete.

```bash
./scripts/deploy-mainnet.sh
```

The script requires explicit typed confirmation before broadcasting. That is intentional. Nothing should deploy to mainnet by accident.

After deployment:

1. confirm the contract address in `deployments/42161.json`
2. update `ARB_EXECUTOR_ADDRESS` in `.env` if needed
3. verify ownership and basic connectivity with `cast call`

## Starting the Mainnet Run

```bash
./scripts/run-mainnet.sh
```

This script also requires explicit typed confirmation. Before it starts, it checks the configured contract address, prints the launch budget model, and then runs the release binary against `config/mainnet.toml`.

## What to Watch During the Run

### Budget health

Run the budget report regularly:

```bash
./scripts/pnl_report.sh
```

This report gives the quickest operator view of:

- remaining budget
- gas spent
- revert rate
- successful arbitrage count
- average profit per successful trade

### Logs worth tailing

```bash
grep "budget_warn\|budget_exhausted\|revert_reason\|l1_gas_cost" logs/mainnet_*.log | tail -20
```

Key warning patterns:

| Signal | What it usually means |
|---|---|
| repeated `No profit` reverts | another bot is winning the race after simulation |
| high `l1_gas_cost` values | mainnet calldata cost is eating the edge |
| `budget_warn` events | the execution budget is nearing the warning threshold |
| `budget_exhausted` event | the kill switch has fired and the bot should stop |

### Success-rate triage

If success rate drops below roughly 20 percent for an extended period:

1. stop the bot cleanly
2. check the most common revert reason
3. check whether L1 calldata cost spiked
4. check whether reserve staleness is showing up in logs
5. do not resume until the failure pattern makes sense

## Emergency Stop

Normal stop:

```bash
Ctrl+C
```

The runtime handles shutdown gracefully. It aborts the supervised task set, persists PnL state, and exits.

If the process does not exit normally:

```bash
pkill -f "cargo run"
pkill -f "arbx"
```

The persisted PnL file allows the next run to resume with the correct budget tracking state.

## Kill Switch Behavior

The kill switch is a core safety feature, not a convenience feature.

Behavior:

1. The PnL tracker records gas spend after each submission.
2. The budget watchdog checks the remaining budget every minute.
3. At or below the warning threshold, the runtime emits `budget_warn` log events.
4. At or below the kill threshold, the watchdog returns an error.
5. That error triggers a supervised shutdown and persists state to disk.

This prevents the bot from slowly grinding the account down through repeated failed submissions.

## Practical Launch Guidance

Start conservatively.

- Keep the pair set small.
- Prefer the configured mid-tier pools over the most competitive routes.
- Do not widen scope during the first live run.
- Treat the first session as an observational run with strict budget discipline.

If the first live session shows repeated failed races or bad economics, the correct response is to stop, inspect, and adjust, not to keep spending.

## Definition of Done

Phase 10 is meaningfully validated when all of the following are true:

- at least one profitable arbitrage completes on mainnet
- cumulative net PnL is positive, even if small
- the bot runs for at least thirty minutes without panic or crash
- `scripts/pnl_report.sh` shows at least one successful arbitrage

Until then, the system should be treated as launch infrastructure with safety rails, not as a proven profitable production engine.
