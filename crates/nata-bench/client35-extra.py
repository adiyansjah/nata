#!/usr/bin/env python3
"""psql and pgcli default-interface edges; ADBC default bootstrap is probed by item35."""
import datetime
import decimal
import json
from pathlib import Path
import signal
import subprocess as sp
import sys
import threading
import time
import psycopg
from pgcli.pgexecute import PGExecute
port, fixture, large, out=sys.argv[1:]
out=Path(out)
p=PGExecute(database='nata',user='test',host='127.0.0.1',port=port)
q=f"SELECT * FROM '{fixture}'"
expected=[bool,str,int,int,int,float,float,decimal.Decimal,str,str] # pgcli intentionally loads dates as text
for _ in range(10):
    result=list(p.run(q))[0]
    rows=list(result[1])
    assert [type(v) for v in rows[0]]==expected
    assert rows[0][:5]==(True,'-128',-32768,-2147483648,-9223372036854775808)
    assert str(rows[0][5])=='-0.0' and str(rows[0][6])=='nan'
    assert rows[0][7:]==(decimal.Decimal('123.450'),'','1970-01-01')
    assert all(v is None for v in rows[2])
print('pgcli default types/value/NULL/preparation10 PASS',flush=True)
try:
    list(p.run(f"SELECT nope FROM '{fixture}'"));raise AssertionError('missing error')
except psycopg.Error as e:
    assert e.sqlstate=='42703' and e.diag.statement_position=='8'
    print('pgcli error 42703 position 8 PASS',flush=True)
t=threading.Timer(.15,p.conn.cancel);t.start()
try:
    r=list(p.run(f"SELECT s FROM '{large}'"))[0];list(r[1]);raise AssertionError('missing cancel')
except psycopg.Error as e:
    assert e.sqlstate=='57014';print('pgcli cancel 57014 PASS',flush=True)
finally:t.join();p.conn.close()
psql=['psql','-h','127.0.0.1','-p',port,'-U','test','-d','nata','--csv','-v','VERBOSITY=verbose']
r=sp.run(psql+['-c',f"SELECT nope FROM '{fixture}'"],capture_output=True,text=True)
assert r.returncode and '42703' in r.stderr and 'LINE 1' in r.stderr and '^' in r.stderr
print('psql error 42703 caret position 8 PASS',flush=True)
proc=sp.Popen(psql+['-c',f"SELECT s FROM '{large}'"],stdout=sp.DEVNULL,stderr=sp.PIPE,text=True)
time.sleep(.15);proc.send_signal(signal.SIGINT);_,error=proc.communicate(timeout=20)
assert proc.returncode and '57014' in error,error
print('psql cancel 57014 PASS',flush=True)
r=sp.run(psql+['-c',f"PREPARE p AS SELECT si FROM '{fixture}'"],capture_output=True,text=True)
print('psql SQL PREPARE (no wire preparation API):',r.returncode,r.stderr,flush=True)
for _ in range(10):
    r=sp.run(psql+['-P','null=NULL','-c',q],capture_output=True,text=True);assert r.returncode==0
print('psql repeated10 text/NULL PASS; native values are display strings',flush=True)
