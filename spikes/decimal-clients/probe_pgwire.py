"""Probe 4b: real Postgres client libraries (psycopg 3 / libpq, asyncpg, pg8000) read an Arrow Decimal64 / Decimal128 table
through pgwire_server.py, which sends it as `numeric` using pg_numeric.py (text and binary result formats).

    .venv/bin/python probe_pgwire.py
"""
import asyncio
import os
import sys
import threading

import asyncpg
import pg8000.native
import psycopg

from common import expected, probe, rows
import pgwire_server

print("versions: psycopg", psycopg.__version__, "| libpq", psycopg.pq.version(), "| asyncpg", asyncpg.__version__, "| pg8000", __import__("pg8000").__version__, flush=True)
loop = asyncio.new_event_loop()
ready = threading.Event(); box = {}


def run_loop():
    asyncio.set_event_loop(loop)
    s = loop.run_until_complete(pgwire_server.start(0))
    box["port"] = s.sockets[0].getsockname()[1]; ready.set(); loop.run_forever()


threading.Thread(target=run_loop, daemon=True).start(); ready.wait()
port = box["port"]
EXP = expected()
COLS = ["d64", "d128", "d64_wide"]


def same(got):  # got: {col: [values]}
    return {"values_equal_expected": all(got[c] == EXP[c] for c in got), **({} if all(got[c] == EXP[c] for c in got) else {"got": got})}


def psy(binary):
    with psycopg.connect(f"host=127.0.0.1 port={port} user=x dbname=x sslmode=disable", autocommit=True) as con, con.cursor(binary=binary) as cur:
        cur.execute("SELECT d64, d128, d64_wide FROM t")
        desc = [(c.name, c.type_code, c.precision, c.scale) for c in cur.description]
        data = cur.fetchall()
        got = {c: [r[i] for r in data] for i, c in enumerate(COLS)}
        return {"description (name, type oid, precision, scale)": desc, "python types": sorted({type(v).__name__ for r in data for v in r if v is not None}), **same(got)}


probe("psycopg3 TEXT result format: Decimal64(15,2), Decimal128(15,2), Decimal64(18,0) as numeric", lambda: psy(False))
probe("psycopg3 BINARY result format", lambda: psy(True))


async def apg():
    con = await asyncpg.connect(host="127.0.0.1", port=port, user="x", database="x", ssl=False)
    try:
        st = await con.prepare("SELECT d64, d128, d64_wide FROM t")
        attrs = [(a.name, a.type.name, a.type.oid) for a in st.get_attributes()]
        data = await st.fetch()   # asyncpg always requests binary results
        got = {c: [r[i] for r in data] for i, c in enumerate(COLS)}
        return {"attributes": attrs, "python types": sorted({type(v).__name__ for r in data for v in r if v is not None}), **same(got)}
    finally:
        await con.close()


probe("asyncpg (binary results always)", lambda: asyncio.run(apg()))


def pg8():
    con = pg8000.native.Connection("x", host="127.0.0.1", port=port, database="x", password="x", ssl_context=None)
    try:
        data = con.run("SELECT d64, d128, d64_wide FROM t")
        got = {c: [r[i] for r in data] for i, c in enumerate(COLS)}
        return {"columns": [(c["name"], c["type_oid"]) for c in con.columns], **same(got)}
    finally:
        con.close()


probe("pg8000 (pure Python, text results)", pg8)


def psy_err():
    with psycopg.connect(f"host=127.0.0.1 port={port} user=x dbname=x sslmode=disable", autocommit=True) as con:
        con.execute("SELECT nope FROM t")
probe("CONTROL psycopg3: unknown column (error text must be recorded)", psy_err)

# control: break the encoder and the same client check must fail
import pg_numeric as N
orig = N.encode_numeric
N.encode_numeric = lambda u, s: orig(u, s) if s != 2 else orig(u, 0)  # mutant: wrong dscale for scale-2 values
probe("CONTROL psycopg3 BINARY with a mutated encoder (values_equal_expected must be False)", lambda: psy(True))
N.encode_numeric = orig

print("\nSUMMARY", len(rows), "probes;", sum(1 for r in rows if r[1] == "ok"), "ok;", sum(1 for r in rows if r[1] != "ok"), "error")
import json
json.dump(rows, open("/tmp/nata-dc-pgwire-rows.json", "w"), default=str, indent=1)
sys.stdout.flush(); os._exit(0)
