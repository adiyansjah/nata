#!/usr/bin/env python3
"""Same-VM reference-first performance gate; smoke has no acceptance verdict."""
import argparse
import json
import os
from pathlib import Path
import socket
import statistics as st
import subprocess as sp
import time
import psycopg
import pyarrow as pa
import pyarrow.parquet as pq

ROOT=Path(__file__).resolve().parents[2]

def summary(primary,twin):
    median=st.median(primary);other=st.median(twin)
    return dict(min=min(primary+twin),median=median,spread=max(primary+twin)-min(primary+twin),twin_median=other,twin_noise=abs(median-other)/min(median,other))

def verdict(reference,current,baseline,kind='stream'):
    if reference['twin_noise']>.05:return 'UNRESOLVED'
    return 'MET' if (current['median']/reference['median']<=1.10 if kind=='stream' else current['median']/baseline['median']<=1.20) else 'NOT MET'

def client(a):
    query='SELECT v FROM million' if a.kind=='stream' else 'SELECT v FROM million LIMIT 3'
    rows=a.rows if a.kind=='stream' else 3
    if a.client=='psql':
        p=sp.run(['psql',a.dsn,'-X','-A','-t','-c',query],check=True,capture_output=True,text=True)
        assert p.stdout.splitlines()==[str(n) for n in range(rows)]
    elif a.client.startswith('raw-'):
        # Same libpq-free std-only reader, built once; validates every value.
        sp.run([str(a.work/'raw'),'binary' if a.client=='raw-binary' else 'text',str(a.port),str(rows)],check=True,stdout=sp.DEVNULL)
    elif a.client=='jdbc':
        sp.run(['java','-cp',str(a.work)+':'+str(a.jar),'P16',str(a.port),str(rows)],check=True,stdout=sp.DEVNULL)
    elif a.client=='adbc-copy':
        import adbc_driver_postgresql.dbapi as db
        with db.connect(f'postgresql://nata@127.0.0.1:{a.port}/nata') as c:
            with c.cursor() as cur:
                cur.execute(query);assert cur.fetch_arrow_table().column(0).to_pylist()==list(range(rows))
    elif a.client=='pgcli':
        from pgcli.pgexecute import PGExecute
        c=PGExecute(database='nata',user='nata',host='127.0.0.1',port=a.port)
        try:
            result=list(c.run(query));assert list(result[0][1])==[(n,) for n in range(rows)]
        finally:c.conn.close()
    else:
        with psycopg.connect(a.dsn) as c:
            c.prepare_threshold=1
            for _ in range(1 if a.kind=='stream' else 8):
                assert c.execute(query).fetchall()==[(n,) for n in range(rows)]

