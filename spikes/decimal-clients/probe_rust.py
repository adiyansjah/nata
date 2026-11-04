"""Probes against the Rust arrow-flight Flight SQL server (target/release/decimal-clients-flight-server):
pyarrow.flight (by hand), ADBC (prepared, unprepared, ingest). JDBC against the same binary: probe_jdbc.py --rust.

    cargo build --release && .venv/bin/python probe_rust.py
"""
import os
import subprocess
import sys
import threading

import adbc_driver_flightsql
import adbc_driver_flightsql.dbapi as fsql
import adbc_driver_manager
import pyarrow as pa
import pyarrow.flight as fl

import flight_server  # only for the protobuf helpers
from common import expected, make_table, probe, rows

here = os.path.dirname(os.path.abspath(__file__))
exe = os.path.join(here, "target/release/decimal-clients-flight-server")
srv = subprocess.Popen([exe, "0"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
port = srv.stdout.readline().split()[1]
import atexit
atexit.register(srv.kill)
ingest_lines, wire_lines = [], []
threading.Thread(target=lambda: [ingest_lines.append(l.rstrip()) for l in srv.stdout], daemon=True).start()
threading.Thread(target=lambda: [wire_lines.append(l.rstrip()) for l in srv.stderr], daemon=True).start()
uri = f"grpc://127.0.0.1:{port}"
print("rust server on", port, "| pyarrow", pa.__version__, "| adbc_driver_flightsql", adbc_driver_flightsql.__version__, flush=True)
EXP = expected()


def schema_of(t):
    return {f.name: str(f.type) for f in t.schema if f.name != "id"}


def check(t, empty=False):
    got = {c: t[c].to_pylist() for c in EXP if c in t.column_names}
    return {"schema": schema_of(t), "values_equal_expected": all(got[c] == ([] if empty else EXP[c]) for c in got)}


c = fl.connect(uri)


def by_hand(sql, empty=False):
    cmd = flight_server.any_pack("CommandStatementQuery", flight_server.f_len(1, sql.encode()))
    info = c.get_flight_info(fl.FlightDescriptor.for_command(cmd))
    t = c.do_get(info.endpoints[0].ticket).read_all()
    return {"info schema": {f.name: str(f.type) for f in info.schema if f.name != "id"}, **check(t, empty)}


probe("rust flight sql, pyarrow by hand: SELECT * FROM t", lambda: by_hand("SELECT * FROM t"))
probe("rust flight sql, pyarrow by hand: SELECT d64 FROM t", lambda: by_hand("SELECT d64 FROM t"))
probe("rust flight sql, pyarrow by hand: SELECT d64_wide FROM t", lambda: by_hand("SELECT d64_wide FROM t"))
probe("rust flight sql, pyarrow by hand: SELECT * FROM empty", lambda: by_hand("SELECT * FROM empty", empty=True))
probe("rust flight sql, pyarrow by hand: read_pandas", lambda: (lambda cmd: {k: str(v) for k, v in c.do_get(c.get_flight_info(fl.FlightDescriptor.for_command(cmd)).endpoints[0].ticket).read_pandas().dtypes.items()})(flight_server.any_pack("CommandStatementQuery", flight_server.f_len(1, b"SELECT * FROM t"))))
probe("rust flight sql, pyarrow by hand: widened (t_widened) schema", lambda: by_hand("SELECT * FROM t_widened"))


def adbc(sql, ingest=None):
    with fsql.connect(uri) as con, con.cursor() as cur:
        cur.execute(sql)
        return check(cur.fetch_arrow_table())


probe("rust flight sql, ADBC (prepared path): SELECT * FROM t", lambda: adbc("SELECT * FROM t"))
probe("rust flight sql, ADBC (prepared path): SELECT d64 FROM t", lambda: adbc("SELECT d64 FROM t"))
probe("rust flight sql, ADBC (prepared path): SELECT d64_wide FROM t", lambda: adbc("SELECT d64_wide FROM t"))


def adbc_unprepared():
    with adbc_driver_flightsql.connect(uri) as db, adbc_driver_manager.AdbcConnection(db) as conn, adbc_driver_manager.AdbcStatement(conn) as st:
        st.set_sql_query("SELECT * FROM t")
        h, _ = st.execute_query()
        return check(pa.RecordBatchReader._import_from_c(h.address).read_all())


probe("rust flight sql, ADBC low-level, no prepare (CommandStatementQuery)", adbc_unprepared)


def ingest(cols):
    n0 = len(ingest_lines)
    t = make_table().select(cols)
    with fsql.connect(uri) as con, con.cursor() as cur:
        n = cur.adbc_ingest("ingested", t, mode="create")
    import time; time.sleep(0.3)
    return {"adbc rows": n, "rust server decoded": ingest_lines[n0:]}


probe("rust flight sql, ADBC ingest Decimal64(15,2) -> Rust decoder", lambda: ingest(["d64"]))
probe("rust flight sql, ADBC ingest Decimal64(18,0) -> Rust decoder", lambda: ingest(["d64_wide"]))
probe("rust flight sql, ADBC ingest Decimal128(15,2) -> Rust decoder", lambda: ingest(["d128"]))

print("\nSERVER WIRE LOG (stderr):", *wire_lines[:12], sep="\n  ")
print("\nSUMMARY", len(rows), "probes;", sum(1 for r in rows if r[1] == "ok"), "ok;", sum(1 for r in rows if r[1] != "ok"), "error")
import json
json.dump(rows, open("/tmp/nata-dc-rust-rows.json", "w"), default=str, indent=1)
sys.stdout.flush()
srv.kill()
os._exit(0)
