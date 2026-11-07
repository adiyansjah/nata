#!/usr/bin/env python3
"""Local CPU-clock symbol profiles only; elapsed time is not attributable evidence."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('stage')
p.add_argument('--candidate', type=Path, required=True)
p.add_argument('--data', type=Path, required=True)
p.add_argument('--out', type=Path, required=True)
a = p.parse_args()
a.out.mkdir(parents=True, exist_ok=True)
root = Path(__file__).resolve().parents[1]
env = dict(os.environ, TMPDIR=str(a.out.resolve()))
env.pop('AI_AGENT', None)
observations = []
for query, spill in [('q1', False), ('high_cardinality_tpch', False), ('high_cardinality_tpch', True)]:
    label = f'{a.stage}-{query}' + ('-64MiB' if spill else '')
    sql = (root / f'check/queries/aggregate/{query}.sql').read_text().replace('{data}', str(a.data.resolve()).replace("'", "''"))
    command = [str(a.candidate.resolve()), 'sql', '--csv', '--report', '--threads', '8', '--aggregate-memory-bytes', str((64 if spill else 8192) << 20), '--spill-directory', str(a.out.resolve()), '--spill-disk-bytes', str(4 << 30), '--command=' + sql]
    profile = a.out / (label + '.data')
    load = os.getloadavg()
    start = time.monotonic()
    with open(os.devnull, 'w') as sink:
        result = subprocess.run(['perf', 'record', '-q', '-e', 'cpu-clock:u', '-F', '199', '-g', '--call-graph', 'dwarf,8192', '-o', str(profile), '--', *command], env=env, stdout=sink, stderr=subprocess.PIPE, text=True, timeout=600)
    if result.returncode:
        raise RuntimeError(result.stderr)
    report = subprocess.check_output(['perf', 'report', '-i', str(profile), '--stdio', '--no-children', '--call-graph', 'none', '--sort', 'symbol', '--fields', 'overhead,sample,symbol', '--percent-limit', '0'], text=True, stderr=subprocess.PIPE)
    report = subprocess.run(['c++filt', '-s', 'rust'], input=report, text=True, capture_output=True, check=True).stdout
    symbols = []
    for line in report.splitlines():
        match = re.match(r'\s*([0-9.]+)%\s+([0-9]+)\s+\[.\]\s+(.*?)\s{2,}', line)
        if match:
            name = re.sub(r'\[[0-9a-f]{16}\]', '', match[3])
            symbols.append(dict(percent=float(match[1]), samples=int(match[2]), symbol=name))
    symbols.sort(key=lambda row: row['percent'], reverse=True)
    (a.out / (label + '.json')).write_text(json.dumps(symbols, indent=2)+'\n')
    (a.out / (label + '.txt')).write_text('CPU-clock user self samples; sorted by share; no elapsed-time attribution.\n' + '\n'.join(f"{row['percent']:6.2f}% {row['samples']:6d} {row['symbol']}" for row in symbols)+'\n')
    counters = next(line for line in result.stderr.splitlines() if line.startswith('aggregate:'))
    observations.append(dict(query=query, spill=spill, load=load, profiled_seconds=time.monotonic()-start, counters=counters))
    print(label, 'profile complete', flush=True)
(a.out / (a.stage + '-profiles.json')).write_text(json.dumps(dict(stage=a.stage, binary_sha256=hashlib.sha256(a.candidate.read_bytes()).hexdigest(), observations=observations), indent=2)+'\n')
