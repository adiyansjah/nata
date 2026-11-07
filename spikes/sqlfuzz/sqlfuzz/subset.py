"""The generator of scalar and aggregate SQL (`SELECT <exprs> FROM <one table> [WHERE ...] [GROUP BY ...] [HAVING ...] [LIMIT n [OFFSET m]]`, no ORDER BY), built from the
same node classes, pools and seeds as the main generator, so that the shrinker and the faults (`faults.py`) work on it. Everything takes a `random.Random`.

What it leaves out on purpose, each a documented difference from DuckDB (docs/dialect.md section 9, docs/decimal-rules.md section 9) or `not supported yet`:
`/` (an exact decimal here, DOUBLE in DuckDB), float arithmetic, float compared with integer or decimal, casts from DOUBLE to integer or decimal (half to even in
DuckDB), casts from strings that are not in the plain form, `date + integer`, `IN` over expressions, a non-boolean `WHERE`. TINYINT and SMALLINT take part only as
columns, casts and negations: `ti + 1` is TINYINT in DuckDB and INTEGER here (section 9)."""
import random
from .render import *
from . import gen
from .gen import sq, DEC_POOL, DATE_POOL, INT_POOL, BIGINT_POOL, STRINGS

COLS = [("id", "INTEGER"), ("a", "INTEGER"), ("b", "BIGINT"), ("ti", "TINYINT"), ("sm", "SMALLINT"), ("c", "DOUBLE"), ("r", "REAL"),
        ("d", "DECIMAL(10,2)"), ("w", "DECIMAL(25,5)"), ("s", "VARCHAR"), ("f", "BOOLEAN"), ("dt", "DATE")]
TABLES = ("t1", "t2")
SESSION = ["SET TimeZone='UTC'"]
ROW_GROUP = {"t1": 3, "t2": 5}
INTS_ALL = ("TINYINT", "SMALLINT", "INTEGER", "BIGINT")
DEC_TYPES = ("DECIMAL(10,2)", "DECIMAL(25,5)")
TI_POOL = ["0", "1", "-1", "127", "-128", "5", "-5", "100"]
SM_POOL = ["0", "1", "-1", "32767", "-32768", "300", "-300", "7"]
W_POOL = ["0.00000", "1.50000", "-1.50000", "12345678901234567890.12345", "-12345678901234567890.12345", "0.00001", "99999999999999999999.99999", "3.14159"]
DBL_POOL = ["CAST('0.0' AS DOUBLE)", "CAST('-0.0' AS DOUBLE)", "CAST('1.5' AS DOUBLE)", "CAST('-1.5' AS DOUBLE)", "CAST('NaN' AS DOUBLE)", "CAST('Infinity' AS DOUBLE)", "CAST('-Infinity' AS DOUBLE)", "CAST('1e308' AS DOUBLE)", "CAST('-1e308' AS DOUBLE)", "CAST('1e-308' AS DOUBLE)", "CAST('0.1' AS DOUBLE)", "CAST('0.30000000000000004' AS DOUBLE)", "CAST('2.0' AS DOUBLE)", "CAST('-2.5' AS DOUBLE)"]
R_POOL = ["0.0", "-0.0", "1.5", "-1.5", "'NaN'::REAL", "'Infinity'::REAL", "'-Infinity'::REAL", "3.4028235e38", "0.1", "2.0"]
SUBSET_STRINGS = STRINGS + ["12", " 7 ", "-3", "2024-02-29", "true", "1.5", "1e2", "NaN"]


class File:
    """A table by name; the Nata adapter replaces it by the path of the file the DuckDB table was written to."""
    def __init__(self, name): self.name = name
    def sql(self, F): return self.name


def value(rng, typ, null_p=0.15):
    if rng.random() < null_p: return "NULL"
    pool = {"INTEGER": INT_POOL, "BIGINT": BIGINT_POOL, "TINYINT": TI_POOL, "SMALLINT": SM_POOL, "DOUBLE": DBL_POOL, "REAL": R_POOL,
            "DECIMAL(10,2)": DEC_POOL, "DECIMAL(25,5)": W_POOL, "DATE": DATE_POOL}
    if typ in pool:
        v = rng.choice(pool[typ] + ([str(rng.randint(-20, 20))] if typ in ("INTEGER", "BIGINT") else []))
        return v
    if typ == "VARCHAR": return sq(rng.choice(SUBSET_STRINGS))
    if typ == "BOOLEAN": return rng.choice(["true", "false"])
    raise ValueError(typ)


