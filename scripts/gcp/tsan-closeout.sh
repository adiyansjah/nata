#!/usr/bin/env bash
# 0004 F: cheap controls locally; workspace mode only on a clean Spot VM.
set -euo pipefail
cd "$(dirname "$0")/../.."
SRC=$PWD
: "${WORK:?set WORK to an on-disk scratch directory}"
: "${TMPDIR:?set TMPDIR to an on-disk scratch directory}"
OUT=${RESULTS:-$WORK/results}
mkdir -p "$OUT" "$TMPDIR"
NIGHTLY=nightly-2026-08-21
T=x86_64-unknown-linux-gnu
[ "$(uname -m)" = x86_64 ]
export RUSTFLAGS=-Zsanitizer=thread RUSTDOCFLAGS=-Zsanitizer=thread
export CARGO_TARGET_DIR="$WORK/target-thread"
SUPPRESSIONS="${TSAN_OPTIONS:+$TSAN_OPTIONS:}suppressions=$SRC/scripts/tsan-suppressions.txt"

case "${1:-}" in
  controls)
    # Copy the existing Tokio-only reproduction; no edits to historical evidence.
    mkdir -p "$WORK/tokio-repro"
    cp bench/results/raw/20261107-debt-a/tokio-repro/{Cargo.toml,main.rs} "$WORK/tokio-repro/"
    cargo +"$NIGHTLY" build -Zbuild-std --target "$T" --manifest-path "$WORK/tokio-repro/Cargo.toml"
    set +e
    "$CARGO_TARGET_DIR/$T/debug/tokio-tsan-repro" > "$OUT/tokio-unsuppressed.txt" 2>&1
    before=$?
    TSAN_OPTIONS="$SUPPRESSIONS" "$CARGO_TARGET_DIR/$T/debug/tokio-tsan-repro" > "$OUT/tokio-suppressed.txt" 2>&1
    after=$?
    TSAN_OPTIONS="$SUPPRESSIONS" cargo +"$NIGHTLY" test -Zbuild-std --target "$T" --manifest-path scripts/tsan-planted/Cargo.toml > "$OUT/planted.txt" 2>&1
    planted=$?
    set -e
    printf 'tokio without suppressions: %s\ntokio with suppressions: %s\nplanted with suppressions: %s\n' "$before" "$after" "$planted" | tee "$OUT/controls-exits.txt"
    [ "$before" = 66 ]
    grep -q 'WARNING: ThreadSanitizer: data race' "$OUT/tokio-unsuppressed.txt"
    [ "$after" = 0 ]
    if grep -q 'WARNING: ThreadSanitizer' "$OUT/tokio-suppressed.txt"; then exit 1; fi
    [ "$planted" -ne 0 ]
    grep -q 'WARNING: ThreadSanitizer: data race' "$OUT/planted.txt"
    grep -q 'nata_tsan_planted::.*nata_unsynchronized_write' "$OUT/planted.txt"
    grep -q 'exit status: 66' "$OUT/planted.txt"
    ;;
  workspace)
    # Same fixture/native RSS companion and exact TSan cargo command as eng-checks.
    RUSTFLAGS= RUSTDOCFLAGS= scripts/gen-tpch.sh 0.01 data/tpch-sf0.01
    RUSTFLAGS= RUSTDOCFLAGS= CARGO_TARGET_DIR="$WORK/target-rss" cargo build --release -p nata
    export TSAN_OPTIONS="$SUPPRESSIONS"
    set +e
    NATA_INSTRUMENTED=1 NATA_RSS_BINARY="$WORK/target-rss/release/nata" cargo +"$NIGHTLY" test -Zbuild-std --target "$T" --workspace --lib --tests --no-fail-fast > "$OUT/workspace.txt" 2>&1
    result=$?
    set -e
    echo "exit status: $result" | tee "$OUT/workspace-exit.txt"
    exit "$result"
    ;;
  *) echo "usage: $0 controls|workspace" >&2; exit 2 ;;
esac
