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

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'wirecapture'))
from protocol import Conn, i
import psycopg
import pyarrow as pa
import pyarrow.parquet as pq
MiB=1024**2
FORMULA='idle RSS + S × (16 MiB snapshot cap + 512 KiB wire buffers) + the engine query budget configured for the run + 64 MiB fixed allowance'
def budget(idle,sessions,engine):
    return idle+sessions*(16*MiB+512*1024)+engine+64*MiB

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

def run(a, count, width, workers, fd_limit):
    work=a.work/f'{count}-{width}-{workers}-{fd_limit}'
    work.mkdir(parents=True,exist_ok=True)
    pq.write_table(pa.table({f'c{j}':pa.array(range(a.rows),type=pa.int64()) for j in range(width)}),work/'data.parquet')
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
            def browse(c):
                rows=c.execute('SHOW TABLES FROM public').fetchall()
                assert len(rows)==count
                assert len(c.execute('DESCRIBE public.t0').fetchall())==width
                assert c.execute('SELECT c0 FROM public.t0 LIMIT 3').fetchall()==[(0,),(1,),(2,)]
            with concurrent.futures.ThreadPoolExecutor(max_workers=a.sessions) as pool:
                list(pool.map(browse,clients))
            # Occupy every admission with positive-fetch portals; healthy client must
            # progress within the existing five-second admission + five-second lease.
            for c in clients: c.close()
            clients=[]
            parked=[]
            for _ in range(workers):
                c=Conn(port);parked.append(c)
                c.query('BEGIN')
                c.send('P',b's\0SELECT c0 FROM public.t0\0\0\0')
                c.send('B',b'p\0s\0'+b'\0\0'*3)
                c.send('E',b'p\0'+i(1));c.send('H')
                assert c.until(('s','E'))[-1][0]=='s'
            started=time.monotonic()
            with psycopg.connect(dsn,autocommit=True) as healthy:
                assert healthy.execute('SELECT c0 FROM public.t0 LIMIT 3').fetchall()==[(0,),(1,),(2,)]
            elapsed=time.monotonic()-started
            assert elapsed<=10, ('admission starvation',elapsed)
            for c in parked:c.close()
            rss,hwm,_=proc(server.pid)
            assert hwm<=ceiling, ('peak RSS excess',hwm,ceiling)
            wait(lambda:proc(server.pid)[0]<=idle+64*MiB and proc(server.pid)[2]==fds)
            final=proc(server.pid)
            result=dict(tables=count,width=width,workers=workers,sessions=a.sessions,peak=hwm,budget=ceiling,idle=idle,final=final,admission_seconds=elapsed,verdict='PASS')
            (work/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result),flush=True)
        finally:
            for c in clients:c.close()
            for child in load:child.terminate()
            for child in load:child.wait(timeout=10)
            server.terminate();server.wait(timeout=10)

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--nata',type=Path,required=True);ap.add_argument('--work',type=Path,required=True)
    ap.add_argument('--smoke',action='store_true');a=ap.parse_args()
    a.sessions=4 if a.smoke else 64;a.rows=100 if a.smoke else 1000000
    for count in ([4] if a.smoke else [64,1024,10000]):
        for width in ([2] if a.smoke else [1,64]):
            for workers in ([1] if a.smoke else [1,8]):
                for fd in [0,256]:run(a,count,width,workers,fd)
