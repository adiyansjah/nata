#!/usr/bin/env python3
"""Byte-exact COPY and seeded float text oracle against a private PostgreSQL.
Usage: python postgres_oracle.py NATA_PORT PG_PORT WORK
Requires pyarrow and psycopg. No approximation/tolerance; fails on any difference.
"""
import sys, struct, random, math, json
from pathlib import Path
import pyarrow as a
import pyarrow.parquet as pq
import psycopg
sys.path.insert(0,str(Path(__file__).parent))
from corpus import connect, message, packet, run
np, pp, work = sys.argv[1:]; work=Path(work);work.mkdir(parents=True,exist_ok=True)
pg=psycopg.connect(f'host=127.0.0.1 port={pp} user=test dbname=nata',autocommit=True)
pg.execute("SET client_encoding = 'UTF8'")
nata=connect(int(np)); seed=322661912839;rng=random.Random(seed)
def copy(s,q):
    s.sendall(packet(b'Q',q.encode()+b'\0')); data=b''; tags=[]
    while True:
        t,b=message(s);tags.append(t)
        assert t!=b'E',b
        if t==b'd':data+=b
        if t==b'Z':break
    assert tags[0]==b'H' and tags[-3:]==[b'c',b'C',b'Z'],tags
    return data
# Explicit same types and three rows: extrema, signed zero/specials, scale,
# UTF8/empty string, epoch/leap date, and every column NULL.
import datetime,decimal
cols=['bo','ti','si','i','bi','r','d','n','s','dt']
types=[a.bool_(),a.string(),a.int16(),a.int32(),a.int64(),a.float32(),a.float64(),a.decimal128(12,3),a.string(),a.date32()]
rows=[[True,'-128',-32768,-2147483648,-9223372036854775808,-0.,float('nan'),decimal.Decimal('123.450'),'',datetime.date(1970,1,1)], [False,'127',32767,2147483647,9223372036854775807,float('inf'),-float('inf'),decimal.Decimal('-0.001'),'a,b\né',datetime.date(2024,2,29)], [None]*10]
pq.write_table(a.table({n:a.array([r[i] for r in rows],type=t) for i,(n,t) in enumerate(zip(cols,types))}),work/'types.parquet')
pg.execute('CREATE TEMP TABLE alltypes(bo bool, ti text, si int2, i int4, bi int8, r float4, d float8, n numeric(12,3), s text, dt date)')
with pg.cursor().copy('COPY alltypes FROM STDIN') as c:
    for row in rows:c.write_row(row)
# Temp table belongs to psycopg session, so use its raw COPY stream.
with pg.cursor().copy('COPY (SELECT * FROM alltypes) TO STDOUT (FORMAT binary)') as c: expected=b''.join(c)
actual=copy(nata,f"COPY (SELECT * FROM '{work/'types.parquet'}') TO STDOUT (FORMAT binary)")
assert actual==expected,(len(actual),len(expected),[(i,x,y) for i,(x,y) in enumerate(zip(actual,expected)) if x!=y][:20])
result={'postgres':pg.execute('SHOW server_version').fetchone()[0], 'copy_bytes':len(actual),'copy_rows':3,'copy_differences':0,'seed':seed}
for width,at,pt,code,bits in [(64,a.float64(),'float8','d','Q'),(32,a.float32(),'float4','f','I')]:
    patterns=[0,1,2,3,(1<<(width-1)),(1<<(width-1))+1]
    # All exponent boundaries with adjacent representations and both signs.
    for exp in range(-324 if width==64 else -45,309 if width==64 else 39):
        try:b=struct.unpack('!'+bits,struct.pack('!'+code,10.**exp))[0]
        except OverflowError:continue
        for d in [-1,0,1]:
            if b+d>=0:patterns.extend([b+d,(b+d)^(1<<(width-1))])
    patterns.extend([(0x7fefffffffffffff if width==64 else 0x7f7fffff), (0x0010000000000000 if width==64 else 0x00800000), (0x3ff0000000000000 if width==64 else 0x3f800000)])
    patterns.extend(rng.getrandbits(width) for _ in range(100000))
    xs=[struct.unpack('!'+code,struct.pack('!'+bits,b))[0] for b in patterns]
    pq.write_table(a.table({'v':a.array(xs,type=at)}),work/f'float{width}.parquet')
    pg.execute(f'CREATE TEMP TABLE f{width}(v {pt})')
    payload=b'PGCOPY\n\xff\r\n\0'+struct.pack('!ii',0,0)+b''.join(struct.pack('!hi',1,width//8)+struct.pack('!'+code,x) for x in xs)+struct.pack('!h',-1)
    with pg.cursor().copy(f'COPY f{width} FROM STDIN (FORMAT binary)') as c:c.write(payload)
    want=[r[0] for r in pg.execute(f'SELECT v::text FROM f{width}')]
    _,_,rows,errors,_,_=run(nata,f"SELECT v FROM '{work/f'float{width}.parquet'}'")
    assert not errors,errors
    diffs=[(hex(b),r[0],w) for b,r,w in zip(patterns,rows,want) if r[0]!=w]
    assert len(rows)==len(want)
    result[f'float{width}']={'count':len(xs),'differences':len(diffs),'examples':diffs[:12]}
    print(json.dumps(result),flush=True)
    assert not diffs, result
print(json.dumps(result,indent=2))
