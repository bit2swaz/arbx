# arbx — Testnet Validation Guide

Phase 9.1: Run the complete bot on Arbitrum Sepolia. Validate the full observability
funnel before any mainnet deployment.

---

## Prerequisites

| Item | How to get it |
|---|---|
| Arbitrum Sepolia RPC URL | Alchemy: [dashboard.alchemy.com](https://dashboard.alchemy.com) → New App → Arbitrum Sepolia |
| Arbitrum Sepolia ETH | Faucet: [faucet.triangleplatform.com/arbitrum/sepolia](https://faucet.triangleplatform.com/arbitrum/sepolia) |
| Arbiscan API key | [arbiscan.io/myapikey](https://arbiscan.io/myapikey) (free, needed for contract verification) |
| Rust toolchain | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Foundry | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |

---

## Step 1 — Configure `.env`

```bash
cp .env.example .env
```

Fill in your values:

```
ARBITRUM_SEPOLIA_RPC_URL=https://arb-sepolia.g.alchemy.com/v2/YOUR_KEY
ARB_EXECUTOR_ADDRESS=0x0000000000000000000000000000000000000000   # filled after step 2
PRIVATE_KEY=0xYOUR_THROWAWAY_TESTNET_PRIVATE_KEY
ARBISCAN_API_KEY=YOUR_ARBISCAN_KEY
BALANCER_VAULT=0xBA12222222228d8Ba445958a75a0704d566BF2C8
```

> **Use a throwaway key.** This key will be hot (in memory) and used to sign
> testnet transactions. Never reuse it for mainnet funds.

---

## Step 2 — Deploy ArbExecutor to Arbitrum Sepolia

```bash
./scripts/deploy-sepolia.sh
```

The script will:
1. Validate all env vars
2. Run `forge script` against Arbitrum Sepolia
3. Verify the contract on Arbiscan
4. Write the deployed address to `contracts/deployments/421614.json`

After deployment, copy the address into `.env`:

```
ARB_EXECUTOR_ADDRESS=0xYOUR_DEPLOYED_CONTRACT_ADDRESS
```

---

## Step 3 — Fund the Deployer Wallet

Get Sepolia ETH to pay gas for test transactions:

```
https://faucet.triangleplatform.com/arbitrum/sepolia
```

You need ~0.01 ETH for the deployment and a few test submissions.

---

## Step 4 — Run the Bot

```bash
./scripts/run_sepolia.sh
```

Or in dry-run mode (no on-chain submissions, safest for initial validation):

```bash
./scripts/run_sepolia.sh --dry-run
```

The bot will:
- Connect to `wss://sepolia-rollup.arbitrum.io/feed`
- Start the pool state store (populated live from feed events)
- Run the detection loop (two-hop path scanning)
- Serve Prometheus metrics at `http://localhost:9090/metrics`
- Log everything at `debug` level to `logs/sepolia_TIMESTAMP.log`

---

## Step 5 — Validate the Funnel

In a second terminal, after the bot has been running for **5 minutes**:

```bash
./scripts/smoke_test.sh
```

For continuous monitoring:

```bash
watch -n 30 ./scripts/smoke_test.sh
```

Expected output after 5 minutes:

```
=== arbx Smoke Test ===
Endpoint: http://localhost:9090/metrics

--- Required (must be > 0 after 5 min) ---
  PASS  opportunities_detected          →  1247
  PASS  opportunities_cleared_threshold →  3

--- Informational (may be 0 on testnet — no real liquidity) ---
  INFO  opportunities_cleared_simulation  →  0
  INFO  transactions_submitted            →  0
  INFO  transactions_succeeded            →  0
  INFO  net_pnl_wei                       →  0
  INFO  gas_spent_wei                     →  0

=== Results: 2 passed, 0 failed ===

SMOKE TEST PASSED — funnel is live on Arbitrum Sepolia
```

> **Why simulation/submission metrics stay at 0 on testnet:** Arbitrum Sepolia
> has very little real DEX liquidity. The sequencer feed is live and we detect
> swaps, but genuine two-hop price dislocations above the profit threshold are
> rare to non-existent. This is expected — the goal of Phase 9 is funnel
> validation, not profit generation.

---

## Definition of Done

Phase 9.1 is complete when **all of the following are true**:

- [ ] `./scripts/smoke_test.sh` shows `PASS` for `opportunities_detected` within 5 minutes of startup
- [ ] `./scripts/smoke_test.sh` shows `PASS` for `opportunities_cleared_threshold`
- [ ] Bot runs for **10 minutes** with zero panics or unhandled errors in the log
- [ ] Log file contains no `ERROR` lines (warnings are OK)
- [ ] `./scripts/run_sepolia.sh` exits 0 on clean shutdown (Ctrl+C)

Run for 10 minutes total:

```bash
# In terminal 1 — run the bot
./scripts/run_sepolia.sh

# In terminal 2 — watch the funnel
watch -n 30 ./scripts/smoke_test.sh

# After 10 minutes, Ctrl+C terminal 1
# Verify exit code
echo "Exit code: $?"
```

---

## Funnel Diagnosis

| Symptom | Likely cause | Fix |
|---|---|---|
| `opportunities_detected = 0` after 5 min | Feed not connecting | Check `ARBITRUM_SEPOLIA_RPC_URL`; look for `ERROR` in log |
| `opportunities_detected > 0` but `cleared_threshold = 0` | Profit floor too high for testnet | Already set to `0.001` in `sepolia.toml` — check gas estimation |
| Panic in log | Bug in pipeline | Paste panic to issue tracker with full log |
| `ERROR: RPC rate limit` | Hit free-tier limit | Rotate to QuickNode free tier |

---

## Useful Commands

```bash
# Tail live logs
tail -f logs/sepolia_*.log | grep -v DEBUG

# Check metrics manually
curl -s http://localhost:9090/metrics | grep -v '^#'

# Run ignored testnet integration test (requires live RPC + deployed contract)
cargo test testnet_full_pipeline_smoke_test -- --ignored --nocapture

# Check bot process
pgrep -la arbx
```

---

## Next Step: Phase 10 — Mainnet Launch

Once Phase 9 definition of done is satisfied:

1. Review `scripts/deploy-mainnet.sh` — it requires typing `deploy mainnet` to confirm
2. Review `scripts/run-mainnet.sh` — it requires typing `run mainnet` to confirm
3. Start with dry-run: `./scripts/run-mainnet.sh --dry-run` for one hour
4. Monitor funnel; only move to live mode when dry-run shows healthy metrics
5. Set `ARBX_BUDGET_USD=60` — kill switch triggers automatically at budget exhaustion

See [SSOT.md](SSOT.md) Phase 10 for the full mainnet launch checklist.
