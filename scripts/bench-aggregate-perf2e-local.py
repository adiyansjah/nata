#!/usr/bin/env python3
"""Unsettled local paired experiments; no frozen-target verdict or cloud calls."""
import argparse
import importlib.util
import json
import hashlib
import os
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location('heavy', Path(__file__).with_name('bench-aggregate-perf1-heavy.py'))
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('stage')
p.add_argument('--before', type=Path, required=True)
p.add_argument('--after', type=Path, required=True)
p.add_argument('--data', type=Path, required=True)
p.add_argument('--out', type=Path, required=True)
p.add_argument('--rounds', type=int, default=5)
p.add_argument('--profile', action='store_true')
a = p.parse_args()
a.out.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(a.out.resolve()))
env.pop('AI_AGENT', None)
env.pop('CODEX_CI', None)
root = Path(__file__).resolve().parents[1]
binary_hashes = {label: h.digest(binary) for label, binary in [('before', a.before), ('after', a.after)]}
assert all(binary.stat().st_mode & 0o222 == 0 for binary in (a.before, a.after)), 'use read-only binary copies'
observations = []
gates = []
# High-cardinality is the general hash control. Twin is the identical generic
# before binary, alternating within every round, rather than another-day data.
for query in h.QUERIES:
    sql = (root / f'check/queries/aggregate/{query}.sql').read_text().replace('{data}', str(a.data.resolve()).replace("'", "''"))
    for threads, budget in ((1, 8 << 30), (8, 8 << 30), (1, 64 << 20), (8, 64 << 20)):
        commands = {label: h.command(binary.resolve(), sql, threads, budget, a.out.resolve()) for label, binary in [('before', a.before), ('after', a.after), ('control', a.before)]}
        expected = a.out / 'expected.csv'
        ref = h.invoke(h.command('duckdb', sql, threads, budget, a.out.resolve()), env, expected)
        assert ref['exit'] == 0, ref
        oracle = h.rows(expected, query)
        byte_gates = {}
        for label, cmd in commands.items():
            actual = a.out / 'actual.csv'
            result = h.invoke(cmd, env, actual)
            assert result['exit'] == 0, result
            assert 0 < result['counters']['peak_reservation'] <= budget, result
            assert h.equal_values(oracle, h.rows(actual, query), query), (query, label, 'RESULT CHANGED')
            # These grouped results have no ORDER BY. Compare complete CSV
            # row bytes as a multiset (neither fixture has multiline fields).
            # Record raw hashes too, including the
            # identical-before control, to expose scheduling-only reordering.
            data = actual.read_bytes()
            lines = data.splitlines(keepends=True)
            canonical = hashlib.sha256(lines[0] + b''.join(sorted(lines[1:]))).hexdigest()
            raw = hashlib.sha256(data).hexdigest()
            byte_gates[label] = canonical
            assert canonical == byte_gates['before'], (query, label, 'CSV BYTES CHANGED')
            gates.append(dict(query=query, threads=threads, budget=budget, arm=label, rows=sum(oracle.values()), csv_sha256=raw, sorted_csv_sha256=canonical, **result))
            h.save(a.out / f'{a.stage}-gates.json', gates)
        del oracle
        expected.unlink()
        actual.unlink()
        for r in range(a.rounds):
            order = list(commands) if r % 2 == 0 else list(reversed(commands))
            for label in order:
                observation = h.invoke(commands[label], env)
                assert observation['exit'] == 0, observation
                assert 0 < observation['counters']['peak_reservation'] <= budget, observation
                observations.append(dict(query=query, threads=threads, budget=budget, round=r, arm=label, **observation))
                h.save(a.out / f'{a.stage}-samples.json', observations)
        if a.profile:
            for label in ('before', 'after'):
                stem = a.out / f'{a.stage}-{query}-{threads}-{budget}-{label}'
                with open(os.devnull, 'w') as sink:
                    subprocess.run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '499', '-g', '--call-graph', 'dwarf,8192', '-o', str(stem)+'.data', '--', *commands[label]], env=env, stdout=sink, stderr=subprocess.PIPE, check=True)
                report = subprocess.check_output(['perf', 'report', '-i', str(stem)+'.data', '--stdio', '--no-children', '--call-graph', 'none', '--sort', 'symbol', '--percent-limit', '0'], stderr=subprocess.PIPE)
                report = subprocess.run(['c++filt', '-s', 'rust'], input=report, capture_output=True, check=True).stdout
                stem.with_suffix('.txt').write_bytes(b'\n'.join(report.splitlines()[:90]) + b'\n')
                dso = subprocess.check_output(['perf', 'report', '-i', str(stem)+'.data', '--stdio', '--no-children', '--call-graph', 'none', '--sort', 'dso', '--percent-limit', '0.1'], stderr=subprocess.PIPE)
                Path(str(stem)+'-dso.txt').write_bytes(dso)
                Path(str(stem)+'.data').unlink()
summary = []
for query in h.QUERIES:
    for threads, budget in ((1, 8 << 30), (8, 8 << 30), (1, 64 << 20), (8, 64 << 20)):
        cell = [o for o in observations if (o['query'], o['threads'], o['budget']) == (query, threads, budget)]
        arms = {label: [o['seconds'] for o in cell if o['arm'] == label] for label in ('before', 'after', 'control')}
        summary.append(dict(query=query, threads=threads, budget=budget, arms={k:h.distribution(v) for k,v in arms.items()}, ratio=h.distribution([x/y for x,y in zip(arms['after'],arms['before'])]), control_ratio=h.distribution([x/y for x,y in zip(arms['control'],arms['before'])])))
assert binary_hashes == {label: h.digest(binary) for label, binary in [('before', a.before), ('after', a.after)]}, 'BINARY CHANGED DURING RUN'
h.save(a.out / f'{a.stage}-summary.json', dict(limits='unsettled, warm SF1, whole CLI incl startup/planning/CSV, 8GiB and 64MiB; no target verdict', binaries=binary_hashes, cells=summary))
print(json.dumps(summary, indent=2))
