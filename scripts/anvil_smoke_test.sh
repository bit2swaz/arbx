#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# scripts/anvil_smoke_test.sh — Check arbx metrics after an Anvil fork run
#
# Run this in a second terminal while scripts/run_anvil_fork.sh is active
# (or at any time while arbx is running against the Anvil fork).
#
# Usage:
#   chmod +x scripts/anvil_smoke_test.sh
#   ./scripts/anvil_smoke_test.sh [--url http://host:port]
#
# Default metrics endpoint: http://localhost:9090/metrics
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Parse args ────────────────────────────────────────────────────────────────
METRICS_URL="http://localhost:9090"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --url) METRICS_URL="$2"; shift 2 ;;
        *)     METRICS_URL="$1"; shift ;;
    esac
done
METRICS_ENDPOINT="${METRICS_URL}/metrics"

# ── Colours ───────────────────────────────────────────────────────────────────
green()  { printf '\033[32mPASS\033[0m  %s\n' "$*"; }
yellow() { printf '\033[33mSKIP\033[0m  %s\n' "$*"; }
red()    { printf '\033[31mFAIL\033[0m  %s\n' "$*"; }

# ── Fetch metrics once ────────────────────────────────────────────────────────
if ! METRICS_BODY=$(curl -sf "$METRICS_ENDPOINT" 2>/dev/null); then
    echo ""
    echo "ERROR: Could not reach metrics at $METRICS_ENDPOINT"
    echo ""
    echo "  Is arbx running?  Start it with:"
    echo "    ./scripts/run_anvil_fork.sh"
    echo ""
    exit 1
fi

# ── Helper: extract a counter / gauge value from the scraped body ─────────────
# Returns the numeric value, or empty string if the metric is absent / zero.
metric_value() {
    # Matches lines like: arbx_foo_total 42
    # Ignores comment lines (# HELP / # TYPE).
    echo "$METRICS_BODY" | grep -E "^${1}(\{[^}]*\})? " | awk '{print $NF}' | head -1
}

# ── Helper: assert metric >= min ──────────────────────────────────────────────
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

check_metric() {
    local name="$1"
    local min="${2:-1}"
    local label="${3:-$name}"
    local value
    value=$(metric_value "$name")

    if [[ -z "$value" ]]; then
        if [[ "$min" -eq 0 ]]; then
            yellow "$label = 0 (absent — acceptable)"
            (( SKIP_COUNT++ )) || true
        else
            red "$label = MISSING (expected >= $min)"
            (( FAIL_COUNT++ )) || true
        fi
        return
    fi

    # bc -l for float comparison
    if (( $(echo "$value >= $min" | bc -l) )); then
        green "$label = $value"
        (( PASS_COUNT++ )) || true
    else
        red "$label = $value (expected >= $min)"
        (( FAIL_COUNT++ )) || true
    fi
}

# ── Run checks ────────────────────────────────────────────────────────────────
echo ""
echo "=== Anvil Fork Smoke Test ==="
echo "Endpoint: $METRICS_ENDPOINT"
echo ""

echo "--- Mandatory: pipeline must be alive ---"
check_metric "arbx_blocks_processed_total"          1  "blocks_processed"
check_metric "arbx_opportunities_detected_total"    1  "opportunities_detected"

echo ""
echo "--- Desired: profit filter and simulation firing ---"
check_metric "arbx_opportunities_cleared_threshold_total" 0 "cleared_threshold"
check_metric "arbx_simulations_run_total"                 0 "simulations_run"
check_metric "arbx_opportunities_cleared_simulation_total" 0 "cleared_simulation"

echo ""
echo "--- Info: submission metrics (expected 0 in dry_run mode) ---"
check_metric "arbx_transactions_submitted_total"  0  "txns_submitted (dry_run)"
check_metric "arbx_transactions_succeeded_total"  0  "txns_succeeded (dry_run)"

echo ""
echo "=== Results ==="
echo "  PASS : $PASS_COUNT"
echo "  SKIP : $SKIP_COUNT  (metric absent or 0 — acceptable)"
echo "  FAIL : $FAIL_COUNT"
echo ""

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    echo "RESULT: FAIL — $FAIL_COUNT mandatory metric(s) missing or below threshold."
    echo ""
    echo "Triage:"
    echo "  1. Pool store seeded?  grep 'bootstrapped\|seeded' logs/anvil_fork_*.log"
    echo "  2. Swaps detected?     grep 'detected\|DetectedSwap' logs/anvil_fork_*.log"
    echo "  3. Profit threshold?   min_profit_floor_usd in config/anvil_fork.toml (currently 0.01)"
    echo "  4. Feed connected?     grep 'sequencer feed\|feed_url' logs/anvil_fork_*.log"
    echo ""
    exit 1
fi

echo "RESULT: PASS — mandatory metrics are healthy."
echo ""
echo "Definition of done for Anvil validation:"
echo "  opportunities_detected > 0   ✓  (feed → detection pipeline firing)"
echo "  blocks_processed > 0         ✓  (block reconciler ticking)"
if (( $(echo "$(metric_value 'arbx_opportunities_cleared_simulation_total' || echo 0) > 0" | bc -l 2>/dev/null || echo 0) )); then
    echo "  cleared_simulation > 0       ✓  BONUS: full arb logic validated!"
else
    echo "  cleared_simulation = 0       —  no profitable opp found yet (not a failure)"
    echo "  Leave running longer or check profit threshold in config/anvil_fork.toml"
fi
echo ""
exit 0
