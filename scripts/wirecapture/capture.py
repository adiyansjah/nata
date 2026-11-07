"""PostgreSQL-only metadata capture and five-round, alternating-twin references."""
import os, sys, json, subprocess as sp, time, statistics, importlib.metadata
from pathlib import Path
from protocol import Proxy, sequences
out=Path(sys.argv[1]); work=Path(os.environ['WCAP_DIR']); root=Path(__file__).parent
DSN='host=127.0.0.1 port=55433 user=nata dbname=nata'
def run(cmd,**kw):
    # A pipe makes communicate wait on EOF; wait(timeout) alone polls in 50 ms steps.
    # Fixed after the two permitted cloud attempts; see the reference record.
    kw.setdefault('stderr',sp.PIPE)
    return sp.run(cmd,check=True,timeout=180,**kw)
versions={x:importlib.metadata.version(x) for x in ['psycopg','pgcli','adbc-driver-postgresql','pyarrow','pglast']}
versions['psql']=sp.check_output(['psql','--version'],text=True).strip()
import psycopg
with psycopg.connect('host=127.0.0.1 port=55432 user=nata dbname=nata',autocommit=True) as c:
    versions['postgresql']=c.execute('SELECT version()').fetchone()[0]
(out/'versions.json').write_text(json.dumps(versions,indent=2)+'\n')
proxy=Proxy(out/'protocol.jsonl')
results={}
def action(name,f):
    first=proxy.counter+1
    try: f(); results[name]='PASS'
    except Exception as e: results[name]=dict(status='FAIL',error=str(e))
    with (out/'action-connections.jsonl').open('a') as log: log.write(json.dumps(dict(action=name,first=first,last=proxy.counter))+'\n')
    (out/'actions.json').write_text(json.dumps(results,indent=2)+'\n')
with (out/'psql.txt').open('w') as f:
    for command in ['\\dt','\\dt *.*','\\dt empty.*','\\dt "Quoted Schema".*','\\d public.all_types','\\d "Quoted Schema"."Mixed Table"','\\dn','\\df','\\l',"\\copy public.all_types TO STDOUT WITH (FORMAT csv, HEADER true)","\\copy (SELECT * FROM public.all_types) TO STDOUT"]:
        action('psql '+command,lambda:run(['psql',DSN,'-c',command],stdout=f,stderr=f))
def jdbc():
    with (out/'jdbc.txt').open('w') as f: run(['java','-cp',str(work)+':'+str(work/'pgjdbc.jar'),'Metadata','55433'],stdout=f,stderr=f)
action('pgjdbc metadata and CopyManager',jdbc)
def psy():
    with psycopg.connect(DSN) as c:
        for q in ["SELECT * FROM information_schema.tables WHERE table_schema=%s", "SELECT * FROM information_schema.columns WHERE table_schema=%s"]: c.execute(q,('public',)).fetchall()
        c.execute('SELECT n.nspname,t.typname,t.oid,t.typtypmod FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace').fetchall()
action('psycopg explicit metadata SQL',psy)
def pgcli():
    from pgcli.pgexecute import PGExecute
    from pgcli.completion_refresher import CompletionRefresher
    p=PGExecute(database='nata',user='nata',host='127.0.0.1',port=55433)
    # Same generator map the interactive completion refresher invokes.
    r=CompletionRefresher()
    for name,fn in r.refreshers.items():
        print('completion',name,flush=True)
        # Methods populate a real PGCompleter, rather than dropping generators.
    from pgcli.pgcompleter import PGCompleter
    completer=PGCompleter()
    r._bg_refresh(p,None,lambda c: None)
    p.conn.close()
action('pgcli completion refresh',pgcli)
def adbc():
    import adbc_driver_postgresql.dbapi as db
    with db.connect('postgresql://nata@127.0.0.1:55433/nata') as c:
        for depth in ['all','catalogs','db_schemas','tables','columns']:
            with c.adbc_get_objects(depth=depth) as r: r.read_all()
        c.adbc_get_table_schema('all_types',db_schema_filter='public')
        with c.cursor() as cur:
            cur.execute('SELECT * FROM public.all_types'); cur.fetch_arrow_table()
action('ADBC get_objects and get_table_schema default COPY',adbc)
sequences(out/'fixtures')
with psycopg.connect('host=127.0.0.1 port=55432 user=nata dbname=nata',autocommit=True) as c:
    rows=c.execute("SELECT n.nspname,c.relname,a.attname FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE a.attnum>0 AND NOT a.attisdropped AND n.nspname IN ('pg_catalog','information_schema')").fetchall()
    catalog={}
    for ns,t,col in rows: catalog.setdefault(ns+'.'+t,[]).append(col)
    (out/'catalog-columns.json').write_text(json.dumps(catalog,indent=2)+'\n')
