#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/deploy-sepolia.sh — Deploy ArbExecutor to Arbitrum Sepolia
#
# Usage:
#   ./scripts/deploy-sepolia.sh
#
# Prerequisites:
#   - .env exists at the repo root (copy from .env.example and fill in values)
#   - PRIVATE_KEY, ARBITRUM_SEPOLIA_RPC_URL, BALANCER_VAULT, ARBISCAN_API_KEY set
#
# Output:
#   - Deployment broadcast logs in contracts/broadcast/
#   - Deployment record at contracts/deployments/421614.json
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
: "${ARBITRUM_SEPOLIA_RPC_URL:?ARBITRUM_SEPOLIA_RPC_URL is not set in .env}"
: "${BALANCER_VAULT:?BALANCER_VAULT is not set in .env}"
: "${ARBISCAN_API_KEY:?ARBISCAN_API_KEY is not set in .env}"

# ── Navigate to contracts/ so foundry.toml is in scope ───────────────────────
cd "$REPO_ROOT/contracts"

echo "=== Deploying ArbExecutor to Arbitrum Sepolia ==="
echo "RPC      : $ARBITRUM_SEPOLIA_RPC_URL"
echo "Deployer : $(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null || echo "(install cast to preview)")"
echo ""

forge script script/Deploy.s.sol:Deploy \
    --rpc-url       "$ARBITRUM_SEPOLIA_RPC_URL" \
    --broadcast     \
    --verify        \
    --etherscan-api-key "$ARBISCAN_API_KEY" \
    -vvvv

echo ""
echo "=== Deployment complete. Record written to deployments/421614.json ==="
