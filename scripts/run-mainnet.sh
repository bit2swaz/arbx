#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"
source .env

required_vars="ARBITRUM_RPC_URL PRIVATE_KEY ARB_EXECUTOR_ADDRESS"
for var in $required_vars; do
  [ -z "${!var:-}" ] && echo "ERROR: $var not set in .env" && exit 1
done

# Sanity check: contract must exist on-chain
echo "Verifying contract is deployed at $ARB_EXECUTOR_ADDRESS..."
CODE=$(cast code "$ARB_EXECUTOR_ADDRESS" --rpc-url "$ARBITRUM_RPC_URL" 2>/dev/null)
if [ "$CODE" = "0x" ] || [ -z "$CODE" ]; then
  echo "ERROR: No contract found at $ARB_EXECUTOR_ADDRESS on Arbitrum mainnet."
  echo "Run scripts/deploy-mainnet.sh first."
  exit 1
fi
echo "Contract verified."

# Show current ETH balance
BALANCE=$(cast balance "$ARB_EXECUTOR_ADDRESS" \
  --rpc-url "$ARBITRUM_RPC_URL" --ether 2>/dev/null || echo "unknown")
echo "Executor ETH balance: $BALANCE ETH"

echo ""
echo "============================================"
echo "  WARNING: arbx MAINNET MODE"
echo "  REAL FUNDS AT RISK."
echo "  Total budget:    \$31.00 USD (~3000 INR)"
echo "  Deploy cost:     ~\$4.00 USD (already spent)"
echo "  Execution budget: ~\$27.00 USD"
echo "  Kill switch:     bot halts at \$2.00 remaining"
echo "  Target pairs:    ARB/USDT, WBTC/ETH (NOT USDC/ETH)"
echo "============================================"
echo ""
echo "Type 'run mainnet' to confirm (anything else aborts):"
read -r confirm
[ "$confirm" != "run mainnet" ] && echo "Aborted." && exit 1

mkdir -p logs
LOG_FILE="logs/mainnet_$(date +%Y%m%d_%H%M%S).log"
echo "Starting arbx. Log: $LOG_FILE"
echo "Monitor PnL: watch -n 60 ./scripts/pnl_report.sh"
echo "Metrics:     curl -s localhost:9090/metrics | grep arbx"
echo ""

cargo run --release -- --config config/mainnet.toml 2>&1 | tee "$LOG_FILE"
