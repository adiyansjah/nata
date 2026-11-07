"""Default clients, independent cells; failures are recorded and give exit 1.
No settings disable preparation, binary, COPY or TLS preference. No timings.
"""
import argparse, json, os, socket, subprocess, sys, threading, time, struct, importlib.metadata, signal, traceback, re
from pathlib import Path
import psycopg
ap=argparse.ArgumentParser(description=__doc__)
ap.add_argument('--nata',type=Path,required=True); ap.add_argument('--work',type=Path,required=True)
ap.add_argument('--jdk',type=Path); ap.add_argument('--jar',type=Path)
a=ap.parse_args(); a.work.mkdir(parents=True,exist_ok=True)
api_limits=json.loads((Path(__file__).with_name('api-limits.json')).read_text())
(a.work/'api-limits.json').write_text(json.dumps(api_limits,indent=2)+'\n')
root=Path(__file__).resolve().parents[2]; results={}
(a.work/'versions.json').write_text(json.dumps({n:importlib.metadata.version(n) for n in ['psycopg','pgcli','adbc-driver-postgresql','pyarrow']},indent=2)+'\n')
def record(name,fn):
    error=None
    try: fn(); results[name]='PASS'
    except Exception as e:
        error=e
        results[name]='FAIL '+str(getattr(e,'sqlstate',None) or getattr(e,'returncode',None) or type(e).__name__)
        (a.work/(re.sub('[^a-z0-9]+','-',name.lower())+'-error.txt')).write_text(traceback.format_exc()+str(getattr(e,'stderr','')))
    print(name,results[name],flush=True)
    return error
file=a.work/'data.parquet'
subprocess.run(['duckdb','-c',f"COPY (SELECT range AS v FROM range(1000000)) TO '{file}' (FORMAT parquet)"],check=True,capture_output=True)
registry=a.work/'catalog.tsv'; registry.write_text(f'nata\tpublic\tall_types\t{file}\n')
s=socket.socket();s.bind(('127.0.0.1',0)); port=s.getsockname()[1];s.close()
dsn=f'host=127.0.0.1 port={port} user=nata dbname=nata'; query='SELECT v FROM nata.public.all_types LIMIT 3'
expected=b'PGCOPY\n\xff\r\n\x00'+bytes(8)+b''.join(struct.pack('!hiq',1,8,n) for n in range(3))+struct.pack('!h',-1)
log=(a.work/'server.log').open('w'); p=subprocess.Popen([str(a.nata.resolve()),'serve','--catalog',str(registry),'--listen',f'127.0.0.1:{port}','--threads','2','--max-queries','2'],stdout=log,stderr=log)
def checked_rows(rows): assert rows==[(0,),(1,),(2,)], rows
def eq(x,y): assert x==y, (x,y)
def raw_checks():
    sys.path.insert(0,str(root/'scripts/wirecapture')); from protocol import Conn,i,frame
    for n in range(5):
        c=Conn(port);c.query('BEGIN')
        # Equivalent to captured generate_series(1,n), including name and type.
        c.send('P',('s\0SELECT v + 1 AS i FROM nata.public.all_types LIMIT '+str(n)+'\0\0\0').encode())
        c.send('B',b'p\0s\0'+b'\0\0'*3);c.send('D',b'Pp\0');c.send('H')
        setup=c.until(('T',))
        fixture=json.loads((root/f'fixtures/wirecapture/postgresql-15.19-x86/execute-limit-{n}.json').read_text())
        expected_setup=next(st['backend'] for st in fixture['steps'] if st.get('backend',[])[-1:]==[['T',setup[-1][1]]])
        eq(setup,expected_setup)
        for step in fixture['steps']:
            front=step['frontend']; front=front if isinstance(front[0],list) else [front]
            if front[0][0]!='E':continue
            for t,b in front:c.send(t,bytes.fromhex(b))
            eq(c.until(('s','C','E')),step['backend'])
        c.query('ROLLBACK');c.close()
