#!/bin/bash
# Verification-plan items 25 (interop) and 26 (planning) on a clean VM (GCP).  usage: spikes/iceberg-plan/gcp-check.sh <results-dir>
# Phases: build; cargo tests (transform vectors, projection property, randomized planner soundness); the 22 mutants; interop in both directions with pyiceberg and
# DuckDB; the truncate-overflow hazard; planning at 10k / 100k / 1M files. Needs cargo, python3, curl, network (GitHub, PyPI, extensions.duckdb.org).
# Exit 0 only if every gating phase passed. Informational phases (the hazard report, the benchmark) never fail the run.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); OUT=${1:?usage: gcp-check.sh <results-dir>}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd); cd "$HERE" || exit 1
W=${IP_WORK:-${WORK:+$WORK/iceberg-plan-work}}; W=${W:-$HOME/iceberg-plan-work}; mkdir -p "$W"
SUMMARY=$OUT/summary.txt; : > "$SUMMARY"; FAILED=0
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
gate() { local n=$1; shift; local t0=$SECONDS; if "$@" > "$OUT/$n.txt" 2>&1; then log "PASS $n ($((SECONDS - t0)) s)"; else log "FAIL $n ($((SECONDS - t0)) s), see $n.txt"; FAILED=1; fi; }
info() { local n=$1; shift; local t0=$SECONDS; "$@" > "$OUT/$n.txt" 2>&1; log "INFO $n: exit $? ($((SECONDS - t0)) s), see $n.txt"; }
log "host: $(uname -m), $(nproc) vCPU, $(awk '/MemTotal/{printf "%d MiB",$2/1024}' /proc/meminfo); $(rustc --version)"
# --- pinned Python tools (uv, then a venv): the versions the spike was developed against
UV_VERSION=0.13.6; A=$(uname -m)
if [ ! -x "$W/uv/uv" ]; then
  mkdir -p "$W/uv"; cd "$W/uv" && curl -fsSL -o uv.tgz "https://github.com/astral-sh/uv/releases/download/$UV_VERSION/uv-$A-unknown-linux-gnu.tar.gz" \
    && curl -fsSL -o uv.sha256 "https://github.com/astral-sh/uv/releases/download/$UV_VERSION/uv-$A-unknown-linux-gnu.tar.gz.sha256" \
    && [ "$(sha256sum uv.tgz | cut -d' ' -f1)" = "$(cut -d' ' -f1 uv.sha256)" ] && tar -xzf uv.tgz --strip-components=1; cd "$HERE" || exit 1
fi
UV=$W/uv/uv; [ -x "$UV" ] || { log "FAIL tools: uv could not be installed"; exit 1; }
if [ ! -x "$W/venv/bin/python" ]; then
  "$UV" venv "$W/venv" --python 3.12 > "$OUT/venv.txt" 2>&1 && "$UV" pip install --python "$W/venv/bin/python" pyiceberg==0.12.0 pyarrow==26.0.1 duckdb==2.0.0 sqlalchemy pyiceberg-core==0.11.0 >> "$OUT/venv.txt" 2>&1
fi
PY=$W/venv/bin/python; "$PY" -c "import pyiceberg, pyarrow, duckdb; print('pyiceberg', pyiceberg.__version__, 'pyarrow', pyarrow.__version__, 'duckdb', duckdb.__version__)" > "$OUT/versions.txt" 2>&1 \
  && log "tools: $(cat "$OUT/versions.txt")" || { log "FAIL tools: python packages"; cat "$OUT/venv.txt" | tail -5; exit 1; }
gate build cargo build --release --locked --bins --tests
gate cargo-tests cargo test --release --locked
gate mutants python3 -I mutate.py --work "$W/mut"
export ICX=$HERE/target/release/icx
gate interop-ours-to-others "$PY" interop/ours_to_others.py "$W/o2o" 1 80 15
gate interop-others-to-ours "$PY" interop/others_to_ours.py "$W/o2u" 1 80 15
info truncate-overflow "$PY" interop/truncate_overflow.py "$W/ov"
info scale ./scale.sh "$W/scale" "$OUT/scale.jsonl"
cp "$OUT/scale.txt" "$OUT/scale-table.txt" 2>/dev/null
rm -rf "$W/mut" "$W/o2o" "$W/o2u" "$W/ov" "$W/scale"
[ $FAILED = 0 ] && log "VERDICT: all gating phases passed" || log "VERDICT: FAILED"
exit $FAILED
