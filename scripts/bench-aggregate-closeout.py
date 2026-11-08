#!/usr/bin/env python3
"""Frozen slice 5 matrix. No provisioning; full CSV, warm cache, alternating twins.

Reuse the settled runner's commands, numeric gates and distributions. Large
multisets use external sort, retaining exact numeric values and multiplicity.
"""
import argparse
import csv
import decimal
import filecmp
import importlib.util
import json
import os
from pathlib import Path
import platform
import resource
import signal
import subprocess as sp
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('settled', ROOT/'scripts/bench-aggregate-perf1-heavy.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
MIB, GIB = 1 << 20, 1 << 30
QUERIES = (*h.QUERIES, 'agg_orderkey', 'agg_partkey')


def cells(scale):
    if scale == 100:
        return [('q1',t,32*GIB) for t in (8,16,32)] + [
            ('high_cardinality_tpch',8,b) for b in (32*GIB,4*GIB)]
    return [(q,t,b) for q in QUERIES for t in (1,4,8)
            for b in (4*GIB,64*MIB,256*MIB)]


def repair_cells(scale):
    # Original cells/queries/budgets only: repair short timers and the 4 GiB
    # boundary, without substituting the 8 GiB mechanism control.
    return [cell for cell in cells(scale)
            if (scale < 100 and cell[0] == 'q1') or cell[2] == 4*GIB]


def sql(query, data):
    if query.startswith('agg_'):
        body = (ROOT/f'spikes/hash-ops/sql/{query}.sql').read_text()
        return body.replace('from lineitem', "from '"+str(data/'lineitem.parquet').replace("'", "''")+"'")
    return (ROOT/f'check/queries/aggregate/{query}.sql').read_text().replace('{data}', str(data).replace("'", "''"))


def canonical(source, query, scratch, name='actual'):
    if query == 'q1':
        return h.rows(source, query)
    raw, dest = scratch/'unsorted', scratch/(name+'.sorted')
    count = 0
    with source.open(newline='') as f, raw.open('w') as out:
        reader = csv.reader(f)
        if len(next(reader)) != 4:
            raise ValueError('unexpected header')
        for row in reader:
            if len(row) != 4:
                raise ValueError('unexpected row width')
            out.write(','.join(str(decimal.Decimal(x).normalize()) for x in row)+'\n')
            count += 1
    with dest.open('w') as out:
        sp.run(['sort','-T',str(scratch),'-S','128M',str(raw)], stdout=out,
               env=dict(os.environ, LC_ALL='C'), check=True)
    raw.unlink()
    result = dict(rows=count, sha256=h.digest(dest), path=dest)
    return result


def invoke(argv, env, output, log, timeout):
    rss = log.with_suffix('.usage')
    load = os.getloadavg()
    start = time.perf_counter_ns()
    with output.open('w') as sink:
        try:
            p = sp.Popen(['/usr/bin/time','-f','%M %U %S','-o',str(rss),*map(str,argv)],
                         env=env, stdout=sink, stderr=sp.PIPE, start_new_session=True)
            # Pipe EOF reports completion without wait(timeout)'s 50 ms polling
            # grid. Keep full CSV output and the process-group safety deadline.
            _, stderr = p.communicate(timeout=timeout)
            rc = p.returncode
        except sp.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            _, stderr = p.communicate()
            rc = 124
    elapsed = (time.perf_counter_ns()-start)/1e9
    log.write_bytes(stderr)
    metrics = {}
    for line in log.read_text().splitlines():
        if line.startswith('aggregate:'):
            metrics = {k:int(v) for k,v in (s.split('=') for s in line.split()[1:])}
    usage = rss.read_text().splitlines()[-1].split() if rss.exists() and rss.stat().st_size else []
    return dict(exit=rc, seconds=elapsed, load=load,
                rss_bytes=int(usage[0])*1024 if len(usage)==3 else None,
                user_seconds=float(usage[1]) if len(usage)==3 else None,
                system_seconds=float(usage[2]) if len(usage)==3 else None,
                output_bytes=output.stat().st_size, counters=metrics)


def resource_gate(row, budget, baseline, disk):
    m = row['counters']
    if row['exit'] or not m or m['peak_reservation'] > budget:
        raise RuntimeError('completion/reservation failure')
    if row['rss_bytes'] is None or row['rss_bytes'] > baseline+budget+32*MIB:
        raise RuntimeError('RSS exceeds baseline+B+32MiB (stacks included)')
    if m['spill_peak_open'] > 32 or m['spill_peak_bytes'] > disk:
        raise RuntimeError('FD/disk bound failure')


def summarize(samples, complete_reference, settled):
    result = dict(verdict='UNRESOLVED' if complete_reference else 'UNASSESSED', arms={})
    # No numeric twin cutoff is frozen. Retain all evidence for the parent's
    # noise judgment; an automated pass would invent a rule. UNSETTLED smoke
    # and failed references can never be promoted to MET.
    for v in ('main','duckdb'):
        for role in ('primary','twin'):
            rows=[s for s in samples if (s['variant'],s['role'])==(v,role)]
            if len(rows)==5 and all(s['exit']==0 for s in rows):
                result['arms'][v+'-'+role]=h.distribution([s['seconds'] for s in rows])
    result['twin_ratios']={}
    for v in ('main','duckdb'):
        pairs=[[s for s in samples if (s['variant'],s['role'])==(v,role)] for role in ('primary','twin')]
        if all(len(rows)==5 and all(s['exit']==0 for s in rows) for rows in pairs):
            result['twin_ratios'][v]=h.distribution([t['seconds']/p['seconds'] for p,t in zip(*pairs)])
    if settled and all(k in result['arms'] for k in ('main-primary','duckdb-primary')):
        result['paired_latency_ratio']=h.distribution([n['seconds']/d['seconds'] for n,d in zip(
            [s for s in samples if (s['variant'],s['role'])==('main','primary')],
            [s for s in samples if (s['variant'],s['role'])==('duckdb','primary')])])
    return result


def run(a):
    decimal.getcontext().prec = 80
    a.out = a.out.resolve(); a.data = a.data.resolve(); a.binary = a.binary.resolve()
    a.out.mkdir(parents=True, exist_ok=False)
    if a.binary.stat().st_mode & 0o222:
        raise ValueError('use a read-only immutable binary copy')
    matrix = repair_cells(a.scale) if getattr(a, 'repair_rerun', False) else cells(a.scale)
    if a.resource_only:
        matrix=[('high_cardinality_tpch',8,{1:64*MIB,10:256*MIB,100:4*GIB}[a.scale])]
    expected = {'c3d-highcpu-8':('x86_64',8), 'n4a-highcpu-8':('aarch64',8),
                'c3d-highcpu-30':('x86_64',30), 'n4a-highcpu-32':('aarch64',32)}[a.machine]
    if not a.smoke:
        if (platform.machine(),os.cpu_count()) != expected or (a.scale==100)!=(expected[1]>8):
            raise ValueError('wrong machine topology for frozen cells')
        if a.machine=='c3d-highcpu-30':
            matrix=[(q,30 if t==32 else t,b) for q,t,b in matrix]
    env = dict(os.environ, TMPDIR=str(a.out), LC_ALL='C')
    for key in ('AI_AGENT','CODEX_CI','CLAUDECODE','CODEX_THREAD_ID'): env.pop(key,None)
    resource.setrlimit(resource.RLIMIT_NOFILE, (a.fd_limit,resource.getrlimit(resource.RLIMIT_NOFILE)[1]))
    metadata = dict(commit=a.commit,binary_sha256=h.digest(a.binary),machine=a.machine,
                    topology=sp.check_output(['lscpu'],text=True),matrix=matrix,scale=a.scale,
                    duckdb=sp.check_output(['duckdb','--version'],env=env,text=True),
                    input_bytes=(a.data/'lineitem.parquet').stat().st_size,
                    input_sha256=h.digest(a.data/'lineitem.parquet'),
                    fd_limit=a.fd_limit,unsettled=a.smoke,rounds=1 if a.smoke else 5,
                    queries={q:sql(q,a.data) for q in QUERIES},
                    diagnostic='SKIPPED: missing task admission, per-level frames/opens/tickets, exclusive teardown and timestamped I/O queue/busy by job kind; requires cross-layer tracing beyond small counters. output_ns is exclusive callback wall time only.',
                    resource_rule='baseline+B+32MiB; stacks included; FD<=32; explicit D; zero residual spill files')
    count=sp.check_output(['duckdb','-csv','-c',f"SELECT count(*) FROM '{a.data}/lineitem.parquet';"],env=env,text=True)
    metadata['input_rows']=int(list(csv.reader(count.splitlines()))[1][0])
    if not a.smoke and metadata['input_rows']!={1:6001215,10:59986052,100:600037902}[a.scale]:
        raise ValueError('input scale does not match frozen matrix')
    h.save(a.out/'metadata.json',metadata)
    observations=[]; summaries=[]; baseline={}; failed=False
    for i,(query,threads,budget) in enumerate(matrix):
        body=sql(query,a.data); scratch=a.out/f'cell-{i}';scratch.mkdir()
        spill=scratch/'spill';spill.mkdir()
        for variant in ('main','duckdb'): (spill/variant).mkdir()
        disk=(128 if a.scale==100 else 32)*GIB
        def command(variant, cap=budget, query_sql=body):
            argv=h.command('duckdb' if variant=='duckdb' else a.binary,query_sql,threads,cap,spill/variant)
            if variant=='main': argv[argv.index('--spill-disk-bytes')+1]=str(disk)
            return argv
        if threads not in baseline:
            probes=[]
            for r in range(1 if a.smoke else 5):
                p=invoke(command('main',query_sql=f"SELECT l_orderkey FROM '{a.data}/lineitem.parquet' LIMIT 0"),env,scratch/'baseline.csv',scratch/f'baseline-{r}.log',a.timeout)
                if p['exit'] or p['rss_bytes'] is None: raise RuntimeError('baseline failed')
                probes.append(p['rss_bytes'])
            baseline[threads]=max(probes)
            h.save(a.out/'baselines.json',baseline)
        output=scratch/'output.csv'
        oracle=invoke(command('duckdb',cap=(32 if a.scale==100 else 4)*GIB),env,output,scratch/'oracle.log',a.timeout)
        if oracle['exit']: raise RuntimeError('unrestricted value oracle failed; stop')
        expected_values=canonical(output,query,scratch,'expected')
        gates=[]
        for variant in ('main','duckdb'):
            row=invoke(command(variant),env,output,scratch/f'gate-{variant}.log',a.timeout)
            if row['exit']==0:
                actual=canonical(output,query,scratch)
                equal=h.equal_values(expected_values,actual,'q1') if query=='q1' else expected_values['rows']==actual['rows'] and filecmp.cmp(expected_values['path'],actual['path'],shallow=False)
                if not equal: raise RuntimeError(f'value gate failed: {query}/{threads}/{budget}/{variant}')
                row['result_rows']=sum(expected_values.values()) if query=='q1' else expected_values['rows']
                if variant=='main':
                    try:
                        resource_gate(row,budget,baseline[threads],disk)
                    except RuntimeError as error:
                        row['error']=str(error)
                    if a.resource_only and not row.get('error') and row['counters']['partitions_spilled']<=a.fd_limit:
                        raise RuntimeError('low-FD fixture did not exceed the process descriptor limit; UNRESOLVED')
            elif variant=='main':
                row['error']=f"completion gate failed (exit {row['exit']}): "+(scratch/'gate-main.log').read_text()
            gates.append(dict(variant=variant,**row))
        h.save(scratch/'gates.json',gates)
        if gates[0].get('error'):
            summary=dict(query=query,threads=threads,budget=budget,verdict='FAIL',
                         error=gates[0]['error'],gates=str(scratch/'gates.json'))
            summaries.append(summary);h.save(a.out/'summary.json',summaries)
            failed=True
            print('completed',query,threads,budget,'FAIL',summary['error'],flush=True)
            output.unlink()
            for sorted_file in scratch.glob('*.sorted'): sorted_file.unlink()
            continue
        settled=False
        if not a.smoke:
            result=sp.run(['bash',str(ROOT/'scripts/gcp/settle.sh'),f'closeout-{i}','0.5','600'],env=env,capture_output=True,text=True)
            (scratch/'settle.txt').write_text(result.stdout+result.stderr)
            settled=result.returncode==0
            if not settled: raise RuntimeError('settle failed; UNRESOLVED, stop')
        samples=[]; timing_error=None
        for r in range(1 if a.smoke else 5):
            for variant,role in h.arms([{'label':'main'}],r):
                if variant=='duckdb' and gates[1]['exit']: continue
                row=invoke(command(variant),env,output,scratch/f'{r}-{variant}-{role}.log',a.timeout)
                row['gated_result_rows']=sum(expected_values.values()) if query=='q1' else expected_values['rows']
                if variant=='main':
                    try:
                        resource_gate(row,budget,baseline[threads],disk)
                        if list((spill/'main').iterdir()): raise RuntimeError('spill cleanup failure')
                        required={1:64*MIB,10:256*MIB,100:4*GIB}[a.scale]
                        if not a.smoke and query=='high_cardinality_tpch' and budget==required and not row['counters']['spill_bytes_written']:
                            raise RuntimeError('required HC spill absent')
                    except RuntimeError as error:
                        timing_error=str(error)+': '+(scratch/f'{r}-{variant}-{role}.log').read_text()
                        row['error']=timing_error
                sample=dict(query=query,threads=threads,budget=budget,round=r,variant=variant,role=role,settled=settled,**row)
                samples.append(sample);observations.append(sample)
                h.save(a.out/'timings.json',observations)
                if timing_error: break
            if timing_error: break
        output.unlink()
        for sorted_file in scratch.glob('*.sorted'): sorted_file.unlink()
        complete=gates[1]['exit']==0 and all(s['exit']==0 for s in samples if s['variant']=='duckdb')
        summary=dict(query=query,threads=threads,budget=budget,
                     target_latency_ratio=(1.5 if query=='q1' else 2.0) if a.scale<100 and budget==4*GIB and query!='high_cardinality_tpch' else (2.0 if gates[0]['counters']['spill_bytes_written'] else None),
                     **summarize(samples,complete,settled))
        if timing_error:
            summary.update(verdict='FAIL',error=timing_error)
            failed=True
        summaries.append(summary);h.save(a.out/'summary.json',summaries)
        print('completed',query,threads,budget,summary['verdict'],flush=True)
    scaling=[]
    if a.scale in (10,100):
        low,high=(1,8) if a.scale==10 else (8,30 if a.machine=='c3d-highcpu-30' and not a.smoke else 32)
        for variant in ('main','duckdb'):
            for role in ('primary','twin'):
                values=[[s for s in observations if s['query']=='q1' and s['budget']==(32 if a.scale==100 else 4)*GIB and s['threads']==t and s['variant']==variant and s['role']==role] for t in (low,high)]
                if all(len(v)==5 and all(s['exit']==0 for s in v) for v in values):
                    scaling.append(dict(variant=variant,role=role,low=low,high=high,speedup=h.distribution([x['seconds']/y['seconds'] for x,y in zip(*values)]),verdict='UNRESOLVED',target=(5 if a.machine.startswith('n4a') else 3.5) if a.scale==10 else (2.5 if a.machine.startswith('n4a') else 'REPORT ONLY')))
        h.save(a.out/'scaling.json',scaling)

    return 1 if failed else 0


def self_test():
    assert len(cells(1))==36 and len(cells(10))==36 and len(cells(100))==5
    assert len(repair_cells(1)) == len(repair_cells(10)) == 18
    assert repair_cells(100) == [('high_cardinality_tpch', 8, 4*GIB)]
    assert set(repair_cells(1)) <= set(cells(1))
    assert all(c in repair_cells(1) for c in cells(1) if c[0] == 'q1' or c[2] == 4*GIB)
    assert {c[0] for c in cells(1)}==set(QUERIES)
    assert {c[1] for c in cells(1)}=={1,4,8}
    assert summarize([],False,True)['verdict']=='UNASSESSED'
    assert summarize([],True,False)['verdict']=='UNRESOLVED'
    samples=[dict(variant=v,role=role,exit=0,seconds=1) for _ in range(5) for v in ('main','duckdb') for role in ('primary','twin')]
    assert summarize(samples,True,True)['verdict']=='UNRESOLVED'
    assert summarize(samples,True,True)['paired_latency_ratio']['median']==1
    import contextlib
    import io
    import tempfile
    from unittest.mock import patch
    with tempfile.TemporaryDirectory() as directory:
        base=Path(directory); binary=base/'nata';binary.write_bytes(b'inert');binary.chmod(0o444)
        data=base/'data';data.mkdir();(data/'lineitem.parquet').write_bytes(b'inert')
        calls=[]
        fail_reference=[False]; fail_candidate=[False]; fail_timing=[False]
        def fake(argv, env, output, log, timeout):
            calls.append(argv)
            width=10 if 'sum_charge' in argv[-1] else 4
            output.write_text(','.join('c'+str(i) for i in range(width))+'\n')
            log.write_text('inert process\n')
            return dict(exit=1 if (fail_timing[0] and log.name=='0-main-primary.log') or (fail_reference[0] and argv[0]=='duckdb' and '67108864B' in argv[3]) or (fail_candidate[0] and argv[0]!='duckdb' and '--aggregate-memory-bytes' in argv and argv[argv.index('--aggregate-memory-bytes')+1]=='67108864' and 'LIMIT 0' not in argv[-1]) else 0,seconds=1,rss_bytes=1,counters=dict(peak_reservation=1,spill_peak_bytes=0,spill_peak_open=0,spill_bytes_written=0,partitions_spilled=256))
        a=argparse.Namespace(out=base/'out',binary=binary,data=data,scale=1,smoke=True,resource_only=False,fd_limit=1024,machine='c3d-highcpu-8',commit='inert',timeout=5)
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            run(a)
        assert len(calls)==255
        left=base/'left.csv';right=base/'right.csv'
        left.write_text('a,b,c,d\n1,2.00,3,4\n1,2,3.0,4\n')
        right.write_text('a,b,c,d\n1.0,2,3,4.000\n1,2,3,4\n')
        x=canonical(left,'agg_orderkey',base,'left')
        y=canonical(right,'agg_orderkey',base,'right')
        assert x['rows']==y['rows']==2 and filecmp.cmp(x['path'],y['path'],shallow=False)
        right.write_text('a,b,c,d\n1,2,3,4\n')
        y=canonical(right,'agg_orderkey',base,'right')
        assert x['rows']!=y['rows'] and not filecmp.cmp(x['path'],y['path'],shallow=False)
        saved=json.loads((a.out/'timings.json').read_text())
        assert len(saved)==144 and all(not row['settled'] for row in saved)
        summaries=json.loads((a.out/'summary.json').read_text())
        assert len(summaries)==36
        assert all(row['target_latency_ratio'] is None for row in summaries if row['query']=='high_cardinality_tpch' and row['budget']==4*GIB)  # HC has a spill latency rule, not a newly invented resident threshold
        a.out=base/'repair-out'; a.repair_rerun=True
        calls.clear()
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            assert run(a)==0
        assert len(calls)==129
        assert len(json.loads((a.out/'summary.json').read_text()))==18
        assert len(json.loads((a.out/'timings.json').read_text()))==72
        a.out=base/'repair-scale-out'; a.scale=100
        calls.clear()
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            assert run(a)==0
        assert len(calls)==8
        assert len(json.loads((a.out/'summary.json').read_text()))==1
        assert len(json.loads((a.out/'timings.json').read_text()))==4
        a.repair_rerun=False; a.scale=1
        a.out=base/'failed-reference-out';fail_reference[0]=True
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            run(a)
        results=json.loads((a.out/'summary.json').read_text())
        assert sum(row['verdict']=='UNASSESSED' for row in results)==12
        a.out=base/'failed-candidate-out';fail_candidate[0]=True;fail_reference[0]=False
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            assert run(a)==1
        results=json.loads((a.out/'summary.json').read_text())
        assert len(results)==36 and sum(row['verdict']=='FAIL' for row in results)==12
        assert all('inert process' in row['error'] for row in results if row['verdict']=='FAIL')
        assert len(json.loads((a.out/'cell-1/gates.json').read_text()))==2
        assert results[-1]['budget']==256*MIB  # failed cells did not abort the matrix
        a.out=base/'failed-timing-out';fail_candidate[0]=False;fail_timing[0]=True
        with patch(__name__+'.invoke',fake), patch.object(sp,'check_output',return_value='n\n1\n'), contextlib.redirect_stdout(io.StringIO()):
            assert run(a)==1
        results=json.loads((a.out/'summary.json').read_text())
        assert len(results)==36 and all(row['verdict']=='FAIL' for row in results)
        assert all('inert process' in row['error'] for row in results)
        bad=dict(exit=0,rss_bytes=1,counters=dict(peak_reservation=2,spill_peak_bytes=0,spill_peak_open=0))
        try:
            resource_gate(bad,1,0,1)
        except RuntimeError:
            pass
        else:
            raise AssertionError('reservation breach accepted')
    print('PASS: 72 frozen cells + 5 scale cells; failed references UNASSESSED; completion failures FAIL/continue/nonzero; no automatic MET')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--self-test',action='store_true');p.add_argument('--plan',action='store_true')
    p.add_argument('--repair-rerun',action='store_true',help='SF1/SF10: Q1 at all frozen budgets/threads; every scale: all 4 GiB cells')
    p.add_argument('--resource-only',action='store_true',help='HC completion budget at 8 threads, for the separate low-FD control')
    p.add_argument('--fd-limit',type=int,choices=(32,1024),default=1024)
    p.add_argument('--scale',type=int,choices=(1,10,100),default=1);p.add_argument('--smoke',action='store_true')
    p.add_argument('--binary',type=Path);p.add_argument('--commit');p.add_argument('--data',type=Path);p.add_argument('--out',type=Path)
    p.add_argument('--machine',choices=('c3d-highcpu-8','n4a-highcpu-8','c3d-highcpu-30','n4a-highcpu-32'),default='c3d-highcpu-8')
    p.add_argument('--timeout',type=int,default=1800)
    a=p.parse_args()
    if a.repair_rerun and a.resource_only: p.error('--repair-rerun cannot be combined with --resource-only')
    if a.self_test: return self_test()
    if a.plan: print(json.dumps(repair_cells(a.scale) if a.repair_rerun else cells(a.scale),indent=2));return
    if not all((a.binary,a.commit,a.data,a.out)):p.error('run requires --binary --commit --data --out')
    return run(a)


if __name__=='__main__': raise SystemExit(main())
