#!/usr/bin/env python3
"""Historical local SF1 timing harness; superseded for attribution.

Do not spend further local rounds on this method. Use CPU-clock profiles
locally and the settled heavy-job harness for attributed timing.

Run each stage with a release candidate. Exact numeric multisets precede
timing; Q1 AVG alone uses absolute 1e-8 tolerance. Startup, planning and CSV
output are included equally; timed output is discarded. No cloud work.
"""
import argparse, csv, decimal, json, os, statistics, subprocess, time, collections
from pathlib import Path
p = argparse.ArgumentParser()
p.add_argument('stage')
p.add_argument('--candidate', default='target/release/nata')
p.add_argument('--data', type=Path, required=True)
p.add_argument('--out', type=Path, required=True)
p.add_argument('--resume', action='store_true', help='Preserve completed cells after an interrupted stage')
a = p.parse_args()
root = Path.cwd()
out = a.out.resolve()
out.mkdir(parents=True, exist_ok=True)
(out / 'spill').mkdir(exist_ok=True)
data = a.data.resolve()
env = dict(os.environ, TMPDIR=str(out), RUST_TEST_THREADS='2')
env.pop('AI_AGENT', None)
queries = {q: (root / f'check/queries/aggregate/{f}.sql').read_text().replace('{data}', str(data)) for q, f in [('q1', 'q1'), ('having', 'high_cardinality_tpch')]}
results_path = out / (a.stage + '.json')
results = json.loads(results_path.read_text()) if a.resume else []

def cmd(engine, q, t, spill):
    s = queries[q]
    if engine == 'duckdb':
        return ['duckdb', '-csv', '-c', f"SET threads={t}; SET memory_limit='{(64 if spill else 8192)}MiB'; SET temp_directory='{out}/spill'; " + s]
    return [str(Path(a.candidate).resolve()), 'sql', '--csv', '--report', '--threads', str(t), '--aggregate-memory-bytes', str((64 if spill else 8192) << 20), '--spill-directory', str(out / 'spill'), '--spill-disk-bytes', str(4 << 30), '--command=' + s]

def run(command, output):
    start = time.perf_counter()
    r = subprocess.run(command, stdout=output, stderr=subprocess.PIPE, text=True, env=env, timeout=600)
    return (r, time.perf_counter() - start)

def canonical(path, q):
    with path.open() as f:
        rows = csv.reader(f)
        next(rows)
        return collections.Counter((tuple((x if i < 2 and q == 'q1' else decimal.Decimal(x) for i, x in enumerate(row))) for row in rows))
for q, spill in [('q1', False), ('having', False), ('having', True)]:
    for t in [1, 8]:
        completed = [o for o in results if o['kind'] == 'timing' and (o['query'], o['threads'], o['spill']) == (q, t, spill)]
        if len(completed) == 10:
            continue
        assert not completed, 'Cannot automatically resume a partially timed cell'
        reference = out / 'reference.csv'
        actual = out / 'actual.csv'
        for engine, dest in [('duckdb', reference), ('nata', actual)]:
            with dest.open('w') as f:
                r, _ = run(cmd(engine, q, t, False if engine == 'duckdb' else spill), f)
            if r.returncode:
                raise RuntimeError((engine, q, t, spill, r.stderr))
        ref = canonical(reference, q)
        got = canonical(actual, q)
        if q == 'q1':
            rr = sorted(ref)
            gg = sorted(got)
            assert len(rr) == len(gg)
            for x, y in zip(rr, gg):
                assert x[:2] == y[:2]
                for slot, (xx, yy) in enumerate(zip(x[2:], y[2:]), 2):
                    assert abs(xx - yy) < decimal.Decimal('1e-8') if 6 <= slot <= 8 else xx == yy, (slot, xx, yy)
        else:
            assert got == ref, (q, t, spill, 'multiset mismatch')
        del ref, got
        reference.unlink()
        actual.unlink()
        metrics = next((s for s in r.stderr.splitlines() if s.startswith('aggregate:')))
        counters = {k: int(v) for k, v in (s.split('=', 1) for s in metrics.split()[1:])}
        assert (counters['spill_bytes_written'] > 0) == spill
        assert counters['peak_reservation'] <= (64 if spill else 8192) << 20
        # Requests also count pending retries; only granted reservations release.
        assert counters['reservation_releases'] <= counters['reservation_calls']
        results.append(dict(kind='gate', query=q, spill=spill, threads=t, counters=counters))
        for repeat in range(5):
            for engine in ['nata', 'duckdb'] if repeat % 2 == 0 else ['duckdb', 'nata']:
                load = os.getloadavg()
                with open(os.devnull, 'w') as f:
                    r, elapsed = run(cmd(engine, q, t, spill), f)
                observation = dict(kind='timing', query=q, spill=spill, threads=t, repeat=repeat, engine=engine, load=load, seconds=elapsed, exit=r.returncode)
                if r.returncode:
                    observation['error'] = r.stderr[-1000:]
                results.append(observation)
                if engine == 'nata' and r.returncode:
                    raise RuntimeError(observation)
                (out / (a.stage + '.json')).write_text(json.dumps(results, indent=2) + '\n')
        summary = []
        for engine in ['nata', 'duckdb']:
            samples = [o for o in results if o['kind'] == 'timing' and o['query'] == q and o['threads'] == t and o['spill'] == spill and o['engine'] == engine]
            seconds = statistics.median(o['seconds'] for o in samples) if all(o['exit'] == 0 for o in samples) else None
            summary.append((engine, round(seconds, 3) if seconds is not None else 'incomplete'))
        print(a.stage, q, t, spill, summary, flush=True)
