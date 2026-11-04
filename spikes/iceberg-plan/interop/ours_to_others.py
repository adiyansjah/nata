#!/usr/bin/env python3
"""Tables WE write (random: partition transforms, spec and schema evolution, position and equality deletes, removed files) read by DuckDB's Iceberg reader
and by pyiceberg: the rows must equal what our reader returns; then random filters: the count of matching rows must be the same in all three.
usage: ours_to_others.py <workdir> <first-seed> <last-seed> [filters-per-table]       exit 0 = no disagreement (unsupported features are counted, not failures)"""
import os, sys, traceback
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
import pyarrow.parquet as pq


def null_sensitive(ast):
    if ast["op"] in ("ne", "not_in", "not"): return True
    return any(null_sensitive(ast[k]) for k in ("l", "r", "x") if k in ast)


def zero_in(ast):
    if ast["op"] in ("in", "not_in") and ast["vals"] and ast["vals"][0]["t"] in ("double", "float"): return True
    return any(zero_in(ast[k]) for k in ("l", "r", "x") if k in ast)


def main():
    work, s0, s1 = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    nf = int(sys.argv[4]) if len(sys.argv) > 4 else 25
    os.makedirs(work, exist_ok=True)
    from pyiceberg.table import StaticTable
    con = duckdb_con()
    stats = dict(py_zero_semantics=0, py_bugs=0, py_null_semantics=0, tables=0, duck_rows_ok=0, py_rows_ok=0, py_unsupported=0, duck_unsupported=0, filters=0, duck_count_ok=0, py_count_ok=0, py_count_unsupported=0, ours_planned=0, py_planned=0, live=0)
    bad = []
    for seed in range(s0, s1 + 1):
        d = os.path.join(work, f"t{seed}")
        md = icx("gen", d, seed, "--nan-free")
        icx("scan", d, d + "-ours.parquet")
        ours = pq.read_table(d + "-ours.parquet")
        cols = ours.column_names
        want = rows_of(ours, cols)
        stats["tables"] += 1
        # --- DuckDB
        try:
            got = rows_of(con.sql(f"select * from iceberg_scan('{md}')").to_arrow_table(), cols)
            if got == want: stats["duck_rows_ok"] += 1
            else: bad.append(f"seed {seed}: DuckDB rows differ from ours ({len(got)} vs {len(want)})")
        except Exception as e:
            stats["duck_unsupported"] += 1; print(f"seed {seed}: DuckDB could not read the table: {str(e)[:160]}")
        # --- pyiceberg
        py = None
        try:
            py = StaticTable.from_metadata(md)
            got = rows_of(py.scan().to_arrow(), cols)
            if got == want: stats["py_rows_ok"] += 1
            else: bad.append(f"seed {seed}: pyiceberg rows differ from ours ({len(got)} vs {len(want)})")
        except Exception as e:
            if "equality deletes" in str(e):
                stats["py_unsupported"] += 1; py = None; print(f"seed {seed}: pyiceberg: {str(e)[:100]}")
            else:
                bad.append(f"seed {seed}: pyiceberg failed to read: {type(e).__name__}: {str(e)[:200]}")
        # --- filters
        icx("filters", d, seed, nf, d + "-filters.json")
        for f in json.load(open(d + "-filters.json")):
            stats["filters"] += 1; stats["ours_planned"] += len(f["planned_files"]); stats["live"] += f["live_files"]
            n_ours = f["matched_rows"]
            try:
                n_duck = con.sql(f"select count(*) from iceberg_scan('{md}') where {f['sql']}").fetchall()[0][0]
                if n_duck == n_ours: stats["duck_count_ok"] += 1
                else: bad.append(f"seed {seed} filter {f['id']}: DuckDB counts {n_duck}, ours {n_ours}: {f['sql']}")
            except Exception as e:
                bad.append(f"seed {seed} filter {f['id']}: DuckDB failed on the filter: {str(e)[:160]} | {f['sql']}")
            if py is not None:
                try:
                    e = pyiceberg_expr(f["ast"])
                    n_py = py.scan(row_filter=e).to_arrow().num_rows
                    stats["py_planned"] += len(list(py.scan(row_filter=e).plan_files()))
                    if n_py == n_ours: stats["py_count_ok"] += 1
                    elif null_sensitive(f["ast"]): stats["py_null_semantics"] += 1       # Iceberg's expressions are two-valued: NULL <> 5 and NULL NOT IN (..) are TRUE; SQL's are unknown
                    elif zero_in(f["ast"]): stats["py_zero_semantics"] += 1             # Arrow's is_in hashes the bits: -0.0 IN (0.0) is false there, true in SQL
                    else: bad.append(f"seed {seed} filter {f['id']}: pyiceberg counts {n_py}, ours {n_ours}: {f['sql']}")
                except Exception as ex:
                  if "equality deletes" in str(ex): stats["py_count_unsupported"] += 1
                  elif isinstance(ex, TypeError) and "NoneType" in str(ex): stats["py_bugs"] += 1     # pyiceberg's residual evaluator compares a NULL partition value with a literal
                  else:
                    bad.append(f"seed {seed} filter {f['id']}: pyiceberg failed on a filter: {type(ex).__name__}: {str(ex)[:160]} | {f['sql']}")
    print(json.dumps(stats))
    for b in bad[:40]: print("DISAGREE", b)
    print(f"{len(bad)} disagreements")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
