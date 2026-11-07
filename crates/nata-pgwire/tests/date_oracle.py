#!/usr/bin/env python3
"""Date32 end-to-end versus PostgreSQL. NATA PG WORK BINARY; psycopg + ADBC."""
import json, struct, subprocess, sys
from pathlib import Path
import pyarrow as a, pyarrow.parquet as pq
import psycopg
from adbc_driver_postgresql import dbapi
from corpus import connect, packet, message, run
np,pp,work,binary=sys.argv[1:];work=Path(work);work.mkdir(parents=True,exist_ok=True)
pg=psycopg.connect(host='127.0.0.1',port=pp,user='test',dbname='nata',autocommit=True)
pg.execute("SET client_encoding='UTF8'")
valid=[-2440588,-735160,-719528,-719162,0,2932896,2932897,2145042905]
bad=[-2147483648,-2147483647,-2440589,2145042906,2147483647]
result={'postgres':pg.execute('show server_version').fetchone()[0],'psycopg':psycopg.__version__,'cases':[]}
def raw(s,q,binary=False,copy=False):
    if copy:s.sendall(packet(b'Q',f'COPY ({q}) TO STDOUT (FORMAT binary)'.encode()+b'\0'))
    else:s.sendall(packet(b'P',b'\0'+q.encode()+b'\0\0\0')+packet(b'B',b'\0\0\0\0\0\0\0\x01'+struct.pack('!h',int(binary)))+packet(b'D',b'P\0')+packet(b'E',b'\0\0\0\0\0')+packet(b'S',b''))
    frames=[]
    while True:
        t,b=message(s);frames.append((t,b))
        if t==b'Z':return frames
for days in valid+bad:
    file=work/f'd{days}.parquet';pq.write_table(a.table({'d':a.array([days,None],type=a.date32())}),file)
    q=f"SELECT d FROM '{file}'";record={'days':days}
    cli=subprocess.run([binary,'sql','-c',q,'--csv'],text=True,capture_output=True,check=True).stdout
    record['cli']=cli.splitlines()[1:]
    if days in bad:
        for bin,copy in [(False,False),(True,False),(True,True)]:
            with connect(int(np)) as s:frames=raw(s,q,bin,copy)
            errors=[b for t,b in frames if t==b'E'];assert len(errors)==1 and b'C22008\0' in errors[0],frames
            assert not any(t==b'D' for t,b in frames)
        record['wire']='22008';result['cases'].append(record);continue
    pgq=f"SELECT ('1970-01-01'::date + {days}) AS d UNION ALL SELECT NULL::date"
    expected=pg.execute(f"SELECT ('1970-01-01'::date + {days})::text").fetchone()[0];record['text']=expected
    with connect(int(np)) as s:
        _,oids,rows,errors,_,_=run(s,q)
    assert oids==[1082] and rows==[[expected],[None]] and not errors
    for bin,copy in [(True,False),(True,True)]:
        with connect(int(np)) as ns, connect(int(pp)) as ps:
            actual=raw(ns,q,bin,copy);want=raw(ps,pgq,bin,copy)
        tag=b'd' if copy else b'D'
        assert b''.join(b for t,b in actual if t==tag)==b''.join(b for t,b in want if t==tag)
        assert not any(t==b'E' for t,b in actual)
    # Driver defaults: Python datetime cannot represent BC or >9999; match PG's explicit loader error.
    def psy(port,query):
        with psycopg.connect(host='127.0.0.1',port=port,user='test',dbname='nata',autocommit=True) as c:
            try:return ['value',str(c.execute(query).fetchone()[0])]
            except (psycopg.DataError,ValueError,OverflowError) as e:return ['error',type(e).__name__]
    assert psy(np,q)==psy(pp,pgq);record['psycopg']=psy(np,q)
    def adbc(port,query):
        with dbapi.connect(f'postgresql://test@127.0.0.1:{port}/nata') as c:
            with c.cursor() as cur:
                cur.execute(query);t=cur.fetch_arrow_table();assert t.schema.field(0).type==a.date32()
                return t.column(0).cast(a.int32()).to_pylist()
    assert adbc(np,q)==adbc(pp,pgq)==[days,None];record['adbc_date32']=[days,None]
    result['cases'].append(record)
print(json.dumps(result,indent=2))
