#!/usr/bin/env python3
"""Reference implementation of the command-target protocol (`CMD in.json out.json`) on top of DuckDB. It is the template for the Nata adapter
and is used to test the protocol. SQLFUZZ_ADAPTER_BUG makes it a known-bad target:
  droprow       the last row of every non-empty result is lost
  nan-null      NaN comes back as NULL
  unsupported   every window query answers "not supported yet: window functions"
  crash         exits with an error
  hang-free     (no bug)
"""
import json, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from sqlfuzz.engine import DuckDB

bug = os.environ.get("SQLFUZZ_ADAPTER_BUG", "")
if bug == "crash": sys.exit(3)
req = json.load(open(sys.argv[1]))
res = DuckDB().run(req["setup"], [(c["id"], c["sql"]) for c in req["cases"]])
out = {}
for cid, r in res.items():
    if not r.ok: out[str(cid)] = {"error": r.error}; continue
    rows = r.rows
    if bug == "droprow" and rows: rows = rows[:-1]
    if bug == "nan-null": rows = [[None if isinstance(v, float) and v != v else v for v in row] for row in rows]
    sql = next(c["sql"] for c in req["cases"] if c["id"] == cid)
    if bug == "unsupported" and " OVER (" in sql: out[str(cid)] = {"error": "not supported yet: window functions"}; continue
    out[str(cid)] = {"cols": r.cols, "types": r.types, "rows": rows}
json.dump({"results": out}, open(sys.argv[2], "w"))
