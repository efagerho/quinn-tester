#!/bin/bash
set -euo pipefail

usage() {
    echo "Usage: $0 <conns_per_sec>"
    echo ""
    echo "Runs three 60-second profiling passes against a QUIC server at the"
    echo "given connection rate and generates flamegraphs for each:"
    echo ""
    echo "  oncpu.data  / oncpu.svg   — CPU profile of the server (perf)"
    echo "  offcpu.data / offcpu.svg  — Off-CPU / blocking profile of the server (bpftrace)"
    echo "  all.data    / all.svg     — System-wide CPU profile (perf)"
    exit 1
}

[ $# -ge 1 ] || usage

RATE=$1
DURATION=60
BIN="./target/release/quinn-tester"
SERVER_PID=""
CHILD_PID=""

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
    sleep 0.5  # allow port to be released before next run
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
    echo ""
    echo "==> [1/3] On-CPU profile..."
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
    echo ""
    echo "==> [2/3] Off-CPU profile (requires sudo for offcputime)..."
    start_server

    # offcputime runs for exactly $DURATION seconds then exits on its own.
    # -p: target PID, -U: user stacks only, -f: pre-folded output for flamegraphs.
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
    echo ""
    echo "==> [3/3] All-processes profile..."
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
run_oncpu
run_offcpu
run_all

echo ""
echo "==> Done."
echo "    oncpu.data  / oncpu.svg"
echo "    offcpu.data / offcpu.svg"
echo "    all.data    / all.svg"
