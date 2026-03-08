#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/run_sepolia.sh — Run arbx against Arbitrum Sepolia testnet
#
# Usage:
#   ./scripts/run_sepolia.sh [--dry-run]
#
# Prerequisites:
#   - .env exists at the repo root (copy from .env.example and fill in values)
#   - ARBITRUM_SEPOLIA_RPC_URL, ARB_EXECUTOR_ADDRESS, PRIVATE_KEY set in .env
#   - Contract deployed via: ./scripts/deploy-sepolia.sh
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DRY_RUN_FLAG=""

# ── Parse args ────────────────────────────────────────────────────────────────
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN_FLAG="--dry-run" ;;
        *) echo "Unknown argument: $arg" && exit 1 ;;
    esac
done

# ── Load environment variables from repo root .env ────────────────────────────
if [[ -f "$REPO_ROOT/.env" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$REPO_ROOT/.env"
    set +a
else
    echo "WARNING: .env not found — relying on environment variables already set."
fi

# ── Validate required vars ────────────────────────────────────────────────────
required_vars="ARBITRUM_SEPOLIA_RPC_URL ARB_EXECUTOR_ADDRESS PRIVATE_KEY"
for var in $required_vars; do
    if [[ -z "${!var:-}" ]]; then
        echo "ERROR: $var is not set."
        echo "Copy .env.example to .env and fill in your Sepolia values."
        exit 1
    fi
done

# ── Prepare log directory ─────────────────────────────────────────────────────
mkdir -p "$REPO_ROOT/logs"
LOG_FILE="$REPO_ROOT/logs/sepolia_$(date +%Y%m%d_%H%M%S).log"

# ── Summary ───────────────────────────────────────────────────────────────────
echo "======================================================"
echo "  arbx — Arbitrum Sepolia Testnet"
echo "======================================================"
echo "  Config      : config/sepolia.toml"
echo "  Chain ID    : 421614 (Arbitrum Sepolia)"
echo "  RPC         : ${ARBITRUM_SEPOLIA_RPC_URL}"
echo "  Contract    : ${ARB_EXECUTOR_ADDRESS}"
echo "  Log file    : ${LOG_FILE}"
[[ -n "$DRY_RUN_FLAG" ]] && echo "  Mode        : DRY RUN (no on-chain submissions)"
echo "======================================================"
echo ""
echo "Metrics available at: http://localhost:9090/metrics"
echo "Run smoke test in another terminal: ./scripts/smoke_test.sh"
echo ""

# ── Run ───────────────────────────────────────────────────────────────────────
cd "$REPO_ROOT"
cargo run --release -- \
    --config config/sepolia.toml \
    $DRY_RUN_FLAG \
    2>&1 | tee "$LOG_FILE"
