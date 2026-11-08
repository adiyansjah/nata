#!/usr/bin/env python3
"""Loaded resource gate. Linux /proc VmHWM fallback; no cgroup is modified."""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import resource
import socket
import subprocess
import sys
import time
import statistics as st

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'wirecapture'))
from protocol import Conn, i
import psycopg
import pyarrow as pa
import pyarrow.parquet as pq
MiB=1024**2
FORMULA='idle RSS + S × (16 MiB snapshot cap + 512 KiB wire buffers) + the engine query budget configured for the run + 64 MiB fixed allowance'
def budget(idle,sessions,engine):
    return idle+sessions*(16*MiB+512*1024)+engine+64*MiB

def browse_ok(ratio):
    return ratio <= 1.20

def peak_ok(peak, ceiling):
    return peak <= ceiling

def cleanup_ok(rss, idle, fds, idle_fds):
    return rss <= idle + 64*MiB and fds == idle_fds

def proc(pid):
    lines=Path(f'/proc/{pid}/status').read_text().splitlines()
    values={l.split(':')[0]:int(l.split()[1])*1024 for l in lines if l.startswith(('VmRSS:','VmHWM:'))}
    return values['VmRSS'],values['VmHWM'],len(list(Path(f'/proc/{pid}/fd').iterdir()))

def wait(fn, seconds=10):
    until=time.monotonic()+seconds
    while time.monotonic()<until:
        if fn(): return
        time.sleep(.02)
    raise AssertionError('cleanup/start deadline exceeded')

def setup_postgres(pg, count, width):
    pg.execute('DROP SCHEMA IF EXISTS resource_scale CASCADE')
    pg.execute('CREATE SCHEMA resource_scale')
    for j in range(count):
        columns=width if width==1 or j%16==0 else 1
        pg.execute(f'CREATE TABLE resource_scale.t{j} ('+','.join(f'c{k} bigint' for k in range(columns))+')')

