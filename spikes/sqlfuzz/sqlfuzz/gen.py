"""Seeded generators: tables with adversarial data, and random queries over them. Everything takes a `random.Random`, so a seed replays exactly."""
import random
from .render import *

COLS = [("id", "INTEGER"), ("a", "INTEGER"), ("b", "BIGINT"), ("c", "DOUBLE"), ("d", "DECIMAL(10,2)"), ("s", "VARCHAR"),
        ("f", "BOOLEAN"), ("dt", "DATE"), ("ts", "TIMESTAMP"), ("tz", "TIMESTAMPTZ")]
TABLES = ("t1", "t2")
SESSION = ["SET TimeZone='America/New_York'", "SET default_null_order='nulls_last'", "SET default_order='asc'"]

STRINGS = ["", " ", "a", "A", "b", "ab", "Ab", "aB", "AB", "Straße", "STRASSE", "İstanbul", "istanbul", "é", "é", "É", "日本語", "😀", "z",
           "ǅ", "ÅB", " a ", "  ", "%", "_", "\\", "a%b", "x_y", "Zz", "zZ", "ß", "ǆ"]
INT_POOL = ["0", "1", "-1", "2", "3", "7", "100", "2147483647", "-2147483648", "5", "-5"]
BIGINT_POOL = ["0", "1", "-1", "9223372036854775807", "-9223372036854775808", "4294967296", "-4294967296", "3", "10"]
DOUBLE_POOL = ["0.0", "-0.0", "1.5", "-1.5", "'NaN'::DOUBLE", "'Infinity'::DOUBLE", "'-Infinity'::DOUBLE", "1e308", "-1e308", "1e-308", "0.1",
               "0.30000000000000004", "2.0", "3.0", "-2.5"]
DEC_POOL = ["0.00", "-0.01", "0.01", "1.50", "99999999.99", "-99999999.99", "12345.67", "2.00", "-3.25", "0.10", "0.20"]
DATE_POOL = ["DATE '1970-01-01'", "DATE '2000-02-29'", "DATE '1999-12-31'", "DATE '2038-01-19'", "DATE '1900-01-01'", "DATE '2021-03-14'",
             "DATE '2021-11-07'", "DATE '2024-02-29'", "DATE '2000-01-01'"]
TS_POOL = ["TIMESTAMP '1970-01-01 00:00:00'", "TIMESTAMP '2000-02-29 23:59:59.999999'", "TIMESTAMP '2021-03-14 02:30:00'",
           "TIMESTAMP '2021-11-07 01:30:00'", "TIMESTAMP '1969-12-31 23:59:59'", "TIMESTAMP '2021-06-15 12:00:00'"]
TZ_POOL = ["TIMESTAMPTZ '2021-03-14 07:00:00+00'", "TIMESTAMPTZ '2021-11-07 05:30:00+00'", "TIMESTAMPTZ '2021-11-07 06:30:00+00'",
           "TIMESTAMPTZ '1970-01-01 00:00:00+00'", "TIMESTAMPTZ '2021-06-15 23:30:00+00'", "TIMESTAMPTZ '2000-02-29 04:59:59+00'"]


def sq(s): return "'" + s.replace("'", "''") + "'"


def value(rng, typ, null_p=0.15):
    if rng.random() < null_p: return "NULL"
    return {"INTEGER": lambda: rng.choice(INT_POOL + [str(rng.randint(-20, 20))]), "BIGINT": lambda: rng.choice(BIGINT_POOL + [str(rng.randint(-20, 20))]),
            "DOUBLE": lambda: rng.choice(DOUBLE_POOL), "DECIMAL(10,2)": lambda: rng.choice(DEC_POOL), "VARCHAR": lambda: sq(rng.choice(STRINGS)),
            "BOOLEAN": lambda: rng.choice(["true", "false"]), "DATE": lambda: rng.choice(DATE_POOL), "TIMESTAMP": lambda: rng.choice(TS_POOL),
            "TIMESTAMPTZ": lambda: rng.choice(TZ_POOL)}[typ]()


