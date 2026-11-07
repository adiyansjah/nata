#!/usr/bin/env python3
"""Default-client aggregate value/metadata matrix. No client execution options.

Requires item35 Python dependencies, psql, Java and PGJDBC_JAR. JDBC executes
10 times with default prepareThreshold/binaryTransfer; ADBC uses binary COPY.
R22: report the original whitespace outcome, then trim trailing whitespace only.
"""
import argparse, collections, csv, decimal, importlib.metadata, io, json, os
from pathlib import Path
import shutil, socket, subprocess as sp, time


def run(argv, **kw):
    return sp.run(list(map(str,argv)),check=True,capture_output=True,text=True,**kw).stdout


def queries(data, fixture):
    q1=Path('check/queries/aggregate/q1.sql').read_text().replace('{data}',str(data))
    having=Path('check/queries/aggregate/high_cardinality_tpch.sql').read_text().replace('{data}',str(data))
    return {'q1':q1,'having':having,
      'global':f"SELECT count(*) AS n,count(i) AS nn,sum(i) AS si,sum(b) AS sb,sum(d) AS sd,avg(i) AS ai,avg(d) AS ad,min(i) AS lo,max(i) AS hi FROM '{fixture}'",
      'empty':f"SELECT k,count(*) AS n,sum(i) AS s,avg(i) AS a FROM '{fixture}' WHERE false GROUP BY k",
      'null-keys':f"SELECT k,count(*) AS n,count(i) AS nn,sum(i) AS s,avg(i) AS a FROM '{fixture}' GROUP BY k"}


def canonical(rows):
    def cell(v):
        if v is None or v=='NULL': return ('null',)
        if isinstance(v,bytes): v=v.decode()
        try: return ('num',decimal.Decimal(str(v)))
        except decimal.InvalidOperation: return ('str',str(v))
    return collections.Counter(tuple(cell(v) for v in r) for r in rows)


