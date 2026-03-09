#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/smoke_test.sh — Validate arbx observability funnel on testnet
#
# Usage:
#   ./scripts/smoke_test.sh [--url http://host:port]
#   ./scripts/smoke_test.sh --self-test [--config path/to/config.toml]
#
# Default mode: connects to a running bot and checks /metrics are non-zero.
# Self-test mode (--self-test): runs `arbx --self-test` directly to validate
#   the detection pipeline with synthetic data — no live sequencer feed needed.
#   Use this on testnets where the sequencer feed has no DEX swap activity.
#
# Note: simulation/submission/on-chain metrics may stay at 0 on testnet —
#       that is expected since there is no real liquidity to arb.
#       The two metrics that MUST be non-zero are:
#         - opportunities_detected       (feed is alive, swaps are parsed)
#         - opportunities_cleared_threshold  (profit math is running)
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Parse arguments ───────────────────────────────────────────────────────────

SELF_TEST=false
METRICS_URL="http://localhost:9090"
CONFIG="config/sepolia.toml"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --self-test)
            SELF_TEST=true
            shift
            ;;
        --config)
            CONFIG="$2"
            shift 2
            ;;
        --url)
            METRICS_URL="$2"
            shift 2
            ;;
        *)
            METRICS_URL="$1"
            shift
            ;;
    esac
done

METRICS_ENDPOINT="${METRICS_URL}/metrics"

# ── Helpers ───────────────────────────────────────────────────────────────────

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n'  "$*"; }

# ── Self-test mode ────────────────────────────────────────────────────────────

if [[ "$SELF_TEST" == "true" ]]; then
    echo ""
    echo "=== arbx Self-Test (synthetic pipeline validation) ==="
    echo ""

    # Source .env if present to supply required env vars
    if [[ -f .env ]]; then
        # shellcheck disable=SC1091
        set -a && source .env && set +a
    fi

    ARBX_BIN="${ARBX_BIN:-./target/release/arbx}"
    if [[ ! -x "$ARBX_BIN" ]]; then
        ARBX_BIN="./target/debug/arbx"
    fi
    if [[ ! -x "$ARBX_BIN" ]]; then
        red "ERROR: arbx binary not found. Run: cargo build --release --bin arbx"
        exit 1
    fi

    echo "Binary : $ARBX_BIN"
    echo "Config : $CONFIG"
    echo ""

    if "$ARBX_BIN" --config "$CONFIG" --self-test 2>&1; then
        echo ""
        green "SMOKE TEST PASSED (self-test mode) — detection pipeline is healthy"
        echo ""
        exit 0
    else
        echo ""
        red "SMOKE TEST FAILED (self-test mode) — see output above"
        echo ""
        exit 1
    fi
fi

# ── Live metrics mode ─────────────────────────────────────────────────────────

PASS=0
FAIL=0

# Fetch metric value by exact name (first non-comment line starting with $name).
get_metric() {
    local name="$1"
    curl -sf "$METRICS_ENDPOINT" \
        | grep -E "^${name}(\{| |$)" \
        | grep -v '^#' \
        | awk '{print $NF}' \
        | head -1
}

# Assert metric is present and > 0.
check_nonzero() {
    local name="$1"
    local label="${2:-$1}"
    local value
    value="$(get_metric "$name")"

    if [[ -z "$value" ]]; then
        red "  FAIL  $label  →  metric missing from /metrics output"
        FAIL=$((FAIL + 1))
    elif [[ "$value" == "0" ]] || [[ "$value" == "0.0" ]]; then
        red "  FAIL  $label  →  value is 0 (expected > 0 after 5 min)"
        FAIL=$((FAIL + 1))
    else
        green "  PASS  $label  →  $value"
        PASS=$((PASS + 1))
    fi
}

# Just print the value without asserting — informational only.
check_info() {
    local name="$1"
    local label="${2:-$1}"
    local value
    value="$(get_metric "$name")"
    dim "  INFO  $label  →  ${value:-<not set>}"
}

# ── Connectivity check ────────────────────────────────────────────────────────

echo ""
echo "=== arbx Smoke Test ==="
echo "Endpoint: $METRICS_ENDPOINT"
echo ""

if ! curl -sf "$METRICS_ENDPOINT" > /dev/null 2>&1; then
    red "ERROR: Cannot reach $METRICS_ENDPOINT"
    red "Is the bot running? (./scripts/run_sepolia.sh)"
    red "Tip: on testnets with no swap activity use: ./scripts/smoke_test.sh --self-test"
    exit 1
fi

# ── Required: top-of-funnel must be non-zero ──────────────────────────────────

echo "--- Required (must be > 0 after 5 min) ---"
check_nonzero "opportunities_detected"          "opportunities_detected"
check_nonzero "opportunities_cleared_threshold" "opportunities_cleared_threshold"

# ── Informational: lower-funnel may be 0 on testnet ──────────────────────────

echo ""
echo "--- Informational (may be 0 on testnet — no real liquidity) ---"
check_info "opportunities_cleared_simulation" "opportunities_cleared_simulation"
check_info "transactions_submitted"           "transactions_submitted"
check_info "transactions_succeeded"           "transactions_succeeded"
check_info "net_pnl_wei"                      "net_pnl_wei"
check_info "gas_spent_wei"                    "gas_spent_wei"

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [[ $FAIL -gt 0 ]]; then
    echo ""
    red "SMOKE TEST FAILED"
    red "If opportunities_detected is 0:"
    red "  1. Check the bot has been running for at least 5 minutes"
    red "  2. Check ARBITRUM_SEPOLIA_RPC_URL is valid"
    red "  3. Check logs/sepolia_*.log for errors"
    red "  4. On testnets with no swap activity: ./scripts/smoke_test.sh --self-test"
    echo ""
    exit 1
fi

echo ""
green "SMOKE TEST PASSED — funnel is live on Arbitrum Sepolia"
echo ""