def make_tables(rng, max_rows=25):
    out = {}
    for t in TABLES:
        n = rng.choice([0, 1, 2, 3, 5, 8, 12, max_rows]) if rng.random() < 0.8 else rng.randint(0, max_rows)
        rows = []
        for i in range(1, n + 1):
            r = [str(i)]
            for name, typ in COLS[1:]:
                if name == "a": r.append("NULL" if rng.random() < 0.15 else rng.choice(["2147483647", "-2147483648"]) if rng.random() < 0.1 else str(rng.randint(-3, 5)))
                else: r.append(value(rng, typ))
            rows.append(r)
        out[t] = rows
    return out


def setup_sql(tables):
    cols = ", ".join(f"{n} {t}" + (" NOT NULL" if n == "id" else "") for n, t in COLS)
    stmts = list(SESSION)
    for t, rows in tables.items():
        stmts.append(f"CREATE TABLE {t} ({cols})")
        for k in range(0, len(rows), 10):
            stmts.append(f"INSERT INTO {t} VALUES " + ", ".join("(" + ", ".join(r) + ")" for r in rows[k:k + 10]))
    return stmts


def cols_of(typ): return [Col(t, n) for n, t in COLS if t == typ]
def lit(rng, typ):
    if typ == "REAL": return rng.choice(cols_of("REAL"))      # there is no REAL literal and `CAST(x AS REAL)` is not supported yet
    # TINYINT and SMALLINT are not CAST targets (docs/dialect.md section 8b.4): no typed NULL for them
    # no NULL literal as an operand: DuckDB does not evaluate the other operand of `x + NULL`, so an error in `x` is raised by Nata and not by DuckDB
    # (docs/dialect.md section 9); NULL comes from the data, from select items and CASE results, and from comparisons with a plain column
    v = value(rng, typ, 0.0)
    return Lit(typ, v)


def null_lit(typ):
    return Lit(typ, "NULL") if typ in ("TINYINT", "SMALLINT") else Lit(typ, f"CAST(NULL AS {typ})")


def family(typ):
    if typ in INTS_ALL: return "int"
    if typ in DEC_TYPES: return "dec"
    return {"DOUBLE": "dbl", "REAL": "real", "VARCHAR": "str", "DATE": "date", "BOOLEAN": "bool"}[typ]


