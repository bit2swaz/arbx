#!/bin/bash
# scripts/flamegraph.sh — generate a CPU flamegraph for the arbx binary.
#
# Usage:
#   ./scripts/flamegraph.sh [--config <path>]
#
# Prerequisites:
#   sudo apt install linux-perf  # or equivalent for your distro
#   echo -1 | sudo tee /proc/sys/kernel/perf_event_paranoid
#
# The flamegraph is saved to flamegraph.svg and opened automatically.
set -euo pipefail

# Install flamegraph cargo plugin if not already present.
if ! cargo flamegraph --version &>/dev/null 2>&1; then
    echo "Installing cargo-flamegraph..."
    cargo install flamegraph --quiet
fi

CONFIG="${1:---config config/default.toml}"

echo "Building arbx with profiling profile..."
cargo build --profile profiling --bin arbx

echo "Running flamegraph for 30 seconds — ensure real traffic is flowing."
echo "Press Ctrl-C to stop early."
sudo cargo flamegraph --profile profiling \
    --bin arbx -- ${CONFIG} &
FLAMEGRAPH_PID=$!

sleep 30
kill "$FLAMEGRAPH_PID" 2>/dev/null || true
wait "$FLAMEGRAPH_PID" 2>/dev/null || true

if [ -f flamegraph.svg ]; then
    echo "Flamegraph saved to flamegraph.svg"
    xdg-open flamegraph.svg 2>/dev/null \
        || open flamegraph.svg 2>/dev/null \
        || echo "Open flamegraph.svg in your browser."
else
    echo "flamegraph.svg not found — check that perf_event_paranoid is set to -1."
fi
