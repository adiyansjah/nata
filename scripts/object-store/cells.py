#!/usr/bin/env python3
"""Nata against DuckDB httpfs and optionally the spike wrapper, cell by cell, on one bucket (card 0006, the frozen ceilings of part 3). Interleaves arms and twins within a round,
checks every answer against DuckDB's before it counts as a time, and prints per-cell medians, the twin spread and the ratio to the
frozen ceiling with the verdict the rule gives. Run it on the machine type the ceilings were frozen on (n4a-highcpu-8, c3d-highcpu-8), in the same session as
the DuckDB arm; a cell whose ratio is inside the twin spread of its ceiling is reported UNRESOLVED, not PASS.

  cells.py --nata target/release/nata --layouts sf1,sf10 [--rounds 5] [--out cells.csv]
           the bucket and prefix come from NATA_OBJ_BUCKET and NATA_OBJ_PREFIX (the git-ignored runtime env file of the run, shipped with the tree; never printed)
           [--store fake --fake-dir DIR --objfake target/release/nata-objfake]   a local trial against the fake endpoint with the same layouts under DIR/PREFIX
Layouts are the ones of spikes/object-store-baseline/mk-layouts.sh (sf1/big1, mid1, small1, files100; sf10/mid10, files10).
Credentials: Nata reads the VM's metadata server (default chain); DuckDB gets the same identity's bearer token as an HTTP secret."""
import atexit, argparse, csv, json, math, os, re, statistics, subprocess, sys, time, urllib.request, tempfile
from pathlib import Path

CEILING = {  # card 0006 part 3, frozen 2026-11-07 (x DuckDB median); every other cell: 1.5
    ('files100', 'meta'): 2.00, ('files100', 'sel'): 2.00, ('small1', 'full'): 1.75, ('mid10', 'proj'): 1.90,
}
CELLS = {'sf1': [('big1', 'part-0000.parquet'), ('mid1', 'part-0000.parquet'), ('small1', 'part-0000.parquet'), ('files100', '*.parquet')],
         'sf10': [('mid10', 'part-0000.parquet'), ('files10', '*.parquet')]}
LI = ['l_orderkey', 'l_partkey', 'l_suppkey', 'l_linenumber', 'l_quantity', 'l_extendedprice', 'l_discount', 'l_tax', 'l_returnflag', 'l_linestatus', 'l_shipdate', 'l_commitdate', 'l_receiptdate', 'l_shipinstruct', 'l_shipmode', 'l_comment']
def aggregate(c):
    if c.endswith('date'): return f"sum(date_diff('day', DATE '1970-01-01', {c}))"
    if c in ('l_returnflag', 'l_linestatus', 'l_shipinstruct', 'l_shipmode', 'l_comment'): return f'sum(strlen({c}))'
    return f'sum({c})'
