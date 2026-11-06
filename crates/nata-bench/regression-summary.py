#!/usr/bin/env python3
"""Reduce raw benchmark runs to machine-specific regression medians.

Usage: regression-summary.py RESULTS_DIR OUTPUT.csv [SQL_ROUNDS]
SQL_ROUNDS is an optional comma-separated subset, e.g. 0,2,4. Scan rounds are
always all five; scan baselines can also be copied from earlier raw records.
Planning is aggregated across the whole corpus; per-query 1 ms verdicts remain
in sql-bench.py's full summary. This gate is deliberately run outside CI.
"""
import csv
from pathlib import Path
import statistics
import sys

source, output = map(Path, sys.argv[1:3])
rounds = set(sys.argv[3].split(',')) if len(sys.argv) > 3 else None
sql = list(csv.DictReader((source/'slice4-close/runs.csv').open()))
scan = list(csv.DictReader((source/'scan-close/runs.csv').open()))
metrics = {}
for r in sql:
    if rounds is not None and r['round'] not in rounds:
        continue
    if r['arm'] != 'nata' and r['arm'] != 'sql':
        continue
    block = r['block']
    if block.startswith('plan-'):
        key = 'sql/planning-corpus'
    elif block in ['sf1', 'sf10', 'scale-sf10']:
        key = f"sql/{block}/threads{r['threads']}"
    else:
        continue
    metrics.setdefault(key, []).append(float(r['secs']))
for r in scan:
    if r['exit'] != '0':
        raise ValueError('scan benchmark failed')
    if r['block'] not in ['sf1', 'sf10'] or r['arm'] != 'scan':
        continue
    if r['threads'] not in ['1','8'] or r['window'] != r['threads']:
        continue
    key = f"scan/{r['block']}/threads{r['threads']}"
    metrics.setdefault(key, []).append(float(r['secs']))
if len(metrics) != 11:
    raise ValueError(f'incomplete regression set: {sorted(metrics)}')
with output.open('w') as f:
    writer = csv.writer(f)
    writer.writerow(['metric','median_secs'])
    for key, values in sorted(metrics.items()):
        writer.writerow([key,statistics.median(values)])
