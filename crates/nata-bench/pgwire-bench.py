#!/usr/bin/env python3
"""Warm-cache paired pgwire benchmark; results never contain generated data.
Raw timings include client decoding (no Python value conversion), command roundtrip,
and CLI startup; persistent wire startup is excluded. Five alternating twin rounds.
"""
import concurrent.futures as futures
import csv
import json
import os
import re
from pathlib import Path
import socket
import statistics as st
import struct
import subprocess as sp
import sys
import threading
import time

os.environ.pop('AI_AGENT', None)
root = Path(__file__).resolve().parents[2]

class Client:
    def __init__(self, port):
        self.s = socket.create_connection(('127.0.0.1', int(port)), timeout=30)
        self.s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.f = self.s.makefile('rb')
        b = b'\0\x03\0\0user\0test\0database\0nata\0\0'
        self.s.sendall(struct.pack('!i', len(b)+4)+b)
        while self.read()[0] != b'Z':
            pass
    def read(self):
        h = self.f.read(5)
        if len(h) != 5:
            raise EOFError('server closed')
        n = struct.unpack('!i', h[1:])[0]
        assert 4 <= n <= 100_000_000
        b = self.f.read(n-4)
        assert len(b) == n-4
        return h[:1], b
    def send(self, tag, b):
        self.s.sendall(tag+struct.pack('!i', len(b)+4)+b)
    def query(self, q, collect=False):
        start = time.perf_counter()
        self.send(b'Q', q.encode()+b'\0')
        first, count, rows, oids = None, 0, [], []
        error = None
        while True:
            tag, b = self.read()
            if tag == b'T':
                offset=2;oids=[]
                for _ in range(struct.unpack('!h',b[:2])[0]):
                    offset=b.index(0,offset)+1
                    oids.append(struct.unpack_from('!i',b,offset+6)[0]);offset+=18
            elif tag == b'D':
                if first is None:
                    first = time.perf_counter()-start
                count += 1
                if collect:
                    row, offset = [], 2
                    for _ in range(struct.unpack('!h', b[:2])[0]):
                        n = struct.unpack_from('!i', b, offset)[0]; offset += 4
                        row.append(None if n == -1 else b[offset:offset+n].decode())
                        if n >= 0: offset += n
                    rows.append(row)
            elif tag == b'E':
                error = b
            elif tag == b'Z':
                if error: raise RuntimeError(repr(error))
                return dict(secs=time.perf_counter()-start, first=first, rows=count, values=rows,oids=oids)
    def extended(self, q, name, binary=False, parse=True):
        def packet(tag,b):return tag+struct.pack('!i',len(b)+4)+b
        body=b''
        if parse:body+=packet(b'P',name.encode()+b'\0'+q.encode()+b'\0'+struct.pack('!hi',1,23))
        value=struct.pack('!i',7) if binary else b'7'
        body+=packet(b'B',b'\0'+name.encode()+b'\0'+struct.pack('!hhh',1,int(binary),1)+struct.pack('!i',len(value))+value+struct.pack('!h',0))
        body+=packet(b'E',b'\0'+struct.pack('!i',0))+packet(b'S',b'')
        self.s.sendall(body)
        count=0
        while True:
            tag,b=self.read()
            if tag==b'E':raise RuntimeError(repr(b))
            if tag==b'D':
                assert b==struct.pack('!hi',1,1)+b'7';count+=1
            if tag==b'Z':assert count==1;return
    def close(self):
        self.f.close(); self.s.close()

class Server:
    def __init__(self, nata, work, threads, admissions):
        with socket.socket() as s:
            s.bind(('127.0.0.1', 0)); self.port = s.getsockname()[1]
        self.log = open(work/f'server-{threads}-{admissions}.log', 'w')
        self.p = sp.Popen([nata,'serve','--listen',f'127.0.0.1:{self.port}','--threads',str(threads),'--max-queries',str(admissions)], stderr=self.log)
        for _ in range(300):
            try:
                c = Client(self.port); c.close(); break
            except (OSError, EOFError): time.sleep(.05)
        else: raise RuntimeError('server startup')
    def close(self):
        self.p.terminate(); self.p.wait(timeout=10); self.log.close()

