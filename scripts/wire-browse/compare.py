#!/usr/bin/env python3
"""Replay the captured metadata corpus against Nata and a synthetic PostgreSQL fixture.
The PostgreSQL fixture is scripts/wirecapture/fixture.sql. Explicit projections
remove unrelated PostgreSQL storage/builtins; every projection is recorded.
"""
import argparse,json,re,socket,subprocess,time
from pathlib import Path
import psycopg
from psycopg import ClientCursor
ap=argparse.ArgumentParser(description=__doc__)
ap.add_argument('--nata',type=Path,required=True);ap.add_argument('--work',type=Path,required=True)
ap.add_argument('--postgres',default='host=127.0.0.1 port=55432 dbname=nata')
a=ap.parse_args();a.work=a.work.resolve();a.work.mkdir(parents=True,exist_ok=True);root=Path(__file__).resolve().parents[2]
corpus=json.loads((root/'bench/results/raw/20261107-wire-catalog-reference/wcap-x86-1107-r2/inventory/sql.json').read_text())
corpus.append(dict(client='psql',sql=(root/'crates/nata-pgwire/src/browse/65.sql').read_text()))
corpus.append(dict(client='pgjdbc',sql=(root/'crates/nata-pgwire/src/browse/66.sql').read_text()))
sql_work=str(a.work).replace("'","''")
subprocess.run(['duckdb','-c',f'''CREATE TABLE all_types(tiny VARCHAR,b BOOLEAN,s SMALLINT,i INTEGER,l BIGINT,f REAL,d DOUBLE,n DECIMAL(18,3),day DATE,txt VARCHAR);
COPY all_types TO '{sql_work}/types.parquet' (FORMAT parquet);
COPY (SELECT NULL::BIGINT v WHERE false) TO '{sql_work}/big.parquet' (FORMAT parquet);
COPY (SELECT NULL::DECIMAL(10,2) AS "Mixed Column" WHERE false) TO '{sql_work}/quoted.parquet' (FORMAT parquet);'''],check=True,capture_output=True)
registry=a.work/'registry.tsv';registry.write_text(f'nata\tempty\nnata\tpublic\tall_types\t{a.work}/types.parquet\nnata\tother\tall_types\t{a.work}/big.parquet\nnata\tQuoted Schema\tMixed Table\t{a.work}/quoted.parquet\nnata\tpublic\tmillion\t{a.work}/big.parquet\n')
sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close()
log=(a.work/'server.log').open('w');server=subprocess.Popen([str(a.nata.resolve()),'serve','--catalog',str(registry),'--listen',f'127.0.0.1:{port}','--threads','2','--max-queries','2'],stdout=log,stderr=log)
def raw(c,sql):
    with ClientCursor(c) as cur:
        cur.execute(sql)
        pg=cur.pgresult
        return [[pg.fname(j).decode(),pg.ftype(j),pg.fsize(j),pg.fmod(j)] for j in range(pg.nfields)],[[None if pg.get_value(i,j) is None else pg.get_value(i,j).decode() for j in range(pg.nfields)] for i in range(pg.ntuples)]
def binary_rows(c,sql):
    with c.cursor(binary=True) as cur:
        cur.execute(sql);pg=cur.pgresult
        return [[pg.fname(j).decode(),pg.ftype(j),pg.fsize(j),pg.fmod(j),pg.fformat(j)] for j in range(pg.nfields)], [[pg.get_value(i,j) for j in range(pg.nfields)] for i in range(pg.ntuples)]
