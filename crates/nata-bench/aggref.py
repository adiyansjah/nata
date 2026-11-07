#!/usr/bin/env python3
"""DuckDB-only aggregate reference. Scratch/output never enters pulled results."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess as sp
import time

ROOT = Path(__file__).resolve().parents[2]
p = argparse.ArgumentParser(__doc__)
p.add_argument('--out', type=Path, required=True)
p.add_argument('--scales', default='1,10')
p.add_argument('--threads', default='1,4,8')
p.add_argument('--rounds', type=int, default=5)
p.add_argument('--trial', action='store_true')
p.add_argument('--scaling', action='store_true', help='Prioritize Q1 thread scaling, then one HAVING budget block')
a = p.parse_args()
assert a.rounds > 0
scratch = Path(os.environ['TMPDIR']).resolve() / 'aggref'
assert not scratch.is_relative_to(Path('/tmp'))
scratch.mkdir(parents=True, exist_ok=True)
a.out.mkdir(parents=True, exist_ok=True)
env = dict(os.environ)
for key in ('AI_AGENT', 'CODEX_CI', 'CLAUDECODE'):
    env.pop(key, None)

def duck(sql, **kw):
    return sp.run(['duckdb', '-csv', '-c', sql], env=env, **kw)

version = duck('select version();', capture_output=True, text=True, check=True).stdout.strip()
assert 'v2.0.0' in version, version
(a.out / 'version.txt').write_text(version + '\n')
(a.out / 'code-tree.txt').write_text(sp.check_output(['bash', 'scripts/code-tree.sh'], cwd=ROOT, text=True))
queries = {
    'q1': """SELECT l_returnflag, l_linestatus,
 sum(l_quantity) AS sum_qty, sum(l_extendedprice) AS sum_base_price,
 sum(l_extendedprice * (1 - l_discount)) AS sum_disc_price,
 sum(l_extendedprice * (1 - l_discount) * (1 + l_tax)) AS sum_charge,
 avg(l_quantity) AS avg_qty, avg(l_extendedprice) AS avg_price,
 avg(l_discount) AS avg_disc, count(*) AS count_order
 FROM lineitem WHERE l_shipdate <= DATE '1998-09-02'
 GROUP BY l_returnflag, l_linestatus""",
    'having': """SELECT l_orderkey, count(*) AS n, sum(l_extendedprice) AS price,
 sum(l_extendedprice * (1 - l_discount)) AS net
 FROM lineitem WHERE l_shipdate <= DATE '1998-09-02'
 GROUP BY l_orderkey HAVING count(*) > 1""",
}
for name in ('agg_orderkey', 'agg_partkey'):
    key, value = ('l_orderkey', 'l_extendedprice') if name == 'agg_orderkey' else ('l_partkey', 'l_quantity')
    queries[name] = f"SELECT {key}, count(*), sum({value}), sum(l_extendedprice * (1 - l_discount)) FROM lineitem WHERE l_shipdate <= DATE '1998-09-02' GROUP BY {key}"
(a.out / 'queries.json').write_text(json.dumps(queries, indent=2) + '\n')
fields = ['sf', 'threads', 'query', 'memory', 'round', 'arm', 'settled', 'ok', 'seconds',
          'rss_kib', 'input_bytes', 'output_bytes', 'temp_peak_bytes', 'buffer_peak_bytes']
f = (a.out / 'runs.csv').open('w')
w = csv.DictWriter(f, fieldnames=fields)
w.writeheader()
failed = 0
for sf in a.scales.split(','):
    data = ROOT / 'data' / ('tpch-sf' + sf)
    if not (data / 'lineitem.parquet').exists():
        sp.run(['bash', 'scripts/gen-tpch.sh', sf, str(data)], env=env, check=True)
    path = data / 'lineitem.parquet'
    input_bytes = path.stat().st_size
    with (a.out / 'inputs.txt').open('a') as inp:
        inp.write(f'SF{sf} lineitem.parquet {input_bytes} bytes sha256={hashlib.file_digest(path.open("rb"), "sha256").hexdigest()}\n')
    budgets = ['4GiB', '64MiB', '256MiB'] if float(sf) < 100 else ['32GiB', '4GiB']
    expected = {}
    blocks = [(t, queries, budgets) for t in map(int, a.threads.split(','))]
    if a.scaling:
        blocks = [(t, {'q1': queries['q1']}, ['32GiB']) for t in map(int, a.threads.split(','))]
        blocks.append((8, {'having': queries['having']}, ['32GiB', '4GiB']))
    for threads, block_queries, block_budgets in blocks:
        # Warm cache and verify every budget/thread before the timed block.
        valid = {}
        for name, body in block_queries.items():
            for memory in block_budgets:
                setup = f"SET threads={threads}; SET memory_limit='{memory}'; SET temp_directory='{scratch}/spill'; CREATE VIEW lineitem AS SELECT * FROM '{path}';"
                probe = duck(setup + f'SELECT count(*), sum(hash(t)) FROM ({body}) t;', capture_output=True, text=True)
                key = (name, memory)
                valid[key] = probe.returncode == 0
                with (a.out / 'checks.txt').open('a') as c:
                    c.write(f'SF{sf} t{threads} {name} {memory}: {probe.stdout.strip()} {probe.stderr.strip()}\n')
                if valid[key]:
                    answer = probe.stdout.strip()
                    if name not in expected:
                        expected[name] = answer
                    assert answer == expected[name], (sf, threads, name, memory, answer, expected[name])
        label = f'sf{sf}-t{threads}'
        settle = sp.run(['bash', str(ROOT / 'scripts/gcp/settle.sh'), label, '0.5', '0' if a.trial else '600'], capture_output=True, text=True)
        with (a.out / 'settle.txt').open('a') as log:
            log.write(settle.stdout)
        settled = settle.returncode == 0
        for name, body in block_queries.items():
            for memory in block_budgets:
                if not valid[name, memory]:
                    failed += 1
                    continue  # OOM is a measured unsupported cell, never a timing.
                for r in range(a.rounds):
                    for arm in (['control', 'twin'] if r % 2 == 0 else ['twin', 'control']):
                        spill = scratch / 'spill'
                        shutil.rmtree(spill, ignore_errors=True)
                        spill.mkdir()
                        profile = scratch / 'profile.json'
                        profile.unlink(missing_ok=True)
                        output = scratch / 'output.csv'
                        rss = scratch / 'rss.txt'
                        sql = f"SET threads={threads}; SET memory_limit='{memory}'; SET temp_directory='{spill}'; CREATE VIEW lineitem AS SELECT * FROM '{path}'; SET enable_profiling='json'; SET profiling_output='{profile}'; {body};"
                        with output.open('wb') as sink:
                            start = time.perf_counter()
                            proc = sp.run(['/usr/bin/time', '-f', '%M', '-o', str(rss), 'duckdb', '-csv', '-c', sql], stdout=sink, stderr=sp.PIPE, env=env)
                            elapsed = time.perf_counter() - start
                        ok = proc.returncode == 0 and profile.exists()
                        info = json.loads(profile.read_text()) if ok else {}
                        if not ok:
                            failed += 1
                            with (a.out / 'errors.txt').open('a') as log:
                                log.write(f'{label} {name} {memory} r{r} {arm}: {proc.stderr.decode()}\n')
                        row = dict(zip(fields, [sf, threads, name, memory, r, arm, settled, ok, elapsed,
                            rss.read_text().strip().splitlines()[-1], input_bytes, output.stat().st_size,
                            info['system']['peak_temp_dir_size'] if ok else '', info['system']['peak_buffer_memory'] if ok else '']))
                        w.writerow(row)
                        f.flush()
                        output.unlink()
        print(label, 'done', flush=True)
f.close()
rows = list(csv.DictReader((a.out / 'runs.csv').open()))
with (a.out / 'medians.csv').open('w') as f:
    keys = ['sf', 'threads', 'query', 'memory', 'arm']
    writer = csv.writer(f)
    writer.writerow(keys + ['n', 'settled', 'min_s', 'q1_s', 'median_s', 'q3_s', 'max_s', 'rss_kib_max', 'temp_bytes_max', 'output_bytes_max'])
    for key in sorted({tuple(row[k] for k in keys) for row in rows}):
        group = [row for row in rows if tuple(row[k] for k in keys) == key and row['ok'] == 'True']
        if not group:
            continue
        times = [float(row['seconds']) for row in group]
        qs = statistics.quantiles(times, n=4, method='inclusive') if len(times) > 1 else times * 3
        writer.writerow(list(key) + [len(group), all(row['settled'] == 'True' for row in group), min(times), qs[0], statistics.median(times), qs[2], max(times), max(int(row['rss_kib']) for row in group), max(int(row['temp_peak_bytes']) for row in group), max(int(row['output_bytes']) for row in group)])
(a.out / 'completion.json').write_text(json.dumps({'rows': len(rows), 'unsupported_or_failed': failed, 'trial': a.trial}) + '\n')
