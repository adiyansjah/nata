#!/usr/bin/env python3
"""Unsettled local paired experiments; no frozen-target verdict or cloud calls."""
import argparse
import importlib.util
import json
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
root = Path(__file__).resolve().parents[1]
observations = []
gates = []
# High-cardinality is the general hash control. Twin is the identical generic
# before binary, alternating within every round, rather than another-day data.
for query in h.QUERIES:
    sql = (root / f'check/queries/aggregate/{query}.sql').read_text().replace('{data}', str(a.data.resolve()).replace("'", "''"))
    for threads in (1, 8):
        commands = {label: h.command(binary.resolve(), sql, threads, 8 << 30, a.out.resolve()) for label, binary in [('before', a.before), ('after', a.after), ('control', a.before)]}
        expected = a.out / 'expected.csv'
        ref = h.invoke(h.command('duckdb', sql, threads, 8 << 30, a.out.resolve()), env, expected)
        assert ref['exit'] == 0, ref
        oracle = h.rows(expected, query)
        for label, cmd in commands.items():
            actual = a.out / 'actual.csv'
            result = h.invoke(cmd, env, actual)
            assert result['exit'] == 0, result
            assert h.equal_values(oracle, h.rows(actual, query), query), (query, label, 'RESULT CHANGED')
            gates.append(dict(query=query, threads=threads, arm=label, rows=sum(oracle.values()), **result))
            h.save(a.out / f'{a.stage}-gates.json', gates)
        del oracle
        expected.unlink()
        actual.unlink()
        for r in range(a.rounds):
            order = list(commands) if r % 2 == 0 else list(reversed(commands))
            for label in order:
                observation = h.invoke(commands[label], env)
                assert observation['exit'] == 0, observation
                observations.append(dict(query=query, threads=threads, round=r, arm=label, **observation))
                h.save(a.out / f'{a.stage}-samples.json', observations)
        if a.profile:
            for label in ('before', 'after'):
                stem = a.out / f'{a.stage}-{query}-{threads}-{label}'
                with open(os.devnull, 'w') as sink:
                    subprocess.run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '499', '-g', '--call-graph', 'dwarf,8192', '-o', str(stem)+'.data', '--', *commands[label]], env=env, stdout=sink, stderr=subprocess.PIPE, check=True)
                report = subprocess.check_output(['perf', 'report', '-i', str(stem)+'.data', '--stdio', '--no-children', '--call-graph', 'none', '--sort', 'symbol', '--percent-limit', '0'], stderr=subprocess.PIPE)
                report = subprocess.run(['c++filt', '-s', 'rust'], input=report, capture_output=True, check=True).stdout
                stem.with_suffix('.txt').write_bytes(report)
                Path(str(stem)+'.data').unlink()
summary = []
for query in h.QUERIES:
    for threads in (1, 8):
        cell = [o for o in observations if (o['query'], o['threads']) == (query, threads)]
        arms = {label: [o['seconds'] for o in cell if o['arm'] == label] for label in ('before', 'after', 'control')}
        summary.append(dict(query=query, threads=threads, arms={k:h.distribution(v) for k,v in arms.items()}, ratio=h.distribution([x/y for x,y in zip(arms['after'],arms['before'])]), control_ratio=h.distribution([x/y for x,y in zip(arms['control'],arms['before'])])))
h.save(a.out / f'{a.stage}-summary.json', dict(limits='unsettled, warm SF1, whole CLI incl startup/planning/CSV, 8GiB; no target verdict', binaries={k:h.digest(v) for k,v in [('before',a.before),('after',a.after)]}, cells=summary))
print(json.dumps(summary, indent=2))
