#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/run_anvil_fork.sh — Anvil mainnet fork end-to-end validation
#
# Starts an Anvil fork of Arbitrum mainnet, funds the executor address, then
# runs arbx against it for 10 minutes using config/anvil_fork.toml.
#
# The sequencer feed is still the LIVE feed (wss://arb1.arbitrum.io/feed).
# Real mainnet swaps are detected and simulated against the fork block state.
# No real transaction is ever broadcast (dry_run = true in the config).
#
# Prerequisites:
#   - foundry installed: anvil, cast  (https://getfoundry.sh)
#   - ARBITRUM_RPC_URL set in .env
#   - ARB_EXECUTOR_ADDRESS set in .env
#   - PRIVATE_KEY set in .env
#   - cargo build --release already done
#
# Usage:
#   chmod +x scripts/run_anvil_fork.sh
#   ./scripts/run_anvil_fork.sh
#
# Optional environment overrides:
#   FORK_BLOCK_NUMBER=105949098  (default, same block as fork integration tests)
#   RUN_DURATION=600             (seconds to run arbx, default 600 = 10 min)
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Colours ───────────────────────────────────────────────────────────────────
green() { printf '\033[32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[33m%s\033[0m\n' "$*"; }
red()   { printf '\033[31m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n'  "$*"; }

# ── Load environment ──────────────────────────────────────────────────────────
if [[ -f .env ]]; then
    # shellcheck disable=SC1091
    set -a && source .env && set +a
    dim "Loaded .env"
else
    red "ERROR: .env not found.  Create it with ARBITRUM_RPC_URL, ARB_EXECUTOR_ADDRESS, and PRIVATE_KEY."
    exit 1
fi

# ── Validate required vars ────────────────────────────────────────────────────
for var in ARBITRUM_RPC_URL ARB_EXECUTOR_ADDRESS PRIVATE_KEY; do
    if [[ -z "${!var:-}" ]]; then
        red "ERROR: $var is not set in .env"
        exit 1
    fi
done

# ── Configuration ─────────────────────────────────────────────────────────────
FORK_BLOCK="${FORK_BLOCK_NUMBER:-105949098}"
RUN_DURATION="${RUN_DURATION:-600}"
ANVIL_PORT=8545
ANVIL_RPC="http://127.0.0.1:${ANVIL_PORT}"
# Anvil account 0 — default funded test key (safe for local use only)
ANVIL_FUNDER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# Anvil's --fork-url requires HTTP(S), not WebSocket (wss://).
# The .env ARBITRUM_RPC_URL is often a wss:// URL used for the sequencer feed.
# Convert it to https:// so Anvil can fetch trie nodes correctly.
FORK_RPC_URL="${ARBITRUM_RPC_URL}"
FORK_RPC_URL="${FORK_RPC_URL/wss:\/\//https://}"
FORK_RPC_URL="${FORK_RPC_URL/ws:\/\//http://}"

mkdir -p logs
LOG_FILE="logs/anvil_fork_$(date +%Y%m%d_%H%M%S).log"

echo ""
yellow "=== arbx — Anvil Mainnet Fork Validation ==="
echo ""
dim "Fork block : $FORK_BLOCK"
dim "RPC source : $FORK_RPC_URL"
dim "Executor   : $ARB_EXECUTOR_ADDRESS"
dim "Run time   : ${RUN_DURATION}s"
dim "Log file   : $LOG_FILE"
echo ""

# ── Check dependencies ────────────────────────────────────────────────────────
for cmd in anvil cast cargo; do
    if ! command -v "$cmd" &>/dev/null; then
        red "ERROR: '$cmd' not found.  Install Foundry: https://getfoundry.sh"
        exit 1
    fi
done

# ── Start Anvil fork ──────────────────────────────────────────────────────────
yellow "Starting Anvil fork at block $FORK_BLOCK..."
anvil \
    --fork-url      "$FORK_RPC_URL" \
    --fork-block-number "$FORK_BLOCK" \
    --host          127.0.0.1 \
    --port          "$ANVIL_PORT" \
    --accounts      10 \
    --balance       10000 \
    --gas-limit     30000000 \
    --no-rate-limit \
    --silent \
    &