def suspended_checks():
    sys.path.insert(0,str(root/'scripts/wirecapture')); from protocol import Conn,i
    outcomes={}
    def error_without_source(body):
        fields=bytes.fromhex(body).split(b'\0')
        return b'\0'.join(f for f in fields if not f[:1] in (b'F',b'L',b'R')).hex()
    for cpu in ('arm','x86'):
        for name in ('suspended-cancel','suspended-timeout-restart'):
            fixture=json.loads((root/f'fixtures/wirecapture/postgresql-15.19-{cpu}/{name}.json').read_text())
            c=Conn(port)
            try:
                for step in fixture['steps']:
                    if 'sleep_ms' in step:time.sleep(step['sleep_ms']/1000)
                    if 'concurrent' in step:
                        with socket.create_connection(('127.0.0.1',port)) as k:
                            k.sendall(i(16)+i(80877102)+c.key);eq(k.recv(1),b'')
                    front=step['frontend'];front=front if isinstance(front[0],list) else [front]
                    for tag,body in front:
                        body=bytes.fromhex(body)
                        if tag=='P':
                            query='SELECT v + 1 AS i'+(", CAST('' AS VARCHAR) AS pg_sleep" if name=='suspended-timeout-restart' else '')+' FROM nata.public.all_types LIMIT 3'
                            body=b's\0'+query.encode()+b'\0\0\0'
                        c.send(tag,body)
                    if 'backend' not in step:continue
                    expected=step['backend'];actual=c.until((expected[-1][0],))
                    # generate_series is replaced with the same ordered Parquet
                    # rows. pg_sleep's empty void result becomes empty text; its
                    # wire OID/size differ. PG implementation-source error fields
                    # are not fabricated by Nata. All other payload bytes agree.
                    if name=='suspended-timeout-restart':
                        expected=[[t,error_without_source(b) if t=='E' else b.replace('000008e60004','00000019ffff') if t=='T' else b] for t,b in expected]
                    eq(actual,expected)
                outcomes[cpu+' '+name]='PASS: exact bytes' if name=='suspended-cancel' else 'PASS after declared void description and PG error-source projection; full byte parity remains open'
            finally:c.close()
    (a.work/'suspended-fixtures.json').write_text(json.dumps(outcomes,indent=2)+'\n')
def malformed():
    sys.path.insert(0,str(root/'scripts/wirecapture')); from protocol import Conn,i,frame
    c=Conn(port);c.s.sendall(b'Q'+i(3));tag,body=frame(c.s);assert tag=='E' and b'C08P01\0' in body;c.s.close();healthy=Conn(port);eq(healthy.query('SELECT 1')[-1][0],'Z');healthy.close()
