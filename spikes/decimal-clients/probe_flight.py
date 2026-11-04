"""Probes 1-2: plain Arrow Flight (pyarrow.flight client) and ADBC Flight SQL, against flight_server.py in-process.

    .venv/bin/python probe_flight.py
"""
import sys
import adbc_driver_flightsql
import adbc_driver_flightsql.dbapi as fsql
import adbc_driver_manager
import pandas as pd
import pyarrow as pa
import pyarrow.flight as fl

import common
from common import expected, make_table, probe, rows
import flight_server

print("versions: pyarrow", pa.__version__, "| adbc_driver_manager", adbc_driver_manager.__version__,
      "| adbc_driver_flightsql", adbc_driver_flightsql.__version__, "| pandas", pd.__version__, flush=True)

srv = flight_server.start()
uri = f"grpc://127.0.0.1:{srv.port}"
EXP = expected()


def schema_of(t):
    return {f.name: str(f.type) for f in t.schema if f.name != "id"}


def vals(t):
    return {c: t[c].to_pylist() for c in EXP if c in t.column_names}


def check(t, empty=False):
    """schema + 'values equal expected'"""
    got = vals(t)
    ok = all(got[c] == ([] if empty else EXP[c]) for c in got)
    return {"schema": schema_of(t), "values_equal_expected": ok, "values": got if not ok else "(identical to input)"}


c = fl.connect(uri)

# ---------------- plain Flight, pyarrow.flight client
probe("flight get_flight_info(path) schema", lambda: schema_of(pa.table({}, schema=c.get_flight_info(fl.FlightDescriptor.for_path("table")).schema).select([])) if False else {f.name: str(f.type) for f in c.get_flight_info(fl.FlightDescriptor.for_path("table")).schema})
probe("flight get_schema(path) schema", lambda: {f.name: str(f.type) for f in c.get_schema(fl.FlightDescriptor.for_path("table")).schema})
probe("flight do_get RecordBatchStream: read_all", lambda: check(c.do_get(fl.Ticket(b"table")).read_all()))
probe("flight do_get GeneratorStream (3 batches): read_all", lambda: check(c.do_get(fl.Ticket(b"generator")).read_all()))
def _chunks(r):
    out = []
    while True:
        try:
            out.append(r.read_chunk().data.num_rows)
        except StopIteration:
            return out
probe("flight do_get GeneratorStream: batch sizes via read_chunk", lambda: _chunks(c.do_get(fl.Ticket(b"generator"))))
probe("flight do_get: read_pandas dtypes", lambda: {k: str(v) for k, v in c.do_get(fl.Ticket(b"table")).read_pandas().dtypes.items()})
probe("flight do_get: read_pandas values", lambda: {k: list(v) for k, v in c.do_get(fl.Ticket(b"table")).read_pandas().items() if k != "id"})
probe("flight do_get d64 only (Decimal64(15,2))", lambda: check(c.do_get(fl.Ticket(b"d64")).read_all()))
probe("flight do_get d64_wide only (Decimal64(18,0))", lambda: check(c.do_get(fl.Ticket(b"d64_wide")).read_all()))
probe("flight list_flights schemas", lambda: [{f.name: str(f.type) for f in i.schema} for i in c.list_flights()][:1])

# do_put (plain)
def put_plain():
    t = make_table()
    w, r = c.do_put(fl.FlightDescriptor.for_path("put1"), t.schema)
    w.write_table(t)
    w.close()
    d, schema, n, py = srv.put_log[-1]
    return {"server saw schema": schema, "rows": n, "values_equal_expected": all(py[k] == EXP[k] for k in EXP)}
probe("flight do_put Decimal64+Decimal128 table (server read_all)", put_plain)

def put_d64_only():
    t = make_table().select(["d64"])
    w, r = c.do_put(fl.FlightDescriptor.for_path("put2"), t.schema)
    w.write_table(t); w.close()
    return {"server saw": srv.put_log[-1][1], "values_equal_expected": srv.put_log[-1][3]["d64"] == EXP["d64"]}
probe("flight do_put Decimal64 only", put_d64_only)

# ---------------- ADBC Flight SQL (Go driver via adbc_driver_manager)
def adbc_query(sql, fetch="arrow_table", empty=False):
    with fsql.connect(uri) as con, con.cursor() as cur:
        cur.execute(sql)
        if fetch == "arrow_table":
            t = cur.fetch_arrow_table()
            return check(t, empty)
        if fetch == "pandas":
            df = cur.fetch_df()
            return {"dtypes": {k: str(v) for k, v in df.dtypes.items()}, "values": {k: list(v) for k, v in df.items() if k != "id"}}
        if fetch == "records":
            return cur.fetchall()
        if fetch == "description":
            cur.fetchall()
            return cur.description
        if fetch == "reader":
            r = cur.fetch_record_batch()
            return {"schema": schema_of(pa.Table.from_batches([], r.schema)), "rows": sum(b.num_rows for b in r)}