ANVIL_PID=$!

# ── Wait for Anvil to be ready ────────────────────────────────────────────────
yellow "Waiting for Anvil to be ready (up to 30 s)..."
READY=false
for i in $(seq 1 30); do
    if cast block-number --rpc-url "$ANVIL_RPC" &>/dev/null 2>&1; then
        READY=true
        break
    fi
    sleep 1
done

if [[ "$READY" != "true" ]]; then
    red "ERROR: Anvil did not become ready within 30 seconds."
    kill "$ANVIL_PID" 2>/dev/null || true
    exit 1
fi

FORK_HEAD=$(cast block-number --rpc-url "$ANVIL_RPC" 2>/dev/null || echo "unknown")
green "Anvil ready — head block: $FORK_HEAD"

# ── Fund the executor address ─────────────────────────────────────────────────
yellow "Funding executor ($ARB_EXECUTOR_ADDRESS) with 1 ETH for gas..."
cast send \
    --rpc-url    "$ANVIL_RPC" \
    --private-key "$ANVIL_FUNDER_KEY" \
    "$ARB_EXECUTOR_ADDRESS" \
    --value 1ether \
    --quiet
green "Executor funded."

# ── Find arbx binary ──────────────────────────────────────────────────────────
ARBX_BIN="${ARBX_BIN:-./target/release/arbx}"
if [[ ! -x "$ARBX_BIN" ]]; then
    ARBX_BIN="./target/debug/arbx"
fi
if [[ ! -x "$ARBX_BIN" ]]; then
    yellow "arbx binary not found — building release binary..."
    cargo build --release --bin arbx
    ARBX_BIN="./target/release/arbx"
fi
dim "Binary: $ARBX_BIN"

# ── Start arbx against the Anvil fork ────────────────────────────────────────
yellow "Starting arbx against Anvil fork (${RUN_DURATION}s run)..."
echo "  Config : config/anvil_fork.toml"
echo "  Feed   : wss://arb1.arbitrum.io/feed (live mainnet)"
echo "  State  : $ANVIL_RPC (fork at block $FORK_BLOCK)"
echo ""
"$ARBX_BIN" --config config/anvil_fork.toml 2>&1 | tee "$LOG_FILE" &
ARBX_PID=$!

# ── Run for the configured duration ──────────────────────────────────────────
echo ""
dim "arbx running (PID $ARBX_PID). Press Ctrl-C to stop early."
dim "Metrics: http://localhost:9090/metrics"
echo ""
sleep "$RUN_DURATION" || true  # ignore interrupts — cleanup runs regardless

# ── Shutdown ──────────────────────────────────────────────────────────────────
echo ""
yellow "Shutting down..."
kill "$ARBX_PID"  2>/dev/null || true
sleep 2
kill "$ANVIL_PID" 2>/dev/null || true

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
green "Run complete.  Log saved to $LOG_FILE"
echo ""
echo "Quick log summary:"
echo "  Pool seed    : $(grep -c 'seed\|seeded\|bootstrapped' "$LOG_FILE" 2>/dev/null || echo 0) lines"
echo "  Swap detect  : $(grep -c 'detected\|DetectedSwap\|swap.*sel\|SWAP_SEL' "$LOG_FILE" 2>/dev/null || echo 0) lines"
echo "  Opp found    : $(grep -c 'opportunity\|two.hop\|path.*found' "$LOG_FILE" 2>/dev/null || echo 0) lines"
echo "  Simulation   : $(grep -c 'simulation\|simulate' "$LOG_FILE" 2>/dev/null || echo 0) lines"
echo "  Dry run hits : $(grep -c 'DRY RUN' "$LOG_FILE" 2>/dev/null || echo 0) lines"
echo ""
dim "Run ./scripts/anvil_smoke_test.sh while arbx is still running to check live metrics."
dim "(Or restart arbx against the same fork for a second look.)"
