#!/usr/bin/env python3
"""Slice 4 clean-VM benchmark. Five alternating rounds, warm OS cache.
Timing failures abort; threshold failures are recorded, never re-thresholded.
Large fixtures live in WORK, never in the pulled results directory.
"""
import csv
import os
from pathlib import Path
import re
import resource
import shutil
import statistics as st
import subprocess as sp
import sys
import time
import duckdb

os.environ.pop('AI_AGENT', None)
nata, planner = map(lambda x: str(Path(x).resolve()), sys.argv[1:3])
sf1, sf10, work, out = map(lambda x: Path(x).resolve(), sys.argv[3:])
work.mkdir(parents=True, exist_ok=True)
out.mkdir(parents=True, exist_ok=True)
# A preempted step can restart against its old scratch/results directory.
shutil.rmtree(work/'small', ignore_errors=True)
(work/'sorted.parquet').unlink(missing_ok=True)
for name in ['settle.txt','reports.txt']:
    (out/name).write_text('')
root = Path(__file__).resolve().parents[2]
con = duckdb.connect()
con.execute('SET threads=1')
rows = []
fields = ['block','arm','threads','round','secs','peak_rss_kib','peak_open_files','peak_footers']
with (out/'runs.csv').open('w') as f:
    csv.DictWriter(f,fieldnames=fields).writeheader()
def record(row):
    rows.append(row)
    with (out/'runs.csv').open('a') as f:
        csv.DictWriter(f,fieldnames=fields).writerow(row)
settled = True

def settle(label):
    global settled
    p = sp.run([str(root/'scripts/gcp/settle.sh'), label, '0.5', '600'], text=True, stdout=sp.PIPE)
    with (out/'settle.txt').open('a') as f:
        f.write(p.stdout)
    settled &= p.returncode == 0
    return p.returncode == 0

def run(cmd, capture=False):
    p = sp.run(cmd, stdout=sp.PIPE if capture else sp.DEVNULL, stderr=sp.PIPE, timeout=300)
    if p.returncode:
        raise RuntimeError(f'command failed {p.returncode}: {p.stderr.decode()[:2000]}')
    return p

def sample(block, arm, threads, round_, cmd):
    start = time.perf_counter()
    p = run(cmd)
    secs = time.perf_counter()-start
    report = dict(re.findall(r'(\w+)=([0-9]+)', p.stderr.decode()))
    with (out/'reports.txt').open('a') as f:
        f.write(f'{block}/{arm}/{threads}/{round_}: {p.stderr.decode()}\n')
    record(dict(block=block, arm=arm, threads=threads, round=round_, secs=secs,
                     peak_rss_kib=report.get('peak_rss_kib', ''),
                     peak_open_files=report.get('peak_open_files',''),
                     peak_footers=report.get('peak_footers','')))

def sqlcmd(q,t):
    return [nata,'sql','--csv','--report','--threads',str(t),'--window',str(2*t),'-c',q]

# The documented example and same physical columns + exact predicates in the scan arm.
predicate = "l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07"
def example(f):
    return f"SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS net FROM '{f}' WHERE {predicate} LIMIT 10"

# Every query is timed, including the parse/bind rejection path. Nothing is skipped.
corpus = []
for directory in ['sql', 'sql-differences']:
    for path in sorted((root/'check/queries'/directory).glob('*.sql')):
        q = path.read_text().replace('{data}',str(sf1))
        corpus.append((directory+'-'+path.stem,q))
corpus.append(('card-example',example(sf1/'lineitem.parquet')))
def duckplan(q):
    try:
        con.execute('EXPLAIN '+q).fetchall()
    except duckdb.Error:
        pass  # Expected rejection paths are timed too.
settle('planning')
for name,q in corpus:
    qp=work/'plan.sql'; qp.write_text(q)
    duckplan(q)
    for r in range(5):
        vals=list(map(float,run([planner,str(qp)],True).stdout.split()))
        record(dict(block='plan-'+name,arm='nata',threads=1,round=r,secs=st.median(vals)))
        vals=[]
        for _ in range(20):
            start=time.perf_counter(); duckplan(q); vals.append(time.perf_counter()-start)
        record(dict(block='plan-'+name,arm='duckdb',threads=1,round=r,secs=st.median(vals)))
        vals=list(map(float,run([planner,str(qp)],True).stdout.split()))
        record(dict(block='plan-'+name,arm='twin',threads=1,round=r,secs=st.median(vals)))

for sf,d in [('sf1',sf1),('sf10',sf10)]:
    f=d/'lineitem.parquet'; q=example(f)
    # Correctness before timing, headers excluded (different expression naming).
    got=run(sqlcmd(q,1),True).stdout.decode().splitlines()[1:]
    want=sp.check_output(['duckdb','-csv','-c',q],text=True).splitlines()[1:]
    assert got == want, (sf,'example differs from DuckDB')
    for t in [1,8]:
        scan=[nata,'scan',str(f),'--csv','--limit','10','--columns','l_orderkey,l_extendedprice,l_discount,l_shipdate',
              '--threads',str(t),'--window',str(2*t),'--filter','l_shipdate >= 1994-01-01',
              '--filter','l_shipdate < 1995-01-01','--filter','l_discount >= 0.05','--filter','l_discount <= 0.07']
        # Same LIMIT, physical columns, pushed filter and CSV output; SQL adds net arithmetic.
        cmds={'sql':sqlcmd(q,t),'scan':scan,'twin':scan,
              'duckdb':['duckdb','-csv','-c',f'SET threads={t}; '+q]}
        for cmd in cmds.values(): run(cmd)
        settle(f'{sf}-t{t}')
        for r in range(5):
            for arm,cmd in cmds.items(): sample(sf,arm,t,r,cmd)

