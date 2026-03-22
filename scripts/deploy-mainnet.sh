#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"
source .env

required_vars="ARBITRUM_RPC_URL PRIVATE_KEY ARBISCAN_API_KEY"
for var in $required_vars; do
    [ -z "${!var:-}" ] && echo "ERROR: $var not set in .env" && exit 1
done

echo "============================================"
echo "  WARNING: ARBITRUM MAINNET DEPLOYMENT"
echo "  This will spend real ETH for gas."
echo "  Estimated cost: ~\$3-5 USD"
echo "  RPC: $ARBITRUM_RPC_URL"
echo "============================================"
echo ""
echo "Type 'deploy mainnet' to confirm (anything else aborts):"
read -r confirm
[ "$confirm" != "deploy mainnet" ] && echo "Aborted." && exit 1

echo "Deploying..."
cd contracts
forge script script/Deploy.s.sol:Deploy \
    --rpc-url "$ARBITRUM_RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    --broadcast \
    --verify \
    --etherscan-api-key "$ARBISCAN_API_KEY" \
    --gas-estimate-multiplier 120 \
    -vvvv

echo ""
echo "Deployment complete."
echo "Contract address saved to deployments/42161.json"
echo "Update ARB_EXECUTOR_ADDRESS in .env before running the bot."