def equal(expected, actual, name):
    if name!='q1': return canonical(expected)==canonical(actual)
    # DuckDB AVG is DOUBLE; only those three Q1 outputs have the declared
    # aggregate gate tolerance. All keys, counts and SUMs compare exactly.
    def bykey(rows): return {tuple(r[:2]):r for r in rows}
    e,a=bykey(expected),bykey(actual)
    if e.keys()!=a.keys():return False
    for k,r in e.items():
        for i,(x,y) in enumerate(zip(r,a[k])):
            if 6<=i<=8:
                if abs(decimal.Decimal(str(x))-decimal.Decimal(str(y)))>decimal.Decimal('0.00000001'):return False
            elif canonical([[x]])!=canonical([[y]]):return False
    return True


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--candidate',type=Path,required=True);ap.add_argument('--data',type=Path,required=True)
    ap.add_argument('--out',type=Path,required=True);ap.add_argument('--port',type=int,default=5544)
    args=ap.parse_args(); out=args.out.resolve();out.mkdir(parents=True,exist_ok=True)
    fixture=out/'aggregate.parquet'
    fixture.unlink(missing_ok=True)
    run(['duckdb','-c',f"COPY (SELECT * FROM (VALUES (NULL::INTEGER,1::INTEGER,9223372036854775807::BIGINT,1.25::DECIMAL(12,2)),(NULL,2,9223372036854775807,2.50),(1,NULL,NULL,NULL),(2,-1,-1,-1.25)) t(k,i,b,d)) TO '{fixture}' (FORMAT parquet)"])
    qs=queries(args.data.resolve(),fixture)
    # AVG tiny fixtures are independently rational: 2/3 and 2.5/3, rounded
    # half away from zero at scales 6 and 8 respectively.
    expected={}
    for name,q in qs.items():
        text=run(['duckdb','-csv','-nullvalue','NULL','-c',q],env={k:v for k,v in os.environ.items() if k!='AI_AGENT'})
        expected[name]=list(csv.reader(io.StringIO(text)))[1:]
    expected['global'][0][5:7]=['0.666667','0.83333333']
    for row in expected['null-keys']:
        if row[0]=='NULL':row[4]='1.500000'
        elif row[0]=='2':row[4]='-1.000000'
    qdir=out/'queries';qdir.mkdir(exist_ok=True)
    for name,q in qs.items():(qdir/(name+'.sql')).write_text(q)
    import psycopg
    import adbc_driver_postgresql.dbapi as adbc
    from pgcli.pgexecute import PGExecute
    versions={n:importlib.metadata.version(n) for n in ['psycopg','adbc-driver-postgresql','pgcli','pyarrow']}
    versions['psql']=run(['psql','--version']).strip()
    jar=os.environ['PGJDBC_JAR']
    run(['javac','-cp',jar,'-d',out,'crates/nata-pgwire/tests/AggregateClients.java'])
    results=[]
    for mode in ['memory','spill']:
        base=out/mode;base.mkdir(exist_ok=True);spill=base/'spill';spill.mkdir(exist_ok=True)
        run([args.candidate.resolve(),'cleanup-spill',spill.resolve()])
        with (base/'server.log').open('w') as log:
            cmd=[args.candidate.resolve(),'serve','--listen',f'127.0.0.1:{args.port}','--threads','2','--max-queries','1','--spill-directory',spill,'--spill-disk-bytes',str(1<<30)]
            if mode=='spill':cmd+=['--aggregate-memory-bytes',str(64<<20),'--force-aggregate-spill']
            server=sp.Popen(list(map(str,cmd)),stdout=log,stderr=log)
            try:
                for _ in range(100):
                    if server.poll() is not None:raise RuntimeError('server failed')
                    try:
                        with socket.create_connection(('127.0.0.1',args.port),timeout=.1):break
                    except OSError:time.sleep(.05)
                else:raise RuntimeError('startup deadline')
                jdbc=base/'jdbc';jdbc.mkdir(exist_ok=True)
                versions['pgjdbc']=run(['java','-cp',str(out)+os.pathsep+jar,'AggregateClients',args.port,qdir,jdbc]).strip()
                uri=f'postgresql://test@127.0.0.1:{args.port}/nata'
                with psycopg.connect(uri) as psy,adbc.connect(uri) as arrow:
                    pgcli=PGExecute(database='nata',user='test',host='127.0.0.1',port=str(args.port))
                    try:
                        for name,q in qs.items():
                            for client in ['psql','pgjdbc','psycopg','pgcli','adbc']:
                                raw=None
                                if client=='psql':
                                    rows=list(csv.reader(io.StringIO(run(['psql','-h','127.0.0.1','-p',args.port,'-U','test','-d','nata','--csv','-P','null=NULL','-c',q]))))[1:]
                                elif client=='pgjdbc':rows=list(csv.reader((jdbc/(name+'.sql.csv')).open()))
                                elif client=='psycopg':
                                    with psy.cursor() as cur:
                                        for _ in range(10):cur.execute(q);rows=cur.fetchall()
                                        wanted={'global':[20,20,20,1700,1700,1700,1700,23,23],'empty':[23,20,20,1700],'null-keys':[23,20,20,20,1700]}.get(name)
                                        if wanted:assert [col.type_code for col in cur.description]==wanted
                                        for row in rows:
                                            for value,col in zip(row,cur.description):
                                                if value is not None and col.type_code==20:assert type(value) is int
                                                if value is not None and col.type_code==1700:assert isinstance(value,decimal.Decimal)
                                elif client=='pgcli':
                                    response=list(pgcli.run(q));rows=list(response[0][1])
                                else:
                                    with arrow.cursor() as cur:
                                        try:cur.execute(q);cur.fetch_arrow_table();raw='PASS'
                                        except Exception:raw='R22 terminal whitespace failure';arrow.rollback()
                                        cur.execute(q.rstrip());table=cur.fetch_arrow_table()
                                        rows=list(zip(*(c.to_pylist() for c in table.columns)))
                                ok=equal(expected[name],rows,name)
                                entry={'mode':mode,'query':name,'client':client,'rows':len(rows),'values':'PASS' if ok else 'FAIL','readback_types':sorted({type(v).__name__ for r in rows for v in r}), 'jdbc_native_assertions':client=='pgjdbc'}
                                if raw:entry['unnormalized']=raw
                                results.append(entry);print(json.dumps(entry),flush=True)
                                if not ok:raise AssertionError((name,client,expected[name][:2],rows[:2]))
                    finally:pgcli.conn.close()
            finally:
                server.terminate();server.wait(timeout=10)
        assert not list(spill.iterdir()), 'spill leak'
    (out/'matrix.json').write_text(json.dumps({'versions':versions,'results':results},indent=2)+'\n')

if __name__=='__main__':main()
