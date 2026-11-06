"""Corpus (c): queries of the subset made with the sqlfuzz generator (spikes/sqlfuzz: gen.expr / gen.boolean over its table scope, with its data pools),
rejection-sampled to the subset, and rendered TWICE from the same tree: as sqlfuzz renders it (every operator parenthesised) and with the minimal
parentheses SQL precedence allows (this form exercises operator precedence). Seeded and replayable: query i of seed S comes from
random.Random(S * 1_000_003 + i); `--only I` regenerates one.
usage: gen_fuzz.py --seed S --trees N --out FILE     (N trees give 2*N queries)  -> {"id", "q", "tree", "form"} per line"""
import argparse, json, os, random, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sqlfuzz"))
from sqlfuzz import gen
from sqlfuzz.render import Col, Lit, Not, Neg, Bin, Case, Cast, IsNull, Between, InList

SUBSET_NODES = (Col, Lit, Not, Neg, Bin, Case, Cast, IsNull, Between, InList)
OPS = {"+", "-", "*", "/", "%", "=", "<>", "<", "<=", ">", ">=", "AND", "OR"}
TYPES = {"INTEGER", "BIGINT", "DOUBLE", "DECIMAL(10,2)", "VARCHAR", "DATE", "BOOLEAN"}


class Scope(gen.Scope):
    """The sqlfuzz scope, but with unqualified column names (the subset has no `t.col`)."""
    def cols(self, typ=None):
        return [Col(t, n) for n, t in gen.COLS if typ is None or t == typ]


def in_subset(x):
    if not isinstance(x, SUBSET_NODES):
        return False
    if isinstance(x, Lit):
        return not any(s in x.text for s in ("::", "INTERVAL", "TIMESTAMP", "true", "false", "Infinity", "NaN"))
    if isinstance(x, Bin):
        return x.op in OPS and in_subset(x.l) and in_subset(x.r)
    if isinstance(x, (Not, Neg)):
        return in_subset(x.x)
    if isinstance(x, Case):
        return all(in_subset(c) and in_subset(v) for c, v in x.whens) and in_subset(x.els)
    if isinstance(x, Cast):
        return x.t in TYPES and in_subset(x.x)
    if isinstance(x, IsNull):
        return in_subset(x.x)
    if isinstance(x, Between):
        return in_subset(x.x) and in_subset(x.lo) and in_subset(x.hi)
    if isinstance(x, InList):
        return in_subset(x.x) and all(in_subset(i) for i in x.items)
    return True


# ---- minimal-parentheses rendering. Levels: 1 OR, 2 AND, 3 NOT, 4 IS NULL, 5 comparison, 6 BETWEEN/IN, 7 + -, 8 * / %, 9 unary minus, 10 atoms.
def lvl(x):
    if isinstance(x, Bin):
        return {"OR": 1, "AND": 2, "+": 7, "-": 7, "*": 8, "/": 8, "%": 8}.get(x.op, 5)
    return {Not: 3, IsNull: 4, Between: 6, InList: 6, Neg: 9}.get(type(x), 9 if isinstance(x, Lit) and x.text.startswith("-") else 10)


def mini(x, need=1):
    s = _mini(x)
    return f"({s})" if lvl(x) < need else s


def _mini(x):
    if isinstance(x, Col):
        return x.name
    if isinstance(x, Lit):
        return x.text
    if isinstance(x, Not):
        return "NOT " + mini(x.x, 3)
    if isinstance(x, Neg):
        return "- " + mini(x.x, 9)
    if isinstance(x, Bin):
        L = lvl(x)
        if L == 5:                      # comparisons do not chain
            return f"{mini(x.l, 6)} {x.op} {mini(x.r, 6)}"
        return f"{mini(x.l, L)} {x.op} {mini(x.r, L + 1)}"
    if isinstance(x, IsNull):
        return f"{mini(x.x, 4)} IS {'NOT ' if x.neg else ''}NULL"
    if isinstance(x, Between):
        return f"{mini(x.x, 7)} BETWEEN {mini(x.lo, 7)} AND {mini(x.hi, 7)}"
    if isinstance(x, InList):
        return f"{mini(x.x, 7)} {'NOT ' if x.neg else ''}IN ({', '.join(mini(i) for i in x.items)})"
    if isinstance(x, Case):
        w = " ".join(f"WHEN {mini(c)} THEN {mini(v)}" for c, v in x.whens)
        return f"CASE {w} ELSE {mini(x.els)} END"
    if isinstance(x, Cast):
        return f"CAST({mini(x.x)} AS {x.t})"
    raise TypeError(type(x))


PATHS = ["'lineitem.parquet'", "'data/*.parquet'", "'/tmp/x y/part-0.parquet'", "'s3://b/k/**/*.parquet'", "'it''s.parquet'", "'f.parquet'"]


def tree(i, seed):
    """Returns (items, where, limit, offset, explain, path, style) for tree i; style = (keyword case, separators)."""
    rng = random.Random(seed * 1_000_003 + i)
    sc = Scope([""])
    while True:
        depth = rng.randint(1, 4)
        n = rng.randint(1, 3)
        items = []
        for k in range(n):
            if rng.random() < 0.1:
                items.append(("*", None))
                continue
            typ = rng.choice([t for _, t in gen.COLS if t not in ("TIMESTAMP", "TIMESTAMPTZ")])
            items.append((gen.expr(rng, sc, typ, depth), rng.choice([f"c{k}", f"\"C {k}\"", None])))
        where = gen.boolean(rng, sc, depth) if rng.random() < 0.8 else None
        if all(e == "*" or in_subset(e) for e, _ in items) and (where is None or in_subset(where)):
            break
    limit = rng.choice([None, None, rng.randint(0, 10), 18446744073709551615])
    offset = rng.randint(0, 5) if limit is not None and rng.random() < 0.4 else None
    explain = rng.choice(["", "", "", "EXPLAIN ", "EXPLAIN ANALYZE "])
    return items, where, limit, offset, explain, rng.choice(PATHS), (rng.choice(["upper", "upper", "upper", "lower", "title"]), rng.choice([" ", " ", " ", "\n", "\t", "  "]))


def render(t, form):
    items, where, limit, offset, explain, path, (case, sep) = t
    kw = {"upper": str.upper, "lower": str.lower, "title": str.title}[case]
    r = (lambda e: e.sql(frozenset())) if form == "full" else mini
    sel = ", ".join("*" if e == "*" else r(e) + (f" {kw('as')} {a}" if a else "") for e, a in items)
    q = f"{kw(explain)}{kw('select')}{sep}{sel}{sep}{kw('from')} {path}"
    if where is not None:
        q += f"{sep}{kw('where')} {r(where)}"
    if limit is not None:
        q += f"{sep}{kw('limit')} {limit}" + (f" {kw('offset')} {offset}" if offset is not None else "")
    return q


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, default=20261106)
    ap.add_argument("--trees", type=int, default=5000)
    ap.add_argument("--out", default="fuzz.jsonl")
    ap.add_argument("--only", type=int)
    a = ap.parse_args()
    rng_range = [a.only] if a.only is not None else range(a.trees)
    n = 0
    with open(a.out, "w") as f:
        for i in rng_range:
            t = tree(i, a.seed)
            for form in ("full", "mini"):
                f.write(json.dumps({"id": 2 * i + (form == "mini"), "tree": i, "form": form, "q": render(t, form)}, ensure_ascii=False) + "\n")
                n += 1
    print(f"wrote {n} queries to {a.out} (seed {a.seed})")