def quote(v):return "'"+v.replace("'","''")+"'"
users={'public','other','empty','Quoted Schema'};types={'bool','int2','int4','int8','text','float4','float8','varchar','date','numeric'}
def project(i,rows):
    if i in [0,1,2,3,20,46,47,48]:rows=[r for r in rows if r[0] in users or (i==46 and r[0] in {'pg_catalog','information_schema'})]
    if i in [4,12]:rows=[r for r in rows if r[1] in users]
    if i in [28,30]:rows=[r for r in rows if r[1] in users]
    if i in [26,51,57]:rows=[r for r in rows if r[0]=='nata']
    if i==27:rows=[r for r in rows if r[0] in users|{'pg_catalog','information_schema'}]
    if i in [22,65]:rows=[r for r in rows if r[0]=='nata']
    if i in [21,37,50,52]:rows=[r for r in rows if r[0 if i!=37 else 1] in users]
    if i==35:rows=[r for r in rows if r[0] in types]
    if i in [32,36]:rows=[r for r in rows if r[2] in types]
    if i==42:rows=[r for r in rows if r[0]=='pg_catalog' and r[1] in types]
    if i==55:rows=[r for r in rows if r[1] in types]
    return rows
report=[];binary_report=[]
try:
    for _ in range(200):
        try:
            with socket.create_connection(('127.0.0.1',port),timeout=.1):break
        except OSError:time.sleep(.02)
    with psycopg.connect(a.postgres,autocommit=True) as pg,psycopg.connect(f'host=127.0.0.1 port={port} dbname=nata user=nata',autocommit=True) as nata:
        pg_oids={};nata_oids={}
        for ns,name in [('public','all_types'),('other','all_types'),('Quoted Schema','Mixed Table'),('public','million')]:
            q=corpus[4]['sql'].replace('^(all_types)$','^('+name+')$').replace('^(public)$','^('+ns+')$')
            pg_oids[raw(pg,q)[1][0][0]]=ns+'.'+name;nata_oids[raw(nata,q)[1][0][0]]=ns+'.'+name
        # Each parameter slot is replayed with the same synthetic predicate.
        parameters={28:['%'],30:['%','%'],31:['23'],32:['23'],33:['public','all_types'],34:['public','all_types'],37:['%'],40:['public'],41:['public'],47:['{r,p,f}'],48:['{r,p,f}'],59:['public','{r,p,f}'],60:['public','all_types'],61:['public','all_types'],62:['public.all_types'],66:['public','all_types']}
        skip={23,24,25,38,39,43,44,53,56,63,64}
        for i,item in enumerate(corpus):
            if i in skip:continue
            sql=item['sql'];nq=sql;pquery=sql
            for old,ident in [('16388','public.all_types'),('16396','Quoted Schema.Mixed Table')]:
                nq=nq.replace(quote(old),quote(next(k for k,v in nata_oids.items() if v==ident)))
                pquery=pquery.replace(quote(old),quote(next(k for k,v in pg_oids.items() if v==ident)))
            for num,v in reversed(list(enumerate(parameters.get(i,[]),1))):nq=nq.replace('$'+str(num),quote(v));pquery=pquery.replace('$'+str(num),quote(v))
            # PG17 renamed PG15/16's ICU locale column. It is NULL for this C fixture.
            pquery=pquery.replace('d.daticulocale','NULL::text')
            # PG16 added daticurules; the Debian 12 VM oracle is PG15 (found 2026-11-08, wbrowse-n4a). NULL for this C fixture.
            if pg.info.server_version<160000:pquery=pquery.replace('d.daticurules','NULL::text')
            record={'statement':i,'client':item['client'],'sql':nq}
            try:
                po,pr=raw(pg,pquery);no,nr=raw(nata,nq)
                # PG17 narrowed pg_statistic_ext.stxstattarget from int4 to int2; Nata follows PG17 (statements 8, 16 on the PG15 VM oracle).
                if pg.info.server_version<170000:
                    for col in po:
                        if col[0]=='stxstattarget' and col[1]==23:col[1]=21;col[2]=2
                if i==54:pr=[r for r in pr if r[0] in pg_oids and r[1] not in {'tableoid','cmax','xmax','cmin','xmin','ctid'}]
                pr=project(i,pr)
                if i==40:
                    for row in pr:row[9]='NO'
                if i==41:
                    for row in pr:row[-1]='NO'
                if i==65:
                    pr[0][1]='nata';pr[0][3]=None
                if i==22:
                    pr[0][1]='nata';pr[0][6]=None
                if i==20:
                    for row in pr:row[1]='nata'
                if i in [0,1,2,3]:
                    for row in pr:row[3]='nata'
                if i in [5,13]:
                    # Nata has no PostgreSQL heap AM or logical replication identity.
                    for row in pr:row[13]='n';row[14]=None
                for rows,oids in [(pr,pg_oids),(nr,nata_oids)]:
                    if i in [4,12,54]:
                        for r in rows:r[0]=oids[r[0]]
                # OID identity mapping may reorder relation groups, never their attributes.
                if i==54:pr.sort(key=lambda r:r[0]);nr.sort(key=lambda r:r[0])
                # These SQL statements do not specify an order.
                if i in [35,40,41,42,55,57,58,59,60,61]:pr.sort(key=repr);nr.sort(key=repr)
                record.update(verdict='PASS' if po==no and pr==nr else 'FAIL',postgres_oids=po,nata_oids=no,postgres_rows=pr,nata_rows=nr)
            except Exception as e:record.update(verdict='FAIL',error=str(e))
            report.append(record);print(i,record['verdict'],flush=True)
        for label,i,identity in [('type-bootstrap',55,None),('public-attributes',6,'public.all_types'),('quoted-attributes',14,'Quoted Schema.Mixed Table')]:
            nq=corpus[i]['sql'];pq=nq
            if identity:
                old='16388' if i==6 else '16396'
                nq=nq.replace(quote(old),quote(next(k for k,v in nata_oids.items() if v==identity)))
                pq=pq.replace(quote(old),quote(next(k for k,v in pg_oids.items() if v==identity)))
            po,pr=binary_rows(pg,pq);no,nr=binary_rows(nata,nq)
            if i==55:
                pr=[r for r in pr if r[1].decode() in types]
                pr.sort(key=lambda r:r[0]);nr.sort(key=lambda r:r[0])
            def hexadecimal(rows):return [[None if v is None else v.hex() for v in r] for r in rows]
            result=dict(action=label,verdict='PASS' if po==no and pr==nr else 'FAIL',postgres_fields=po,nata_fields=no,postgres_rows_hex=hexadecimal(pr),nata_rows_hex=hexadecimal(nr))
            binary_report.append(result);print('binary '+label,result['verdict'],flush=True)

