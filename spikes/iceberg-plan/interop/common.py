"""Shared helpers for the interop checks: row normalisation, DuckDB and pyiceberg readers, the JSON filter AST -> pyiceberg expression."""
import datetime, decimal, json, math, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ICX = os.environ.get("ICX", os.path.join(HERE, "..", "target", "release", "icx"))


def icx(*args):
    p = subprocess.run([ICX, *map(str, args)], capture_output=True, text=True)
    if p.returncode != 0:
        raise RuntimeError(f"icx {' '.join(map(str, args))} failed: {p.stderr[-500:]}")
    return p.stdout.strip()


def norm_cell(v):
    if v is None: return "NULL"
    if isinstance(v, float):
        return "nan" if math.isnan(v) else ("-0.0" if v == 0 and math.copysign(1, v) < 0 else repr(v))
    if isinstance(v, decimal.Decimal): return "dec:" + format(v, "f")
    if isinstance(v, (datetime.datetime,)): return "ts:" + v.replace(tzinfo=None).isoformat()
    if isinstance(v, datetime.date): return "date:" + v.isoformat()
    if isinstance(v, (bytes, bytearray)): return "bin:" + bytes(v).hex()
    return repr(v)


def rows_of(arrow_table, cols):
    """Rows as a sorted list of strings, in the column order `cols` (matched by name)."""
    data = arrow_table.select(cols).to_pylist()
    return sorted("|".join(norm_cell(r[c]) for c in cols) for r in data)


def duckdb_con():
    import duckdb
    con = duckdb.connect()
    con.execute("INSTALL iceberg; LOAD iceberg;")
    return con


def pyiceberg_expr(ast):
    from pyiceberg import expressions as E
    from decimal import Decimal
    op = ast["op"]
    if op == "true": return E.AlwaysTrue()
    if op == "false": return E.AlwaysFalse()
    if op == "and": return E.And(pyiceberg_expr(ast["l"]), pyiceberg_expr(ast["r"]))
    if op == "or": return E.Or(pyiceberg_expr(ast["l"]), pyiceberg_expr(ast["r"]))
    if op == "not": return E.Not(pyiceberg_expr(ast["x"]))
    col = ast["col"]

    def lit(v):
        t, s = v["t"], v["v"]
        if t in ("int", "long"): return int(s)
        if t in ("double", "float"): return float(s.replace("'", "").split("::")[0].replace("Infinity", "inf"))
        if t.startswith("decimal"): return Decimal(s)
        return s                      # string, and ISO strings for date and timestamp (pyiceberg converts them to the column type)
    if op in ("eq", "lt", "le", "gt", "ge", "ne"):
        cls = {"eq": E.EqualTo, "lt": E.LessThan, "le": E.LessThanOrEqual, "gt": E.GreaterThan, "ge": E.GreaterThanOrEqual, "ne": E.NotEqualTo}[op]
        return cls(col, lit(ast["val"]))
    if op == "in": return E.In(col, [lit(v) for v in ast["vals"]])
    if op == "not_in": return E.NotIn(col, [lit(v) for v in ast["vals"]])
    if op == "is_null": return E.IsNull(col)
    if op == "not_null": return E.NotNull(col)
    if op == "is_nan": return E.IsNaN(col)
    if op == "not_nan": return E.NotNaN(col)
    if op == "starts_with": return E.StartsWith(col, lit(ast["val"]))
    if op == "not_starts_with": return E.NotStartsWith(col, lit(ast["val"]))
    raise ValueError(op)
