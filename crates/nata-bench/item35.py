#!/usr/bin/env python3
"""Default-client corpus probes. Capture each mismatch; never hide partial coverage.
Native readback/preparation/cancel use the permanent Clients4 probes separately.
"""
import csv
import io
import json
import math
import struct
from decimal import Decimal
import runpy
import os
from pathlib import Path
import subprocess as sp
import sys
import psycopg
import adbc_driver_postgresql.dbapi as adbc
from pgcli.pgexecute import PGExecute

nata, port, data, out, jdbc = sys.argv[1:]
data=Path(data).resolve(); out=Path(out);out.mkdir(parents=True,exist_ok=True)
os.environ.pop('AI_AGENT',None)
uri=f'postgresql://test@127.0.0.1:{port}/nata'
import importlib.metadata
versions='\n'.join(f'{n}=={importlib.metadata.version(n)}' for n in ['psycopg','pgcli','adbc-driver-postgresql','pyarrow'])+'\n'
(out/'versions.txt').write_text(versions)
clients={}
for name, connect in [('psycopg',lambda:psycopg.connect(uri)),('adbc',lambda:adbc.connect(uri)),('pgcli',lambda:PGExecute(database='nata',user='test',host='127.0.0.1',port=port))]:
    try:clients[name]=connect()
    except Exception as e: clients[name]=None;print(name,'CONNECT FAIL',str(e),flush=True)

def records(text): return list(csv.reader(io.StringIO(text)))
def normalize(rows):
    # Same text comparator policy as nata-check: floating spellings are compared
    # numerically only when the corpus explicitly supplies a tolerance directive.
    return sorted(rows)
raw_type=runpy.run_path(str(Path(__file__).with_name('pgwire-bench.py')))['Client']
raw=raw_type(port)
def canonical(rows,oids):
    def cell(v,oid):
        if v=='NULL':return ('null',)
        if oid==16:return ('bool',v.lower() in ('t','true'))
        if oid in (700,701):
            n=float(v)
            if math.isnan(n):return ('nan',)
            if oid==700:n=struct.unpack('!f',struct.pack('!f',n))[0]
            return ('float',n)
        if oid in (20,21,23,1700):return ('numeric',Decimal(v))
        return ('text',v)
    return sorted([tuple(cell(v,o) for v,o in zip(row,oids)) for row in rows],key=repr)
results=[]
queries=sorted((Path('check/queries/sql')).glob('*.sql'))
for path in queries:
    q=path.read_text().replace('{data}',str(data))
    p=sp.run([nata,'sql','--csv','-c',q],capture_output=True,text=True)
    if p.returncode:continue # only queries nata sql answers, as required
    expected=records(p.stdout)
    oids=raw.query(q)['oids']
    # Full oracle, including its documented tolerances, runs as nata-check job step.
    entry={'query':path.name,'clients':{}}
    for name in ['psql','pgjdbc','psycopg','pgcli','adbc']:
        try:
            if name=='psql':
                r=sp.run(['psql','-h','127.0.0.1','-p',port,'-U','test','-d','nata','--csv','-P','null=NULL','-c',q],capture_output=True,text=True)
                if r.returncode:raise RuntimeError(r.stderr)
                actual=records(r.stdout)
            elif name=='pgjdbc':
                actual=records((Path(jdbc)/(path.name+'.csv')).read_text())
            elif clients[name] is None:raise RuntimeError('connection unavailable')
            elif name=='psycopg':
                c=clients[name];cur=c.execute(q)
                native=cur.fetchall(); pg=cur.pgresult
                actual=[[d.name for d in cur.description]]+[["NULL" if pg.get_value(i,j) is None else pg.get_value(i,j).decode() for j in range(pg.nfields)] for i in range(pg.ntuples)]
            elif name=='pgcli':
                r=list(clients[name].run(q))
                actual=[list(r[0][2])]+[["NULL" if v is None else str(v) for v in row] for row in r[0][1]]
            else:
                cur=clients[name].cursor();cur.execute(q);table=cur.fetch_arrow_table()
                actual=[table.column_names]+[["NULL" if v is None else str(v) for v in row.values()] for row in table.to_pylist()]
            ok=actual[0]==expected[0] and normalize(actual[1:])==normalize(expected[1:])
            values_ok=actual[0]==expected[0] and canonical(actual[1:],oids)==canonical(expected[1:],oids)
            entry['clients'][name]={'status':'PASS' if ok else 'TEXT DIFFERENCE','values':'PASS' if values_ok else 'FAIL','rows':len(actual)-1}
            if not ok:entry['clients'][name]['cause']='client/PG type text representation' if values_ok else 'unclassified value or metadata difference; investigate'

            if not ok:entry['clients'][name]['example']={'expected':expected[:2],'actual':actual[:2]}
        except Exception as e:
            entry['clients'][name]={'status':'FAIL','error':str(e)}
            if name=='psycopg':clients[name].rollback()
    results.append(entry)
    print(json.dumps(entry),flush=True)
(out/'corpus.json').write_text(json.dumps(results,indent=2)+'\n')
for name,c in clients.items():
    if c is not None:
        try:c.close() if hasattr(c,'close') else c.conn.close()
        except Exception:pass
raw.close()
assert len(results)==221, f'incomplete successful corpus: {len(results)}'