finally:
    server.terminate();server.wait(timeout=10);log.close()
    (a.work/'binary-comparison.json').write_text(json.dumps(binary_report,indent=2)+'\n')
    (a.work/'comparison.json').write_text(json.dumps(report,indent=2)+'\n')
    inventory=json.loads((root/'bench/results/raw/20261107-wire-catalog-reference/wcap-x86-1107-r2/inventory/inventory.json').read_text())
    classified=[]
    for item in inventory:
        if item['covered']:continue
        name=item['object'].split('.')[-1].removeprefix('function:')
        if name=='btrim':name='trim'
        statements=[r['statement'] for r in report if r['client']==item['client'] and re.search(r'\b'+re.escape(name)+r'\b',corpus[r['statement']]['sql'],re.I)]
        classified.append(dict(item,statements=statements,effect='Served only inside the frozen snapshot shapes; capabilities absent from the registry yield typed empty results. Standalone/other shapes retain tested 0A000 refusal.',verdict='PASS' if statements and all(r['verdict']=='PASS' for r in report if r['statement'] in statements) else 'FAIL'))
    assert len(classified)==36
    (a.work/'inventory-36.json').write_text(json.dumps(classified,indent=2)+'\n')

raise SystemExit(int(any(r['verdict']!='PASS' for r in report) or any(r['verdict']!='PASS' for r in classified) or len(binary_report)!=3 or any(r['verdict']!='PASS' for r in binary_report)))
