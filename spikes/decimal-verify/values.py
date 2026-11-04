"""Value-level cases: generation, DuckDB evaluation (cached), comparison with the reference."""
import math
import os
import pickle
import random
import re
from decimal import Context, Decimal, ROUND_HALF_UP
from fractions import Fraction

from duck import run_jobs
from rules_ref import MAXP, PROFILE, BindErr, Ref, RunErr, fmt, norm, round_div
from typecheck import DBL, duck_mod_model

FULL = PROFILE == "full"
SMALL_TYPES = [(1, 0), (1, 1), (2, 0), (2, 1), (2, 2)] if FULL else [(1, 0), (1, 1)]
BIG_TYPES = [
    (1, 0), (1, 1), (2, 1), (3, 2), (5, 0), (9, 4), (10, 0), (10, 2), (10, 9), (15, 2), (17, 8), (18, 0),
    (18, 2), (18, 18), (19, 0), (19, 9), (20, 10), (27, 13), (30, 2), (37, 36), (38, 0), (38, 2), (38, 19),
    (38, 38),
] if FULL else [(1, 0), (2, 1), (10, 2), (18, 0), (19, 9), (38, 0), (38, 19), (38, 38)]
MAX_VALUES = 44 if FULL else 14


def small_values(p, s):
    M = 10 ** p - 1
    return list(range(-M, M + 1))