def expr(rng, typ, depth):
    """An expression of exactly type `typ`."""
    cols = cols_of(typ)
    if typ in ("TINYINT", "SMALLINT"):
        # an integer literal next to a TINYINT or SMALLINT is that type in DuckDB and INTEGER here (docs/dialect.md section 9): only columns, except in comparisons
        if depth <= 0 or rng.random() < 0.5: return rng.choice(cols)
    elif depth <= 0 or rng.random() < 0.25:
        return rng.choice(cols) if cols and rng.random() < 0.8 else lit(rng, typ)
    r = rng.random()
    if typ in ("INTEGER", "BIGINT"):
        if r < 0.50: return Bin(typ, rng.choice(["+", "-", "*", "%", "//"]), expr(rng, typ, depth - 1), expr(rng, typ, depth - 1))
        if r < 0.58: return Neg(typ, expr(rng, typ, depth - 1))
        if r < 0.70: return case(rng, typ, depth)
        if r < 0.85: return Cast(typ, expr(rng, "INTEGER" if typ == "BIGINT" else "BIGINT", depth - 1))
        if r < 0.93: return Cast(typ, expr(rng, rng.choice(DEC_TYPES), depth - 1))
        return Cast(typ, Lit("VARCHAR", sq(rng.choice(["12", " 7 ", "-3", "0", "+5", "9999999999"]))))
    if typ in ("TINYINT", "SMALLINT"):
        if r < 0.3: return Neg(typ, expr(rng, typ, depth - 1))
        if r < 0.5: return case(rng, typ, depth)
        return rng.choice(cols)
    if typ in DEC_TYPES:
        if r < 0.45:
            other = rng.choice(DEC_TYPES + ("INTEGER", "BIGINT"))
            op = rng.choice(["+", "-", "*", "%"])
            return Bin(typ, op, expr(rng, typ, depth - 1), expr(rng, other, depth - 1)) if rng.random() < 0.5 else Bin(typ, op, expr(rng, other, depth - 1), expr(rng, typ, depth - 1))
        if r < 0.55: return Neg(typ, expr(rng, typ, depth - 1))
        if r < 0.67: return case(rng, typ, depth)
        if r < 0.80: return Cast(typ, expr(rng, rng.choice(INTS_ALL + DEC_TYPES), depth - 1))
        if r < 0.90: return Cast(typ, Lit("VARCHAR", sq(rng.choice(["1.005", "-1.005", "12", "1e2", " 3.5 ", ".5", "0.995", "99999999.995"]))))
        return lit(rng, typ)
    if typ == "DOUBLE":
        if r < 0.3: return case(rng, typ, depth)
        if r < 0.5: return Cast(typ, expr(rng, rng.choice(INTS_ALL + DEC_TYPES), depth - 1))
        if r < 0.6: return Cast(typ, expr(rng, "REAL", depth - 1))
        if r < 0.7: return Cast(typ, Lit("VARCHAR", sq(rng.choice(["1.5", "-2", "NaN", "inf", "-Infinity", "1e300", " 0.1 ", "1e400"]))))
        return rng.choice(cols_of("DOUBLE") + [lit(rng, "DOUBLE")])
    if typ == "REAL": return rng.choice(cols_of("REAL")) if r < 0.7 else case(rng, typ, depth)
    if typ == "VARCHAR":
        if r < 0.4: return case(rng, typ, depth)
        t2 = rng.choice(INTS_ALL + DEC_TYPES + ("DATE", "BOOLEAN"))
        sub = expr(rng, t2, depth - 1)
        # the text of a decimal literal below one differs (docs/dialect.md section 3: 0.20 is DECIMAL(2,2), printed .20): name the type
        if t2 in DEC_TYPES and isinstance(sub, Lit): sub = Cast(t2, sub)
        return Cast(typ, sub)
    if typ == "DATE":
        if r < 0.4: return case(rng, typ, depth)
        return Cast(typ, Lit("VARCHAR", sq(rng.choice(["2024-02-29", "1994-01-01", " 2000-01-01 ", "2023-02-29", "0001-01-01"]))))
    if typ == "BOOLEAN": return boolean(rng, depth)
    return lit(rng, typ)


def case(rng, typ, depth):
    n = rng.randint(1, 2)
    res = lambda: null_lit(typ) if rng.random() < 0.15 and typ != "REAL" else expr(rng, typ, depth - 1)
    els = null_lit(typ) if rng.random() < 0.2 and typ != "REAL" else expr(rng, typ, depth - 1)
    return Case(typ, tuple((boolean(rng, depth - 1), res()) for _ in range(n)), els)


def cmp_type(rng):
    return rng.choice(INTS_ALL + DEC_TYPES + ("DOUBLE", "REAL", "VARCHAR", "DATE", "BOOLEAN"))


def boolean(rng, depth):
    if depth <= 0:
        c = cols_of("BOOLEAN")
        return rng.choice(c) if rng.random() < 0.5 else IsNull(rng.choice([Col(t, n) for n, t in COLS]), rng.random() < 0.5)
    r = rng.random()
    if r < 0.04:
        typ = rng.choice([t for _, t in COLS if t != "REAL"])
        c = Col(typ, rng.choice([n for n, t in COLS if t == typ]))
        return Bin("BOOLEAN", rng.choice(["=", "<>", "<", ">="]), c, null_lit(typ)) if rng.random() < 0.5 else IsNull(c, rng.random() < 0.5)
    if r < 0.40:
        typ = cmp_type(rng)
        x, y = expr(rng, typ, depth - 1), expr(rng, typ, depth - 1)
        if typ in ("TINYINT", "SMALLINT") and rng.random() < 0.5: y = lit(rng, typ)
        if typ in INTS_ALL + DEC_TYPES and rng.random() < 0.3:      # integers meet decimals and integers of other widths
            y = expr(rng, rng.choice(INTS_ALL + DEC_TYPES), depth - 1)
        return Bin("BOOLEAN", rng.choice(["=", "<>", "<", "<=", ">", ">="]), x, y)
    if r < 0.50: return IsNull(expr(rng, rng.choice([t for _, t in COLS]), depth - 1), rng.random() < 0.5)
    if r < 0.65: return Bin("BOOLEAN", rng.choice(["AND", "OR"]), boolean(rng, depth - 1), boolean(rng, depth - 1))
    if r < 0.72: return Not(boolean(rng, depth - 1))
    if r < 0.85:
        typ = rng.choice(INTS_ALL + DEC_TYPES + ("DOUBLE", "DATE", "VARCHAR"))
        return Between(expr(rng, typ, depth - 1), expr(rng, typ, depth - 1) if rng.random() < 0.3 else lit(rng, typ), expr(rng, typ, depth - 1) if rng.random() < 0.3 else lit(rng, typ))
    if r < 0.95:
        typ = rng.choice(INTS_ALL + DEC_TYPES + ("DATE", "VARCHAR"))
        return InList(expr(rng, typ, depth - 1), tuple(lit(rng, typ) for _ in range(rng.randint(1, 4))), rng.random() < 0.4)
    c = cols_of("BOOLEAN")
    return rng.choice(c)