def make_tables(rng, max_rows=25):
    """Returns {name: [row, ...]} where a row is a list of SQL literal strings; `id` is unique and not NULL; `a` is a small join key."""
    out = {}
    for t in TABLES:
        n = rng.choice([0, 1, 2, 3, 5, 8, 12, max_rows]) if rng.random() < 0.8 else rng.randint(0, max_rows)
        rows = []
        for i in range(1, n + 1):
            r = [str(i)]
            for name, typ in COLS[1:]:
                if name == "a": r.append("NULL" if rng.random() < 0.2 else rng.choice(["2147483647", "-2147483648"]) if rng.random() < 0.1 else str(rng.randint(0, 4)))
                elif name == "s" and rng.random() < 0.5: r.append("NULL" if rng.random() < 0.15 else sq(rng.choice(STRINGS[:12])))
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


# ---------------------------------------------------------------- expressions

class Scope:
    def __init__(self, aliases): self.aliases = aliases   # e.g. ["l", "r"]
    def cols(self, typ=None):
        return [Col(t, f"{al}.{n}") for al in self.aliases for n, t in COLS if typ is None or t == typ]


def lit(rng, typ):
    x = lit_(rng, typ)
    return Lit(typ, f"CAST(NULL AS {typ})") if x.text == "NULL" else x


def lit_(rng, typ):
    return Lit(typ, value(rng, typ, 0.1) if rng.random() < 0.7 else {"INTEGER": "1", "BIGINT": "2", "DOUBLE": "1.5", "DECIMAL(10,2)": "1.00", "VARCHAR": "'a'",
               "BOOLEAN": "true", "DATE": "DATE '2000-01-01'", "TIMESTAMP": "TIMESTAMP '2000-01-01 00:00:00'", "TIMESTAMPTZ": "TIMESTAMPTZ '2000-01-01 00:00:00+00'"}[typ])


def numeric_type(rng): return rng.choice(["INTEGER", "BIGINT", "DOUBLE", "DECIMAL(10,2)"])