def boundary_values(p, s, seed=0):
    """Boundary-biased unscaled values: 0, +-1, +-max, +-max/2, powers of ten and 10^k-1, rounding-tie digits,
    the i64 edge and its square-root edge. Reproducible subset of at most MAX_VALUES (core always kept)."""
    M = 10 ** p - 1
    core = {0, 1, M, M // 2, M // 2 + 1, 5, 10 ** s if s < p else 1, 10 ** s - 1}
    if s >= 1:
        core.add(5 * 10 ** (s - 1))
    more = set()
    ks = set(range(1, p + 1)) if p <= 6 else {
        1, 2, 3, 4, p // 4, p // 3, p // 2 - 1, p // 2, p // 2 + 1, 9, 10, 17, 18, 19, p - 3, p - 2, p - 1, p}
    for k in ks:
        if 1 <= k <= p:
            more.add(10 ** k - 1)
            if k < p:
                more.add(10 ** k)
            more.add(5 * 10 ** (k - 1))
    for v in (15, 25, 45, 95, 105, 2 ** 63 - 1, 2 ** 63, 2 ** 63 + 1, 2 ** 64 - 1, 3037000499, 3037000500, 10 ** 9 + 1):
        more.add(v)
    core = {v for v in core if 0 <= v <= M}
    more = sorted(v for v in more if 0 <= v <= M and v not in core)
    rng = random.Random(f"{p}-{s}-{seed}")
    room = max(0, MAX_VALUES // 2 - len(core))
    pick = set(core) | set(rng.sample(more, min(room, len(more))))
    return sorted({sgn * v for v in pick for sgn in (1, -1)})


def sql_tables(types_vals):
    """CREATE TEMP TABLE per (type) with columns i, v."""
    out = []
    for k, g in enumerate(types_vals):
        t, vals = g[0], g[1]
        if len(g) == 3:  # an integer group: (name, values, SQL type name); unscaled value = the value
            rows = ",".join(f"({i},'{v}')" for i, v in enumerate(vals))
            out.append(f"CREATE TEMP TABLE t{k} AS SELECT i::INTEGER AS i, CAST(x AS {g[2]}) AS v FROM (VALUES {rows}) q(i,x);\n")
            continue
        p, s = t
        rows = ",".join(f"({i},'{fmt(v, s)}')" for i, v in enumerate(vals))
        out.append(f"CREATE TEMP TABLE t{k} AS SELECT i::INTEGER AS i, CAST(x AS DECIMAL({p},{s})) AS v FROM (VALUES {rows}) q(i,x);\n")
    return "".join(out)


def build_groups(types, valfn):
    return [(t, valfn(*t)) for t in types]


def op_job_sql(op, ia, ib):
    e = f"a.v {op} b.v"
    val = f"CAST({e} AS VARCHAR)" if op == "/" else f"coalesce(CAST(TRY({e}) AS VARCHAR),'NULL')"
    return f"SELECT replace(typeof({e}),',','.') || '|' || {val} FROM t{ia} a, t{ib} b ORDER BY a.i, b.i"


def is_int_group(g):
    return len(g) == 3


def duck_values(op, name, groups, cache_dir, mixed=False):
    """{(ia,ib): [lines]} for every ordered group pair whose base-reference type is OK for op (cached).
    mixed=True: groups hold integer groups (3-tuples) and decimal groups; only integer-with-decimal pairs are used."""
    path = os.path.join(cache_dir, f"vals_{name}_{op.replace('/', 'div').replace('%', 'mod').replace('*', 'mul')}.pkl")
    if os.path.exists(path):
        with open(path, "rb") as f:
            return pickle.load(f)
    base = Ref()
    jobs = []
    for ia, ga in enumerate(groups):
        for ib, gb in enumerate(groups):
            if mixed and is_int_group(ga) == is_int_group(gb):
                continue
            if op != "%":  # for % Nata's bind error is a documented difference: DuckDB is still queried (DOUBLE)
                try:
                    base.result_type(op, base.as_dec(ga[0]), base.as_dec(gb[0]))
                except BindErr:
                    continue
            jobs.append((f"{ia}_{ib}", op_job_sql(op, ia, ib)))
    res = run_jobs(jobs, prelude=sql_tables(groups))
    out = {}
    for jid, (data, errs) in res.items():
        assert not errs, (op, name, jid, errs[:2])
        ia, ib = map(int, jid.split("_"))
        out[(ia, ib)] = data
    os.makedirs(cache_dir, exist_ok=True)
    with open(path, "wb") as f:
        pickle.dump(out, f)
    return out


DEC_CTX = Context(prec=300, rounding=ROUND_HALF_UP, Emax=999, Emin=-999, traps=[])


def decimal_module(op, a, t1, b, t2, S):
    """Second implementation with Python's decimal module (ROUND_HALF_UP = ties away from zero).
    Returns unscaled integer at scale S, or None for division by zero."""
    A = Decimal(a).scaleb(-t1[1], DEC_CTX)
    B = Decimal(b).scaleb(-t2[1], DEC_CTX)
    c = DEC_CTX
    if op == "+":
        r = c.add(A, B)
    elif op == "-":
        r = c.subtract(A, B)
    elif op == "*":
        r = c.multiply(A, B)
    elif op == "/":
        if b == 0:
            return None
        r = c.divide(A, B)
    else:
        if b == 0:
            return None
        r = c.remainder(A, B)
    q = r.quantize(Decimal(1).scaleb(-S, DEC_CTX), rounding=ROUND_HALF_UP, context=DEC_CTX)
    return int(q.scaleb(S, DEC_CTX))


def safe_norm(text, S):
    """norm() that returns the raw text when DuckDB's scale differs from S (so the comparison fails, not asserts)."""
    try:
        return norm(text, S)
    except AssertionError:
        return "SCALE?" + text


def exact_quotient(a, t1, b, t2):
    return Fraction(a * 10 ** t2[1], b * 10 ** t1[1])


def check_op_values(ref, op, name, groups, duck, stats):
    """Compare the reference with DuckDB for one operator over all cached group pairs.
    stats is a dict of counters that are updated; returns a list of up to 8 mismatch samples."""
    bad = []
    st = stats

    def mism(kind, sample):
        st[f"MISMATCH:{kind}"] = st.get(f"MISMATCH:{kind}", 0) + 1
        if len(bad) < 8:
            bad.append((kind,) + sample)

    for (ia, ib), lines in duck.items():
        ta0, va = groups[ia][:2]
        tb0, vb = groups[ib][:2]
        ta, tb = ref.as_dec(ta0), ref.as_dec(tb0)  # integers are read as exact decimals (section 2)
        try:
            P, S = ref.result_type(op, ta, tb)
            ref_ok = True
        except BindErr:
            ref_ok = False
        if not ref_ok:
            over = op == "%" and max(ta[0] - ta[1], tb[0] - tb[1]) + max(ta[1], tb[1]) > MAXP  # independent of the reference
            if not over:
                mism("ref_bind_error_but_duckdb_returns", (ta, tb))
            elif any(not line.startswith("DOUBLE|") for line in lines):
                mism("mod_bind_error_but_duckdb_not_double", (ta, tb))
            else:
                key = "mod: Nata bind error (alignment needs > 38 digits), DuckDB DOUBLE: documented difference (cases)"
                st[key] = st.get(key, 0) + len(va) * len(vb)
            continue
        if op == "%" and max(ta[0] - ta[1], tb[0] - tb[1]) + max(ta[1], tb[1]) > MAXP:
            mism("mod_binds_but_alignment_over_38", (ta, tb))
            continue
        exp_type = f"DECIMAL({P}.{S})"
        lim = None
        k = 0
        for a in va:
            for b in vb:
                line = lines[k]
                k += 1
                dtype, _, dtext = line.partition("|")
                st["cases"] = st.get("cases", 0) + 1
                try:
                    u = ref.v_op(op, a, ta, b, tb)
                    err = None
                except RunErr as e:
                    u, err = None, e.kind
                # decision 1, written independently of the reference: for + and - an operand that does not fit the
                # result type once rescaled to S is an overflow even if the exact sum fits
                fit_fail = op in "+-" and (abs(a) * 10 ** (S - ta[1]) >= 10 ** P or abs(b) * 10 ** (S - tb[1]) >= 10 ** P)
                # ---- second implementation (decimal module) vs reference
                dm = decimal_module(op, a, ta, b, tb, S)
                if err == "div0":
                    if dm is not None:
                        mism("decimal_module_disagrees_on_div0", (ta, tb, a, b))
                elif dm is None:
                    mism("decimal_module_says_div0", (ta, tb, a, b))
                elif err is None:
                    if dm != u or abs(dm) >= 10 ** P:
                        mism("decimal_module_vs_ref", (ta, tb, a, b, u, dm))
                    else:
                        st["decimal_module_agree"] = st.get("decimal_module_agree", 0) + 1
                elif abs(dm) < 10 ** P and not fit_fail:
                    mism("ref_overflow_but_value_fits", (ta, tb, a, b, dm))
                # ---- DuckDB
                if op == "/":
                    if dtype != "DOUBLE":
                        mism("duckdb_div_not_double", (ta, tb, dtype))
                        continue
                    if b == 0:
                        if err != "div0":
                            mism("div0_not_an_error_in_ref", (ta, tb, a))
                        elif dtext in ("inf", "-inf", "nan", "-nan"):
                            st["div0: ref error / duckdb inf-or-nan (documented difference)"] = st.get("div0: ref error / duckdb inf-or-nan (documented difference)", 0) + 1
                        else:
                            mism("duckdb_div0_unexpected", (ta, tb, a, dtext))
                        continue
                    if err is not None:
                        mism("ref_error_on_nonzero_divisor", (ta, tb, a, b, err))
                        continue
                    q = exact_quotient(a, ta, b, tb)
                    refv = Fraction(u, 10 ** S)
                    half = Fraction(1, 2 * 10 ** S)
                    # exact property of "nearest, ties away from zero" at scale S
                    diff = abs(refv - q)
                    if diff > half or (diff == half and abs(refv) < abs(q)):
                        mism("div_not_nearest_or_tie_not_away", (ta, tb, a, b, u))
                        continue
                    dd = float(dtext)
                    tol = float(half) + 1e-15 * abs(float(q)) + 1e-300
                    if abs(dd - float(refv)) > tol:
                        mism("div_vs_duckdb_double_outside_tolerance", (ta, tb, a, b, u, dtext))
                        continue
                    st["div ok (exact property + within DuckDB DOUBLE tolerance)"] = st.get("div ok (exact property + within DuckDB DOUBLE tolerance)", 0) + 1
                    continue
                # + - * %
                if op == "%":
                    # decision 3: Nata's type is tighter than DuckDB's (documented difference); DuckDB's own type is
                    # the wider operand type or DOUBLE, and its value must still equal Nata's remainder
                    model = duck_mod_model(ta, tb)
                    exp_d = "DOUBLE" if model == DBL else f"DECIMAL({model[0]}.{model[1]})"
                    if dtype != exp_d:
                        mism("duckdb_mod_type_not_the_model", (ta, tb, exp_d, dtype))
                        continue
                    if S != max(ta[1], tb[1]) or P > model[0]:
                        mism("mod_type_not_tighter_or_equal_with_same_scale", (ta, tb, (P, S), model))
                        continue
                    if b == 0:
                        if err != "div0":
                            mism("mod_zero_not_error", (ta, tb, a))
                        else:
                            st["mod by zero: both error"] = st.get("mod by zero: both error", 0) + 1
                            if dtext != "NULL":
                                mism("duckdb_mod_zero_no_error", (ta, tb, a, dtext))
                        continue
                    if err is not None:
                        mism("mod_cannot_overflow_but_ref_errors", (ta, tb, a, b, err))
                        continue
                    if safe_norm(dtext, S) != fmt(u, S):
                        mism("value", (ta, tb, op, a, b, "ref=" + fmt(u, S), "duckdb=" + dtext))
                    else:
                        st["value equal"] = st.get("value equal", 0) + 1
                        if (P, S) != model:
                            st["  of which Nata type tighter than DuckDB's (documented)"] = st.get(
                                "  of which Nata type tighter than DuckDB's (documented)", 0) + 1
                    continue
                if dtype != exp_type:
                    mism("type", (ta, tb, op, exp_type, dtype))
                    continue
                exp = "NULL" if err else fmt(u, S)
                if safe_norm(dtext, S) != exp:
                    mism("value", (ta, tb, op, a, b, "ref=" + exp, "duckdb=" + dtext))
                else:
                    if err and fit_fail:
                        key = "F1 resolved: operand does not fit, exact sum fits, both error"
                    else:
                        key = "overflow: both error" if err else "value equal"
                    st[key] = st.get(key, 0) + 1
    return bad


def ref_only_division_checks(ref, groups, stats):
    """Division by zero must be an error in the reference for every pair that types (documented expectation)."""
    bad = 0
    for ta, va in groups:
        for tb, vb in groups:
            try:
                ref.result_type("/", ta, tb)
            except BindErr:
                continue
            for a in va[:6]:
                stats["div0 cases"] = stats.get("div0 cases", 0) + 1
                try:
                    ref.v_div(a, ta, 0, tb)
                    bad += 1
                except RunErr as e:
                    if e.kind != "div0":
                        bad += 1
    return bad
