#!/usr/bin/env python3
"""Nata against DuckDB httpfs, cell by cell, on one bucket (card 0006, the frozen ceilings of part 3). Alternates A, B, A', B' within a round (a twin of each arm
measures the noise), checks every answer against DuckDB's before it counts as a time, and prints per-cell medians, the twin spread and the ratio to the
frozen ceiling with the verdict the rule gives. Run it on the machine type the ceilings were frozen on (n4a-highcpu-8, c3d-highcpu-8), in the same session as
the DuckDB arm; a cell whose ratio is inside the twin spread of its ceiling is reported UNRESOLVED, not PASS.

  cells.py --nata target/release/nata --layouts sf1,sf10 [--rounds 5] [--out cells.csv]
           the bucket and prefix come from NATA_OBJ_BUCKET and NATA_OBJ_PREFIX (the git-ignored gcs.env of the run, shipped with the tree; never printed)
           [--store fake --fake-dir DIR --objfake target/release/nata-objfake]   a local trial against the fake endpoint with the same layouts under DIR/PREFIX
Layouts are the ones of spikes/object-store-baseline/mk-layouts.sh (sf1/big1, mid1, small1, files100; sf10/mid10, files10).
Credentials: Nata reads the VM's metadata server (default chain); DuckDB gets the same identity's bearer token as an HTTP secret."""
import argparse, csv, json, os, re, statistics, subprocess, sys, time, urllib.request
from pathlib import Path

CEILING = {  # card 0006 part 3, frozen 2026-11-07 (x DuckDB median); every other cell: 1.5
    ('files100', 'meta'): 2.00, ('files100', 'sel'): 2.00, ('small1', 'full'): 1.75, ('mid10', 'proj'): 1.90,
}
CELLS = {'sf1': [('big1', 'part-0000.parquet'), ('mid1', 'part-0000.parquet'), ('small1', 'part-0000.parquet'), ('files100', '*.parquet')],
         'sf10': [('mid10', 'part-0000.parquet'), ('files10', '*.parquet')]}
