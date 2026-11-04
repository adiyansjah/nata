"""Decision 4: integer with integer arithmetic stays integer (docs/decimal-rules.md sections 2 and 4).

Checks against DuckDB 2.0.0, with the document as the specification:
  * result types of `+ - * % //` over all 16 ordered pairs of TINYINT/SMALLINT/INTEGER/BIGINT: the wider type;
    `/` is a decimal (DuckDB: DOUBLE, a documented difference);
  * values at boundary values (and exhaustive for TINYINT x TINYINT in the full profile): result, or the error;
    error CATEGORY (overflow, division by zero) is compared, never the message text;
  * a second implementation: plain Python integers with an explicit range test (`oracle_int`);
  * integer with a decimal: the integer is read as an exact decimal (TINYINT (3,0), SMALLINT (5,0), INTEGER (10,0),
    BIGINT (19,0)): type matrix over all decimal types and values.
"""
import math
import random
import re
from fractions import Fraction

from duck import csv_row, run_jobs
from extras import _cache
from rules_ref import INT_BITS, INT_DEC, INT_NAME, INT_TYPES, MAXP, PROFILE, TYPES, BindErr, Ref, RunErr, fmt, norm
import typecheck as tc
import values as vl

FULL = PROFILE == "full"
OPS = ["+", "-", "*", "%", "//", "/"]
PROBES_PER_CLASS = 6 if FULL else 2
SQLOP = {"+": "+", "-": "-", "*": "*", "%": "%", "//": "//", "/": "/"}


def lo_hi(name):
    w = INT_BITS[name]
    return -(1 << (w - 1)), (1 << (w - 1)) - 1


