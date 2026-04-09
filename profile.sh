#!/bin/bash
set -euo pipefail

usage() {
    echo "Usage: $0 <type> <conns_per_sec>"
    echo ""
    echo "Types:"
    echo "  on   — On-CPU profile of the server (perf)          → oncpu.data / oncpu.svg"
    echo "  off  — Off-CPU / blocking profile of the server (offcputime) → offcpu.data / offcpu.svg"
    echo "  all  — System-wide CPU profile (perf)               → all.data / all.svg"
    exit 1
}

[ $# -ge 2 ] || usage

TYPE=$1
RATE=$2
DURATION=60
BIN="./target/release/quinn-tester"
SERVER_PID=""
CHILD_PID=""

case "$TYPE" in
    on|off|all) ;;
    *) echo "Error: unknown type '$TYPE' (expected: on, off, all)" >&2; usage ;;
esac

cleanup() {
    [ -n "$CHILD_PID" ]  && { kill -INT "$CHILD_PID" 2>/dev/null || true; wait "$CHILD_PID" 2>/dev/null || true; }
    [ -n "$SERVER_PID" ] && { kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; }
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

build() {
    echo "==> Building release binary with debug symbols..."
    cargo build --release 2>&1
}

start_server() {
    "$BIN" server &
    SERVER_PID=$!
    sleep 1
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "Error: server failed to start" >&2
        exit 1
    fi
    echo "    Server PID: $SERVER_PID"
}

stop_server() {
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""
    sleep 0.5
}

run_client() {
    echo "    Load test: $RATE conns/sec for ${DURATION}s..."
    "$BIN" client "$RATE" "$DURATION"
}

stop_child() {
    kill -INT "$CHILD_PID" 2>/dev/null || true
    wait "$CHILD_PID" 2>/dev/null || true
    CHILD_PID=""
}

perf_to_svg() {
    local data=$1 svg=$2
    if command -v inferno-collapse-perf &>/dev/null && command -v inferno-flamegraph &>/dev/null; then
        perf script -i "$data" | inferno-collapse-perf | inferno-flamegraph > "$svg"
    elif command -v stackcollapse-perf.pl &>/dev/null && command -v flamegraph.pl &>/dev/null; then
        perf script -i "$data" | stackcollapse-perf.pl | flamegraph.pl > "$svg"
    else
        echo "    Warning: no flamegraph tools found (install inferno: cargo install inferno)"
        return
    fi
    echo "    SVG: $svg"
}

folded_to_svg() {
    local data=$1 svg=$2
    if command -v inferno-flamegraph &>/dev/null; then
        inferno-flamegraph < "$data" > "$svg"
    elif command -v flamegraph.pl &>/dev/null; then
        flamegraph.pl "$data" > "$svg"
    else
        echo "    Warning: no flamegraph tools found (install inferno: cargo install inferno)"
        return
    fi
    echo "    SVG: $svg"
}

# ---------------------------------------------------------------------------
# Profiling passes
# ---------------------------------------------------------------------------

run_oncpu() {
    echo "==> On-CPU profile..."
    start_server

    perf record -F 99 -p "$SERVER_PID" -g -o oncpu.data &
    CHILD_PID=$!
    sleep 0.5

    run_client
    echo ""
    stop_child
    stop_server

    echo "    Data: oncpu.data"
    perf_to_svg oncpu.data oncpu.svg
}

run_offcpu() {
    echo "==> Off-CPU profile (requires sudo for offcputime)..."
    start_server

    sudo python3 /usr/share/bcc/tools/offcputime -p "$SERVER_PID" -U -f "$DURATION" > offcpu.data &
    CHILD_PID=$!
    sleep 0.5

    run_client
    echo ""
    wait "$CHILD_PID" 2>/dev/null || true
    CHILD_PID=""
    stop_server

    echo "    Data: offcpu.data"
    folded_to_svg offcpu.data offcpu.svg
}

run_all() {
    echo "==> All-processes profile..."
    start_server

    perf record -F 99 -a -g -o all.data &
    CHILD_PID=$!
    sleep 0.5

    run_client
    echo ""
    stop_child
    stop_server

    echo "    Data: all.data"
    perf_to_svg all.data all.svg
}

# ---------------------------------------------------------------------------

build
case "$TYPE" in
    on)  run_oncpu ;;
    off) run_offcpu ;;
    all) run_all ;;
esac
