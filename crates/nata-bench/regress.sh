#!/bin/bash
# Compare machine-specific medians.csv files: metric,median_secs.
# Deliberately not gating on noisy CI runners. GCP jobs may set REGRESSION_BASELINE.
set -euo pipefail
python3 -I - "${1:?baseline medians.csv}" "${2:?new medians.csv}" "${3:-0.20}" <<'PY'
import csv, math, sys, statistics

def read(path):
    rows = list(csv.DictReader(open(path)))
    if rows and 'block' in rows[0]:
        groups = {}
        for r in rows:
            if r.get('exit', '0') != '0':
                raise ValueError('failed benchmark sample')
            key = '/'.join(r[k] for k in ['block','arm','threads','window'])
            groups.setdefault(key, []).append(float(r['secs']))
        rows = [dict(metric=k, median_secs=statistics.median(v)) for k,v in groups.items()]
    out = {r['metric']: float(r['median_secs']) for r in rows}
    if not out or len(out) != len(rows) or any(not math.isfinite(v) or v <= 0 for v in out.values()):
        raise ValueError('empty, duplicate or invalid medians')
    return out
base, new = map(read, sys.argv[1:3])
tolerance = float(sys.argv[3])
assert 0 < tolerance < 1
failed = False
for key, value in base.items():
    ratio = new.get(key, math.inf) / value
    ok = ratio <= 1 + tolerance
    print(f'{key}: {ratio:.3f}x (limit {1+tolerance:.3f}x): {"PASS" if ok else "FAIL"}')
    failed |= not ok
sys.exit(failed)
PY
