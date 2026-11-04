"""Item 17: do real clients accept Arrow Decimal64? Reads the files written by the Rust writer.

    spikes/decimal-verify/.venv/bin/python spikes/decimal-verify/clients.py /tmp/nata-dv-clients
"""
import sys
import traceback

import duckdb
import pandas as pd
import polars as pl
import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq

D = sys.argv[1]
COLS = ["d64", "d128", "d64_wide"]
rows = []


def show(label, fn):
    """fn() -> something printable. Records the outcome (value or exact error)."""
    try:
        out = fn()
        print(f"[ok ] {label}\n      {out}")
        rows.append((label, "ok", out))
    except BaseException as e:  # noqa: BLE001 - we want every failure text
        msg = f"{type(e).__name__}: {e}".replace("\n", " | ")
        print(f"[ERR] {label}\n      {msg[:600]}")
        rows.append((label, "ERROR", msg[:600]))


def schema_of(t):
    return {f.name: str(f.type) for f in t.schema if f.name != "id"}


def vals(t):
    return {c: t[c].to_pylist() for c in COLS if c in t.column_names}


print("versions:", "pyarrow", pa.__version__, "polars", pl.__version__, "duckdb", duckdb.__version__, "pandas", pd.__version__)

# ---------------- pyarrow
show("pyarrow ipc file: schema", lambda: schema_of(ipc.open_file(f"{D}/d64.arrow").read_all()))
show("pyarrow ipc file: values", lambda: vals(ipc.open_file(f"{D}/d64.arrow").read_all()))
show("pyarrow ipc stream: schema", lambda: schema_of(ipc.open_stream(f"{D}/d64.arrows").read_all()))
show("pyarrow ipc stream: values", lambda: vals(ipc.open_stream(f"{D}/d64.arrows").read_all()))
show("pyarrow parquet: schema", lambda: schema_of(pq.read_table(f"{D}/d64.parquet")))
show("pyarrow parquet: values", lambda: vals(pq.read_table(f"{D}/d64.parquet")))
show("pyarrow parquet: file schema (physical/logical)", lambda: str(pq.ParquetFile(f"{D}/d64.parquet").schema))

# ---------------- pandas (via pyarrow)
show("pandas read_parquet(pyarrow): dtypes", lambda: {k: str(v) for k, v in pd.read_parquet(f"{D}/d64.parquet").dtypes.items()})
show("pandas read_parquet(pyarrow): values", lambda: {c: list(pd.read_parquet(f"{D}/d64.parquet")[c]) for c in COLS})
show("pandas from ipc file (Table.to_pandas): dtypes", lambda: {k: str(v) for k, v in ipc.open_file(f"{D}/d64.arrow").read_all().to_pandas().dtypes.items()})
show("pandas from ipc file (Table.to_pandas): values", lambda: {c: list(ipc.open_file(f"{D}/d64.arrow").read_all().to_pandas()[c]) for c in COLS})
show("pandas from ipc stream (Table.to_pandas): values", lambda: {c: list(ipc.open_stream(f"{D}/d64.arrows").read_all().to_pandas()[c]) for c in COLS})

# ---------------- polars
show("polars read_parquet: schema", lambda: {k: str(v) for k, v in pl.read_parquet(f"{D}/d64.parquet").schema.items()})
show("polars read_parquet: values", lambda: pl.read_parquet(f"{D}/d64.parquet").select(COLS).to_dict(as_series=False))
show("polars read_ipc (file): schema", lambda: {k: str(v) for k, v in pl.read_ipc(f"{D}/d64.arrow").schema.items()})
show("polars read_ipc (file): values", lambda: pl.read_ipc(f"{D}/d64.arrow").select(COLS).to_dict(as_series=False))
show("polars read_ipc_stream: schema", lambda: {k: str(v) for k, v in pl.read_ipc_stream(f"{D}/d64.arrows").schema.items()})
show("polars read_ipc_stream: values", lambda: pl.read_ipc_stream(f"{D}/d64.arrows").select(COLS).to_dict(as_series=False))
show("polars scan_ipc (lazy, file): collect", lambda: pl.scan_ipc(f"{D}/d64.arrow").select(COLS).collect().to_dict(as_series=False))
show("polars from_arrow(pyarrow table with decimal64)", lambda: pl.from_arrow(ipc.open_file(f"{D}/d64.arrow").read_all()).select(COLS).to_dict(as_series=False))
# the pyarrow Table built directly with a decimal64 type (not via a file) - rules out the IPC reader of the other library
show("polars from_arrow(pa.decimal64 built in Python)", lambda: pl.from_arrow(pa.table({"x": pa.array([123, None], pa.decimal64(15, 2))})).to_dict(as_series=False))

# ---------------- DuckDB
show("duckdb read_parquet: types", lambda: duckdb.sql(f"DESCRIBE SELECT d64, d128, d64_wide FROM read_parquet('{D}/d64.parquet')").fetchall())
show("duckdb read_parquet: values", lambda: duckdb.sql(f"SELECT d64, d128, d64_wide FROM read_parquet('{D}/d64.parquet') ORDER BY id").fetchall())
show("duckdb read_parquet: typeof(d64*d64), d64*d64 values", lambda: duckdb.sql(f"SELECT typeof(d64*d64), d64*d64 FROM read_parquet('{D}/d64.parquet') WHERE id <> 4 ORDER BY id").fetchall())
tbl_file = ipc.open_file(f"{D}/d64.arrow").read_all()
show("duckdb arrow scan (pyarrow Table from ipc file): types", lambda: duckdb.sql("DESCRIBE SELECT d64, d128, d64_wide FROM tbl_file").fetchall())
show("duckdb arrow scan (pyarrow Table from ipc file): values", lambda: duckdb.sql("SELECT d64, d128, d64_wide FROM tbl_file ORDER BY id").fetchall())
tbl_stream = ipc.open_stream(f"{D}/d64.arrows").read_all()
show("duckdb arrow scan (pyarrow Table from ipc stream): values", lambda: duckdb.sql("SELECT d64, d128, d64_wide FROM tbl_stream ORDER BY id").fetchall())
show("duckdb arrow scan via RecordBatchReader (streaming C interface): values", lambda: duckdb.sql("SELECT d64 FROM r ORDER BY 1", params=None).fetchall() if False else duckdb.from_arrow(ipc.open_stream(f"{D}/d64.arrows")).project("d64, d128").fetchall())
show("duckdb arrow scan: only the d64 column, sum", lambda: duckdb.sql("SELECT sum(d64), typeof(sum(d64)) FROM tbl_file").fetchall())

print("\nSUMMARY")
for r in rows:
    print(f"{r[1]:5s} | {r[0]}")
