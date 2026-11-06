#!/usr/bin/env bash
# Regenerates every table of bench/results/20261106-sql-parser-baseline.md. Run from spikes/sql-parser. Needs duckdb (with the tpch extension) on the PATH.
# The timing and robustness runs are separate because they must run alone on the machine:
#   target/release/bake time target/corpus/timing.rs 20 5 > target/corpus/timing.jsonl   (timing.rs: `export_rs` of the accepted test set plus fuzz.jsonl, see corpus/lib.py)
#   target/release/bake robust > target/corpus/robust.jsonl ; stack-need lines into stack.jsonl
#   the same two with a build `--no-default-features --features arm-a` into robust-nostacker.jsonl / stack-nostacker.jsonl
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -I corpus/testset.py corpus/testset.jsonl
cargo build --release
cargo test --release
cargo clippy --release --all-targets -- -D warnings
env -u AI_AGENT python3 -I corpus/report.py corpus accept errors literals mutants robust > target/corpus/tables.md
python3 -I corpus/build_results.py