def expr(rng, sc, typ, depth):
    cols = sc.cols(typ)
    if depth <= 0 or rng.random() < 0.25:
        return rng.choice(cols) if cols and rng.random() < 0.8 else lit(rng, typ)
    r = rng.random()
    if typ in NUM:
        if r < 0.45:
            op = rng.choice(["+", "-", "*", "/", "%"]) if typ != "DECIMAL(10,2)" else rng.choice(["+", "-", "*"])
            if typ in INTS and op == "/": op = rng.choice(["+", "-", "*", "%"]) if rng.random() < 0.5 else "/"
            return Bin(typ, op, expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1))
        if r < 0.55: return Neg(typ, expr(rng, sc, typ, depth - 1))
        if r < 0.65: return Fn(typ, "abs", (expr(rng, sc, typ, depth - 1),))
        if r < 0.75: return Case(typ, ((boolean(rng, sc, depth - 1), expr(rng, sc, typ, depth - 1)),), expr(rng, sc, typ, depth - 1))
        if r < 0.85: return Fn(typ, "coalesce", (expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1)))
        if typ in INTS and r < 0.93: return Fn(typ, "length", (expr(rng, sc, "VARCHAR", depth - 1),))
        if typ == "DOUBLE": return Fn(typ, rng.choice(["round", "floor", "ceil"]), (expr(rng, sc, typ, depth - 1),)) if rng.random() < 0.5 else Cast(typ, expr(rng, sc, rng.choice(["INTEGER", "DECIMAL(10,2)"]), depth - 1))
        if typ in INTS: return Fn(typ, "date_part", (Lit("VARCHAR", sq(rng.choice(["year", "month", "day", "dow"]))), expr(rng, sc, rng.choice(["DATE", "TIMESTAMP"]), depth - 1)))
        return Cast(typ, expr(rng, sc, "DOUBLE", depth - 1)) if rng.random() < 0.3 else Fn(typ, "round", (expr(rng, sc, typ, depth - 1), Lit("INTEGER", "1")))
    if typ == "VARCHAR":
        if r < 0.3: return Bin(typ, "||", expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1))
        if r < 0.5: return Fn(typ, rng.choice(["lower", "upper", "trim"]), (expr(rng, sc, typ, depth - 1),))
        if r < 0.65: return Fn(typ, "substr", (expr(rng, sc, typ, depth - 1), Lit("INTEGER", str(rng.randint(1, 3))), Lit("INTEGER", str(rng.randint(0, 3)))))
        if r < 0.8: return Fn(typ, "coalesce", (expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1)))
        if r < 0.9: return Case(typ, ((boolean(rng, sc, depth - 1), expr(rng, sc, typ, depth - 1)),), expr(rng, sc, typ, depth - 1))
        return Cast(typ, expr(rng, sc, rng.choice(["INTEGER", "DATE", "BOOLEAN"]), depth - 1))
    if typ == "BOOLEAN": return boolean(rng, sc, depth)
    if typ == "DATE":
        if r < 0.4: return Bin(typ, "+", expr(rng, sc, "DATE", depth - 1), Lit("VARCHAR", f"INTERVAL '{rng.randint(-40, 40)}' DAY"))
        if r < 0.6: return Fn(typ, "date_trunc", (Lit("VARCHAR", sq(rng.choice(["month", "year", "week"]))), expr(rng, sc, "DATE", depth - 1)))
        if r < 0.8: return Cast(typ, expr(rng, sc, "TIMESTAMP", depth - 1))
        return Fn(typ, "coalesce", (expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1)))
    if typ == "TIMESTAMP":
        if r < 0.4: return Bin(typ, "+", expr(rng, sc, typ, depth - 1), Lit("VARCHAR", f"INTERVAL '{rng.randint(-100, 100)}' HOUR"))
        if r < 0.6: return Fn(typ, "date_trunc", (Lit("VARCHAR", sq(rng.choice(["day", "hour", "month"]))), expr(rng, sc, typ, depth - 1)))
        if r < 0.8: return Cast(typ, expr(rng, sc, "DATE", depth - 1))
        return Cast(typ, expr(rng, sc, "TIMESTAMPTZ", depth - 1))      # uses the session time zone
    if typ == "TIMESTAMPTZ":
        if r < 0.4: return Bin(typ, "+", expr(rng, sc, typ, depth - 1), Lit("VARCHAR", f"INTERVAL '{rng.randint(-30, 30)}' HOUR"))
        if r < 0.7: return Fn(typ, "date_trunc", (Lit("VARCHAR", sq(rng.choice(["day", "month", "hour"]))), expr(rng, sc, typ, depth - 1)))
        return Cast(typ, expr(rng, sc, "TIMESTAMP", depth - 1))
    return lit(rng, typ)


def boolean(rng, sc, depth):
    if depth <= 0:
        c = sc.cols("BOOLEAN")
        return rng.choice(c) if c else IsNull(rng.choice(sc.cols()), rng.random() < 0.5)
    r = rng.random()
    if r < 0.45:
        typ = rng.choice(["INTEGER", "BIGINT", "DOUBLE", "DECIMAL(10,2)", "VARCHAR", "DATE", "TIMESTAMP", "TIMESTAMPTZ", "BOOLEAN"])
        return Bin("BOOLEAN", rng.choice(["=", "<>", "<", "<=", ">", ">="]), expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1))
    if r < 0.55: return IsNull(expr(rng, sc, rng.choice([t for _, t in COLS]), depth - 1), rng.random() < 0.5)
    if r < 0.65:
        typ = rng.choice(["INTEGER", "DOUBLE", "VARCHAR", "DATE"])
        return Distinct(expr(rng, sc, typ, depth - 1), expr(rng, sc, typ, depth - 1), rng.random() < 0.5)
    if r < 0.75: return Bin("BOOLEAN", rng.choice(["AND", "OR"]), boolean(rng, sc, depth - 1), boolean(rng, sc, depth - 1))
    if r < 0.8: return Not(boolean(rng, sc, depth - 1))
    if r < 0.87:
        typ = rng.choice(["INTEGER", "DOUBLE", "DECIMAL(10,2)", "DATE"])
        return Between(expr(rng, sc, typ, depth - 1), lit(rng, typ), lit(rng, typ))
    if r < 0.93: return InList(expr(rng, sc, "INTEGER", depth - 1), tuple(lit(rng, "INTEGER") for _ in range(rng.randint(1, 4))), rng.random() < 0.4)
    return Like(expr(rng, sc, "VARCHAR", depth - 1), sq(rng.choice(["a%", "%a", "%A%", "_b", "%", "a_b", "%ß%", "é%"])))


