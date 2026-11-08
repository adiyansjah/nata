#!/usr/bin/env python3
"""Package-0 2b-style off-cost twins; local evidence is always unsettled."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import subprocess
from bench_verdict import invoke, assess_off_cost

spec = importlib.util.spec_from_file_location('heavy', Path(__file__).with_name('bench-aggregate-perf1-heavy.py'))
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)


def run(a):
    for binary in (a.without, a.disabled):
        if binary.stat().st_mode & 0o222:
            raise ValueError('use immutable read-only binary copies')
    if a.settled and (os.cpu_count() != 8 or platform.machine() not in ('x86_64', 'aarch64')):
        raise ValueError('reference run requires the eight-vCPU topology')
    a.out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, TMPDIR=str(a.out.resolve()), LC_ALL='C')
    for key in ('AI_AGENT', 'CODEX_CI', 'NATA_QUERY_STATS', 'NATA_STATS_JSON', 'NATA_QUERY_TRACE'):
        env.pop(key, None)
    metadata = dict(rule='within 1±N; N=max median abs(primary/twin-1), N<=5%; median and four of five both bounds',
                    source_commit=getattr(a,'commit','working-tree'), compiled_out_cfg='nata_stats_disabled',
                    instrumentation_environment='all stats destination/enabling variables removed',
                    settled=a.settled, label='settled reference' if a.settled else 'unsettled local',
                    binaries={k:h.digest(v) for k,v in [('without',a.without),('disabled',a.disabled)]},
                    input_sha256=h.digest(a.data/'lineitem.parquet'), topology=subprocess.check_output(['lscpu'],text=True),
                    queries=a.queries, budgets=a.budgets, threads=a.threads, rounds=5)
    h.save(a.out/'metadata.json',metadata)
    samples=[]; results=[]
    for query in a.queries:
        sql=(Path(__file__).resolve().parents[1]/f'check/queries/aggregate/{query}.sql').read_text().replace('{data}',str(a.data.resolve()))
        for threads in a.threads:
            for budget in a.budgets:
                cell=a.out/f'{query}-{threads}-{budget}'; cell.mkdir()
                commands={k:h.command(v.resolve(),sql,threads,budget,cell) for k,v in [('without',a.without),('disabled',a.disabled)]}
                # Value gate before timing: both builds must match DuckDB.
                oracle=cell/'oracle.csv'
                ref=invoke(h.command('duckdb',sql,threads,8<<30,cell),env,oracle,cell/'oracle.log',600)
                assert ref['exit']==0, ref
                expected=h.rows(oracle,query)
                for label,command in commands.items():
                    actual=cell/f'{label}.csv'
                    row=invoke(command,env,actual,cell/f'gate-{label}.log',600)
                    assert row['exit']==0 and h.equal_values(expected,h.rows(actual,query),query), (label,row)
                    actual.unlink()
                oracle.unlink()
                if a.settled:
                    settled=subprocess.run(['bash','scripts/gcp/settle.sh','package0-off-cost','0.5','600'],env=env,capture_output=True,text=True)
                    (cell/'settle.txt').write_text(settled.stdout+settled.stderr)
                    if settled.returncode:
                        raise RuntimeError('off-cost did not settle')
                rows=[]
                for r in range(5):
                    order=[(v,role) for v in commands for role in ('primary','twin')]
                    for label,role in order if r%2==0 else reversed(order):
                        row=invoke(commands[label],env,cell/'output.csv',cell/f'{r}-{label}-{role}.log',600)
                        assert row['exit']==0 and row['stats'] is None, row
                        rows.append(dict(query=query,threads=threads,budget=budget,variant=label,role=role,round=r,settled=a.settled,**row))
                        samples.append(rows[-1]); h.save(a.out/'samples.json',samples)
                (cell/'output.csv').unlink()
                result=dict(query=query,threads=threads,budget=budget,**assess_off_cost(rows,'disabled','without',settled=a.settled))
                results.append(result); h.save(a.out/'summary.json',results)
                print(json.dumps(result),flush=True)
    # Unsettled evidence is an experiment, never a certification pass.
    return int(a.settled and any(r['verdict']!='MET' for r in results))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--without',type=Path,required=True)
    p.add_argument('--disabled',type=Path,required=True)
    p.add_argument('--data',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True)
    p.add_argument('--queries',nargs='+',choices=h.QUERIES,default=['q1','high_cardinality_tpch'])
    p.add_argument('--threads',nargs='+',type=int,default=[1,8])
    p.add_argument('--budgets',nargs='+',type=int,default=[4<<30,64<<20,256<<20])
    p.add_argument('--commit',default='working-tree')
    p.add_argument('--settled',action='store_true')
    a=p.parse_args()
    if any(x<=0 for x in a.threads+a.budgets): p.error('positive threads/budgets required')
    raise SystemExit(run(a))
