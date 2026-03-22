#!/bin/bash
PNL_FILE="${PNL_FILE:-arbx_pnl_state.json}"
BUDGET_USD=27.00
KILL_AT_USD=2.00

if [ ! -f "$PNL_FILE" ]; then
  echo "No PnL file found at $PNL_FILE — bot may not have run yet."
  exit 1
fi

echo "=== arbx PnL Report — $(date) ==="
python3 - <<EOF
import json

with open("$PNL_FILE") as f:
    d = json.load(f)

budget       = $BUDGET_USD
kill_at      = $KILL_AT_USD
net          = d.get("net_pnl_usd", 0.0)
spent        = d.get("total_gas_spent_usd", 0.0)
remaining    = budget - spent
successful   = d.get("successful_arbs", 0)
reverted     = d.get("reverted_arbs", 0)
total        = successful + reverted
success_rate = (successful / total * 100) if total > 0 else 0.0

print(f"  Net PnL:          ${net:+.4f} USD")
print(f"  Gas spent:        ${spent:.4f} USD")
print(f"  Budget remaining: ${remaining:.4f} USD  (kill at ${kill_at})")
print(f"  Successful arbs:  {successful}")
print(f"  Reverted arbs:    {reverted}")
print(f"  Success rate:     {success_rate:.1f}%")

if remaining <= kill_at:
    print(f"\n  ⚠️  WARNING: budget near kill switch threshold")
if reverted > 0 and success_rate < 20:
    print(f"\n  ⚠️  HIGH REVERT RATE: check logs for revert reasons")
    print(f"     grep 'revert_reason' logs/mainnet_*.log | tail -20")
if successful > 0:
    avg = net / successful
    print(f"\n  Avg profit/arb:   ${avg:.4f} USD")
EOF
