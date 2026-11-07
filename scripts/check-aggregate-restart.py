#!/usr/bin/env python3
"""SIGKILL nata serve after observing nonempty aggregate spill; clean on restart.

Uses std-only PostgreSQL framing. No live cancel secrets are printed or saved.
"""
import argparse, json, socket, struct, subprocess as sp, time
from pathlib import Path


def read(s,n):
    b=b''
    while len(b)<n:
        part=s.recv(n-len(b))
        if not part:raise EOFError('closed')
        b+=part
    return b


def packet(s):
    tag=read(s,1);n=struct.unpack('!I',read(s,4))[0]
    assert 4<=n<=1<<20
    return tag,read(s,n-4)


def connect(port):
    s=socket.create_connection(('127.0.0.1',port),timeout=10)
    body=b'\x00\x03\x00\x00user\x00test\x00database\x00nata\x00\x00'
    s.sendall(struct.pack('!I',len(body)+4)+body)
    while packet(s)[0]!=b'Z':pass
    return s


def query(s,q):
    body=q.encode()+b'\x00';s.sendall(b'Q'+struct.pack('!I',len(body)+4)+body)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--candidate',type=Path,required=True);p.add_argument('--data',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True);p.add_argument('--port',type=int,default=5544)
    a=p.parse_args();a.out.mkdir(parents=True,exist_ok=True);base=a.out/'spill';base.mkdir(exist_ok=True)
    sp.run([str(a.candidate.resolve()),'cleanup-spill',str(base.resolve())],check=True,capture_output=True)
    cmd=[str(a.candidate.resolve()),'serve','--listen',f'127.0.0.1:{a.port}','--threads','2','--max-queries','1','--aggregate-memory-bytes',str(64<<20),'--spill-disk-bytes',str(1<<30),'--spill-directory',str(base.resolve()),'--force-aggregate-spill']
    def start(log):
        child=sp.Popen(cmd,stdout=log,stderr=log)
        for _ in range(100):
            if child.poll() is not None:raise RuntimeError('server failed')
            try:return child,connect(a.port)
            except OSError:time.sleep(.05)
        child.kill();child.wait();raise TimeoutError('startup')
    with (a.out/'server.log').open('w') as log:
        server,s=start(log)
        try:
            q=Path('check/queries/aggregate/high_cardinality_tpch.sql').read_text().replace('{data}',str(a.data.resolve()))
            query(s,q)
            deadline=time.monotonic()+15
            def spilled():
                for f in base.glob('nata-spill-*/partition-*.ipc'):
                    try:
                        if f.stat().st_size:return True
                    except FileNotFoundError:pass
                return False
            while not spilled():
                if time.monotonic()>deadline:raise TimeoutError('no spill')
                time.sleep(.001)
            server.kill();server.wait(timeout=10);s.close()
            abandoned=list(base.glob('nata-spill-*/partition-*.ipc'))
            assert any(f.stat().st_size for f in abandoned),'no abandoned bytes'
            # A new server starts and queries the SOURCE, never old IPC states.
            server,s=start(log)
            query(s,f"SELECT count(*) FROM '{a.data.resolve()}/lineitem.parquet' WHERE false")
            rows=[]
            while True:
                tag,body=packet(s)
                assert tag!=b'E',body
                if tag==b'D':rows.append(body)
                if tag==b'Z':break
            assert rows==[b'\x00\x01\x00\x00\x00\x010']
            cleaned=sp.run([str(a.candidate.resolve()),'cleanup-spill',str(base)],capture_output=True,text=True,check=True)
            assert not list(base.iterdir()),'abandoned files remain'
            (a.out/'restart.json').write_text(json.dumps({'killed_during_spill':True,'abandoned_files':len(abandoned),'new_query_count':0,'cleanup':cleaned.stderr.strip(),'verdict':'PASS'},indent=2)+'\n')
        finally:
            s.close()
            if server.poll() is None:server.terminate()
            server.wait(timeout=10)
    print('PASS: killed spill owner, restarted without replay, removed only stale files')

if __name__=='__main__':main()