LI = ['l_orderkey', 'l_partkey', 'l_suppkey', 'l_linenumber', 'l_quantity', 'l_extendedprice', 'l_discount', 'l_tax', 'l_shipdate', 'l_commitdate', 'l_receiptdate']
QUERIES = {
    'full': 'SELECT count(*), ' + ', '.join(f'sum({c})' for c in LI) + ' FROM {T}',
    'proj': 'SELECT sum(l_orderkey), sum(l_partkey), sum(l_suppkey), sum(l_linenumber) FROM {T}',
    'sel': 'SELECT count(*), sum(l_partkey) FROM {T} WHERE l_orderkey BETWEEN 1000000 AND 1100000',
    'meta': 'SELECT count(*) FROM {T}',
}

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--nata', type=Path, required=True)
ap.add_argument('--layouts', default='sf1,sf10')
ap.add_argument('--rounds', type=int, default=5)
ap.add_argument('--out', type=Path, default=Path('cells.csv'))
ap.add_argument('--store', default='gcs', choices=['gcs', 'fake'])
ap.add_argument('--fake-dir', type=Path)
ap.add_argument('--objfake', type=Path)
ap.add_argument('--duckdb', default='duckdb')
a = ap.parse_args()
os.environ.pop('AI_AGENT', None)
bucket, prefix = os.environ.get('NATA_OBJ_BUCKET', ''), os.environ.get('NATA_OBJ_PREFIX', '')
env = dict(os.environ)
fake = None
if a.store == 'fake':
    fake = subprocess.Popen([str(a.objfake), '--dir', str(a.fake_dir), '--bucket', 'bkt'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    endpoint = fake.stdout.readline().strip().split('=', 1)[1]
    env.update(AWS_ENDPOINT=endpoint, AWS_ALLOW_HTTP='true', AWS_ACCESS_KEY_ID='AKIDCELLS', AWS_SECRET_ACCESS_KEY='cells-secret', AWS_REGION='us-east-1')
    bucket, prefix = 'bkt', prefix or ''
elif not bucket:
    sys.exit('NATA_OBJ_BUCKET (and NATA_OBJ_PREFIX) must be set: source the run\'s gcs.env')

def token():
    req = urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token', headers={'Metadata-Flavor': 'Google'})
    return json.load(urllib.request.urlopen(req, timeout=5))['access_token']

def uri(layout, files, scheme):
    base = f'{bucket}/{prefix}/{layout}'.replace('//', '/')
    return f'{scheme}://{base}/{files}'

def nata_arm(layout, files, q):
    sql = QUERIES[q].replace('{T}', f"'{uri(layout, files, 's3' if a.store == 'fake' else 'gs')}'")
    t = time.perf_counter()
    r = subprocess.run([str(a.nata), 'sql', '--csv', '-c', sql], capture_output=True, text=True, env=env)
    return time.perf_counter() - t, (r.stdout if r.returncode == 0 else 'ERROR ' + r.stderr[:120])

def duck_arm(layout, files, q):
    if a.store == 'fake':
        host = env['AWS_ENDPOINT'].split('://', 1)[1]
        pre = (f"SET s3_endpoint='{host}'; SET s3_url_style='path'; SET s3_use_ssl=false; SET s3_region='us-east-1'; "
               f"CREATE OR REPLACE SECRET s (TYPE S3, KEY_ID 'AKIDCELLS', SECRET 'cells-secret', ENDPOINT '{host}', URL_STYLE 'path', USE_SSL false, REGION 'us-east-1');")
        url = uri(layout, files, 's3')
    else:
        pre = f"CREATE OR REPLACE SECRET s (TYPE http, BEARER_TOKEN '{token()}', SCOPE 'https://storage.googleapis.com/');"
        url = 'https://storage.googleapis.com/' + uri(layout, files, 'gs')[5:]
    sql = QUERIES[q].replace('{T}', f"read_parquet('{url}')")
    script = f"LOAD httpfs; .output /dev/null\n{pre}\n.output\n.timer on\n{sql};\n"
    r = subprocess.run([a.duckdb, '-csv'], input=script, capture_output=True, text=True)
    if r.returncode != 0:
        return float('nan'), 'ERROR ' + r.stderr[:120]
    m = re.search(r'Run Time \(s\): real ([0-9.]+)', r.stdout + r.stderr)
    out = '\n'.join(l for l in r.stdout.splitlines() if not l.startswith('Run Time'))
    return float(m.group(1)) if m else float('nan'), out + '\n'

def norm(text):
    return [re.sub(r'(\d+\.\d*?)0+$', r'\1', c) for line in text.strip().splitlines() for c in line.split(',')]

rows, verdicts = [], []
for sf in a.layouts.split(','):
    for layout, files in CELLS[sf]:
        for q in (QUERIES if sf == 'sf1' else ['full', 'proj', 'sel']):
            # correctness before timing: Nata's answer must be DuckDB's
            _, want = duck_arm(layout, files, q); _, got = nata_arm(layout, files, q)
            if want.startswith('ERROR') or got.startswith('ERROR') or norm(want) != norm(got):
                verdicts.append((f'{sf}/{layout}/{q}', 'WRONG-ANSWER', want[:60].strip() + ' | ' + got[:60].strip())); continue
            t = {'nata': [], 'nata2': [], 'duck': [], 'duck2': []}
            for _ in range(a.rounds):  # A, B, A', B' each round
                t['nata'].append(nata_arm(layout, files, q)[0]); t['duck'].append(duck_arm(layout, files, q)[0])
                t['nata2'].append(nata_arm(layout, files, q)[0]); t['duck2'].append(duck_arm(layout, files, q)[0])
            med = {k: statistics.median(v) for k, v in t.items()}
            ratio = med['nata'] / med['duck']
            twin = max(med['nata'] / med['nata2'], med['nata2'] / med['nata'], med['duck'] / med['duck2'], med['duck2'] / med['duck'])
            ceil = CEILING.get((layout, q), 1.5)
            verdict = 'PASS' if ratio * twin <= ceil and ratio <= ceil else ('MISS' if ratio / twin > ceil else 'UNRESOLVED')
            verdicts.append((f'{sf}/{layout}/{q}', verdict, f'nata {med["nata"]:.3f}s duck {med["duck"]:.3f}s ratio {ratio:.2f} (ceiling {ceil}) twin spread {twin:.2f}'))
            rows.append([sf, layout, q, a.rounds] + [f'{med[k]:.4f}' for k in t] + [f'{min(t["nata"]):.4f}', f'{min(t["duck"]):.4f}', f'{ratio:.3f}', f'{twin:.3f}', ceil, verdict])
with a.out.open('w', newline='') as f:
    w = csv.writer(f); w.writerow(['sf', 'layout', 'query', 'rounds', 'nata_med', 'nata2_med', 'duck_med', 'duck2_med', 'nata_min', 'duck_min', 'ratio', 'twin_spread', 'ceiling', 'verdict']); w.writerows(rows)
for name, v, why in verdicts:
    print(f'{v:12} {name}  {why}')
if fake: fake.terminate()
print('SUMMARY', {v: sum(1 for x in verdicts if x[1] == v) for v in sorted({x[1] for x in verdicts})})
sys.exit(0 if all(v[1] in ('PASS',) for v in verdicts) else 1)
