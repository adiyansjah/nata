#!/usr/bin/env python3
"""Default ADBC COPY fetch and documented no-COPY fetch."""
import datetime
import json
from pathlib import Path
import sys
import adbc_driver_postgresql.dbapi as db
port, fixture, out=sys.argv[1:]
uri=f'postgresql://test@127.0.0.1:{port}/nata'
r={}
with db.connect(uri) as c:
    r['connect']='PASS'
    for _ in range(10):
        with c.cursor() as cur:
            cur.execute(f"SELECT * FROM '{fixture}'"); table=cur.fetch_arrow_table()
            rows=table.to_pylist(); vals=list(rows[0].values())
            assert vals[:5]==[True,'-128',-32768,-2147483648,-9223372036854775808]
            assert str(vals[5])=='-0.0' and str(vals[6])=='nan'
            assert vals[7:]==['123.450','',datetime.date(1970,1,1)]
            assert all(v is None for v in rows[2].values())
    r['default_fetch']={'status':'PASS','rows':table.num_rows,'repeats':10,'schema':str(table.schema)}
    c.rollback()
    with c.cursor() as cur:
        cur.adbc_statement.set_options(**{'adbc.postgresql.use_copy':'false'})
        cur.execute(f"SELECT * FROM '{fixture}'")
        table=cur.fetch_arrow_table();rows=table.to_pylist()
        vals=list(rows[0].values())
        assert vals[:5]==[True,'-128',-32768,-2147483648,-9223372036854775808]
        assert str(vals[5])=='-0.0' and str(vals[6])=='nan'
        assert vals[7:]==['123.450','',datetime.date(1970,1,1)]
        assert all(v is None for v in rows[2].values())
        r['no_copy_arrow']={'status':'PASS','rows':table.num_rows,'schema':str(table.schema)}
Path(out).write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r),flush=True)