def query(rng):
    d = rng.randint(1, 3)
    k = rng.randint(1, 4)
    def item(i):
        t = rng.choice([t for _, t in COLS])
        e = null_lit(t) if rng.random() < 0.04 and t not in ("TINYINT", "SMALLINT", "REAL") else expr(rng, t, d)
        return Item(e, f"c{i}")
    items = tuple(item(i) for i in range(k))
    where = boolean(rng, d) if rng.random() < 0.7 else None
    lim = rng.choice([None, None, rng.randint(0, 8)])
    off = rng.choice([None, None, rng.randint(0, 5)]) if lim is not None else None
    return SubsetSelect(items, File(rng.choice(TABLES)), where, lim, off)


from dataclasses import dataclass
from typing import Any, Optional


@dataclass(frozen=True)
class SubsetSelect:
    items: tuple
    frm: Any
    where: Optional[Any] = None
    limit: Optional[int] = None
    offset: Optional[int] = None
    group: tuple = ()
    having: Optional[Any] = None
    order: tuple = ()
    distinct: bool = False
    total: bool = True

    def sql(self, F):
        s = "SELECT " + ", ".join(i.sql(F) for i in self.items) + f" FROM {self.frm.sql(F)}"
        if self.where is not None: s += f" WHERE {self.where.sql(F)}"
        if self.group: s += " GROUP BY " + ", ".join(e.sql(F) for e in self.group)
        if self.having is not None: s += f" HAVING {self.having.sql(F)}"
        if self.limit is not None:
            n = self.limit - 1 if ("limit-off-by-one" in F and self.limit > 0) else self.limit
            s += f" LIMIT {n}"
            if self.offset is not None: s += f" OFFSET {self.offset}"
        return s

    @property
    def ordered(self): return not self.group        # insertion order is preserved by DuckDB and by Nata: the rows are compared by position


def aggregate_query(rng):
    # Bounded keys/arguments let numeric differences be checked independently;
    # existing scalar queries continue to exercise strict expression failures.
    keys = [Col(t, n) for n, t in COLS]
    keys += [Bin("INTEGER", "%", Col("INTEGER", "id"), Lit("INTEGER", "3")),
             Cast("BIGINT", Col("INTEGER", "a"))]
    group = tuple(rng.sample(keys, rng.randint(1, 2))) if rng.random() < 0.75 else ()
    items = [Item(e, f"k{i}") for i, e in enumerate(group)]
    for i in range(rng.randint(1, 5)):
        fn = rng.choice(["count", "sum", "avg", "min", "max"])
        if fn == "avg" and any(isinstance(e, Col) and e.t not in INTS_ALL + DEC_TYPES for e in group): fn = "sum"
        pool = COLS if fn in ("count", "min", "max") else [(n,t) for n,t in COLS if t in INTS_ALL + DEC_TYPES]
        n, t = rng.choice(pool)
        arg = None if fn == "count" and rng.random() < 0.5 else Col(t, n)
        items.append(Item(Agg("BIGINT" if fn == "count" else t, fn, arg), f"v{i}"))
    where = Bin("BOOLEAN", ">", Col("INTEGER", "id"), Lit("INTEGER", str(rng.choice([0, 1, 5, 30])))) if rng.random() < 0.5 else None
    having = Bin("BOOLEAN", rng.choice([">", "=", "<="]), Agg("BIGINT", "count", None), Lit("INTEGER", str(rng.randint(0, 3)))) if rng.random() < 0.6 else None
    return SubsetSelect(tuple(items), File(rng.choice(TABLES)), where, group=group, having=having)
