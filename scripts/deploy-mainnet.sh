#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/deploy-mainnet.sh — Deploy ArbExecutor to Arbitrum Mainnet
#
# WARNING: This script broadcasts a REAL transaction using REAL funds.
#          Double-check your PRIVATE_KEY and wallet balance before running.
#
# Usage:
#   ./scripts/deploy-mainnet.sh
#
# Prerequisites:
#   - .env exists at the repo root with all required values
#   - PRIVATE_KEY, ARBITRUM_RPC_URL, BALANCER_VAULT, ARBISCAN_API_KEY set
#   - Wallet holds enough ETH for deployment gas (~$2-5 on Arbitrum)
#
# Output:
#   - Deployment broadcast logs in contracts/broadcast/
#   - Deployment record at contracts/deployments/42161.json
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Load environment variables from repo root .env ────────────────────────────
if [[ ! -f "$REPO_ROOT/.env" ]]; then
    echo "ERROR: .env not found at $REPO_ROOT/.env"
    echo "Copy .env.example to .env and fill in your values."
    exit 1
fi
set -a
# shellcheck source=/dev/null
source "$REPO_ROOT/.env"
set +a

# ── Validate required vars ────────────────────────────────────────────────────
: "${PRIVATE_KEY:?PRIVATE_KEY is not set in .env}"
: "${ARBITRUM_RPC_URL:?ARBITRUM_RPC_URL is not set in .env}"
: "${BALANCER_VAULT:?BALANCER_VAULT is not set in .env}"
: "${ARBISCAN_API_KEY:?ARBISCAN_API_KEY is not set in .env}"

# ── Mandatory confirmation — prevents accidental mainnet deployments ──────────
echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║  WARNING: Deploying to Arbitrum MAINNET with REAL funds.        ║"
echo "║  This will consume ETH from your wallet for deployment gas.     ║"
echo "║                                                                  ║"
echo "║  Deployer : $(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null | head -c 42 || printf '%-42s' '(install cast to preview)')  ║"
echo "║  RPC      : $ARBITRUM_RPC_URL" | head -c 68
echo "╚══════════════════════════════════════════════════════════════════╝"
echo ""
echo -n "Type 'yes' to continue, anything else to abort: "
read -r confirm

if [[ "$confirm" != "yes" ]]; then
    echo "Deployment aborted."
    exit 1
fi

# ── Navigate to contracts/ so foundry.toml is in scope ───────────────────────
cd "$REPO_ROOT/contracts"

echo ""
echo "=== Deploying ArbExecutor to Arbitrum Mainnet ==="

forge script script/Deploy.s.sol:Deploy \
    --rpc-url           "$ARBITRUM_RPC_URL" \
    --broadcast         \
    --verify            \
    --etherscan-api-key "$ARBISCAN_API_KEY" \
    --gas-estimate-multiplier 120 \
    -vvvv

echo ""
echo "=== Deployment complete. Record written to deployments/42161.json ==="
