#!/usr/bin/env python3
"""A documented hazard of the spec's truncate formula `v - (((v % W) + W) % W)`: for values within W of the type's minimum it overflows, and Java (and so every
implementation that follows it) writes a WRAPPED partition value (not a lower bound of v). Engines that prune with the standard inclusive projection then lose
those rows on range predicates. This builds a truncate[10] table with values at both extremes and counts, per engine, the rows each range predicate returns.
Our reader is exact (it never prunes on a partition value that is not a multiple of W). usage: truncate_overflow.py <workdir>   exit 0 always (it reports)."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *

MIN, MAX = -2**31, 2**31 - 1
KS = [MIN, MIN + 1, MIN + 9, MIN + 10, -5, 3, 100, MAX - 9, MAX]
PREDS = [("k < 0", lambda k: k < 0), ("k <= -5", lambda k: k <= -5), ("k < -2000000000", lambda k: k < -2000000000), (f"k = {MIN}", lambda k: k == MIN),
         ("k > 50", lambda k: k > 50), (f"k >= {MAX - 9}", lambda k: k >= MAX - 9)]


def main():
    d = os.path.join(sys.argv[1], "overflow")
    md = icx("overflow", d)
    from pyiceberg.table import StaticTable
    from pyiceberg import expressions as E
    py = StaticTable.from_metadata(md)
    con = duckdb_con()
    res = {}
    for sql, fn in PREDS:
        truth = sum(1 for k in KS if fn(k))
        duck = con.sql(f"select count(*) from iceberg_scan('{md}') where {sql}").fetchall()[0][0]
        e = {"k < 0": E.LessThan("k", 0), "k <= -5": E.LessThanOrEqual("k", -5), "k < -2000000000": E.LessThan("k", -2000000000), f"k = {MIN}": E.EqualTo("k", MIN),
             "k > 50": E.GreaterThan("k", 50), f"k >= {MAX - 9}": E.GreaterThanOrEqual("k", MAX - 9)}[sql]
        pyn = py.scan(row_filter=e).to_arrow().num_rows
        res[sql] = dict(truth=truth, duckdb=duck, pyiceberg=pyn)
        print(f"{sql:22} truth {truth}  DuckDB {duck}  pyiceberg {pyn}  {'OK' if truth == duck == pyn else 'ROWS LOST' }")
    # ours: exact
    out = icx("filters", d, 1, 0, os.path.join(sys.argv[1], "overflow-filters.json"))
    print(json.dumps(res))
    return 0


if __name__ == "__main__":
    sys.exit(main())
