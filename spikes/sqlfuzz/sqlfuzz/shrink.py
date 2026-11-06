"""Delta-debugging for a failing case: simplify the query (drop clauses and select items, replace sub-expressions by a child or a constant), then
drop table rows, as long as `still_fails` stays true."""
from dataclasses import fields, replace, is_dataclass
from .render import *

ZERO = {"INTEGER": "0", "BIGINT": "0", "DOUBLE": "0.0", "DECIMAL(10,2)": "0.00", "VARCHAR": "''", "BOOLEAN": "true", "DATE": "DATE '2000-01-01'",
        "TIMESTAMP": "TIMESTAMP '2000-01-01 00:00:00'", "TIMESTAMPTZ": "TIMESTAMPTZ '2000-01-01 00:00:00+00'"}
SIMPLE = (Bin, Fn, Case, Neg, Cast, Not, Between, InList, Like, IsNull, Distinct, Agg, Win)


def here(n):
    """Simpler replacements for node n itself."""
    if isinstance(n, Select) or type(n).__name__ == "SubsetSelect":
        if n.where is not None: yield replace(n, where=None)
        if n.having is not None: yield replace(n, having=None)
        if n.order: yield replace(n, order=(), total=False, limit=None)
        if n.limit is not None: yield replace(n, limit=None, offset=None) if hasattr(n, "offset") else replace(n, limit=None)
        if getattr(n, "offset", None) is not None: yield replace(n, offset=None)
        if n.distinct: yield replace(n, distinct=False)
        if len(n.items) > 1:
            for i in range(len(n.items)): yield replace(n, items=n.items[:i] + n.items[i + 1:])
        return
    if isinstance(n, Join):
        if n.kind != "INNER": yield replace(n, kind="INNER")
        return
    if isinstance(n, SetOp):
        yield n.l; yield n.r
        return
    if isinstance(n, SIMPLE) or isinstance(n, Exists):
        t = getattr(n, "t", None)
        for f in fields(n):
            v = getattr(n, f.name)
            kids = [v] if hasattr(v, "sql") else [x for x in v if hasattr(x, "sql")] if isinstance(v, tuple) else []
            for k in kids:
                if getattr(k, "t", None) == t and not isinstance(n, Agg): yield k
        if t in ZERO and not isinstance(n, Lit):
            yield Lit(t, ZERO[t]); yield Lit(t, "NULL")


def shrinks(n):
    yield from here(n)
    if not is_dataclass(n): return
    for f in fields(n):
        v = getattr(n, f.name)
        if hasattr(v, "sql"):
            for s in shrinks(v): yield replace(n, **{f.name: s})
        elif isinstance(v, tuple) and f.name not in ("keys",):
            for i, x in enumerate(v):
                if hasattr(x, "sql"):
                    for s in shrinks(x): yield replace(n, **{f.name: v[:i] + (s,) + v[i + 1:]})
                elif isinstance(x, tuple):                      # Case.whens: (cond, value) pairs
                    for j, y in enumerate(x):
                        for s in shrinks(y):
                            pair = x[:j] + (s,) + x[j + 1:]
                            yield replace(n, **{f.name: v[:i] + (pair,) + v[i + 1:]})


def shrink(query, tables, still_fails, max_checks=400):
    """still_fails(query, tables) -> bool. Returns (query, tables, checks_used)."""
    checks = 0
    size = lambda x: len(x.sql(frozenset()))
    improved = True
    while improved and checks < max_checks:
        improved = False
        for cand in shrinks(query):
            if size(cand) >= size(query) and not isinstance(cand, type(query)): continue
            checks += 1
            if checks > max_checks: break
            if size(cand) < size(query) and still_fails(cand, tables):
                query, improved = cand, True
                break
    for t in list(tables):                              # rows: try dropping halves, then single rows
        rows = tables[t]; step = max(1, len(rows) // 2)
        while step >= 1 and checks < max_checks + 200:
            i = 0
            while i < len(rows):
                cand_rows = rows[:i] + rows[i + step:]
                checks += 1
                if cand_rows != rows and still_fails(query, {**tables, t: cand_rows}):
                    rows = cand_rows; tables = {**tables, t: rows}
                else:
                    i += step
            step //= 2
    return query, tables, checks
