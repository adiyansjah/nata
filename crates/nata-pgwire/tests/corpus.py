#!/usr/bin/env python3
"""Raw simple-query protocol versus `nata sql --csv` for check/queries/sql.
Usage: corpus.py --nata target/debug/nata --data /on-disk/data
Generate inputs with scripts/gen-sql-check-data.sh and gen-tpch.sh (unset AI_AGENT).
No third-party client library. Compares every successful row, names, NULLs and
command count; failed CLI queries must return ErrorResponse and ReadyForQuery.
"""
import argparse
import csv
import io
from pathlib import Path
import socket
import struct
import subprocess
import time

ROOT=Path(__file__).resolve().parents[3]

def packet(tag,body): return tag+struct.pack('!i',len(body)+4)+body

def exact(s,n):
    out=bytearray()
    while len(out)<n:
        b=s.recv(n-len(out))
        if not b: raise AssertionError('truncated backend frame')
        out.extend(b)
    return bytes(out)

def message(s):
    h=exact(s,5); n=struct.unpack('!i',h[1:])[0]
    assert 4<=n<=100_000_000
    return h[:1],exact(s,n-4)

def connect(port):
    s=socket.create_connection(('127.0.0.1',port),timeout=20)
    s.sendall(packet(b'',b'\0\x03\0\0user\0test\0database\0nata\0\0'))
    while True:
        t,b=message(s)
        assert t!=b'E',b
        if t==b'Z':return s

def run(s,q):
    s.sendall(packet(b'Q',q.encode()+b'\0'))
    names=[];oids=[];rows=[];errors=[];command=None;tags=[]
    while True:
        t,b=message(s);tags.append(t)
        if t==b'T':
            n=struct.unpack('!h',b[:2])[0];at=2
            for _ in range(n):
                end=b.index(0,at);names.append(b[at:end].decode());at=end+1
                oids.append(struct.unpack('!i',b[at+6:at+10])[0]);at+=18
        if t==b'D':
            n=struct.unpack('!h',b[:2])[0];at=2;row=[]
            for _ in range(n):
                length=struct.unpack('!i',b[at:at+4])[0];at+=4
                if length==-1:row.append(None)
                else:row.append(b[at:at+length].decode());at+=length
            rows.append(row)
        if t==b'E':errors.append(b)
        if t==b'C':command=b
        if t==b'Z':return names,oids,rows,errors,command,tags

def main():
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--nata',type=Path,required=True);ap.add_argument('--data',type=Path,required=True);a=ap.parse_args()
    binary=str(a.nata.resolve());data=str(a.data.resolve())
    # Reserve a loopback port, release immediately before the server binds.
    with socket.socket() as reserve:reserve.bind(('127.0.0.1',0));port=reserve.getsockname()[1]
    log=a.data/'pgwire-corpus.log'
    with log.open('w') as err:
        p=subprocess.Popen([binary,'serve','--listen',f'127.0.0.1:{port}','--threads','2'],stderr=err)
        try:
            until=time.monotonic()+15
            while True:
                try:s=connect(port);break
                except ConnectionRefusedError:
                    if time.monotonic()>until:raise
                    time.sleep(.02)
            ok=failed=0
            for file in sorted((ROOT/'check/queries/sql').glob('*.sql')):
                q=file.read_text().replace('{data}',data)
                expected=subprocess.run([binary,'sql','--csv','--threads','2','-c',q],capture_output=True,text=True,timeout=60)
                names,oids,rows,errors,command,tags=run(s,q)
                if expected.returncode:
                    assert errors and tags[-1]==b'Z' and command is None,(file,tags)
                    failed+=1
                elif q.lstrip().upper().startswith('EXPLAIN'):
                    assert not errors and rows and command is not None,file
                    if not q.lstrip().upper().startswith('EXPLAIN ANALYZE'):
                        assert '\n'.join(row[0] for row in rows)+'\n'==expected.stdout,file
                    ok+=1
                else:
                    assert not errors,(file,errors)
                    want=list(csv.reader(io.StringIO(expected.stdout)))
                    assert names==want[0],(file,names,want[:1])
                    normalized=[]
                    for row in rows:
                        out=[]
                        for oid,v in zip(oids,row):
                            if v is None:v='NULL'
                            elif oid==16:v={'t':'true','f':'false'}[v]
                            elif oid in (700,701):v={'NaN':'nan','Infinity':'inf','-Infinity':'-inf'}.get(v,v)
                            out.append(v)
                        normalized.append(out)
                    assert normalized==want[1:],(file,normalized[:3],want[1:4])
                    assert command==f'SELECT {len(rows)}\0'.encode(),(file,command)
                    ok+=1
            print(f'corpus: {ok} successful row sets, {failed} matching error outcomes, {ok+failed} total; PASS')
            s.sendall(packet(b'X',b''));s.close()
        finally:p.terminate();p.wait(timeout=10)

if __name__=='__main__':main()
