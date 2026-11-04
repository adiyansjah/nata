"""Probe 3: Apache Arrow Flight SQL JDBC driver (Java) against flight_server.py running in a separate process.

Needs: the driver jar (fetch_jdbc.sh puts it in /tmp/nata-dc-jars) and a JVM: java is not installed system-wide, so the
pip package jdk4py's JRE is used, and JPype calls java.sql from Python (no javac needed).

    .venv/bin/python probe_jdbc.py [path/to/flight-sql-jdbc-driver.jar]
"""
import glob
import os
import subprocess
import sys
import time

import jdk4py
import jpype

args = [a for a in sys.argv[1:] if not a.startswith("--")]
RUST = "--rust" in sys.argv  # use the Rust arrow-flight server binary instead of the pyarrow one
jar = args[0] if args else glob.glob("/tmp/nata-dc-jars/flight-sql-jdbc-driver-*.jar")[0]
here = os.path.dirname(os.path.abspath(__file__))
cmd = [os.path.join(here, "target/release/decimal-clients-flight-server"), "0"] if RUST else [sys.executable, os.path.join(here, "flight_server.py"), "0"]
srv = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=here)
port = srv.stdout.readline().split()[1]
import atexit
atexit.register(srv.kill)
print("server:", "rust arrow-flight" if RUST else "pyarrow.flight", "on", port, "| jar", os.path.basename(jar), "| jvm", jdk4py.JAVA_VERSION, flush=True)

jvm = str(jdk4py.JAVA.parent.parent / "lib" / "server" / "libjvm.so")
# Arrow's memory module needs these opens on JDK 17+
# JDK 25 + the driver's shaded Netty need exactly these two flags (without --sun-misc-unsafe-memory-access=allow the allocator
# fails to initialise: see the results file); they are not related to the Decimal64 rejection (the Decimal128 control passes with them).
jpype.startJVM(jvm, "--add-opens=java.base/java.nio=ALL-UNNAMED", "--sun-misc-unsafe-memory-access=allow",
               "--enable-native-access=ALL-UNNAMED", classpath=[jar], convertStrings=True)
DriverManager = jpype.JClass("java.sql.DriverManager")
Types = jpype.JClass("java.sql.Types")

TYPES = {int(getattr(Types, n)): n for n in ("DECIMAL", "NUMERIC", "BIGINT", "INTEGER", "VARCHAR", "DOUBLE", "OTHER", "JAVA_OBJECT", "ARRAY", "STRUCT", "NULL")}
out = []


def run(label, fn):
    try:
        r = fn()
        print(f"[ok ] {label}\n      {r}", flush=True)
        out.append((label, "ok", r))
    except BaseException as e:  # noqa: BLE001
        chain, c = [], e
        while c is not None:
            chain.append(f"{c.__class__.__name__}: {c}")
            c = c.getCause() if hasattr(c, "getCause") else None
        root = e
        while hasattr(root, "getCause") and root.getCause() is not None:
            root = root.getCause()
        frames = [str(f) for f in root.getStackTrace()[:4]] if hasattr(root, "getStackTrace") else []
        msg = " <- caused by ".join(dict.fromkeys(chain)).replace("\n", " | ") + " | root-cause frames: " + " ; ".join(frames)
        print(f"[ERR] {label}\n      {msg[:1200]}", flush=True)
        out.append((label, "ERROR", msg[:1200]))


url = f"jdbc:arrow-flight-sql://127.0.0.1:{port}/?useEncryption=false"


def query(sql, prepared=False):
    con = DriverManager.getConnection(url)
    try:
        st = con.prepareStatement(sql) if prepared else con.createStatement()
        rs = st.executeQuery() if prepared else st.executeQuery(sql)
        md = rs.getMetaData()
        cols = []
        for i in range(1, md.getColumnCount() + 1):
            cols.append({"name": str(md.getColumnName(i)), "type": TYPES.get(int(md.getColumnType(i)), int(md.getColumnType(i))),
                         "typeName": str(md.getColumnTypeName(i)), "precision": int(md.getPrecision(i)), "scale": int(md.getScale(i)),
                         "class": str(md.getColumnClassName(i))})
        rowsv = []
        while rs.next():
            r = []
            for i in range(1, len(cols) + 1):
                o = rs.getObject(i)
                r.append(None if o is None else (type(o).__name__ if False else str(o.getClass().getName()) + ":" + str(o.toString())) if cols[i - 1]["name"] != "id" else str(o))
            rowsv.append(r)
        # getBigDecimal / getString / getDouble for the decimal columns
        extra = {}
        rs.close()
        rs = st.executeQuery() if prepared else st.executeQuery(sql)
        bd, strs, dbl = [], [], []
        while rs.next():
            for i, c in enumerate(cols, 1):
                if c["name"] == "id":
                    continue
                b = rs.getBigDecimal(i)
                bd.append(None if b is None else str(b.toPlainString()) + f"(scale={int(b.scale())})")
                s_ = rs.getString(i)
                strs.append(None if s_ is None else str(s_))
                d_ = rs.getDouble(i)
                dbl.append(None if rs.wasNull() else float(d_))
        return {"columns": cols, "getObject": rowsv, "getBigDecimal": bd, "getString": strs, "getDouble": dbl}
    finally:
        con.close()


run("jdbc connect + metadata (driver name/version)", lambda: (lambda c: (str(c.getMetaData().getDriverName()), str(c.getMetaData().getDriverVersion())))(DriverManager.getConnection(url)))
run("jdbc SELECT d128 (Decimal128(15,2)) - control", lambda: query("SELECT d128 FROM t"))
run("jdbc SELECT d64 (Decimal64(15,2))", lambda: query("SELECT d64 FROM t"))
run("jdbc SELECT d64_wide (Decimal64(18,0))", lambda: query("SELECT d64_wide FROM t"))
run("jdbc SELECT * (all columns)", lambda: query("SELECT * FROM t"))
run("jdbc SELECT d64 as PreparedStatement", lambda: query("SELECT d64 FROM t", prepared=True))
run("jdbc SELECT d64 widened server-side to Decimal128(15,2) + Flight SQL PRECISION/SCALE metadata", lambda: query("SELECT d64 FROM t_widened"))
run("jdbc SELECT d64_wide widened server-side to Decimal128(18,0) + metadata", lambda: query("SELECT d64_wide FROM t_widened"))
run("jdbc SELECT * widened server-side", lambda: query("SELECT * FROM t_widened"))
run("jdbc SELECT d64 widened, PreparedStatement", lambda: query("SELECT d64 FROM t_widened", prepared=True))
run("jdbc SELECT * FROM empty (schema only)", lambda: query("SELECT * FROM empty"))
srv.kill()
import json
json.dump(out, open("/tmp/nata-dc-jdbc-rust-rows.json" if RUST else "/tmp/nata-dc-jdbc-rows.json", "w"), default=str, indent=1)
os._exit(0)