def run(a, count, width, workers, fd_limit):
    work=a.work/f'{count}-{width}-{workers}-{fd_limit}'
    work.mkdir(parents=True,exist_ok=True)
    pq.write_table(pa.table({f'c{j}':pa.array(range(a.rows),type=pa.int64()) for j in range(width)}),work/'data.parquet')
    pg_context=None
    if a.postgres:
        with psycopg.connect(a.postgres,autocommit=True) as pg:
            setup_postgres(pg, count, width)
            pid=pg.execute('SELECT pg_backend_pid()').fetchone()[0]
            pg_context=dict(version=pg.execute('SELECT version()').fetchone()[0],backend_RSS=proc(pid)[0],backend_peak=proc(pid)[1],scope='backend only; shared PostgreSQL memory not aggregated; catalog metadata context')
    registry=work/'catalog.tsv'
    pq.write_table(pa.table({'c0':pa.array(range(a.rows),type=pa.int64())}),work/'narrow.parquet')
    registry.write_text(''.join(f'nata\tpublic\tt{j}\t{work.resolve()}/{"data.parquet" if width==1 or j%16==0 else "narrow.parquet"}\n' for j in range(count)))
    sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close()
    def limits():
        if fd_limit: resource.setrlimit(resource.RLIMIT_NOFILE,(fd_limit,fd_limit))
    engine=64*MiB
    with (work/'server.log').open('w') as log:
        server=subprocess.Popen([str(a.nata.resolve()),'serve','--catalog',str(registry),'--listen',f'127.0.0.1:{port}','--threads',str(workers),'--max-queries',str(workers),'--aggregate-memory-bytes',str(engine)],stdout=log,stderr=log,preexec_fn=limits)
        clients=[]
        parked=[]
        load=[subprocess.Popen([sys.executable,'-c','while True: pass'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL) for _ in range(workers)]
        try:
            def ready():
                assert server.poll() is None, 'server startup failed (snapshot adequacy is a gate)'
                try:
                    with socket.create_connection(('127.0.0.1',port),timeout=.1): return True
                except OSError:return False
            wait(ready,60)
            time.sleep(.1)
            idle,_,fds=proc(server.pid)
            ceiling=budget(idle,a.sessions,engine)
            print(json.dumps(dict(formula=FORMULA,idle_RSS=idle,S=a.sessions,snapshot_cap=16*MiB,wire_buffers=512*1024,engine_query_budget=engine,fixed_allowance=64*MiB,budget=ceiling,fd_idle=fds,fd_limit=fd_limit)),flush=True)
            dsn=f'host=127.0.0.1 port={port} user=nata dbname=nata connect_timeout=10'
            for _ in range(a.sessions): clients.append(psycopg.connect(dsn,autocommit=True))
            browse_gate=None
            if a.postgres:
                # Scope projection removes unrelated PostgreSQL system/fixture tables.
                # Nata has only this registered fixture in its snapshot.
                nq=["SELECT table_name FROM information_schema.tables ORDER BY table_name",
                    "SELECT table_name,column_name,data_type,is_nullable,ordinal_position FROM information_schema.columns ORDER BY table_name,ordinal_position"]
                pq_queries=[q.replace(' ORDER BY', " WHERE table_schema='resource_scale' ORDER BY") for q in nq]
                def materialize(c,queries):return [c.execute(q).fetchall() for q in queries]
                with psycopg.connect(a.postgres,autocommit=True) as pg:
                    expected=materialize(pg,pq_queries)
                    assert materialize(clients[0],nq)==expected, 'large-catalog typed values differ'
                    samples={'postgres':[],'nata':[]}
                    for rnd in range(1 if a.smoke else 5):
                        for name in (['postgres','nata'] if rnd%2==0 else ['nata','postgres']):
                            started=time.perf_counter()
                            got=materialize(pg,pq_queries) if name=='postgres' else materialize(clients[0],nq)
                            elapsed=time.perf_counter()-started
                            assert got==expected
                            samples[name].append(elapsed)
                    ratio=st.median(samples['nata'])/st.median(samples['postgres'])
                    browse_gate=dict(samples=samples,ratio=ratio,budget=1.20,verdict='SMOKE ONLY' if a.smoke else ('PASS' if browse_ok(ratio) else 'FAIL'))
                    assert a.smoke or browse_ok(ratio), ('large catalog browse budget exceeded',browse_gate)
            def browse(c):
                rows=c.execute('SHOW TABLES FROM public').fetchall()
                assert len(rows)==count
                assert len(c.execute('DESCRIBE public.t0').fetchall())==width
                assert c.execute('SELECT c0 FROM public.t0 LIMIT 3').fetchall()==[(0,),(1,),(2,)]
            with concurrent.futures.ThreadPoolExecutor(max_workers=a.sessions) as pool:
                list(pool.map(browse,clients))
            # Occupy every admission with positive-fetch portals; healthy client must
            # progress within the existing five-second admission deadline.
            for c in clients[:workers]: c.close()
            clients=clients[workers:]
            parked=[]
            for _ in range(workers):
                c=Conn(port);parked.append(c)
                c.query('BEGIN')
                c.send('P',b's\0SELECT c0 FROM public.t0\0\0\0')
                c.send('B',b'p\0s\0'+b'\0\0'*3)
                c.send('E',b'p\0'+i(1));c.send('H')
                assert c.until(('s','E'))[-1][0]=='s'
            started=time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(max_workers=len(clients)) as pool:
                list(pool.map(browse,clients))
            elapsed=time.monotonic()-started
            assert elapsed<=5, ('admission deadline exceeded',elapsed)
            yielded=0
            for c in parked:
                c.send('E',b'p\0'+i(1));c.send('H')
                response=c.until(('s','E'))[-1]
                if response[0]=='E':
                    error=bytes.fromhex(response[1])
                    assert b'34000' in error and b'suspended portal expired' in error, response
                    yielded+=1
                c.close()
            parked=[]
            assert yielded>=1, 'no suspended admission yielded'
            for c in clients:c.close()
            clients=[]
            rss,hwm,_=proc(server.pid)
            assert peak_ok(hwm,ceiling), ('peak RSS excess',hwm,ceiling)
            wait(lambda:cleanup_ok(proc(server.pid)[0],idle,proc(server.pid)[2],fds))
            final=proc(server.pid)
            result=dict(browse_gate=browse_gate,postgres_context=pg_context,tables=count,width=width,workers=workers,sessions=a.sessions,peak=hwm,budget=ceiling,idle=idle,final=final,admission_seconds=elapsed,yielded_portals=yielded,verdict='PASS')
            (work/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result),flush=True)
        finally:
            for c in clients:c.close()
            for c in parked:
                try:c.close()
                except OSError:pass
            for child in load:child.terminate()
            for child in load:child.wait(timeout=10)
            server.terminate();server.wait(timeout=10)

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--nata',type=Path,required=True);ap.add_argument('--work',type=Path,required=True)
    ap.add_argument('--postgres');ap.add_argument('--smoke',action='store_true')
    ap.add_argument('--workers',type=int,nargs='+',choices=[1,8],help='Override the worker grid, including smoke')
    ap.add_argument('--sessions',type=int,help='Override session count (at most the existing 64-session cap)')
    ap.add_argument('--tables',type=int,nargs='+',help='Override catalog sizes for a local smoke')
    ap.add_argument('--widths',type=int,nargs='+',help='Override column widths for a local smoke')
    ap.add_argument('--repeat',type=int,default=1,help='Repeat every loaded fairness/resource arm')
    a=ap.parse_args()
    assert a.repeat > 0
    a.sessions=a.sessions or (max(4,2*max(a.workers or [1])) if a.smoke else 64)
    assert max(a.workers or [1]) < a.sessions <= 64
    a.rows=100 if a.smoke else 1000000
    base=a.work
    for repetition in range(a.repeat):
        a.work=base/f'repeat-{repetition+1}'
        for count in (a.tables or ([4] if a.smoke else [64,1024,10000])):
            for width in (a.widths or ([2] if a.smoke else [1,64])):
                for workers in (a.workers or ([1] if a.smoke else [1,8])):
                    for fd in [0,256]:run(a,count,width,workers,fd)