# ---------------------------------------------------------------- queries

def select_items(rng, sc, n, depth):
    items = []
    for k in range(n):
        typ = rng.choice([t for _, t in COLS])
        items.append(Item(expr(rng, sc, typ, depth), f"c{k}"))
    return items


def q_project(rng):
    sc = Scope(["l"]); depth = rng.randint(1, 3)
    order = (Order(Col("INTEGER", "l.id")),)
    lim = rng.choice([None, None, rng.randint(0, 6)])
    ord_ = (Order(Col("INTEGER", "l.id")),)
    items = (Item(Col("INTEGER", "l.id"), "id"),) + tuple(select_items(rng, sc, rng.randint(1, 3), depth))
    return Select(items, Table(rng.choice(TABLES), "l"), where=boolean(rng, sc, depth) if rng.random() < 0.7 else None, order=ord_, limit=lim, total=True)


def q_filter_unordered(rng):
    sc = Scope(["l"]); d = rng.randint(1, 3)
    return Select(tuple(select_items(rng, sc, rng.randint(1, 3), d)), Table(rng.choice(TABLES), "l"), where=boolean(rng, sc, d), distinct=rng.random() < 0.3)


def agg_expr(rng, sc):
    fn = rng.choice(["count", "count", "sum", "avg", "min", "max", "count_star"])
    if fn == "count_star": return Agg("BIGINT", "count", None)
    if fn == "count": return Agg("BIGINT", "count", expr(rng, sc, rng.choice([t for _, t in COLS]), 1), rng.random() < 0.2)
    if fn in ("sum", "avg"):
        typ = rng.choice(["INTEGER", "BIGINT", "DECIMAL(10,2)", "DOUBLE"])
        return Agg("DOUBLE" if fn == "avg" else typ, fn, expr(rng, sc, typ, 1))
    typ = rng.choice([t for _, t in COLS if t != "BOOLEAN"])
    return Agg(typ, fn, expr(rng, sc, typ, 1))


def q_aggregate(rng):
    sc = Scope(["l"]); keys = rng.choice([("a",), ("s",), ("a", "f"), ("dt",), ("c",), ("tz",), ()])
    kcols = tuple(Col(dict(COLS)[k], f"l.{k}") for k in keys)
    items = tuple(Item(c, f"k{i}") for i, c in enumerate(kcols)) + tuple(Item(agg_expr(rng, sc), f"g{i}") for i in range(rng.randint(1, 3)))
    having = None
    if kcols and rng.random() < 0.3: having = Bin("BOOLEAN", rng.choice([">", ">="]), Agg("BIGINT", "count", None), Lit("BIGINT", str(rng.randint(0, 3))))
    order = tuple(Order(c, rng.random() < 0.3, rng.choice(["first", "last"])) for c in kcols)
    return Select(items, Table(rng.choice(TABLES), "l"), where=boolean(rng, sc, 1) if rng.random() < 0.3 else None, group=kcols, having=having,
                  order=order, total=bool(kcols))