def main():
    nata, work, out = sys.argv[1:]
    nata = str(Path(nata).resolve()); work = Path(work).resolve(); out = Path(out).resolve()
    work.mkdir(parents=True,exist_ok=True); out.mkdir(parents=True,exist_ok=True)
    f = work/'million.parquet'
    sp.run(['duckdb','-c',f"COPY (SELECT i::BIGINT AS i FROM range(1000000) t(i)) TO '{f}' (FORMAT PARQUET, ROW_GROUP_SIZE 8192)"],check=True,stdout=sp.DEVNULL)
    q = f"SELECT i FROM '{f}'"
    # Exact ordered output before timing, against both independent CLI engines.
    cli = sp.check_output([nata,'sql','--csv','-c',q])
    oracle = sp.check_output(['duckdb','-csv','-c',q])
    assert cli == oracle
    samples = []
    def settle(label):
        with (out/'settle.txt').open('a') as log:
            sp.run([str(root/'scripts/gcp/settle.sh'),label,'0.5','600'],check=True,stdout=log)
    def sample(block, arm, round_, fn):
        before=Path(server.log.name).stat().st_size
        start=time.perf_counter(); r=fn(); elapsed=time.perf_counter()-start
        with open(server.log.name) as log:
            log.seek(before); events=log.read()
        waits=[int(x) for x in re.findall(r'queue_wait_us=(\d+)',events)]
        shares=[int(x) for x in re.findall(r'worker_share=(\d+)',events)]
        samples.append(dict(block=block,arm=arm,round=round_,secs=r['secs'] if isinstance(r,dict) else elapsed,first=r.get('first') if isinstance(r,dict) else None,scan_end=r.get('scan_end') if isinstance(r,dict) else None,queue_wait_us=waits,worker_shares=sorted(set(shares))))
    for threads in (1,8):
        server=Server(nata,work,threads,1); c=Client(server.port)
        try:
            r=c.query(q,True)
            assert r['values'] == [[str(i)] for i in range(1000000)]
            psql=['psql','-X','-h','127.0.0.1','-p',str(server.port),'-U','test','-d','nata','--csv','-c',q]
            assert sp.check_output(psql)==cli
            c.query(q) # warm all arms
            settle(f'stream-{threads}')
            for round_ in range(5):
                arms=['cli','raw','psql','cli-twin','raw-twin','psql-twin']
                if round_%2: arms.reverse()
                for arm in arms:
                    if arm.startswith('raw'):
                        def fn():
                            r=c.query(q)
                            scans=re.findall(r'scan_end_us=(\d+)',Path(server.log.name).read_text())
                            r['scan_end']=int(scans[-1])/1e6 if scans else None
                            return r
                    else:
                        cmd=psql if arm.startswith('psql') else [nata,'sql','--csv','--threads',str(threads),'-c',q]
                        fn=lambda cmd=cmd:sp.run(cmd,check=True,stdout=sp.DEVNULL,stderr=sp.DEVNULL)
                    sample(f'stream/{threads}',arm,round_,fn)
        finally: c.close(); server.close()
    server=Server(nata,work,8,8)
    clients=[Client(server.port) for _ in range(64)]
    small=f"SELECT i FROM '{f}' LIMIT 1"
    try:
        for c in clients: assert c.query(small,True)['values']==[['0']]
        settle('preparation')
        param=f"SELECT CAST($1 AS INTEGER) FROM '{f}' LIMIT 1"
        clients[0].extended(param,'named',parse=True)
        for round_ in range(5):
            for arm in ['parse-text','named-text','named-binary','parse-text-twin','named-text-twin','named-binary-twin']:
                def repeat():
                    for _ in range(100):clients[0].extended(param,'' if arm.startswith('parse') else 'named',binary='binary' in arm,parse=arm.startswith('parse'))
                sample('preparation100',arm,round_,repeat)
        settle('fixed-share')
        for round_ in range(5):
            sample('share1-of8','raw',round_,lambda:clients[0].query(q))
            sample('share1-of8','raw-twin',round_,lambda:clients[0].query(q))
        settle('concurrency')
        with futures.ThreadPoolExecutor(max_workers=64) as pool:
            for round_ in range(5):
                for arm in (['sequential','concurrent','sequential-twin','concurrent-twin'] if round_%2==0 else ['concurrent-twin','sequential-twin','concurrent','sequential']):
                    def runs():
                        if arm.startswith('sequential'):
                            for c in clients: c.query(small)
                        else:
                            gate=threading.Barrier(64)
                            def run(c): gate.wait(); return c.query(small)
                            list(pool.map(run, clients))
                    sample('connections64',arm,round_,runs)
    finally:
        for c in clients:c.close()
        server.close()
    # Slow-reader certified by the permanent live test, including produced-row and RSS bounds.
    # Keep it a separate phase, never a timed comparison of unlike fixtures.
    medians={}
    summary=[]
    for block in sorted({r['block'] for r in samples}):
        for arm in sorted({r['arm'] for r in samples if r['block']==block}):
            values=[r['secs'] for r in samples if r['block']==block and r['arm']==arm]
            assert len(values)==5
            m=st.median(values); medians[f'pgwire/{block}/{arm}']=m
            summary.append(f'{block}/{arm}: min={min(values):.9f} median={m:.9f} max={max(values):.9f} spread={(max(values)-min(values))/m:.4%}')
            matching=[r for r in samples if r['block']==block and r['arm']==arm]
            waits=[v for r in matching for v in r['queue_wait_us']]
            shares=sorted({v for r in matching for v in r['worker_shares']})
            if waits:summary.append(f'{block}/{arm}: queue_wait_us median={st.median(waits)} max={max(waits)} configured_worker_shares={shares}')
    for t in (1,8):
        base=medians[f'pgwire/stream/{t}/cli']
        for arm in ('raw','psql'):
            ratio=medians[f'pgwire/stream/{t}/{arm}']/base
            summary.append(f'VERDICT streaming/{t}/{arm}: {ratio:.6f}x <= 1.20x: {"MET" if ratio<=1.2 else "NOT MET"}')
        r=[r for r in samples if r['block']==f'stream/{t}' and r['arm']=='raw']
        ok=all(r['scan_end'] is not None and 0<r['first']<r['scan_end'] for r in r)
        summary.append(f'VERDICT first-DataRow/{t}: first/scan-end/total={[(r["first"],r["scan_end"],r["secs"]) for r in r]}: {"MET" if ok else "NOT MET"}; scan-end is producer completion including bounded-bridge backpressure')
    ratio=medians['pgwire/connections64/concurrent']/medians['pgwire/connections64/sequential']
    summary.append(f'VERDICT concurrency: elapsed concurrent/sequential={ratio:.6f} <= 2.0: {"MET" if ratio<=2 else "NOT MET"}')
    (out/'samples.json').write_text(json.dumps(samples,indent=2)+'\n')
    (out/'summary.txt').write_text('\n'.join(summary)+'\n')
    with (out/'medians.csv').open('w') as f:
        w=csv.writer(f);w.writerow(['metric','median_secs']);w.writerows(sorted(medians.items()))
    for path in work.glob('server-*.log'):
        # No query text, identifiers, keys or nonloopback addresses in server logs.
        (out/path.name).write_text(path.read_text())
    print('\n'.join(summary),flush=True)

if __name__=='__main__': main()