probe("adbc flightsql: SELECT * -> fetch_arrow_table", lambda: adbc_query("SELECT * FROM t"))
probe("adbc flightsql: SELECT d64 -> fetch_arrow_table", lambda: adbc_query("SELECT d64 FROM t"))
probe("adbc flightsql: SELECT d128 -> fetch_arrow_table", lambda: adbc_query("SELECT d128 FROM t"))
probe("adbc flightsql: SELECT d64_wide -> fetch_arrow_table", lambda: adbc_query("SELECT d64_wide FROM t"))
probe("adbc flightsql: fetch_record_batch (stream)", lambda: adbc_query("SELECT * FROM t", "reader"))
probe("adbc flightsql: empty result keeps schema", lambda: adbc_query("SELECT * FROM empty", empty=True))
probe("adbc flightsql: fetch_df (pandas)", lambda: adbc_query("SELECT * FROM t", "pandas"))
probe("adbc flightsql: fetchall (DBAPI python rows)", lambda: adbc_query("SELECT * FROM t", "records"))
probe("adbc flightsql: cursor.description", lambda: adbc_query("SELECT * FROM t", "description"))

def adbc_prepared():
    with fsql.connect(uri) as con, con.cursor() as cur:
        cur.adbc_prepare("SELECT * FROM t")
        cur.execute("SELECT * FROM t")
        return check(cur.fetch_arrow_table())
probe("adbc flightsql: prepared statement path", adbc_prepared)

def adbc_schema():
    with fsql.connect(uri) as con, con.cursor() as cur:
        return {f.name: str(f.type) for f in cur.adbc_execute_schema("SELECT * FROM t")}
probe("adbc flightsql: adbc_execute_schema", adbc_schema)

def adbc_ingest(cols):
    t = make_table().select(cols)
    n0 = len(srv.put_log)
    with fsql.connect(uri) as con, con.cursor() as cur:
        n = cur.adbc_ingest("ingested", t, mode="create")
        con.commit() if False else None
    d, schema, rows_, py = srv.put_log[n0]
    return {"adbc returned rows": n, "server saw schema": schema, "values_equal_expected": all(py[k] == EXP[k] for k in py if k in EXP)}
probe("adbc flightsql: adbc_ingest (client -> server) Decimal64(15,2)", lambda: adbc_ingest(["d64"]))
probe("adbc flightsql: adbc_ingest Decimal64(18,0)", lambda: adbc_ingest(["d64_wide"]))
probe("adbc flightsql: adbc_ingest Decimal128(15,2)", lambda: adbc_ingest(["d128"]))
probe("adbc flightsql: adbc_ingest all columns", lambda: adbc_ingest(["id", "d64", "d128", "d64_wide"]))

def adbc_lowlevel_unprepared():
    """adbc_driver_manager low-level API: set_sql_query + execute_query, no prepare -> CommandStatementQuery on the wire."""
    n0 = srv.seen_commands[:]
    with adbc_driver_flightsql.connect(uri) as db, adbc_driver_manager.AdbcConnection(db) as conn, adbc_driver_manager.AdbcStatement(conn) as st:
        st.set_sql_query("SELECT * FROM t")
        handle, _ = st.execute_query()
        t = pa.RecordBatchReader._import_from_c(handle.address).read_all()
    return {"wire commands": srv.seen_commands[len(n0):], **check(t)}
probe("adbc flightsql low-level execute_query WITHOUT prepare (CommandStatementQuery)", adbc_lowlevel_unprepared)

# the same command sent by a pyarrow.flight client by hand
def raw_flight_sql():
    cmd = flight_server.any_pack("CommandStatementQuery", flight_server.f_len(1, b"SELECT * FROM t"))
    info = c.get_flight_info(fl.FlightDescriptor.for_command(cmd))
    t = c.do_get(info.endpoints[0].ticket).read_all()
    return {"info schema": {f.name: str(f.type) for f in info.schema}, **check(t)}
probe("flight sql by hand over pyarrow.flight: GetFlightInfo(CommandStatementQuery) + DoGet(TicketStatementQuery)", raw_flight_sql)

# ---------------- negative controls: the checks must be able to fail
probe("CONTROL flight do_get of a table with one value changed (check must say False)", lambda: check(c.do_get(fl.Ticket(b"mutated")).read_all()))
probe("CONTROL flight do_get of an unknown ticket (error text must be recorded)", lambda: c.do_get(fl.Ticket(b"nope")).read_all())
probe("CONTROL adbc unknown query (error text must be recorded)", lambda: adbc_query("SELECT 1"))

print("\nSUMMARY", len(rows), "probes;", sum(1 for r in rows if r[1] == "ok"), "ok;", sum(1 for r in rows if r[1] != "ok"), "error")
import json
json.dump(rows, open("/tmp/nata-dc-flight-rows.json", "w"), default=str, indent=1)
sys.stdout.flush()
import os
os._exit(0)