QUERIES = {
    'full': 'SELECT count(*), ' + ', '.join(aggregate(c) for c in LI) + ' FROM {T}',
    'proj': 'SELECT sum(l_orderkey), sum(l_partkey), sum(l_suppkey), sum(l_linenumber) FROM {T}',
    'sel': 'SELECT count(*), sum(l_partkey) FROM {T} WHERE l_orderkey BETWEEN {LO} AND {HI}',
    'meta': 'SELECT count(*) FROM {T}',
}
def query(layout, q):
    sf = 10 if layout == 'mid10' or layout == 'files10' else 1
    return QUERIES[q].replace('{LO}', str(2400000 * sf)).replace('{HI}', str(2460000 * sf))

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--nata', type=Path, required=True)
ap.add_argument('--layouts', default='sf1,sf10')
ap.add_argument('--rounds', type=int, default=5)
ap.add_argument('--out', type=Path, default=Path('cells.csv'))
ap.add_argument('--store', default='gcs', choices=['gcs', 'fake'])
ap.add_argument('--fake-dir', type=Path)
ap.add_argument('--objfake', type=Path)
ap.add_argument('--duckdb', default='duckdb')
ap.add_argument('--workers', type=int, default=8)
ap.add_argument('--spike', type=Path, help='osb binary: interleave the frozen merged wrapper control')
ap.add_argument('--expected', type=Path, help='local-DuckDB expected.txt from run-reference data')
ap.add_argument('--queries', default='full,proj,sel,meta')
a = ap.parse_args()
if a.spike and not a.expected: ap.error('--spike requires --expected')
os.environ.pop('AI_AGENT', None)
bucket, prefix = os.environ.get('NATA_OBJ_BUCKET', ''), os.environ.get('NATA_OBJ_PREFIX', '')
env = dict(os.environ)
fake = None
if a.store == 'fake':
    fake = subprocess.Popen([str(a.objfake), '--dir', str(a.fake_dir), '--bucket', 'bkt'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    atexit.register(lambda: (fake.terminate(), fake.wait(timeout=5)))
    endpoint = fake.stdout.readline().strip().split('=', 1)[1]
    env.update(AWS_ENDPOINT=endpoint, AWS_ALLOW_HTTP='true', AWS_ACCESS_KEY_ID='AKIDCELLS', AWS_SECRET_ACCESS_KEY='cells-secret', AWS_REGION='us-east-1')
    bucket, prefix = 'bkt', prefix or ''
elif not bucket:
    sys.exit('NATA_OBJ_BUCKET (and NATA_OBJ_PREFIX) must be set: source the run\'s runtime env file')

def token():
    req = urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token', headers={'Metadata-Flavor': 'Google'})
    return json.load(urllib.request.urlopen(req, timeout=5))['access_token']

def uri(layout, files, scheme):
    sf = 'sf10' if layout in ('mid10', 'files10') else 'sf1'
    base = f'{bucket}/{prefix}/{sf}/{layout}'.replace('//', '/')
    return f'{scheme}://{base}/{files}'

def nata_arm(layout, files, q):
    sql = query(layout, q).replace('{T}', f"'{uri(layout, files, 's3' if a.store == 'fake' else 'gs')}'")
    t = time.perf_counter()
    r = subprocess.run([str(a.nata), 'sql', '--threads', str(a.workers), '--csv', '-c', sql], capture_output=True, text=True, env=env)
    return time.perf_counter() - t, (r.stdout if r.returncode == 0 else 'ERROR (see supported SQL and auth configuration)' )

def duck_arm(layout, files, q):
    if a.store == 'fake':
        host = env['AWS_ENDPOINT'].split('://', 1)[1]
        pre = (f"SET s3_endpoint='{host}'; SET s3_url_style='path'; SET s3_use_ssl=false; SET s3_region='us-east-1'; "
               f"CREATE OR REPLACE SECRET s (TYPE S3, KEY_ID 'AKIDCELLS', SECRET 'cells-secret', ENDPOINT '{host}', URL_STYLE 'path', USE_SSL false, REGION 'us-east-1');")
        url = uri(layout, files, 's3')
    else:
        pre = f"CREATE OR REPLACE SECRET s (TYPE http, BEARER_TOKEN '{token()}', SCOPE 'https://storage.googleapis.com/');"
        url = 'https://storage.googleapis.com/' + uri(layout, files, 'gs')[5:]
    sql = query(layout, q).replace('{T}', f"read_parquet('{url}')")
    script = f".output /dev/null\nLOAD httpfs;\nSET threads={a.workers};\n{pre}\n.output\n.timer on\n{sql};\n"
    r = subprocess.run([a.duckdb, '-bail', '-csv'], input=script, capture_output=True, text=True)
    if r.returncode != 0:
        return float('nan'), 'ERROR (see supported SQL and auth configuration)'
    m = re.search(r'Run Time \(s\): real ([0-9.]+)', r.stdout + r.stderr)
    out = '\n'.join(l for l in r.stdout.splitlines() if not l.startswith('Run Time'))
    return float(m.group(1)) if m else float('nan'), out + '\n'

def norm(text):
    from decimal import Decimal
    # Headers differ between clients; every cell's answer is numeric.
    return [None if c.strip().upper() in ('', 'NULL') else Decimal(c) for line in text.strip().splitlines()[1:] for c in line.split(',')]

def spike_arm(sf, layout, files, q):
    with tempfile.TemporaryDirectory(prefix='nata-pair-') as tmp:
        out = Path(tmp) / 'control.csv'
        env2 = dict(env, NATA_SETTLE='')
        if a.store == 'fake': env2.update(NATA_OBJ_KIND='s3', NATA_OBJ_BUCKET=bucket, NATA_OBJ_PREFIX=prefix, NATA_OBJ_ENDPOINT=env['AWS_ENDPOINT'], NATA_OBJ_REGION=env['AWS_REGION'], NATA_OBJ_ACCESS_KEY=env['AWS_ACCESS_KEY_ID'], NATA_OBJ_SECRET_KEY=env['AWS_SECRET_ACCESS_KEY'])
        r = subprocess.run([str(a.spike), 'bench', '--cells', f'{sf}/{layout}:{100 if layout == "files100" else 10 if layout == "files10" else 1}', '--queries', q, '--arms', 'wrap', '--rounds', '1', '--workers', str(a.workers), '--io-threads', '2', '--expected', str(a.expected), '--out', str(out)], capture_output=True, text=True, env=env2)
        if r.returncode != 0: raise RuntimeError('spike control failed verification or execution')
        data = list(csv.DictReader(out.open()))
        if not data or any(x['ok'] != '1' for x in data): raise RuntimeError('spike control wrong answer')
        x = next(x for x in data if x['arm'] == 'wrap' and x['rep'] == '0')
        return float(x['wall_s'])

rows, verdicts, raw = [], [], []
for sf in a.layouts.split(','):
    for layout, files in CELLS[sf]:
        for q in (q for q in a.queries.split(',') if sf == 'sf1' or q != 'meta'):
            # correctness before timing: Nata's answer must be DuckDB's
            _, want = duck_arm(layout, files, q); _, got = nata_arm(layout, files, q)
            if want.startswith('ERROR') or got.startswith('ERROR'):
                verdicts.append((f'{sf}/{layout}/{q}', 'NOT-MEASURED', 'execution failed; check supported SQL and auth configuration')); continue
            if not norm(want) or norm(want) != norm(got):
                verdicts.append((f'{sf}/{layout}/{q}', 'WRONG-ANSWER', want[:60].strip() + ' | ' + got[:60].strip())); continue
            t = {'nata': [], 'nata2': [], 'duck': [], 'duck2': []}
            if a.spike: t.update(spike=[], spike2=[])
            for round_id in range(a.rounds):
                for twin in ('', '2'):
                    elapsed, got = nata_arm(layout, files, q)
                    if got.startswith('ERROR') or norm(got) != norm(want): raise RuntimeError('timed Nata answer changed')
                    t['nata'+twin].append(elapsed)
                    if a.spike: t['spike'+twin].append(spike_arm(sf, layout, files, q))
                    elapsed, got = duck_arm(layout, files, q)
                    if got.startswith('ERROR') or norm(got) != norm(want) or not math.isfinite(elapsed) or elapsed <= 0: raise RuntimeError('timed DuckDB answer/timer invalid')
                    t['duck'+twin].append(elapsed)
                    raw.append([sf, layout, q, round_id, twin or '1', t['nata'+twin][-1], t['spike'+twin][-1] if a.spike else '', elapsed])
            med = {k: statistics.median(v) for k, v in t.items()}
            ratio = med['nata'] / med['duck']
            twin = max(med['nata'] / med['nata2'], med['nata2'] / med['nata'], med['duck'] / med['duck2'], med['duck2'] / med['duck'])
            if a.spike: twin = max(twin, med['spike']/med['spike2'], med['spike2']/med['spike'])
            ceil = CEILING.get((layout, q), 1.5)
            verdict = 'PASS' if ratio * twin <= ceil and ratio <= ceil else ('MISS' if ratio / twin > ceil else 'UNRESOLVED')
            spike_ratio = med['nata'] / med['spike'] if a.spike else None
            if a.spike and verdict == 'PASS': verdict = 'PASS' if spike_ratio*twin <= 1.2 else ('MISS' if spike_ratio/twin > 1.2 else 'UNRESOLVED')
            if a.store == 'fake': verdict = 'INDICATIVE'
            verdicts.append((f'{sf}/{layout}/{q}', verdict, f'nata {med["nata"]:.3f}s duck {med["duck"]:.3f}s ratio {ratio:.2f} (ceiling {ceil}) twin spread {twin:.2f}'))
            rows.append([sf, layout, q, a.rounds] + [f'{med[k]:.4f}' for k in t] + [f'{spike_ratio:.3f}' if a.spike else 'NOT MEASURED', f'{min(t["nata"]):.4f}', f'{min(t["duck"]):.4f}', f'{ratio:.3f}', f'{twin:.3f}', ceil, verdict])
with a.out.open('w', newline='') as f:
    w = csv.writer(f); w.writerow(['sf', 'layout', 'query', 'rounds', 'nata_med', 'nata2_med', 'duck_med', 'duck2_med'] + (['spike_med', 'spike2_med'] if a.spike else []) + ['spike_ratio', 'nata_min', 'duck_min', 'ratio', 'twin_spread', 'ceiling', 'verdict']); w.writerows(rows)
with a.out.with_suffix('.raw.csv').open('w', newline='') as f:
    w = csv.writer(f); w.writerow(['sf','layout','query','round','twin','nata_s','spike_s','duck_s']); w.writerows(raw)
for name, v, why in verdicts:
    print(f'{v:12} {name}  {why}')
if fake: fake.terminate()
print('SUMMARY', {v: sum(1 for x in verdicts if x[1] == v) for v in sorted({x[1] for x in verdicts})})
sys.exit(0 if all(v[1] in (('PASS', 'INDICATIVE') if a.store == 'fake' else ('PASS',)) for v in verdicts) else 1)
