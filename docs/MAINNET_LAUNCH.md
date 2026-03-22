# arbx Mainnet Launch Runbook

Total budget: $31 USD (~3000 INR)
Deploy cost:  ~$4 USD (one-time)
Execution:    ~$27 USD remaining
Kill switch:  halts at $2.00 remaining

---

## Pre-Launch Checklist

### System
- [ ] cargo test --workspace passes, zero failures
- [ ] cargo audit passes, no unpatched vulnerabilities
- [ ] cargo build --release succeeds
- [ ] Anvil fork smoke test passed (opportunities_detected > 0)

### Wallet
- [ ] Funded with 0.015 ETH minimum (~$35 at current price)
- [ ] PRIVATE_KEY in .env only — never committed to git
- [ ] Private key backed up securely offline
- [ ] ARB_EXECUTOR_ADDRESS updated in .env after deploy

### Contract
- [ ] Deployed via scripts/deploy-mainnet.sh
- [ ] Verified on Arbiscan (check deployments/42161.json)
- [ ] cast call $ARB_EXECUTOR_ADDRESS "owner()(address)" confirms your address

### Configuration
- [ ] config/mainnet.toml: min_profit_floor_usd = 0.50
- [ ] config/mainnet.toml: max_gas_gwei = 0.1
- [ ] config/mainnet.toml: dry_run = false
- [ ] Target pairs: ARB/USDT and WBTC/ETH ONLY
- [ ] USDC/ETH NOT in known_pools (too competitive)
- [ ] budget.kill_at_usd = 2.00 confirmed

### Monitoring
- [ ] curl localhost:9090/metrics responds
- [ ] arbx_pnl_state.json path is writable
- [ ] Second terminal open for: watch -n 60 ./scripts/pnl_report.sh

---

## During the Run

Check every 30 minutes:
  ./scripts/pnl_report.sh

Watch for warning signs:
  # High revert rate — you're being raced
  grep "revert_reason" logs/mainnet_*.log | tail -20

  # L1 gas spikes eating profit
  grep "l1_gas_cost" logs/mainnet_*.log | tail -20

  # Kill switch approaching
  grep "budget_warn" logs/mainnet_*.log | tail -5

If success rate drops below 20% for 30+ minutes:
  1. Stop the bot (Ctrl+C)
  2. Check most common revert reason
  3. If "No profit": simulation-to-submission latency too high
  4. If "STF" (transfer failed): reserve staleness issue
  5. Do not resume until you understand why

---

## Emergency Stop

Ctrl+C triggers graceful shutdown.
PnL state persists to arbx_pnl_state.json automatically.
Bot can be restarted — it resumes from saved PnL state.

If bot won't stop:
  pkill -f "cargo run"
  pkill -f "arbx"

---

## Definition of Done

Phase 10 is complete when:
- At least one successful profitable arb executed on mainnet
- Positive net PnL even if small (e.g. +$0.01)
- Bot ran for at least 30 minutes without panic or crash
- pnl_report.sh shows successful_arbs >= 1