try:
    for _ in range(200):
        try:
            with socket.create_connection(('127.0.0.1',port),timeout=.1):pass
            break
        except OSError:
            if p.poll() is not None:raise RuntimeError('server exited')
            time.sleep(.02)
    def cli(q):return subprocess.run(['psql',dsn,'-c',q],capture_output=True,check=True,timeout=20).stdout
    for name,q in [('connect','SELECT 1'),('select',query),('d','\\d public.all_types'),('dt','\\dt'),('dn','\\dn'),('df','\\df'),('l','\\l')]: record('psql '+name,lambda q=q:cli(q))
    record('psql chunked-delivery',lambda:eq(subprocess.run(['psql',dsn,'-qAt','-v','FETCH_COUNT=2','-c',query],capture_output=True,check=True,timeout=20).stdout,b'0\n1\n2\n'))
    record('psql copy',lambda:eq(cli(f'COPY ({query}) TO STDOUT (FORMAT binary)'),expected))
    def copy_sql(form):
        if form=='table':return 'COPY nata.public.all_types TO STDOUT WITH (FORMAT csv, HEADER true)'
        return f'COPY ({query}) TO STDOUT'+(' (FORMAT csv, HEADER true)' if form=='csv' else '')
    def check_copy(form,b):
        if form=='table':
            lines=b.decode().splitlines();eq(lines[0],'v');eq([int(x) for x in lines[1:]],list(range(1000000)))
        else:eq(b,b'v\n0\n1\n2\n' if form=='csv' else b'0\n1\n2\n')
    for form in ('text','csv','table'):record('psql copy-'+form,lambda form=form:check_copy(form,cli(copy_sql(form))))

    def psql_script(action):
        prefix = 'SET statement_timeout=1;' if action=='timeout' else ''
        failing = 'SELECT v FROM nata.public.all_types;' if action in ('timeout','cancel') else 'SELECT v/0 FROM nata.public.all_types LIMIT 1;'
        proc=subprocess.Popen(['psql',dsn,'-qAt'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        if action=='cancel':
            # Establish startup completion before SIGINT, without changing driver settings.
            proc.stdin.write('\\echo READY\n');proc.stdin.flush()
            eq(proc.stdout.readline().strip(),'READY')
            proc.stdin.write(failing+'\n');proc.stdin.flush()
            time.sleep(.05);proc.send_signal(signal.SIGINT)
            proc.stdin.write('SET statement_timeout=0;\n'+query+';\n');proc.stdin.close();proc.stdin=None
            out,err=proc.communicate(timeout=30)
        else:out,err=proc.communicate(prefix+failing+'\nSET statement_timeout=0;\n'+query+';\n',timeout=30)
        assert ('canceling statement' in err if action in ('timeout','cancel') else 'division by zero' in err),err
        if action=='cancel':cli(query) # psql may terminate its input script on SIGINT.
        else:assert out.endswith('0\n1\n2\n'),out[-100:]
    for action in ('timeout','cancel','recovery'):record('psql '+action,lambda action=action:psql_script(action))

    def psy_action(action):
        with psycopg.connect(dsn) as c:
            if action=='select':
                for _ in range(10):checked_rows(c.execute(query).fetchall())
            elif action=='sql-cursor-refusal':
                try:
                    with c.cursor(name='matrix_cursor') as cur:cur.execute(query)
                    raise AssertionError('SQL cursor unexpectedly accepted')
                except psycopg.errors.FeatureNotSupported:pass
                c.rollback();checked_rows(c.execute(query).fetchall())
            elif action=='fetchmany-client-buffer':
                with c.cursor() as cur:
                    cur.execute(query);eq(cur.fetchmany(2),[(0,),(1,)]);eq(cur.fetchmany(2),[(2,)]);eq(cur.fetchmany(2),[])
            elif action=='tables':eq(c.execute('SELECT table_name FROM information_schema.tables').fetchall(),[('all_types',)])
            elif action=='columns':eq(c.execute('SELECT column_name FROM information_schema.columns').fetchall(),[('v',)])
            elif action in ('tables-filtered','columns-filtered'):
                relation='tables' if action=='tables-filtered' else 'columns'
                c.execute('SELECT * FROM information_schema.'+relation+' WHERE table_schema=%s',('public',)).fetchall()
            elif action=='types':
                c.execute('SELECT n.nspname,t.typname,t.oid,t.typtypmod FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace').fetchall()
            elif action.startswith('copy-'):
                form=action.split('-',1)[1]
                with c.cursor().copy(copy_sql(form)) as cp:check_copy(form,b''.join(bytes(x) for x in cp))
            elif action=='copy':
                with c.cursor().copy(f'COPY ({query}) TO STDOUT (FORMAT binary)') as cp:eq(b''.join(bytes(x) for x in cp),expected)
            elif action=='recovery':
                try:c.execute('SELECT v/0 FROM nata.public.all_types LIMIT 1');raise AssertionError('no error')
                except psycopg.errors.DivisionByZero:pass
                try:c.execute('SELECT 1');raise AssertionError('no aborted state')
                except psycopg.errors.InFailedSqlTransaction:pass
                c.rollback();checked_rows(c.execute(query).fetchall())
            elif action in ('cancel','timeout'):
                t=None
                if action=='timeout':c.execute('SET statement_timeout=1')
                else:t=threading.Thread(target=lambda:(time.sleep(.03),c.cancel()));t.start()
                try:c.execute('SELECT v FROM nata.public.all_types').fetchall();raise AssertionError('no interruption')
                except psycopg.errors.QueryCanceled:pass
                finally:
                    if t:t.join()
                c.rollback();c.execute('SET statement_timeout=0');checked_rows(c.execute(query).fetchall())
    record('psycopg connect',lambda:psycopg.connect(dsn).close())
    for action in ('select','sql-cursor-refusal','fetchmany-client-buffer','tables','columns','tables-filtered','columns-filtered','types','copy','copy-text','copy-csv','copy-table','timeout','cancel','recovery'):record('psycopg '+action,lambda action=action:psy_action(action))
    record('raw fixture-byte-comparison',raw_checks)
    record('raw suspended-fixture-projections',suspended_checks)
    record('raw malformed-frame',malformed)
    def pgcli(action):
        from pgcli.pgexecute import PGExecute
        p=PGExecute(database='nata',user='nata',host='127.0.0.1',port=port)
        def pg_rows(sql):
            # PGExecute's ProtocolSafeCursor.execute intentionally returns None.
            with p.conn.cursor() as cur:cur.execute(sql);return cur.fetchall()
        try:
            if action=='connect':pass
            elif action=='select':
                with p.conn.cursor() as c:c.execute(query);checked_rows(c.fetchall())
            elif action=='copy':
                with p.conn.cursor().copy(f'COPY ({query}) TO STDOUT (FORMAT binary)') as cp:eq(b''.join(bytes(x) for x in cp),expected)
            elif action=='fetchmany-client-buffer':
                with p.conn.cursor() as cur:
                    cur.execute(query);eq(cur.fetchmany(2),[(0,),(1,)]);eq(cur.fetchmany(2),[(2,)]);eq(cur.fetchmany(2),[])
            elif action=='recovery':
                try:p.conn.execute('SELECT v/0 FROM nata.public.all_types LIMIT 1');raise AssertionError('no error')
                except psycopg.errors.DivisionByZero:pass
                checked_rows(pg_rows(query))
            elif action in ('timeout','cancel'):
                t=None
                if action=='timeout':p.conn.execute('SET statement_timeout=1')
                else:t=threading.Thread(target=lambda:(time.sleep(.03),p.conn.cancel()));t.start()
                try:pg_rows('SELECT v FROM nata.public.all_types');raise AssertionError('no interruption')
                except psycopg.errors.QueryCanceled:pass
                finally:
                    if t:t.join()
                p.conn.execute('SET statement_timeout=0');checked_rows(pg_rows(query))
            else:
                # Exercise each default refresher synchronously: the normal background
                # wrapper logs errors and can mask a failed refresh from its caller.
                from pgcli.completion_refresher import CompletionRefresher
                from pgcli.pgcompleter import PGCompleter
                r=CompletionRefresher(); completer=PGCompleter()
                failure=None
                for name,fn in r.refreshers.items():
                    e=record('pgcli completion '+name,lambda fn=fn:fn(completer,p))
                    if e is not None:failure=e
                if failure is not None:raise failure
        finally:p.conn.close()
    for action in ('connect','select','fetchmany-client-buffer','completion','copy','timeout','cancel','recovery'):record('pgcli '+action,lambda action=action:pgcli(action))
    def adbc(action):
        import adbc_driver_postgresql.dbapi as db
        with db.connect(f'postgresql://nata@127.0.0.1:{port}/nata') as c:
            if action=='connect':pass
            elif action=='info':
                info=c.adbc_get_info();eq(info['vendor_name'],'PostgreSQL');assert info['vendor_version']
            elif action=='fetchmany-client-buffer':
                with c.cursor() as cur:
                    cur.execute(query);eq(cur.fetchmany(2),[(0,),(1,)]);eq(cur.fetchmany(2),[(2,)]);eq(cur.fetchmany(2),[])
            elif action in ('select','copy'):
                with c.cursor() as cur:cur.execute(query);eq(cur.fetch_arrow_table().column(0).to_pylist(),[0,1,2])
            elif action in ('timeout','cancel','recovery'):
                t=None
                with c.cursor() as cur:
                    if action=='timeout':cur.execute('SET statement_timeout=1')
                    elif action=='cancel':t=threading.Thread(target=lambda:(time.sleep(.03),cur.adbc_cancel()));t.start()
                    try:
                        cur.execute('SELECT v/0 FROM nata.public.all_types LIMIT 1' if action=='recovery' else 'SELECT v FROM nata.public.all_types')
                        cur.fetch_arrow_table();raise AssertionError('no interruption')
                    except db.Error as e:
                        assert ('22012' in str(e) or 'division by zero' in str(e)) if action=='recovery' else ('57014' in str(e) or 'cancel' in str(e)),str(e)
                    finally:
                        if t:t.join()
                    c.rollback();cur.execute('SET statement_timeout=0');cur.execute(query);eq(cur.fetch_arrow_table().column(0).to_pylist(),[0,1,2])
            elif action=='schema':eq(c.adbc_get_table_schema('all_types',db_schema_filter='public').names,['v'])
            else:
                with c.adbc_get_objects(depth=action.split('-',1)[1]) as r:r.read_all()
    for action in ('connect','info','select','fetchmany-client-buffer','copy','objects-all','objects-catalogs','objects-db_schemas','objects-tables','objects-columns','schema','timeout','cancel','recovery'):record('ADBC '+action,lambda action=action:adbc(action))
    if a.jar:
        javac=str(a.jdk/'javac') if a.jdk else 'javac'; java=str(a.jdk/'java') if a.jdk else 'java'
        subprocess.run([javac,'-cp',str(a.jar),'-d',str(a.work),str(root/'scripts/wire-skeleton/Clients.java')],check=True,capture_output=True)
        j=subprocess.run([java,'-cp',str(a.work)+os.pathsep+str(a.jar),'Clients',str(port),str(file)],capture_output=True,text=True,timeout=120,check=True)
        (a.work/'jdbc.txt').write_text(j.stdout+j.stderr);print(j.stdout,flush=True)
        for line in j.stdout.splitlines():
            if ': ' in line:k,v=line.split(': ',1);results['pgjdbc '+k]=v
    else:results['pgjdbc']='not run: --jar absent'
    # DBeaver 25.2.0 source-derived native PostgreSQL navigator path.
    # Explicit fallback JAR is distinct from the generic JDBC matrix JAR.
    dbeaver_jar=os.environ.get('DBEAVER_JAR')
    if dbeaver_jar:
        corpus=json.loads((root/'scripts/wire-skeleton/dbeaver.json').read_text())
        files=[]
        for n,item in enumerate(corpus['queries']):
            f=a.work/f'dbeaver-{n}.sql';f.write_text(item['sql']);files.append(str(f))
        subprocess.run(['javac','-cp',dbeaver_jar,'-d',str(a.work),str(root/'scripts/wire-skeleton/DBeaverReplay.java')],check=True)
        replay=subprocess.run(['java','-cp',str(a.work)+':'+dbeaver_jar,'DBeaverReplay',str(port),*files],check=True,capture_output=True,text=True)
        (a.work/'dbeaver-replay.txt').write_text(replay.stdout)
        for line in replay.stdout.splitlines():
            if ': ' in line:
                n,result=line.split(': ',1)
                results['DBeaver source '+corpus['queries'][int(n)]['action']]=result
finally:
    for client in ('psql','pgjdbc','psycopg','pgcli','ADBC'):
        for action in ('positive-fetch','copy','timeout','cancel','recovery','malformed-frame'):
            key=client+' '+action
            if key not in results:
                results[key]='not run: '+api_limits.get(key,{}).get('reason','client-specific probe absent; raw/protocol suite is separate evidence')
    p.terminate()
    try:p.wait(timeout=10)
    except subprocess.TimeoutExpired:p.kill();p.wait()
    log.close();(a.work/'matrix.json').write_text(json.dumps(results,indent=2)+'\n')
raise SystemExit(int(any(v!='PASS' and not v.startswith('PASS ') for v in results.values())))
