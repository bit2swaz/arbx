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
#   FORK_BLOCK_NUMBER=285000000  override fork block (default: latest - 20)
#   SEED_BLOCKS=20000            blocks to scan for pool events (default: 20000)
#   RUN_DURATION=600             seconds to run arbx (default: 600 = 10 min)
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
RUN_DURATION="${RUN_DURATION:-600}"
ANVIL_PORT=8545
ANVIL_RPC="http://127.0.0.1:${ANVIL_PORT}"

# Anvil's --fork-url requires HTTP(S), not WebSocket (wss://).
# The .env ARBITRUM_RPC_URL is often a wss:// URL used for the sequencer feed.
# Convert it to https:// so Anvil can fetch trie nodes correctly.
FORK_RPC_URL="${ARBITRUM_RPC_URL}"
FORK_RPC_URL="${FORK_RPC_URL/wss:\/\//https://}"
FORK_RPC_URL="${FORK_RPC_URL/ws:\/\//http://}"

# ── Determine fork block ──────────────────────────────────────────────────────
# Alchemy free tier only has trie state for recent blocks (not old archive).
# Default: fork at latest - 20 so the state is definitely available.
# The pool seeder is told to scan the last SEED_BLOCKS blocks only — scanning
# billions of historical blocks would blow through compute unit budgets.
SEED_BLOCKS="${SEED_BLOCKS:-20000}"

if [[ -n "${FORK_BLOCK_NUMBER:-}" ]]; then
    FORK_BLOCK="$FORK_BLOCK_NUMBER"
else
    yellow "Fetching current Arbitrum mainnet head to choose fork block..."
    LATEST=$(cast block-number --rpc-url "$FORK_RPC_URL" 2>/dev/null || echo "")
    if [[ -z "$LATEST" || "$LATEST" -le 0 ]]; then
        red "ERROR: Could not fetch latest block from $FORK_RPC_URL"
        exit 1
    fi
    # 20 blocks behind latest for stability (avoids reorg/indexing races)
    FORK_BLOCK=$(( LATEST - 20 ))
fi

mkdir -p logs
LOG_FILE="logs/anvil_fork_$(date +%Y%m%d_%H%M%S).log"

echo ""
yellow "=== arbx — Anvil Mainnet Fork Validation ==="
echo ""
dim "Fork block : $FORK_BLOCK  (seed window: last $SEED_BLOCKS blocks)"
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

# ── Kill any stale Anvil process ──────────────────────────────────────────────
# If a previous run left Anvil alive on port $ANVIL_PORT, the readiness check
# would succeed against that old instance.  The seed_from_block we compute is
# derived from the NEW fork block — if the stale Anvil is at a different block,
# pool_seeder sees seed_from_block > head and skips all scanning (seeded=0).
if pkill -x anvil 2>/dev/null; then
    dim "Killed stale Anvil on port $ANVIL_PORT — waiting 2 s for port to free..."
    sleep 2
fi

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

FORK_HEAD=$(cast block-number --rpc-url "$ANVIL_RPC" 2>/dev/null || echo "0")
green "Anvil ready — head block: $FORK_HEAD"

# ── Build temp config — seed_from_block derived from ACTUAL Anvil head ───────────
# seed_from_block must be <= head or pool_seeder exits immediately with seeded=0.
# We compute it HERE (after Anvil is up) so the value is always grounded in the
# real Anvil block, not in a pre-start estimate that can be stale or mismatched.
SEED_FROM=$(( FORK_HEAD > SEED_BLOCKS ? FORK_HEAD - SEED_BLOCKS : 0 ))
TEMP_CONFIG=$(mktemp /tmp/arbx_anvil_fork_XXXXXX.toml)
trap 'rm -f "$TEMP_CONFIG"' EXIT
sed "s|^seed_from_block.*|seed_from_block = $SEED_FROM|" config/anvil_fork.toml > "$TEMP_CONFIG"
green "Temp config written — seed_from_block=$SEED_FROM  head=$FORK_HEAD"

# ── Fund the executor address ─────────────────────────────────────────────────
# Use anvil_setBalance (Anvil JSON-RPC) instead of cast send.
# cast send would trigger a trie node fetch from the fork to verify the sender's
# nonce/balance — which fails when Alchemy doesn't have archive state.
# anvil_setBalance writes directly to Anvil's in-memory state: zero trie access.
yellow "Funding executor ($ARB_EXECUTOR_ADDRESS) with 1 ETH via anvil_setBalance..."
# 0xDE0B6B3A7640000 = 1_000_000_000_000_000_000 wei = 1 ETH
cast rpc --rpc-url "$ANVIL_RPC" \
    anvil_setBalance "$ARB_EXECUTOR_ADDRESS" "0xDE0B6B3A7640000" \
    > /dev/null
green "Executor funded (anvil_setBalance — no trie access needed)."

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
echo "  Config : $TEMP_CONFIG (seed_from_block=$SEED_FROM)"
echo "  Feed   : wss://arb1.arbitrum.io/feed (live mainnet)"
echo "  State  : $ANVIL_RPC (fork at block $FORK_BLOCK)"
echo ""
"$ARBX_BIN" --config "$TEMP_CONFIG" 2>&1 | tee "$LOG_FILE" &
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