def main(a):
    a.work=a.work.resolve();a.work.mkdir(parents=True,exist_ok=True)
    if a.client:return client(a)
    clients=['psql','raw-text','raw-binary','jdbc','psycopg','adbc-copy','pgcli']
    a.rows=100 if a.smoke else 1000000
    raw=(ROOT/'scripts/wirecapture/raw.rs').read_text().replace('127.0.0.1:55432','127.0.0.1:{}').replace('TcpStream::connect("127.0.0.1:{}")','TcpStream::connect(format!("127.0.0.1:{}",std::env::args().nth(2).unwrap()))').replace('assert_eq!(n,1000000)','assert_eq!(n,std::env::args().nth(3).unwrap().parse::<i64>().unwrap())')
    # Reader query is bounded identically for stream and small-query arms.
    raw=raw.replace('SELECT v FROM million','SELECT v FROM million LIMIT ').replace('b"\\0SELECT v FROM million LIMIT \\0\\0\\0"','format!("\\0SELECT v FROM million LIMIT {}\\0\\0\\0",std::env::args().nth(3).unwrap()).as_bytes()').replace('b"SELECT v FROM million LIMIT \\0"','format!("SELECT v FROM million LIMIT {}\\0",std::env::args().nth(3).unwrap()).as_bytes()')
    (a.work/'raw.rs').write_text(raw)
    sp.run(['rustc','--edition=2021','-O',str(a.work/'raw.rs'),'-o',str(a.work/'raw')],check=True)
    java='''import java.sql.*; public class P16 {public static void main(String[] a)throws Exception{try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata?user=nata")){c.setAutoCommit(false);try(PreparedStatement s=c.prepareStatement("SELECT v FROM million LIMIT "+a[1])){s.setFetchSize(1000);for(int k=0;k<(Long.parseLong(a[1])>3?1:8);k++){ResultSet r=s.executeQuery();long n=0;while(r.next())if(r.getLong(1)!=n++)throw new Exception("value");if(n!=Long.parseLong(a[1]))throw new Exception("count");}}c.rollback();}}}'''
    (a.work/'P16.java').write_text(java)
    sp.run(['javac','-cp',str(a.jar),'-d',str(a.work),str(a.work/'P16.java')],check=True)
    pq.write_table(pa.table({'v':pa.array(range(a.rows),type=pa.int64())}),a.work/'million.parquet')
    (a.work/'catalog.tsv').write_text(f'nata\tpublic\tmillion\t{a.work}/million.parquet\n')
    with psycopg.connect(a.postgres,autocommit=True) as c:
        c.execute('DROP TABLE IF EXISTS million');c.execute('CREATE TABLE million(v bigint)')
        c.execute(f'INSERT INTO million SELECT generate_series(0,{a.rows-1})::bigint')
        context=dict(postgres=c.execute('SELECT version()').fetchone()[0],source=sp.check_output(['bash','scripts/code-tree.sh'],cwd=ROOT,text=True).strip(),rows=a.rows,smoke=a.smoke,limits='warm, loopback, client process startup and validation included; PostgreSQL heap vs Nata Parquet end-to-end, no pure codec claim')
    (a.work/'context.json').write_text(json.dumps(context,indent=2)+'\n')
    samples={};rounds=1 if a.smoke else 5
    def measure(label,port,workers,peer_port=None,server_pid=None):
        dsn=a.postgres if label=='postgres' else f'host=127.0.0.1 port={port} user=nata dbname=nata'
        for name in clients:
            for kind in ['stream','small']:
                if not a.smoke:sp.run(['bash',str(ROOT/'scripts/gcp/settle.sh'),f'p16-{label}-{workers}-{name}-{kind}','0.5','600'],check=True)
                cmd=[os.sys.executable,__file__,'--client',name,'--kind',kind,'--dsn',dsn,'--port',str(port),'--rows',str(a.rows),'--work',str(a.work),'--jar',str(a.jar)]
                sp.run(cmd,check=True,timeout=180) # correctness and warmup before timing
                values={'primary':[],'twin':[]}
                baseline=[]
                for rnd in range(rounds):
                    for arm in (['primary','twin'] if rnd%2==0 else ['twin','primary']):
                        paired=[('current',cmd)]
                        if peer_port:
                            oldcmd=[str(peer_port) if x==str(port) else x.replace(f'port={port}',f'port={peer_port}') for x in cmd]
                            paired=[('current',cmd),('baseline',oldcmd)]
                            if rnd%2:paired.reverse()
                        for variant,command in paired:
                            start=time.perf_counter();sp.run(command,check=True,timeout=180);elapsed=time.perf_counter()-start
                            if variant=='current':values[arm].append(elapsed)
                            else:baseline.append(elapsed)
                samples[f'{label}/{workers}/{name}/{kind}']=dict(**summary(**values),samples=values)
                if baseline:samples[f'baseline/{workers}/{name}/{kind}']=dict(**summary(baseline[::2],baseline[1::2]),samples=baseline)
                (a.work/f'{label}-timings.json').write_text(json.dumps(samples,indent=2)+'\n')
                if label=='nata' and not a.smoke:
                    key=f'{workers}/{name}/{kind}'
                    state=verdict(samples['postgres/'+key],samples['nata/'+key],samples['baseline/'+key],kind)
                    if state=='NOT MET':
                        profile=a.work/f'profile-{workers}-{name}-{kind}.data'
                        sp.run(['sudo','perf','record','-q','-g','-o',str(profile),'-p',str(server_pid),'--',*cmd],check=True,timeout=180)
                        with (profile.with_suffix('.txt')).open('w') as output:
                            sp.run(['sudo','perf','report','--stdio','-i',str(profile)],stdout=output,check=True,timeout=60)

    workers_list=[1] if a.smoke else [1,8]
    # Finish and persist ALL PostgreSQL reference arms before any Nata timing.
    pgport=int(psycopg.conninfo.conninfo_to_dict(a.postgres).get('port',5432))
    for workers in workers_list:
        with psycopg.connect(a.postgres,autocommit=True) as c:
            c.execute(f'ALTER SYSTEM SET max_parallel_workers_per_gather={workers-1}');c.execute('SELECT pg_reload_conf()')
        measure('postgres',pgport,workers)
    for workers in workers_list:
        processes=[];ports=[];logs=[]
        try:
            for label,binary in [('baseline',a.baseline),('nata',a.nata)]:
                sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close();ports.append(port)
                log=(a.work/f'{label}-{workers}.log').open('w');logs.append(log)
                server=sp.Popen([str(binary.resolve()),'serve','--catalog',str(a.work/'catalog.tsv'),'--listen',f'127.0.0.1:{port}','--threads',str(workers),'--max-queries',str(workers)],stdout=log,stderr=log);processes.append(server)
                for _ in range(300):
                    assert server.poll() is None,'server startup'
                    try:
                        with socket.create_connection(('127.0.0.1',port),timeout=.1):break
                    except OSError:time.sleep(.02)
            measure('nata',ports[1],workers,ports[0],processes[1].pid)
        finally:
            for server in processes:server.terminate()
            for server in processes:server.wait(timeout=10)
            for log in logs:log.close()
    report=[]
    for workers in workers_list:
        for name in clients:
            for kind in ['stream','small']:
                key=f'{workers}/{name}/{kind}'
                pg=samples['postgres/'+key];current=samples['nata/'+key];old=samples['baseline/'+key]
                state=verdict(pg,current,old,kind)
                report.append(dict(arm=key,postgres=pg,nata=current,baseline=old,ratio_pg=current['median']/pg['median'],ratio_baseline=current['median']/old['median'],verdict='SMOKE ONLY' if a.smoke else state))
    (a.work/'verdict.json').write_text(json.dumps(report,indent=2)+'\n')
    if not a.smoke:assert all(r['verdict']=='MET' for r in report), 'P16 failed or unresolved; retain miss profiles'

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ['nata','baseline','work','jar']:ap.add_argument('--'+name,type=Path)
    ap.add_argument('--postgres');ap.add_argument('--smoke',action='store_true')
    ap.add_argument('--client');ap.add_argument('--kind');ap.add_argument('--dsn');ap.add_argument('--port',type=int);ap.add_argument('--rows',type=int)
    main(ap.parse_args())
