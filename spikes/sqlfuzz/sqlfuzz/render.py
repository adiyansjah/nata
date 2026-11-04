"""SQL model (small frozen dataclasses) and its rendering to DuckDB-dialect SQL.

Every node renders with `sql(F)` where F is a set of FAULT names. With F empty the SQL is the faithful query; each fault changes the rendering
the way a classic engine bug would (see faults.py for the list and what each one emulates). A faulty "engine" is therefore the oracle running
the faulty rendering, which gives known-bad engines that need nothing but DuckDB.
"""
from dataclasses import dataclass
from typing import Any, Optional

INTS = ("INTEGER", "BIGINT")
NUM = ("INTEGER", "BIGINT", "DOUBLE", "DECIMAL(10,2)")


def q(items, F, sep=", "):
    return sep.join(i.sql(F) for i in items)


@dataclass(frozen=True)
class Col:
    t: str
    name: str
    def sql(self, F): return self.name


@dataclass(frozen=True)
class Lit:
    t: str
    text: str
    def sql(self, F): return self.text


@dataclass(frozen=True)
class Not:
    x: Any
    t: str = "BOOLEAN"
    def sql(self, F): return f"(NOT {self.x.sql(F)})"


@dataclass(frozen=True)
class Neg:
    t: str
    x: Any
    def sql(self, F): return f"(- {self.x.sql(F)})"


@dataclass(frozen=True)
class Bin:
    t: str
    op: str
    l: Any
    r: Any

    def sql(self, F):
        l, r, op = self.l.sql(F), self.r.sql(F), self.op
        lt = getattr(self.l, "t", None)
        if op in ("<", "<=", ">", ">=", "=", "<>") and lt == "VARCHAR" and "string-compare-case-insensitive" in F and op in ("=", "<>"):
            return f"(lower({l}) {op} lower({r}))"
        if op in ("<", "<=", ">", ">=", "=", "<>") and lt == "DOUBLE" and "nan-is-ieee" in F:
            if op == "<>": return f"(({l} <> {r}) OR isnan({l}) OR isnan({r}))"
            return f"(({l} {op} {r}) AND NOT isnan({l}) AND NOT isnan({r}))"
        if op == "/" and self.t in INTS and "int-div-truncates" in F:
            return f"({l} // {r})"
        if op in ("+", "-", "*") and self.t == "INTEGER" and "int-overflow-yields-null" in F:
            return f"TRY_CAST((CAST({l} AS BIGINT) {op} CAST({r} AS BIGINT)) AS INTEGER)"
        if op in ("+", "-", "*") and self.t == "DECIMAL(10,2)" and "decimal-via-double" in F:
            return f"CAST((CAST({l} AS DOUBLE) {op} CAST({r} AS DOUBLE)) AS DECIMAL(10,2))"
        return f"({l} {op} {r})"


@dataclass(frozen=True)
class Fn:
    t: str
    name: str
    args: tuple

    def sql(self, F):
        a = [x.sql(F) for x in self.args]
        n = self.name
        if n == "length" and "length-in-bytes" in F: return f"strlen({a[0]})"
        if n == "upper" and "upper-ascii-only" in F: return f"translate({a[0]}, 'abcdefghijklmnopqrstuvwxyz', 'ABCDEFGHIJKLMNOPQRSTUVWXYZ')"
        if n == "trim" and "trim-leading-only" in F: return f"ltrim({a[0]})"
        if n == "substr" and "substr-zero-based" in F: return f"substr({a[0]}, {a[1]} + 1, {a[2]})"
        return f"{n}({', '.join(a)})"


@dataclass(frozen=True)
class Case:
    t: str
    whens: tuple   # ((cond, value), ...)
    els: Any
    def sql(self, F):
        w = " ".join(f"WHEN {c.sql(F)} THEN {v.sql(F)}" for c, v in self.whens)
        return f"(CASE {w} ELSE {self.els.sql(F)} END)"


@dataclass(frozen=True)
class Cast:
    t: str
    x: Any
    def sql(self, F): return f"CAST({self.x.sql(F)} AS {self.t})"


@dataclass(frozen=True)
class IsNull:
    x: Any
    neg: bool = False
    t: str = "BOOLEAN"
    def sql(self, F): return f"({self.x.sql(F)} IS {'NOT ' if self.neg else ''}NULL)"


@dataclass(frozen=True)
class Distinct:
    l: Any
    r: Any
    neg: bool = False   # IS NOT DISTINCT FROM
    t: str = "BOOLEAN"
    def sql(self, F): return f"({self.l.sql(F)} IS {'NOT ' if self.neg else ''}DISTINCT FROM {self.r.sql(F)})"


@dataclass(frozen=True)
class Between:
    x: Any
    lo: Any
    hi: Any
    t: str = "BOOLEAN"
    def sql(self, F):
        x, lo, hi = self.x.sql(F), self.lo.sql(F), self.hi.sql(F)
        if "between-exclusive" in F: return f"({x} > {lo} AND {x} < {hi})"
        return f"({x} BETWEEN {lo} AND {hi})"


@dataclass(frozen=True)
class InList:
    x: Any
    items: tuple
    neg: bool = False
    t: str = "BOOLEAN"
    def sql(self, F): return f"({self.x.sql(F)} {'NOT ' if self.neg else ''}IN ({q(self.items, F)}))"


@dataclass(frozen=True)
class Like:
    x: Any
    pat: str   # SQL literal text
    t: str = "BOOLEAN"
    def sql(self, F): return f"({self.x.sql(F)} {'ILIKE' if 'like-case-insensitive' in F else 'LIKE'} {self.pat})"


