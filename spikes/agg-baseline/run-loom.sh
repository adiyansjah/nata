#!/usr/bin/env bash
# Short sequential models/mutants. For the parent: LOOM_MAX_PREEMPTIONS=3.
# INVALID is never evidence: build/timeout/unrecognized failures fail this script.
set -euo pipefail
ulimit -c 0
cd "$(dirname "$0")"
: "${TMPDIR:?set an on-disk TMPDIR}"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export RUSTFLAGS="--cfg nata_loom"
export CARGO_TARGET_DIR=target/loom
cargo test --locked --test grants --no-run
python3 - <<'PY'
import glob, os, re, subprocess, sys
bins = [p for p in glob.glob('target/loom/debug/deps/grants-*') if os.access(p, os.X_OK) and not p.endswith('.d')]
assert len(bins) == 1, bins
binary = bins[0]
models = ['release_before_during_after_park', 'cancel_drop_races', 'competing_grants', 'one_worker_requeues']
mutants = {'mutant_cap_removed': 'grant cap exceeded', 'mutant_wake_dropped': 'deadlock', 'mutant_release_twice': 'double release'}
failed = False
for name in models + list(mutants):
    cmd = [binary, 'models::' + name, '--exact', '--nocapture', '--test-threads=1']
    if name in mutants:
        cmd += ['--ignored']
    try:
        out = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        print(name, 'INVALID timeout', flush=True); failed = True; continue
    log = out.stdout
    # Do not commit backtraces containing machine paths or thread identities.
    if name in mutants:
        status = 'CAUGHT' if out.returncode != 0 and mutants[name] in log else ('SURVIVED' if out.returncode == 0 else 'INVALID')
    else:
        status = 'PASS' if out.returncode == 0 and '1 passed' in log else 'INVALID'
    lines = re.findall(r'executions=\d+ bound=\d+', log)
    if name in mutants and status == 'CAUGHT':
        lines.append('matched=' + mutants[name])
    print(name, status, 'rc='+str(out.returncode), *lines, flush=True)
    if status not in ('PASS', 'CAUGHT'):
        print(log[-2000:]); failed = True
sys.exit(int(failed))
PY