# Full SF10 SQL execution for scaling, with and without per-row CSV rendering.
q=f"SELECT l_orderkey FROM '{sf10}/lineitem.parquet'"
for t in [1,8]:
    run(sqlcmd('EXPLAIN ANALYZE '+q,t))
    settle(f'scale-t{t}')
    for r in range(5):
        sample('scale-sf10','sql',t,r,sqlcmd('EXPLAIN ANALYZE '+q,t))
        sample('scale-sf10','twin',t,r,sqlcmd('EXPLAIN ANALYZE '+q,t))
        sample('scale-rendered-sf10','sql',t,r,sqlcmd(q,t))
# Startup probes: no file access or row rendering. Process wall time, same timer.
settle('startup')
for r in range(5):
    sample('startup','nata',1,r,[nata,'--version'])
    sample('startup','duckdb',1,r,['duckdb','-csv','-c','SELECT 1 WHERE false'])

# The card's R2 pruning check uses the sorted SF1 layout, as slice 3 did.
sorted_file=work/'sorted.parquet'
con.execute(f"COPY (SELECT * FROM '{sf1}/lineitem.parquet' ORDER BY l_shipdate) TO '{sorted_file}' (FORMAT parquet, ROW_GROUP_SIZE 122880)")
q=example(sorted_file)
assert run(sqlcmd(q,8),True).stdout.decode().splitlines()[1:] == sp.check_output(['duckdb','-csv','-c',q],text=True).splitlines()[1:]
sample('sorted-r2','sql',8,0,sqlcmd(q,8))

# Resource tests use default fresh-machine descriptor limit, never raise it.
(out/'limits.txt').write_text(f'default RLIMIT_NOFILE={resource.getrlimit(resource.RLIMIT_NOFILE)}\n')
small=work/'small'; small.mkdir(exist_ok=True)
seed=small/'00000.parquet'
con.execute(f"COPY (SELECT l_orderkey FROM '{sf1}/lineitem.parquet' LIMIT 1) TO '{seed}' (FORMAT parquet)")
for i in range(1,10000): os.link(seed,small/f'{i:05}.parquet')
q=f"SELECT l_orderkey FROM '{small}/*.parquet'"
assert len(run(sqlcmd(q,8),True).stdout.splitlines()) == 10001
sample('files10000','sql',8,0,sqlcmd(q,8))
sample('files10000','scan',8,0,[nata,'scan',*map(str,sorted(small.glob('*.parquet'))),'--columns','l_orderkey','--csv','--threads','8','--window','16'])
# Full SF10 scan through SQL; output cost included. No materialization in driver.
q=f"SELECT l_orderkey FROM '{sf10}/lineitem.parquet'"
sample('resource-sf10','sql',8,0,sqlcmd(q,8))
sample('resource-sf10','scan',8,0,[nata,'scan',str(sf10/'lineitem.parquet'),'--columns','l_orderkey','--csv','--threads','8','--window','16'])
with (out/'runs.csv').open('w') as f:
    w=csv.DictWriter(f,fieldnames=['block','arm','threads','round','secs','peak_rss_kib','peak_open_files','peak_footers'])
    w.writeheader(); w.writerows(rows)
g={}
for r in rows: g.setdefault((r['block'],r['arm'],r['threads']),[]).append(r['secs'])
med={k:st.median(v) for k,v in g.items()}
with (out/'medians.csv').open('w') as f:
    w=csv.writer(f);w.writerow(['metric','median_secs'])
    for k,v in sorted(med.items()):w.writerow(['/'.join(map(str,k)),v])
with (out/'summary.md').open('w') as f:
    f.write(f'# Slice 4 SQL benchmark\n\nSettled all blocks: {settled}. Planning corpus: {len(corpus)} queries (including rejection paths).\n')
    f.write('Warm OS cache; process timing includes startup, metadata, output. Nata planning includes parser thread startup and footer lookup; DuckDB EXPLAIN in-process includes plan rendering. Both example arms stop at 10 and render CSV; SQL adds the net expression. Startup probes are separate, not subtracted.\n\n')
    f.write('| metric | min s | median s | max s | verdict |\n|---|---:|---:|---:|---|\n')
    for k,v in sorted(g.items()):
        verdict=''
        if k[0].startswith('plan-') and k[1]=='nata': verdict='MET' if med[k]<.001 and settled else 'NOT MET'
        f.write(f'| {"/".join(map(str,k))} | {min(v):.9f} | {med[k]:.9f} | {max(v):.9f} | {verdict} |\n')
    for sf in ['sf1','sf10']:
        for t in [1,8]:
            ratio=med[sf,'sql',t]/med[sf,'scan',t]
            twin=med[sf,'twin',t]/med[sf,'scan',t]
            f.write(f'\n{sf} threads {t}: SQL/scan {ratio:.4f} (threshold <=1.2): {"MET" if ratio<=1.2 and settled else "NOT MET"}; twin/scan {twin:.4f}.\n')
    ratio=med['scale-sf10','sql',1]/med['scale-sf10','sql',8]
    f.write(f'\nSF10 full SQL speedup {ratio:.4f} (threshold >=5): {"MET" if ratio>=5 and settled else "NOT MET"}. EXPLAIN ANALYZE executes every row without rendering; rendered SQL is also measured.\n')
    for block in ['files10000','resource-sf10']:
        r=next(r for r in rows if r['block']==block)
        f.write(f'\n{block}: {r}\n')
if os.environ.get('REGRESSION_BASELINE'):
    sp.run([str(root/'crates/nata-bench/regress.sh'),os.environ['REGRESSION_BASELINE'],str(out/'medians.csv')],check=True)
