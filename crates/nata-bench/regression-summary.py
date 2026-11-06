#!/usr/bin/env python3
"""Reduce raw benchmark runs to machine-specific regression medians.

Usage: regression-summary.py RESULTS_DIR OUTPUT.csv [SQL_ROUNDS]
SQL_ROUNDS is an optional comma-separated subset, e.g. 0,2,4. Scan rounds are
always all five; scan baselines can also be copied from earlier raw records.
Planning is aggregated across the whole corpus; per-query 1 ms verdicts remain
in sql-bench.py's full summary. This gate is deliberately run outside CI.
"""
import csv
from collections import defaultdict
from pathlib import Path
import statistics
import sys

source, output = map(Path, sys.argv[1:3])
rounds = set(sys.argv[3].split(',')) if len(sys.argv) > 3 else None
sql = list(csv.DictReader((source/'slice4-close/runs.csv').open()))
scan = list(csv.DictReader((source/'scan-close/runs.csv').open()))
# A pulled CSV may be a partial checkpoint. Never turn it into a baseline.
repo = Path(__file__).resolve().parents[2]
expected = {'plan-card-example'} | {
    'plan-'+directory+'-'+p.stem
    for directory in ['sql','sql-differences']
    for p in (repo/'check/queries'/directory).glob('*.sql')
}
observed = {r['block'] for r in sql if r['block'].startswith('plan-')}
if expected != observed:
    raise ValueError('incomplete or different planning corpus')
sql_groups = defaultdict(list)
for r in sql:
    sql_groups[r['block'],r['arm'],r['threads']].append(r['round'])
required = {(b,a,'1') for b in expected for a in ['nata','duckdb','twin']}
required |= {(b,a,t) for b in ['sf1','sf10','scale-sf10'] for a in ['sql','twin'] for t in ['1','8']}
for key in required:
    if sorted(sql_groups[key]) != ['0','1','2','3','4']:
        raise ValueError(f'incomplete or duplicate SQL rounds: {key}')
for block in ['files10000','resource-sf10']:
    for arm in ['sql','scan']:
        if sql_groups[block,arm,'8'] != ['0']:
            raise ValueError('resource probe missing or duplicated')
scan_groups = defaultdict(list)
for r in scan:
    scan_groups[r['block'],r['arm'],r['threads'],r['window']].append(r['round'])
for sf in ['sf1','sf10']:
    for t in ['1','8']:
        for arm in ['scan','twin']:
            if sorted(scan_groups[sf,arm,t,t]) != ['1','2','3','4','5']:
                raise ValueError('incomplete or duplicate scan rounds')
if rounds is not None and (not rounds or not rounds <= set('01234')):
    raise ValueError('invalid SQL round selection')
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
    writer = csv.writer(f, lineterminator="\n")
    writer.writerow(['metric','median_secs'])
    for key, values in sorted(metrics.items()):
        writer.writerow([key,statistics.median(values)])