# Stop server SQL logging before timing: proxy is bypassed, stdout discarded.
with psycopg.connect('host=127.0.0.1 port=55432 user=nata dbname=nata',autocommit=True) as c:
    c.execute("ALTER SYSTEM SET log_statement='none'"); c.execute('SELECT pg_reload_conf()')
base='host=127.0.0.1 port=55432 user=nata dbname=nata'
query='SELECT v FROM million'
def psybench():
    with psycopg.connect(base) as c:
        with c.cursor() as cur:
            cur.execute(query)
            for n,row in enumerate(cur): assert row==(n,)
            assert n==999999
def adbcbench():
    import adbc_driver_postgresql.dbapi as db
    with db.connect('postgresql://nata@127.0.0.1:55432/nata') as c:
        with c.cursor() as cur:
            cur.execute(query); t=cur.fetch_arrow_table()
            assert t.column(0).to_pylist()==list(range(1000000))
commands={
 'psql':lambda:run(['psql',base,'-X','-A','-t','-c',query],stdout=sp.DEVNULL),
 'raw-text':lambda:run([str(work/'raw'),'text'],stdout=sp.DEVNULL),
 'raw-binary':lambda:run([str(work/'raw'),'binary'],stdout=sp.DEVNULL),
 'jdbc-fetch1000':lambda:run(['java','-cp',str(work)+':'+str(work/'pgjdbc.jar'),'Metadata','55432','bench'],stdout=sp.DEVNULL),
 'psycopg':psybench,'adbc-copy':adbcbench}
# Independently check the psql output before timing it to /dev/null.
p=sp.Popen(['psql',base,'-X','-A','-t','-c',query],stdout=sp.PIPE,text=True)
for n,line in enumerate(p.stdout): assert line.strip()==str(n)
assert n==999999 and p.wait()==0
samples=[]
for workers in [1,8]:
    # PostgreSQL parallel workers are a ceiling, not Nata's fixed scheduler share.
    with psycopg.connect(base,autocommit=True) as c:
        c.execute(f'ALTER SYSTEM SET max_parallel_workers_per_gather={workers-1}')
        c.execute('SELECT pg_reload_conf()')
    for name,fn in commands.items():
        settle=sp.run(['bash',str(root.parent/'gcp/settle.sh'),name,'0.5','600'],capture_output=True,text=True)
        (out/f'settle-{workers}-{name}.txt').write_text(settle.stdout+settle.stderr)
        if settle.returncode: results[f'timing-{workers}-{name}']='UNSETTLED'; continue
        fn() # warm correctness-checked control
        for rnd in range(5):
            for arm in (['primary','twin'] if rnd%2==0 else ['twin','primary']):
                start=time.perf_counter(); fn(); secs=time.perf_counter()-start
                samples.append(dict(workers=workers,client=name,round=rnd,arm=arm,seconds=secs))
                (out/'timings.json').write_text(json.dumps(samples,indent=2)+'\n')
(out/'actions.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))

# Additional bounded catalog-shape reference; results are metadata, no real data.
browse=[]
with psycopg.connect(base,autocommit=True) as c:
    c.execute('CREATE SCHEMA scale')
    previous=0
    for size in [64,1024,10000]:
        for n in range(previous,size):
            fields=','.join('c'+str(j)+' bigint' for j in range(64 if n%16==0 else 1))
            c.execute('CREATE TABLE scale.t'+str(n)+' ('+fields+')')
        previous=size
        settle=sp.run(['bash',str(root.parent/'gcp/settle.sh'),'browse-'+str(size),'0.5','600'],capture_output=True,text=True)
        (out/f'settle-browse-{size}.txt').write_text(settle.stdout+settle.stderr)
        if settle.returncode: continue
        for rnd in range(5):
            for arm in (['primary','twin'] if rnd%2==0 else ['twin','primary']):
                start=time.perf_counter()
                tables=c.execute("SELECT table_name FROM information_schema.tables WHERE table_schema='scale' ORDER BY table_name").fetchall()
                cols=c.execute("SELECT table_name,column_name,data_type,is_nullable,ordinal_position FROM information_schema.columns WHERE table_schema='scale' ORDER BY table_name,ordinal_position").fetchall()
                elapsed=time.perf_counter()-start
                assert len(tables)==size
                browse.append(dict(tables=size,columns=len(cols),round=rnd,arm=arm,seconds=elapsed,json_bytes=len(json.dumps(cols))))
        (out/'browse.json').write_text(json.dumps(browse,indent=2)+'\n')
