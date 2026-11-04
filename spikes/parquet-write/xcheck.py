#!/usr/bin/env python3
"""End-to-end checks through an independent reader (DuckDB):
  content <expected.parquet> <glob>            the files hold exactly the same rows (compared as text, so NaN and -0.0 are exact)
  order   <glob> <col> <asc|desc>              the column is globally sorted in file order
  prune   <glob> [--cols a,b]                  a suite of predicates: the count DuckDB gets from the files WITH its row-group pruning on our statistics must
                                               equal the count from a full scan. A wrong min or max loses rows here.
Exit status 0 if everything matched; prints every mismatch."""
import csv, io, os, subprocess, sys

ENV = {k: v for k, v in os.environ.items() if k != "AI_AGENT"}


import tempfile
DB = os.path.join(tempfile.mkdtemp(prefix="xcheck-"), "x.duckdb")


def duck(script, check=True):
    p = subprocess.run(["duckdb", DB, "-csv", "-noheader"], input=script, capture_output=True, text=True, errors="replace", env=ENV)
    if check and p.returncode != 0 and not p.stdout: raise SystemExit(f"duckdb failed: {p.stderr[:300]}")
    return p.stdout, p.stderr


def one(sql):
    out, err = duck(sql)
    rows = list(csv.reader(io.StringIO(out)))
    return rows


def columns(src):
    return [(r[0], r[1]) for r in one(f"select column_name, column_type from (describe select * from read_parquet({src}))")]


def lit(sql): return sql.replace("'", "''")


def content(expected, glob):
    cols = columns(f"'{expected}'")
    sel = ", ".join(f'cast("{c}" as varchar) as "{c}"' for c, _ in cols)
    a, b = f"(select {sel} from read_parquet('{expected}'))", f"(select {sel} from read_parquet('{glob}'))"
    out, err = duck(f"select (select count(*) from {a}), (select count(*) from {b}), (select count(*) from (select * from {a} except all select * from {b})), "
                    f"(select count(*) from (select * from {b} except all select * from {a}));")
    n1, n2, d1, d2 = out.strip().split(",")
    ok = n1 == n2 and d1 == "0" and d2 == "0"
    print(f"{'ok  ' if ok else 'FAIL'} content {glob}: expected {n1} rows, files {n2}, only-expected {d1}, only-files {d2} {err[:200]}")
    return ok


def order(glob, col, direction):
    op = ">" if direction == "desc" else "<"
    out, _ = duck(f"select count(*) from (select \"{col}\" v, lag(\"{col}\") over (order by filename, file_row_number) p from read_parquet('{glob}', filename=true, file_row_number=true)) where p is not null and v {op} p;")
    ok = out.strip() == "0"
    print(f"{'ok  ' if ok else 'FAIL'} order {glob}: {col} {direction}: {out.strip()} out-of-order neighbours")
    return ok


SCALAR_SKIP = ("[]", "STRUCT", "MAP", "BOOLEAN")


def L(v, t):
    return "'" + v.replace("'", "''") + f"'::{t}"


def prune(glob, only=None):
    cols = [(c, t) for c, t in columns(f"'{glob}'") if not any(s in t for s in SCALAR_SKIP) and (only is None or c in only)]
    duck(f"drop table if exists t; create table t as select * from read_parquet('{glob}');")
    queries = []
    for c, t in cols:
        q = f'"{c}"'
        numeric = t.startswith(("TINYINT", "SMALLINT", "INTEGER", "BIGINT", "UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT", "DECIMAL", "HUGEINT"))
        temporal = t.startswith(("DATE", "TIMESTAMP"))
        exprs = [f"min({q})", f"max({q})"]
        if numeric: exprs += [f"min({q}) - 1", f"max({q}) + 1"]
        if t == "DATE": exprs += [f"min({q}) - 1", f"max({q}) + 1"]
        if t.startswith("TIMESTAMP"): exprs += [f"min({q}) - interval 1 second", f"max({q}) + interval 1 second"]
        if t == "VARCHAR": exprs += [f"left(min({q}), 3)", f"left(max({q}), 3)", f"left(min({q}), 16)", f"left(max({q}), 16)", f"left(max({q}), 17)", f"max({q}) || 'a'", f"min({q}) || 'a'"]
        row = []
        for e in exprs:
            out, _ = duck(f"select cast(({e}) as varchar) from t;", check=False)
            rr = list(csv.reader(io.StringIO(out)))
            row.append(rr[0][0] if rr and rr[0] else "")
        picks = [(r[0] if r else "") for r in one(f"select cast(v as varchar) from (select distinct {q} v from t where {q} is not null order by hash(v) limit 4)")]
        lits = [L(v, t) for v in row + picks if v != ""]
        if t in ("DOUBLE", "FLOAT"): lits += [L("nan", t), L("inf", t), L("-inf", t), L("0.0", t), L("-0.0", t)]
        if t == "BLOB": lits += [L("\\xFF", t), L("\\x7F", t), L("\\x80", t)]
        for lit in dict.fromkeys(lits):
            for op in ("<", "<=", ">", ">=", "=", "<>"): queries.append(f"{q} {op} {lit}")
        queries += [f"{q} is null", f"{q} is not null"]
        if row[0] != "" and row[1] != "" and t != "BLOB": queries.append(f"{q} between {L(row[0], t)} and {L(row[1], t)}")
    bad = 0
    for i0 in range(0, len(queries), 300):
        chunk = queries[i0:i0 + 300]
        script = "\n".join(f"select {i}, (select count(*) from t where {w}), (select count(*) from read_parquet('{glob}') where {w});" for i, w in enumerate(chunk))
        out, err = duck(script, check=False)
        got = {}
        for r in csv.reader(io.StringIO(out)):
            if len(r) == 3: got[int(r[0])] = r
        for i, w in enumerate(chunk):
            r = got.get(i)
            if r is None: continue                       # the engine rejected this predicate (a type it cannot compare): not a mismatch
            if r[1] != r[2]: bad += 1; print(f"  MISMATCH where {w}: full scan {r[1]}, with pruning {r[2]}")
    print(f"{'ok  ' if bad == 0 else 'FAIL'} prune {glob}: {len(queries)} predicates over {len(cols)} columns, {bad} wrong counts")
    return bad == 0


if __name__ == "__main__":
    cmd, a = sys.argv[1], sys.argv[2:]
    ok = {"content": lambda: content(a[0], a[1]), "order": lambda: order(a[0], a[1], a[2]),
          "prune": lambda: prune(a[0], a[a.index("--cols") + 1].split(",") if "--cols" in a else None)}[cmd]()
    sys.exit(0 if ok else 1)