def q_join(rng):
    kind = rng.choice(["INNER", "LEFT", "RIGHT", "FULL"])
    key = rng.choice(["a", "a", "s", "b"]) if rng.random() < 0.3 else "a"
    typ = dict(COLS)[key]
    keyeq = Bin("BOOLEAN", "=", Col(typ, f"l.{key}"), Col(typ, f"r.{key}"))
    on = keyeq
    if rng.random() < 0.4: on = Bin("BOOLEAN", "AND", keyeq, Bin("BOOLEAN", rng.choice(["<>", "<", ">="]), Col("INTEGER", "l.id"), Col("INTEGER", "r.id")))
    sc = Scope(["l", "r"])
    items = (Item(Col("INTEGER", "l.id"), "lid"), Item(Col("INTEGER", "r.id"), "rid")) + tuple(select_items(rng, sc, rng.randint(0, 2), 1))
    j = Join(kind, Table("t1", "l"), Table("t2", "r"), on, keys=((f"l.{key}", f"r.{key}"),))
    where = boolean(rng, sc, 1) if rng.random() < 0.3 else None
    return Select(items, j, where=where, order=(Order(Col("INTEGER", "l.id")), Order(Col("INTEGER", "r.id"))), total=True)


def q_semi_anti(rng):
    neg = rng.random() < 0.5
    key = "a" if rng.random() < 0.7 else "s"
    typ = dict(COLS)[key]
    extra = Bin("BOOLEAN", "<>", Col("INTEGER", "r.id"), Col("INTEGER", "l.id")) if rng.random() < 0.3 else None
    ex = Exists(neg, "t2", "r", Col(typ, f"l.{key}"), Col(typ, f"r.{key}"), extra)
    sc = Scope(["l"])
    return Select((Item(Col("INTEGER", "l.id"), "id"), Item(Col(typ, f"l.{key}"), "k")), Table("t1", "l"), where=ex, order=(Order(Col("INTEGER", "l.id")),), total=True)


def q_window(rng):
    sc = Scope(["l"]); part = (Col(dict(COLS)[k], f"l.{k}") for k in [rng.choice(["a", "s", "f"])]); part = tuple(part)
    ordcol = rng.choice(["b", "c", "d", "s", "dt"]); oc = Col(dict(COLS)[ordcol], f"l.{ordcol}")
    order = (Order(oc, rng.random() < 0.4, rng.choice(["first", "last"])), Order(Col("INTEGER", "l.id")))
    fn = rng.choice(["row_number", "lag", "lead", "sum", "rank", "dense_rank"])
    if fn in ("rank", "dense_rank"): w = Win("BIGINT", fn, None, part, (Order(oc, rng.random() < 0.4, rng.choice(["first", "last"])),))  # ties are the point
    elif fn == "row_number": w = Win("BIGINT", fn, None, part, order)
    elif fn in ("lag", "lead"):
        typ = rng.choice(["INTEGER", "DOUBLE", "VARCHAR", "DATE"]); w = Win(typ, fn, expr(rng, sc, typ, 1), part, order)
    else:
        typ = rng.choice(["INTEGER", "DECIMAL(10,2)"]); w = Win("DECIMAL(38,2)" if typ != "INTEGER" else "HUGEINT", "sum", expr(rng, sc, typ, 1), part, ())
    return Select((Item(Col("INTEGER", "l.id"), "id"), Item(w, "w")), Table(rng.choice(TABLES), "l"), order=(Order(Col("INTEGER", "l.id")),), total=True)


def q_setop(rng):
    sc = Scope(["l"]); sc2 = Scope(["r"])
    typs = [rng.choice(["INTEGER", "VARCHAR", "DOUBLE", "DATE"]) for _ in range(rng.randint(1, 2))]
    mk = lambda al, s, t: Select(tuple(Item(expr(rng, s, ty, 1), f"c{i}") for i, ty in enumerate(typs)), Table(t, al),
                                  where=boolean(rng, s, 1) if rng.random() < 0.4 else None)
    return SetOp(rng.choice(["UNION", "UNION", "INTERSECT", "EXCEPT"]), rng.random() < 0.3, mk("l", sc, "t1"), mk("r", sc2, "t2"))


def q_topn_sub(rng):
    sc = Scope(["x"]); inner = q_aggregate(rng)
    return inner


GENS = [(q_project, 4), (q_filter_unordered, 2), (q_aggregate, 4), (q_join, 4), (q_semi_anti, 3), (q_window, 3), (q_setop, 3)]


def gen_query(rng):
    fs, ws = zip(*GENS)
    return rng.choices(fs, ws)[0](rng)