@dataclass(frozen=True)
class Agg:
    t: str
    fn: str
    arg: Optional[Any]    # None = *
    distinct: bool = False

    def sql(self, F):
        a = "*" if self.arg is None else self.arg.sql(F)
        d = "DISTINCT " if self.distinct else ""
        if self.fn == "count" and self.arg is not None and "count-col-counts-nulls" in F: return "count(*)"
        if self.fn == "avg" and "avg-divides-by-rows" in F: return f"(CAST(sum({d}{a}) AS DOUBLE) / count(*))"
        if self.fn == "sum" and "sum-null-is-zero" in F: return f"COALESCE(sum({d}{a}), 0)"
        return f"{self.fn}({d}{a})"


@dataclass(frozen=True)
class Order:
    e: Any
    desc: bool = False
    nulls: str = "last"   # explicit, so the faulty renderings can flip it

    def sql(self, F):
        desc = self.desc and "sort-desc-ignored" not in F
        nulls = self.nulls
        if "sort-nulls-flipped" in F: nulls = "first" if nulls == "last" else "last"
        return f"{self.e.sql(F)} {'DESC' if desc else 'ASC'} NULLS {nulls.upper()}"


@dataclass(frozen=True)
class Win:
    t: str
    fn: str            # row_number rank dense_rank lag lead sum
    arg: Optional[Any]
    part: tuple
    order: tuple

    def sql(self, F):
        fn = self.fn
        if fn == "rank" and "rank-as-row-number" in F: fn = "row_number"
        a = "" if self.arg is None else self.arg.sql(F)
        over = []
        if self.part: over.append("PARTITION BY " + q(self.part, F))
        if self.order: over.append("ORDER BY " + q(self.order, F))
        call = f"{fn}({a}) OVER ({' '.join(over)})"
        if fn == "lag" and "lag-default-zero" in F and self.t in NUM: call = f"COALESCE({call}, 0)"
        return call


@dataclass(frozen=True)
class Exists:
    """EXISTS / NOT EXISTS (SELECT 1 FROM table alias WHERE inner = outer [AND extra]); the key equality is what the faults change."""
    neg: bool
    table: str
    alias: str
    outer: Any
    inner: Any
    extra: Optional[Any] = None
    t: str = "BOOLEAN"

    def sql(self, F):
        o, i = self.outer.sql(F), self.inner.sql(F)
        ex = f" AND {self.extra.sql(F)}" if self.extra is not None else ""
        if self.neg and "anti-as-not-in" in F and self.extra is None:
            return f"({o} NOT IN (SELECT {i} FROM {self.table} {self.alias}))"
        eq = f"{i} IS NOT DISTINCT FROM {o}" if "semi-null-matches" in F else f"{i} = {o}"
        return f"({'NOT ' if self.neg else ''}EXISTS (SELECT 1 FROM {self.table} {self.alias} WHERE {eq}{ex}))"


# ---------------------------------------------------------------- relations and queries

@dataclass(frozen=True)
class Item:
    e: Any
    alias: str
    def sql(self, F): return f"{self.e.sql(F)} AS {self.alias}"


@dataclass(frozen=True)
class Table:
    name: str
    alias: str
    def sql(self, F): return f"{self.name} {self.alias}"


@dataclass(frozen=True)
class Join:
    kind: str    # INNER LEFT RIGHT FULL
    l: Any
    r: Any
    on: Any
    keys: tuple = ()   # ((left_col, right_col), ...) the equality keys inside `on`, rendered by the fault-aware code below

    def sql(self, F):
        kind = self.kind
        if kind == "LEFT" and "join-left-as-inner" in F: kind = "INNER"
        if kind == "RIGHT" and "join-right-as-inner" in F: kind = "INNER"
        if kind == "FULL" and "join-full-as-left" in F: kind = "LEFT"
        on = self.on.sql(F)
        if "join-null-keys-match" in F:
            for lk, rk in self.keys:
                on = on.replace(f"({lk} = {rk})", f"({lk} IS NOT DISTINCT FROM {rk})")
        return f"{self.l.sql(F)} {kind} JOIN {self.r.sql(F)} ON {on}"


@dataclass(frozen=True)
class Sub:
    query: Any
    alias: str
    def sql(self, F): return f"({self.query.sql(F)}) {self.alias}"


@dataclass(frozen=True)
class Select:
    items: tuple
    frm: Any
    where: Optional[Any] = None
    group: tuple = ()
    having: Optional[Any] = None
    order: tuple = ()
    limit: Optional[int] = None
    distinct: bool = False
    total: bool = False   # the ORDER BY is a total order, so the result order is defined and is compared

    def sql(self, F):
        s = "SELECT " + ("DISTINCT " if self.distinct else "") + q(self.items, F) + f" FROM {self.frm.sql(F)}"
        if self.where is not None: s += f" WHERE {self.where.sql(F)}"
        if self.group: s += " GROUP BY " + q(self.group, F)
        if self.having is not None: s += f" HAVING {self.having.sql(F)}"
        if self.order: s += " ORDER BY " + q(self.order, F)
        if self.limit is not None:
            n = self.limit - 1 if ("limit-off-by-one" in F and self.limit > 0) else self.limit
            s += f" LIMIT {n}"
        return s

    @property
    def ordered(self): return self.total and bool(self.order)


@dataclass(frozen=True)
class SetOp:
    op: str      # UNION INTERSECT EXCEPT
    all: bool
    l: Any
    r: Any
    ordered: bool = False

    def sql(self, F):
        all_ = self.all
        if self.op == "UNION" and not all_ and "union-distinct-as-all" in F: all_ = True
        if self.op in ("INTERSECT", "EXCEPT") and not all_ and "setop-distinct-as-all" in F: all_ = True
        return f"{self.l.sql(F)} {self.op}{' ALL' if all_ else ''} {self.r.sql(F)}"