def int_values(name, dense=True):
    """Boundary-biased values of an integer type: the limits and their neighbours, 0, +-1, +-2, powers of two and
    their neighbours, the square-root edge of the limit (for multiplication), powers of ten, half the limit.
    TINYINT: every value in the full profile; SMALLINT: also a stride-97 sample in the full profile."""
    lo, hi = lo_hi(name)
    w = INT_BITS[name]
    if FULL and dense and w == 8:
        return list(range(lo, hi + 1))
    s = {lo, lo + 1, lo + 2, hi, hi - 1, hi - 2, 0, 1, 2, 3, -1, -2, -3, hi // 2, hi // 2 + 1, -(hi // 2), -(hi // 2) - 1}
    r = math.isqrt(hi)
    for d in (-1, 0, 1, 2):
        s.add(r + d)
        s.add(-(r + d))
    for k in range(1, w):
        for d in (-1, 0, 1):
            s.add((1 << k) + d)
            s.add(-(1 << k) + d)
    k = 1
    while 10 ** k <= hi:
        for d in (-1, 0, 1):
            s.add(10 ** k + d)
            s.add(-(10 ** k) + d)
        k += 1
    s = {v for v in s if lo <= v <= hi}
    if FULL and dense and w == 16:
        s |= set(range(lo, hi + 1, 97))
    out = sorted(s)
    if not FULL and len(out) > 40:  # quick profile: keep the extremes and a deterministic sample
        keep = {lo, lo + 1, hi, hi - 1, 0, 1, -1, 2, -2, r, r + 1, -r, -(r + 1), hi // 2 + 1, -(hi // 2) - 1}
        rest = [v for v in out if v not in keep]
        random.Random(f"{name}").shuffle(rest)
        out = sorted(keep | set(rest[: 40 - len(keep)]))
    return out


# ------------------------------------------------------------------------------------ independent oracle
def oracle_int(op, a, b, wa, wb):
    """Second implementation for integer op integer, from the document alone, with plain Python integers.
    Returns an int, 'overflow' or 'div0'. The result type is the wider integer type; the result must fit it."""
    w = max(wa, wb)
    lo, hi = -(1 << (w - 1)), (1 << (w - 1)) - 1
    if op in ("%", "//") and b == 0:
        return "div0"
    if op == "+":
        r = a + b
    elif op == "-":
        r = a - b
    elif op == "*":
        r = a * b
    elif op == "//":
        r = int(Fraction(a, b))  # Fraction -> int truncates toward zero
    else:
        if b == -1 and a == lo:
            return "overflow"  # decision N2: MIN % -1 is an overflow error (the checked remainder overflows), like MIN // -1
        r = a - b * int(Fraction(a, b))  # remainder, sign of the dividend
    return r if lo <= r <= hi else "overflow"


# ------------------------------------------------------------------------------------ DuckDB queries
def duck_int_types(cd):
    """{(n1, n2, op): DuckDB type name} for the 16 ordered pairs and 6 operators."""

    def run():
        keys = [(n1, n2, op) for n1 in INT_TYPES for n2 in INT_TYPES for op in OPS]
        sql = "SELECT " + ", ".join(f"typeof(NULL::{n1} {SQLOP[op]} NULL::{n2})" for n1, n2, op in keys)
        data, errs = run_jobs([("t", sql)])["t"]
        assert not errs, errs[:2]
        return dict(zip(keys, csv_row(data[0])))

    return _cache(cd, "int_types", run)


def int_groups():
    return [(n, int_values(n), n) for n in INT_TYPES]


def duck_int_values(op, groups, cd):
    """{(ia, ib): [text]}: per-row TRY(a op b) as text ('NULL' for an error), '/' as DOUBLE text."""

    def run():
        jobs = []
        for ia in range(len(groups)):
            for ib in range(len(groups)):
                e = f"a.v {SQLOP[op]} b.v"
                val = f"CAST({e} AS VARCHAR)" if op == "/" else f"coalesce(CAST(TRY({e}) AS VARCHAR),'NULL')"
                jobs.append((f"{ia}_{ib}", f"SELECT {val} FROM t{ia} a, t{ib} b ORDER BY a.i, b.i"))
        res = run_jobs(jobs, prelude=vl.sql_tables(groups))
        out = {}
        for jid, (data, errs) in res.items():
            assert not errs, (op, jid, errs[:2])
            ia, ib = map(int, jid.split("_"))
            out[(ia, ib)] = data
        return out

    return _cache(cd, f"int_vals_{profile_tag(op)}", run)


def profile_tag(op):
    return {"+": "add", "-": "sub", "*": "mul", "%": "mod", "//": "idiv", "/": "div"}[op]


ERR_CATEGORY = [(re.compile(r"Overflow|out of range|Out of Range", re.I), "overflow"),
                (re.compile(r"Division by zero|Modulo by zero", re.I), "div0")]


def categorize(lines):
    text = " ".join(lines)
    for rx, cat in ERR_CATEGORY:
        if rx.search(text):
            return cat
    return "other:" + text[:60]


def probe_cases(ref0, groups, k):
    """Up to k error cases per (type pair, operator, category) as chosen by the BASE reference, plus the exact limits,
    to be run without TRY so the DuckDB error category can be read."""
    cases = []
    for ia, (n1, v1, _) in enumerate(groups):
        for ib, (n2, v2, _) in enumerate(groups):
            for op in ("+", "-", "*", "%", "//"):
                seen = {"overflow": [], "div0": []}
                rng = random.Random(f"{n1}{n2}{op}")
                pairs = [(a, b) for a in v1 for b in v2]
                rng.shuffle(pairs)
                # always include the limits themselves
                lo1, hi1 = lo_hi(n1)
                lo2, hi2 = lo_hi(n2)
                front = [(hi1, hi2), (lo1, lo2), (hi1, 1), (lo1, -1), (lo1, 1), (hi1, -1), (1, 0), (lo1, 0)]
                for a, b in front + pairs:
                    if not (lo1 <= a <= hi1 and lo2 <= b <= hi2):
                        continue
                    try:
                        ref0.v_int(op, a, n1, b, n2)
                    except RunErr as e:
                        if e.kind in seen and len(seen[e.kind]) < k and (a, b) not in seen[e.kind]:
                            seen[e.kind].append((a, b))
                for kind, lst in seen.items():
                    for a, b in lst:
                        cases.append((n1, a, op, n2, b, kind))
    return cases


def duck_int_probes(cases, cd):
    def run():
        jobs = [(f"p{i}", f"SELECT CAST('{a}' AS {n1}) {SQLOP[op]} CAST('{b}' AS {n2})") for i, (n1, a, op, n2, b, _) in enumerate(cases)]
        res = run_jobs(jobs)
        return {i: ("ok" if not res[f"p{i}"][1] else categorize(res[f"p{i}"][1])) for i in range(len(cases))}

    return _cache(cd, "int_probes", run)


# ------------------------------------------------------------------------------------ the checks
def run_int_checks(suite):
    ref, cd = suite.ref, suite.cache
    groups = int_groups()
    base = Ref()
    # ---- result types
    want_ops = [op for op in OPS if suite.want("i" + op)]
    if want_ops:
        dt = duck_int_types(cd)
        cnt = {"equal": 0, "doc_diff (/ is DOUBLE in DuckDB; Nata exact decimal)": 0, "disagree": 0}
        for n1 in INT_TYPES:
            for n2 in INT_TYPES:
                for op in want_ops:
                    kind, t = ref.t_int(op, n1, n2)
                    d = dt[(n1, n2, op)]
                    if op == "/":
                        # document: only `/` between integers is a decimal, by the decimal rule on (p,0) and (p,0)
                        exp_dec = base.t_div(*INT_DEC[n1], *INT_DEC[n2])
                        ok = kind == "dec" and t == exp_dec and d == "DOUBLE"
                        cnt["doc_diff (/ is DOUBLE in DuckDB; Nata exact decimal)" if ok else "disagree"] += 1
                    else:
                        wider = INT_NAME[max(INT_BITS[n1], INT_BITS[n2])]  # the document: the wider of the two
                        ok = kind == "int" and t == wider and d == wider
                        cnt["equal" if ok else "disagree"] += 1
                    if not ok:
                        suite.fail(f"int_types[{op}]", 1, (n1, n2, op, (kind, t), d))
        suite.counts["integer result types (16 pairs x operators)"] = cnt
    # ---- values
    for op in want_ops:
        duck = duck_int_values(op, groups, cd)
        st = {}
        for (ia, ib), lines in duck.items():
            n1, v1, _ = groups[ia]
            n2, v2, _ = groups[ib]
            wa, wb = INT_BITS[n1], INT_BITS[n2]
            k = 0
            for a in v1:
                for b in v2:
                    text = lines[k]
                    k += 1
                    st["cases"] = st.get("cases", 0) + 1
                    try:
                        u = ref.v_int(op, a, n1, b, n2)
                        err = None
                    except RunErr as e:
                        u, err = None, e.kind
                    if op == "/":
                        _check_int_div(ref, suite, st, n1, a, n2, b, u, err, text)
                        continue
                    # second implementation (plain Python integers)
                    orc = oracle_int(op, a, b, wa, wb)
                    exp_ref = err if err else u
                    if orc != exp_ref:
                        st["MISMATCH:oracle_vs_ref"] = st.get("MISMATCH:oracle_vs_ref", 0) + 1
                        suite.fail(f"int[{op}] oracle_vs_ref", 1, (n1, a, op, n2, b, "oracle", orc, "ref", exp_ref))
                        # no `continue`: DuckDB is compared as well, so the mutation table shows who sees what
                    # DuckDB
                    if err is None:
                        if text == str(u):
                            st["value equal"] = st.get("value equal", 0) + 1
                        else:
                            st["MISMATCH:value"] = st.get("MISMATCH:value", 0) + 1
                            suite.fail(f"int[{op}] value", 1, (n1, a, op, n2, b, "ref", u, "duckdb", text))
                    else:
                        cat = "div0" if (b == 0 and op in ("%", "//")) else "overflow"  # TRY gives NULL for both
                        if text == "NULL" and cat == err:
                            st[f"error ({err}): both"] = st.get(f"error ({err}): both", 0) + 1
                        else:
                            st["MISMATCH:error"] = st.get("MISMATCH:error", 0) + 1
                            suite.fail(f"int[{op}] error", 1, (n1, a, op, n2, b, "ref", err, "duckdb", text))
        suite.counts[f"integer values {op}"] = st
    # ---- error categories read from DuckDB's own error messages (no TRY)
    err_ops = [op for op in ("+", "-", "*", "%", "//") if suite.want("i" + op)]
    if err_ops:
        allc = probe_cases(base, groups, PROBES_PER_CLASS)
        got = duck_int_probes(allc, cd)
        st = {"probes": 0, "category equal": 0}
        for i, (n1, a, op, n2, b, kind) in enumerate(allc):
            if op not in err_ops:
                continue
            st["probes"] += 1
            try:
                ref.v_int(op, a, n1, b, n2)
                rk = "ok"
            except RunErr as e:
                rk = e.kind
            if rk == got[i]:
                st["category equal"] += 1
            else:
                suite.fail("int_error_category", 1, (n1, a, op, n2, b, "ref", rk, "duckdb", got[i]))
        suite.counts["integer error categories (DuckDB message, not TRY)"] = st


def _check_int_div(ref, suite, st, n1, a, n2, b, u, err, text):
    """`/` between integers: an exact decimal (decimal rule on (p,0) and (p,0)); DuckDB returns DOUBLE."""
    ta, tb = INT_DEC[n1], INT_DEC[n2]
    P, S = Ref().t_div(*ta, *tb)  # the document's type, computed with the unmutated reference
    if b == 0:
        if err != "div0":
            suite.fail("int[/] div0_not_an_error", 1, (n1, a, n2))
        elif text in ("inf", "-inf", "nan", "-nan"):
            st["div0: ref error / duckdb inf-or-nan (documented difference)"] = st.get("div0: ref error / duckdb inf-or-nan (documented difference)", 0) + 1
        else:
            suite.fail("int[/] duckdb_div0_unexpected", 1, (n1, a, text))
        return
    if err is not None or u is None:
        suite.fail("int[/] ref_error_on_nonzero_divisor", 1, (n1, a, n2, b, err))
        return
    q = Fraction(a, b)
    refv = Fraction(u, 10 ** S)
    half = Fraction(1, 2 * 10 ** S)
    diff = abs(refv - q)
    if diff > half or (diff == half and abs(refv) < abs(q)):
        suite.fail("int[/] not_nearest_or_tie_not_away", 1, (n1, a, n2, b, u))
        return
    tol = float(half) + 1e-15 * abs(float(q)) + 1e-300
    if abs(float(text) - float(refv)) > tol:
        suite.fail("int[/] vs_duckdb_double_outside_tolerance", 1, (n1, a, n2, b, u, text))
        return
    st["div ok (exact property + within DuckDB DOUBLE tolerance)"] = st.get("div ok (exact property + within DuckDB DOUBLE tolerance)", 0) + 1


# ------------------------------------------------------------------------------------ integer with decimal
MIXED_DEC_TYPES = [(1, 0), (1, 1), (2, 1), (5, 2), (10, 2), (18, 0), (19, 0), (19, 9), (20, 10), (38, 0), (38, 2), (38, 38)] if FULL else [
    (1, 1), (5, 2), (19, 0), (38, 0), (38, 38)]


def mixed_int_values(name):
    s = set(int_values(name, dense=False))
    if not FULL:
        lo, hi = lo_hi(name)
        s = {v for v in s if v in (lo, lo + 1, hi, hi - 1, 0, 1, -1, 2, -2, 10, -10, 100, 127, -128) or abs(v) <= 3}
    return sorted(s)


def duck_mixed_types(cd):
    """{(iname, p, s, order, op): DuckDB type} for every integer type, every decimal type, both orders."""

    def run():
        keys, exprs = [], []
        for n in INT_TYPES:
            for (p, s) in TYPES:
                for order in ("id", "di"):
                    for op in ("+", "-", "*", "%", "/"):
                        a, b = (f"NULL::{n}", f"NULL::DECIMAL({p},{s})") if order == "id" else (f"NULL::DECIMAL({p},{s})", f"NULL::{n}")
                        keys.append((n, p, s, order, op))
                        exprs.append(f"typeof({a} {op} {b})")
        jobs = [(f"m{c}", "SELECT " + ", ".join(exprs[c:c + 3000])) for c in range(0, len(exprs), 3000)]
        res = run_jobs(jobs)
        out = {}
        for c in range(0, len(exprs), 3000):
            data, errs = res[f"m{c}"]
            assert not errs, errs[:2]
            for k, ty in zip(keys[c:c + 3000], csv_row(data[0])):
                out[k] = ty
        return out

    return _cache(cd, "mixed_types", run)


def prepare(cd):
    """Run every DuckDB query of this module once (cached), so that timing and mutation runs need none."""
    groups = int_groups()
    duck_int_types(cd)
    for op in OPS:
        duck_int_values(op, groups, cd)
    duck_int_probes(probe_cases(Ref(), groups, PROBES_PER_CLASS), cd)
    duck_mixed_types(cd)
    mg = mixed_groups()
    for op in ("+", "-", "*", "%", "/"):
        vl.duck_values(op, "mixed", mg, cd, mixed=True)


def mixed_groups():
    return [(n, mixed_int_values(n), n) for n in INT_TYPES] + [(t, vl.boundary_values(*t)) for t in MIXED_DEC_TYPES]


def run_mixed_checks(suite):
    ref, cd = suite.ref, suite.cache
    ops = [op for op in ("+", "-", "*", "%", "/") if suite.want("idec") or suite.want(op)]
    if not ops:
        return
    # ---- type matrix over every decimal type
    dt = duck_mixed_types(cd)
    cnt = {"equal": 0, "both_error": 0, "doc_diff": 0, "disagree": 0}
    for (n, p, s, order, op), ty in dt.items():
        if op not in ops:
            continue
        di, dd = ref.as_dec(n), (p, s)
        t1, t2 = (di, dd) if order == "id" else (dd, di)
        try:
            r = ref.result_type(op, t1, t2)
        except BindErr:
            r = None
        d = tc.decode(tc.code_of(tc.parse_type(ty)))
        if op == "/":
            ok, kind = d == tc.DBL, "doc_diff"
        elif op == "*":
            ok, kind = (d == tc.ERR if r is None else d == r), ("both_error" if r is None else "equal")
        elif op == "%":
            model = tc.duck_mod_model(t1, t2)
            over = max(t1[0] - t1[1], t2[0] - t2[1]) + max(t1[1], t2[1]) > MAXP  # alignment needs > 38 digits
            if over:
                ok = r is None and d == tc.DBL
            else:
                ok = r is not None and d == model and r[1] == model[1] and r[0] <= model[0]
            kind = "doc_diff" if (ok and (over or r != model)) else "equal"
        else:
            ok, kind = (r is not None and d == r), "equal"
        if ok:
            cnt[kind] += 1
        else:
            cnt["disagree"] += 1
            suite.fail(f"mixed_types[{op}]", 1, (n, (p, s), order, r, ty))
    suite.counts["integer with decimal: result types (4 integer types x decimal types x 2 orders x operators)"] = cnt
    # ---- values (the integer is read as an exact decimal; DuckDB's own cast of an integer to a decimal is the same)
    groups = mixed_groups()
    for op in ops:
        duck = vl.duck_values(op, "mixed", groups, cd, mixed=True)
        st = {}
        bad = vl.check_op_values(ref, op, "mixed", groups, duck, st)
        suite.counts[f"integer with decimal values {op}"] = st
        for key, c in st.items():
            if key.startswith("MISMATCH:"):
                suite.fail(f"mixed_values[{op}] {key[9:]}", c, bad[0] if bad else None)
