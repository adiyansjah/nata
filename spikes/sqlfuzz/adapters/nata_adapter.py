#!/usr/bin/env python3
"""The command target for `nata sql` (protocol: README, `CMD in.json out.json`). The setup statements build the tables in a DuckDB database; the adapter writes
each table to a Parquet file (several row groups) and runs every case as `nata sql --csv -c <sql with the file in place of the table>`. The CSV is read back with
the column types of the ORACLE's answer (the adapter asks DuckDB to DESCRIBE the query), because the CSV text does not say which cells are numbers.

Environment: NATA_BIN (default ../../target/debug/nata relative to the spike), NATA_THREADS (passed as --threads), DUCKDB (default duckdb).
A case that Nata fails with `not supported yet` is reported as such (a coverage gap); a crash, a signal, an internal error or a timeout is reported as an error
whose text says so, which the fuzzer counts as a difference."""
import csv, io, json, os, re, subprocess, sys, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
from sqlfuzz.engine import DuckDB, duckdb_env

NATA = os.environ.get("NATA_BIN", os.path.join(HERE, "..", "..", "..", "target", "debug", "nata"))
DUCK = os.environ.get("DUCKDB", "duckdb")
ROW_GROUP = {"t1": 3, "t2": 5}


def cell(text, typ):
    t = typ.upper()
    if text == "NULL": return None
    if t in ("TINYINT", "SMALLINT", "INTEGER", "BIGINT"): return int(text)
    if t == "DOUBLE": return float(text)
    if t in ("REAL", "FLOAT"):      # DuckDB's JSON shows a REAL as the double of the float: the same widening here
        import struct
        return struct.unpack("f", struct.pack("f", float(text)))[0]
    if t == "BOOLEAN": return text == "true"
    return text


def main():
    req = json.load(open(sys.argv[1]))
    cases = [(c["id"], c["sql"]) for c in req["cases"]]
    oracle = DuckDB(DUCK).run(req["setup"], cases)
    out = {}
    with tempfile.TemporaryDirectory(dir=os.environ.get("NATA_TMP")) as d:
        db = os.path.join(d, "db.duckdb")
        copies = [f"COPY {t} TO '{d}/{t}.parquet' (FORMAT parquet, ROW_GROUP_SIZE {rg})" for t, rg in ROW_GROUP.items()]
        script = "\n".join(s.rstrip(";") + ";" for s in req["setup"] + copies) + "\n"
        p = subprocess.run([DUCK, db], input=script, capture_output=True, text=True, env=duckdb_env())
        if p.returncode != 0: raise SystemExit("setup failed: " + p.stderr[:300])
        for cid, sql in cases:
            q = re.sub(r"\bFROM (t1|t2)\b", lambda m: f"FROM '{d}/{m.group(1)}.parquet'", sql)
            args = [NATA, "sql", "--csv"] + (["--threads", os.environ["NATA_THREADS"]] if os.environ.get("NATA_THREADS") else []) + ["-c", q]
            try:
                r = subprocess.run(args, capture_output=True, text=True, timeout=60)
            except subprocess.TimeoutExpired:
                out[str(cid)] = {"error": "timeout: nata did not finish in 60 s"}; continue
            if r.returncode != 0:
                msg = next((l for l in r.stderr.splitlines() if l.strip()), "(no message)")
                if r.returncode < 0 or r.returncode > 2: msg = f"exit {r.returncode}: {msg}"
                if "internal error" in r.stderr: msg = "internal error: " + msg
                out[str(cid)] = {"error": msg}; continue
            rows = list(csv.reader(io.StringIO(r.stdout)))
            header, body = (rows[0], rows[1:]) if rows else ([], [])
            if len(header) == 1: body = [row if row else [""] for row in body]      # a one-column row holding an empty string is an empty line
            o = oracle[cid]
            types = o.types if o.ok else [""] * len(header)
            try:
                out[str(cid)] = {"cols": header, "types": types, "rows": [[cell(c, types[i] if i < len(types) else "") for i, c in enumerate(row)] for row in body]}
            except ValueError as e:
                out[str(cid)] = {"error": f"unparseable cell: {e}"}
    json.dump({"results": out}, open(sys.argv[2], "w"))


main()
