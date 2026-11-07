#!/usr/bin/env python3
"""Indicative independent per-process RSS against baseline + B + 32 MiB.

Runs fresh CLI children sequentially; no timing verdict. wait4 ru_maxrss is
independent of the reservation ledger. Include all aggregate gate queries,
expected errors retained, plus the SF1 HAVING query. Explicit D=1 GiB is a
measurement control, not a default change. Use --threads 1 or 8 in separate runs.
"""
import argparse, json, os, re, subprocess as sp, time
from pathlib import Path


def measure(command, log):
    with open(os.devnull,'w') as sink,log.open('w') as err:
        child=sp.Popen(list(map(str,command)),stdout=sink,stderr=err)
        peaks={'threads_peak':0,'stack_rss':0,'stack_virtual':0}
        deadline=time.monotonic()+600
        while True:
            if time.monotonic()>deadline:
                child.kill();child.wait();raise TimeoutError("RSS query exceeded 600 seconds")
            pid,status,usage=os.wait4(child.pid,os.WNOHANG)
            if pid:break
            try:
                status_text=Path(f'/proc/{child.pid}/status').read_text()
                threads=int(re.search(r'^Threads:\s+(\d+)',status_text,re.M)[1])
                peaks['threads_peak']=max(peaks['threads_peak'],threads)
                # Main [stack] plus anonymous RW mappings immediately following
                # a guard mapping with the native no-huge-page stack flag are
                # the thread stack candidates (guard adjacency alone also finds
                # allocator arenas). Keep
                # this heuristic explicit; OS peak RSS itself is not heuristic.
                stack_rss=stack_virtual=0;guard=False;stack=False;main=False;region_rss=region_size=0
                for line in Path(f'/proc/{child.pid}/smaps').read_text().splitlines():
                    if re.match(r'^[0-9a-f]+-[0-9a-f]+ ',line):
                        parts=line.split();perms=parts[1]
                        main='[stack]' in line
                        stack=guard and perms=='rw-p' and len(parts)==5
                        region_rss=region_size=0
                        guard=perms=='---p'
                    elif line.startswith('Rss:'):region_rss=int(line.split()[1])*1024
                    elif line.startswith('Size:'):region_size=int(line.split()[1])*1024
                    elif line.startswith('VmFlags:') and (main or stack and 'nh' in line.split()[1:]):
                        stack_rss+=region_rss;stack_virtual+=region_size
                peaks['stack_rss']=max(peaks['stack_rss'],stack_rss)
                peaks['stack_virtual']=max(peaks['stack_virtual'],stack_virtual)
            except (OSError,TypeError):pass
            time.sleep(.02)
        child.returncode=os.waitstatus_to_exitcode(status)
    return child.returncode, int(usage.ru_maxrss)*1024, peaks



def validate_observations(observations):
    for row in observations:
        expected_error=False
        if row['query']!='sf1-having':
            expected_error='-- expect-error' in (Path('check/queries/aggregate')/row['query']).read_text()
        assert not row.get('usage_error'), (row['query'],'CLI invocation failed')
        if expected_error:
            text=(Path('check/queries/aggregate')/row['query']).read_text()
            if '-- expect-error-nata:' in text:assert row.get('expected_error_matched'),row
        assert (row['exit']!=0)==expected_error, (row['query'],row['exit'],'unexpected query outcome')
        if row['budget'] and row['counters']:
            assert row['counters']['peak_reservation']<=row['budget'],row
        if row['query']=='sf1-having':
            assert (row['counters']['spill_bytes_written']>0)==(row['budget'] is not None),row


