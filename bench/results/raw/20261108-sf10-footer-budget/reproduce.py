#!/usr/bin/env python3
"""Local Q1 completion controls; no timing or settled-performance verdict.

Run from the repository root. Positional arguments: SF10 directory, immutable
baseline binary, candidate binary, fresh output directory.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess as sp
import sys

data, before, after, out = map(lambda p: Path(p).resolve(), sys.argv[1:])
out.mkdir(parents=True, exist_ok=False)
sql = Path('check/queries/aggregate/q1.sql').read_text().replace(
    '{data}', str(data).replace("'", "''"))
spec = importlib.util.spec_from_file_location('h', 'scripts/bench-aggregate-perf1-heavy.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
env = dict(os.environ, TMPDIR=str(out))
env.pop('AI_AGENT', None)
records = []
for threads in (1, 8):
    for mib in (64, 256):
        oracle = out / f'duckdb-{threads}-{mib}.csv'
        with oracle.open('w') as sink, oracle.with_suffix('.log').open('w') as err:
            sp.run(['duckdb', '-csv', '-c',
                    f"SET threads={threads}; SET memory_limit='{mib}MiB'; " + sql],
                   env=env, stdout=sink, stderr=err, check=True, timeout=600)
        expected = h.rows(oracle, 'q1')
        for variant, binary in [('before', before), ('after', after)]:
            label = f'{variant}-{threads}-{mib}'
            csv = out / (label + '.csv')
            log = out / (label + '.log')
            with csv.open('w') as sink, log.open('w') as err:
                p = sp.run([str(binary), 'sql', '--csv', '--report', '--threads', str(threads),
                            '--aggregate-memory-bytes', str(mib << 20), '--command=' + sql],
                           env=env, stdout=sink, stderr=err, timeout=600)
            diagnostic = log.read_text()
            row = dict(variant=variant, threads=threads, budget=mib << 20,
                       exit=p.returncode, log=log.name)
            if variant == 'before' and mib == 64:
                assert p.returncode != 0 and 'aggregate footer metadata exceeds memory budget' in diagnostic
                row['error'] = 'aggregate footer metadata exceeds memory budget'
            else:
                assert p.returncode == 0, diagnostic
                assert h.equal_values(expected, h.rows(csv, 'q1'), 'q1')
                row['duckdb_equal'] = True
                row['peak_reservation'] = int(re.search(r'peak_reservation=(\d+)', diagnostic)[1])
                row['active_threads'] = int(re.search(r' threads=(\d+)', diagnostic)[1])
                assert row['peak_reservation'] <= mib << 20
            records.append(row)
            print(row, flush=True)
identity = {variant: hashlib.sha256(binary.read_bytes()).hexdigest()
            for variant, binary in [('before', before), ('after', after)]}
(out / 'results.json').write_text(json.dumps(dict(binaries=identity, cells=records), indent=2) + '\n')