def server_measure(a):
    """Two simultaneous SF1 queries: one fixed node pool and one process RSS."""
    import concurrent.futures, runpy
    wire=runpy.run_path(str(Path(__file__).with_name('check-aggregate-restart.py')))
    observations=[]
    q=Path('check/queries/aggregate/high_cardinality_tpch.sql').read_text().replace('{data}',str(a.sf1.resolve()))
    for repeat in range(a.rounds):
        for per_query in [None,64<<20,256<<20]:
            label='default' if per_query is None else str(per_query>>20)
            base=a.out/f'server-{label}-{repeat}';base.mkdir(parents=True,exist_ok=True)
            log=base/'server.log';spill=base/'spill';spill.mkdir(exist_ok=True)
            sp.run([str(a.candidate.resolve()),'cleanup-spill',str(spill.resolve())],check=True,capture_output=True)
            command=[str(a.candidate.resolve()),'serve','--listen','127.0.0.1:5544','--threads',str(max(2,a.threads)),'--max-queries','2','--spill-directory',str(spill.resolve()),'--spill-disk-bytes',str(1<<30)]
            if per_query:command+=['--aggregate-memory-bytes',str(per_query*2)]
            with log.open('w') as output:
                child=sp.Popen(command,stdout=output,stderr=output)
                clients=[]
                try:
                    for _ in range(100):
                        if child.poll() is not None:raise RuntimeError('server failed')
                        try:clients.append(wire['connect'](5544));break
                        except OSError:time.sleep(.05)
                    else:raise TimeoutError('startup')
                    clients.append(wire['connect'](5544))
                    for c in clients:c.settimeout(600)
                    def status():
                        text=Path(f'/proc/{child.pid}/status').read_text()
                        return {k:int(re.search(r'^'+k+r':\s+(\d+)',text,re.M)[1]) for k in ['VmRSS','VmHWM','Threads']}
                    baseline=status()['VmHWM']*1024
                    pool=int(re.search(r'aggregate_node_pool=(\d+)',log.read_text())[1])
                    def consume(c):
                        wire['query'](c,q);rows=0
                        while True:
                            tag,body=wire['packet'](c)
                            if tag==b'E':raise AssertionError('aggregate wire error')
                            if tag==b'D':rows+=1
                            if tag==b'Z':return rows
                    spilled=False;threads_peak=0;deadline=time.monotonic()+600
                    with concurrent.futures.ThreadPoolExecutor(2) as executor:
                        jobs=[executor.submit(consume,c) for c in clients]
                        while not all(j.done() for j in jobs):
                            if time.monotonic()>deadline:
                                child.kill();raise TimeoutError('concurrent SF1 queries')
                            threads_peak=max(threads_peak,status()['Threads'])
                            if not spilled:
                                for f in spill.glob('nata-spill-*/partition-*.ipc'):
                                    try:spilled=spilled or f.stat().st_size>0
                                    except FileNotFoundError:pass
                            time.sleep(.02)
                        counts=[j.result() for j in jobs]
                    rss=status()['VmHWM']*1024
                    assert counts==[1273748,1273748],counts
                    if per_query:assert spilled,'small-budget server queries did not spill'
                    assert not list(spill.iterdir()),'server spill leak'
                    limit=baseline+pool+(32<<20)
                    row=dict(round=repeat,query_budget=pool//2,node_pool=pool,baseline=baseline,rss=rss,allowance=limit,threads_peak=threads_peak,rows=counts,observed_spill=spilled,verdict='PASS' if rss<=limit else 'MISS')
                    observations.append(row);print(json.dumps(row),flush=True)
                finally:
                    for c in clients:c.close()
                    if child.poll() is None:child.terminate()
                    child.wait(timeout=10)
    (a.out/'server-rss.json').write_text(json.dumps(observations,indent=2)+'\n')
    if any(r['verdict']=='MISS' for r in observations):raise SystemExit(1)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--candidate',type=Path,required=True);p.add_argument('--data',type=Path,required=True)
    p.add_argument('--sf1',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    p.add_argument('--server',action='store_true',help='two concurrent SF1 serve queries at each budget; aggregate gate set remains in CLI mode')
    p.add_argument('--threads',type=int,default=1);p.add_argument('--rounds',type=int,default=3)
    a=p.parse_args();a.out.mkdir(parents=True,exist_ok=True)
    if a.server:return server_measure(a)
    sp.run([str(a.candidate.resolve()),'cleanup-spill',str(a.out.resolve())],check=True,capture_output=True)
    common=[a.candidate.resolve(),'sql','--csv','--report','--threads',str(a.threads),'--spill-directory',a.out.resolve(),'--spill-disk-bytes',str(1<<30)]
    baseline=[]
    for r in range(a.rounds):
        rc,rss,stacks=measure(common+[f"--command=SELECT l_orderkey FROM '{a.data.resolve()}/lineitem.parquet' LIMIT 0"],a.out/f'baseline-{r}.log')
        assert rc==0;baseline.append(rss)
    observations=[]
    queries=[(p.name,p.read_text().replace('{data}',str(a.data.resolve()))) for p in sorted(Path('check/queries/aggregate').glob('*.sql'))]
    queries.append(('sf1-having',Path('check/queries/aggregate/high_cardinality_tpch.sql').read_text().replace('{data}',str(a.sf1.resolve()))))
    for r in range(a.rounds):
        for name,q in queries:
            for budget in [None,64<<20,256<<20]:
                label='default' if budget is None else str(budget>>20)
                log=a.out/f'{name}-{label}-{r}.log'
                rc,rss,stacks=measure(common+([] if budget is None else ['--aggregate-memory-bytes',str(budget)])+['--command='+q],log)
                metrics={}
                for line in log.read_text().splitlines():
                    if line.startswith('aggregate:'): metrics={k:int(v) for k,v in (s.split('=') for s in line.split()[1:])}
                limit=max(baseline)+budget+(32<<20) if budget else None
                stderr=log.read_text()
                expected=re.search(r'-- expect-error-nata:\s*([^\n]+)',q)
                row=dict(usage_error='Usage: nata sql' in stderr,expected_error_matched=bool(expected and expected[1].lower() in stderr.lower()),query=name,budget=budget,round=r,exit=rc,rss=rss,allowance=limit,verdict='PASS' if limit and rss<=limit else 'MISS' if limit else 'CONTROL',counters=metrics,**stacks)
                observations.append(row);print(json.dumps(row),flush=True)
    (a.out/'rss.json').write_text(json.dumps(dict(baseline=baseline,threads=a.threads,architecture=os.uname().machine,stack_sampling="20ms; main stack plus guarded no-huge-page anonymous RW candidates",observations=observations),indent=2)+'\n')
    validate_observations(observations)
    if any(r['verdict']=='MISS' for r in observations):raise SystemExit(1)

if __name__=='__main__':main()
