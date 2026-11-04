"""Division ties, AVG, ROUND/CAST/FLOOR/..., and the Decimal64 overflow analysis."""
import os
import pickle
import random
from fractions import Fraction

from duck import csv_row, run_jobs
from rules_ref import ALL_TYPES, MAXP, NT, PROFILE, TYPES, BindErr, Ref, RunErr, fmt, norm, round_div
from values import BIG_TYPES, SMALL_TYPES, boundary_values, decimal_module, sql_tables, small_values

FULL = PROFILE == "full"


def _cache(cache_dir, name, fn):
    path = os.path.join(cache_dir, name + ".pkl")
    if os.path.exists(path):
        with open(path, "rb") as f:
            return pickle.load(f)
    v = fn()
    os.makedirs(cache_dir, exist_ok=True)
    with open(path, "wb") as f:
        pickle.dump(v, f)
    return v


# =========================================================================== division ties
def tie_cases():
    """(t1, a, t2, b) where a/b scaled to S=min(s1+6,38) is EXACTLY a half-integer, plus near-miss neighbours.
    Built from exact arithmetic only (independent of Ref's rounding)."""
    base = Ref()
    rng = random.Random(7)
    pairs = []
    for t1 in BIG_TYPES:
        for t2 in BIG_TYPES:
            try:
                base.t_div(*t1, *t2)
                pairs.append((t1, t2))
            except BindErr:
                pass
    rng.shuffle(pairs)
    cases, ties = [], 0
    for t1, t2 in pairs[: 140 if FULL else 14]:
        (p1, s1), (p2, s2) = t1, t2
        S = min(s1 + 6, MAXP)
        k = s2 + S - s1
        As = {1, 3, 5, 7, 9, 11, 13, 15, 99, 101, 125} | set(rng.sample([v for v in boundary_values(p1, s1) if v > 0], min(10, len([v for v in boundary_values(p1, s1) if v > 0]))))
        found = []
        for A in sorted(a for a in As if 0 < a < 10 ** p1):
            for m in (1, 3, 5, 7, 9, 11, 15, 25, 125, 625):
                num = 2 * A * 10 ** k
                if num % m == 0:
                    B = num // m
                    if 1 <= B < 10 ** p2:
                        fr = Fraction(A * 10 ** k, B)
                        if fr.denominator == 2:  # exactly half-way
                            found.append((A, B))
        for A, B in rng.sample(found, min(len(found), 24)):
            ties += 1
            for sa in (1, -1):
                for sb in (1, -1):
                    cases.append((t1, sa * A, t2, sb * B))
            for A2, B2 in ((A + 1, B), (A, B + 1), (A, max(1, B - 1))):  # near-misses on either side
                if A2 < 10 ** p1 and B2 < 10 ** p2:
                    cases.append((t1, A2, t2, B2))
                    cases.append((t1, -A2, t2, B2))
    return cases, ties


def duck_hugeint_division(cases, cache_dir):
    """DuckDB integer arithmetic as a second implementation of round-half-away(n / d):
    n = a * 10^(s2+S-s1), q = sign * ((2|n| + |d|) // (2|d|)) over HUGEINT. Only cases with |n| < 2^120."""
    base = Ref()

    def run():
        jobs, ids = [], []
        rows = []
        for i, (t1, a, t2, b) in enumerate(cases):
            P, S = base.t_div(*t1, *t2)
            n = a * 10 ** (t2[1] + S - t1[1])
            if abs(n) < 2 ** 120:
                rows.append(f"({i},CAST('{n}' AS HUGEINT),CAST('{b}' AS HUGEINT))")
        for c in range(0, len(rows), 2000):
            sql = (
                "SELECT id, CASE WHEN (n<0) = (d<0) THEN 1 ELSE -1 END * ((2*abs(n) + abs(d)) // (2*abs(d))) "
                f"FROM (VALUES {','.join(rows[c:c+2000])}) t(id,n,d)"
            )
            jobs.append((f"h{c}", sql))
        res = run_jobs(jobs)
        out = {}
        for jid, (data, errs) in res.items():
            assert not errs, errs[:2]
            for line in data:
                i, q = line.split(",")
                out[int(i)] = int(q)
        return out

    return _cache(cache_dir, "hugeint_ties", run)


def check_ties(ref, cases, hug, stats):
    bad = []
    for i, (t1, a, t2, b) in enumerate(cases):
        try:
            P, S = ref.result_type("/", t1, t2)
            u = ref.v_div(a, t1, b, t2)
        except RunErr as e:
            bad.append(("ref_error", t1, t2, a, b, e.kind))
            continue
        except BindErr:
            bad.append(("ref_bind_error_on_valid_pair", t1, t2, a, b))
            continue
        # exact rational property: nearest at scale S, ties away from zero
        q = Fraction(a * 10 ** t2[1], b * 10 ** t1[1])
        refv = Fraction(u, 10 ** S)
        half = Fraction(1, 2 * 10 ** S)
        d = abs(refv - q)
        if d > half or (d == half and abs(refv) < abs(q)):
            bad.append(("not_nearest_or_tie_not_away", t1, t2, a, b, u))
        elif d == half:
            stats["tie cases (exact half-way)"] = stats.get("tie cases (exact half-way)", 0) + 1
        dm = decimal_module("/", a, t1, b, t2, S)
        if dm != u:
            bad.append(("decimal_module", t1, t2, a, b, u, dm))
        if i in hug:
            if hug[i] != u:
                bad.append(("duckdb_hugeint", t1, t2, a, b, u, hug[i]))
            else:
                stats["division cases equal to DuckDB HUGEINT arithmetic"] = stats.get("division cases equal to DuckDB HUGEINT arithmetic", 0) + 1
        stats["division tie-set cases"] = stats.get("division tie-set cases", 0) + 1
    return bad


# =========================================================================== AVG
AVG_TYPES = [(15, 2), (18, 2), (10, 8), (38, 2), (38, 0), (5, 5), (1, 0), (3, 3), (20, 5), (9, 4), (18, 0), (2, 1)] if FULL else [(15, 2), (38, 2), (38, 0), (5, 5)]


def avg_groups():
    rng = random.Random(11)
    out = []
    for t in AVG_TYPES:
        bv = boundary_values(*t)
        M = 10 ** t[0] - 1
        for g in range(40 if FULL else 6):
            n = rng.choice([1, 2, 3, 3, 4, 6, 7, 10])
            vals = [rng.choice(bv) if rng.random() < 0.6 else rng.randint(-M, M) for _ in range(n)]
            if abs(sum(vals)) < 10 ** 37:  # DuckDB's SUM(DECIMAL) is (38,s); stay inside it
                out.append((t, vals))
        # tie groups: average exactly half a unit at the result scale
        s = t[1]
        ref = Ref()
        P, S = ref.t_avg(*t)
        # n=2: avg = (x+y)/2 at scale S: ties need (x+y)*10^(S-s) odd/2 -> only if S == s, otherwise exact
        if S == s:
            for x in (1, 2, 3, 5, 7):
                out.append((t, [x, x + 1]))
                out.append((t, [-x, -(x + 1)]))
    return out


def duck_avg(groups, cache_dir):
    def run():
        jobs = []
        for ti, t in enumerate(AVG_TYPES):
            rows = ",".join(f"({gi},'{fmt(v, t[1])}')" for gi, (gt, vals) in enumerate(groups) if gt == t for v in vals)
            sql = (
                f"SELECT g, replace(typeof(avg(v)),',','.'), replace(typeof(sum(v)),',','.'), CAST(avg(v) AS VARCHAR), "
                f"CAST(sum(v) AS VARCHAR) FROM (SELECT g::INTEGER AS g, CAST(x AS DECIMAL({t[0]},{t[1]})) AS v "
                f"FROM (VALUES {rows}) q(g,x)) GROUP BY g"
            )
            jobs.append((f"a{ti}", sql))
        res = run_jobs(jobs)
        out = {}
        for jid, (data, errs) in res.items():
            assert not errs, errs[:2]
            for line in data:
                r = csv_row(line)
                out[int(r[0])] = tuple(r[1:])
        # second DuckDB pass: round-half-away(sum * 10^(S-s) / n) in HUGEINT, from DuckDB's own exact SUM
        base = Ref()
        rows = []
        for gi, (t, vals) in enumerate(groups):
            P, S = base.t_avg(*t)
            su = int(out[gi][3].replace(".", "").replace("-", "") or "0") * (-1 if out[gi][3].startswith("-") else 1)
            rows.append(f"({gi},CAST('{su * 10 ** (S - t[1])}' AS HUGEINT),{len(vals)})")
        sql = ("SELECT id, CASE WHEN num < 0 THEN -1 ELSE 1 END * ((2*abs(num) + n) // (2*n)) FROM (VALUES "
               + ",".join(rows) + ") t(id,num,n)")
        res = run_jobs([("h", sql)])
        assert not res["h"][1], res["h"][1][:2]
        hug = {}
        for line in res["h"][0]:
            i, q = line.split(",")
            hug[int(i)] = int(q)
        return out, hug

    return _cache(cache_dir, "avg2", run)


def check_avg(ref, groups, duck_hug, stats):
    duck, hug = duck_hug
    bad = []
    for gi, (t, vals) in enumerate(groups):
        P, S = ref.t_avg(*t)
        s = t[1]
        try:
            u = ref.v_avg(vals, t)
        except RunErr as e:
            bad.append(("ref_error", t, vals, e.kind))
            continue
        exact = Fraction(sum(vals), len(vals) * 10 ** s)
        refv = Fraction(u, 10 ** S)
        half = Fraction(1, 2 * 10 ** S)
        d = abs(refv - exact)
        if d > half or (d == half and abs(refv) < abs(exact)):
            bad.append(("avg_not_nearest_or_tie_not_away", t, vals, u))
        if d == half:
            stats["AVG exact ties"] = stats.get("AVG exact ties", 0) + 1
        if abs(u) >= 10 ** P:
            bad.append(("avg_overflow", t, vals, u))
        if hug[gi] != u:
            bad.append(("avg_vs_duckdb_hugeint", t, vals, u, hug[gi]))
        else:
            stats["AVG groups equal to DuckDB HUGEINT arithmetic on DuckDB's SUM"] = stats.get("AVG groups equal to DuckDB HUGEINT arithmetic on DuckDB's SUM", 0) + 1
        if gi not in duck:
            continue
        atype, stype, atext, stext = duck[gi]
        if atype != "DOUBLE":
            bad.append(("duckdb_avg_not_double", t, atype))
            continue
        if norm(stext, s) != fmt(sum(vals), s):
            bad.append(("sum_vs_duckdb", t, vals, stext))
            continue
        tol = float(half) + 1e-15 * abs(float(exact)) + 1e-300
        if abs(float(atext) - float(refv)) > tol:
            bad.append(("avg_vs_duckdb_double", t, vals, u, atext))
            continue
        stats["AVG groups (nearest/ties-away exact; DuckDB DOUBLE within tolerance; SUM equal)"] = stats.get(
            "AVG groups (nearest/ties-away exact; DuckDB DOUBLE within tolerance; SUM equal)", 0) + 1
    # AVG can never be a bind error and the integer digits never grow: exhaustive over all 779 types
    n = 0
    for p, s in TYPES:
        P, S = ref.t_avg(p, s)
        n += 1
        if not (1 <= P <= MAXP and 0 <= S <= P):
            bad.append(("avg_type_invalid", (p, s), (P, S)))
        if P - S != p - s:
            bad.append(("avg_int_digits_changed", (p, s), (P, S)))
        if S < s:
            bad.append(("avg_scale_below_input", (p, s), (P, S)))
        if S != min(s + 6, MAXP - (p - s)):
            bad.append(("avg_scale_spec", (p, s), (P, S)))
    stats["AVG types checked (all)"] = n
    return bad


# =========================================================================== ROUND / CAST / FLOOR...
# ROUND with n < 0 gets the most types: s = 0 at p = 1..3, 18, 19, 37, 38 and s = 1, because the precision rule
# (decision 2) turns on s1 = 0 and -n <= p1, so every side of both tests needs a type.
FUNC_TYPES = (SMALL_TYPES + [(5, 2), (5, 0), (4, 3), (10, 2), (18, 6), (19, 9), (38, 0), (38, 10), (38, 38),
                             (3, 0), (10, 1), (18, 0), (19, 0), (37, 0), (38, 1)]) if FULL else (
    SMALL_TYPES + [(5, 0), (5, 2), (18, 0), (38, 0), (38, 1), (10, 1)])
ROUND_NS = [-45, -40, -39, -38, -37, -20, -19, -18, -10, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 5, 10, 38] if FULL else [
    -40, -38, -19, -6, -5, -2, -1, 0, 2, 38]
CAST_TARGETS = [(1, 0), (2, 1), (3, 3), (5, 2), (10, 5), (18, 0), (18, 9), (19, 0), (38, 10), (38, 0)] if FULL else [
    (2, 1), (5, 2), (18, 0), (38, 10)]
ROUND_TYPE_NS = list(range(-45, 41))  # typeof(round(NULL::DECIMAL(p,s), n)) for every type and these n


def func_groups():
    return [(t, small_values(*t) if t[0] <= 2 else boundary_values(*t)) for t in FUNC_TYPES]


def duck_funcs(groups, cache_dir):
    def run():
        jobs = []
        for k, (t, _) in enumerate(groups):
            for n in ROUND_NS:
                e = f"round(v, {n})"
                jobs.append((f"r_{k}_{n}", f"SELECT replace(typeof({e}),',','.') || '|' || coalesce(CAST(TRY({e}) AS VARCHAR),'NULL') FROM t{k} ORDER BY i"))
            for (p2, s2) in CAST_TARGETS:
                e = f"CAST(v AS DECIMAL({p2},{s2}))"
                jobs.append((f"c_{k}_{p2}_{s2}", f"SELECT coalesce(CAST(TRY({e}) AS VARCHAR),'NULL') FROM t{k} ORDER BY i"))
            for fn in ("floor", "ceil", "trunc"):
                e = f"{fn}(v)"
                jobs.append((f"{fn}_{k}", f"SELECT replace(typeof({e}),',','.') || '|' || coalesce(CAST(TRY({e}) AS VARCHAR),'NULL') FROM t{k} ORDER BY i"))
        res = run_jobs(jobs, prelude=sql_tables(groups))
        out = {}
        for jid, (data, errs) in res.items():
            assert not errs, (jid, errs[:2])
            out[jid] = data
        return out

    return _cache(cache_dir, "funcs", run)


def check_funcs(ref, groups, duck, stats):
    bad = []

    def note(k, n=1):
        stats[k] = stats.get(k, 0) + n

    def mism(*x):
        if len(bad) < 200:
            bad.append(x)

    for k, (t, vals) in enumerate(groups):
        for n in ROUND_NS:
            lines = duck[f"r_{k}_{n}"]
            P, S = ref.t_round(*t, n)
            for u, line in zip(vals, lines):
                dtype, _, dtext = line.partition("|")
                try:
                    exp = fmt(ref.v_round(u, t, n), S)
                except RunErr:
                    exp = "NULL"
                if dtype != f"DECIMAL({P}.{S})":
                    mism("round_type", t, n, dtype, (P, S))
                elif norm(dtext, S) != exp:
                    mism("round_value", t, n, u, exp, dtext)
                else:
                    note("ROUND cases equal" if exp != "NULL" else "ROUND cases: both error")
        for (p2, s2) in CAST_TARGETS:
            lines = duck[f"c_{k}_{p2}_{s2}"]
            for u, line in zip(vals, lines):
                try:
                    exp = fmt(ref.v_cast(u, t, (p2, s2)), s2)
                except RunErr:
                    exp = "NULL"
                if norm(line, s2) != exp:
                    mism("cast_value", t, (p2, s2), u, exp, line)
                else:
                    note("CAST DECIMAL->DECIMAL cases equal" if exp != "NULL" else "CAST cases: both error")
        for fn, f in (("floor", ref.v_floor), ("ceil", ref.v_ceil), ("trunc", ref.v_trunc)):
            lines = duck[f"{fn}_{k}"]
            P, S = ref.t_floor(*t)
            for u, line in zip(vals, lines):
                dtype, _, dtext = line.partition("|")
                exp = fmt(f(u, t), 0)
                if dtype != f"DECIMAL({P}.{S})":
                    mism(fn + "_type", t, dtype, (P, S))
                elif dtext != exp:
                    mism(fn + "_value", t, u, exp, dtext)
                else:
                    note(f"{fn.upper()} cases equal")
    return bad


def duck_func_types(cache_dir):
    """typeof of ROUND(x, n) for all 779 types and n in -3..40, and of FLOOR/CEIL/TRUNC/-x/ABS."""

    def run():
        jobs = []
        exprs = []
        for (p, s) in TYPES:
            for n in ROUND_TYPE_NS:
                exprs.append(("round", p, s, n, f"typeof(round(NULL::DECIMAL({p},{s}), {n}))"))
            for fn in ("floor", "ceil", "trunc", "abs"):
                exprs.append((fn, p, s, 0, f"typeof({fn}(NULL::DECIMAL({p},{s})))"))
            exprs.append(("neg", p, s, 0, f"typeof(-NULL::DECIMAL({p},{s}))"))
        for c in range(0, len(exprs), 3000):
            jobs.append((f"f{c}", "SELECT " + ", ".join(e[4] for e in exprs[c : c + 3000])))
        res = run_jobs(jobs)
        out = {}
        for c in range(0, len(exprs), 3000):
            data, errs = res[f"f{c}"]
            assert not errs, errs[:2]
            row = csv_row(data[0])
            for e, ty in zip(exprs[c : c + 3000], row):
                out[e[:4]] = ty
        return out

    return _cache(cache_dir, "functypes", run)


def round_neg_oracle(p, s, n):
    """First principles for ROUND(DECIMAL(p,s), n) with n < 0: the largest |result| (as an integer at scale 0) is the
    rounding of the largest |input| 10^p - 1 (rounding is monotone), so the result needs digits(that) digits.
    Returns that many digits. No use of Ref."""
    M = 10 ** p - 1
    q = round_div(M, 10 ** (s - n))  # M / 10^s rounded to a multiple of 10^-n, in units of 10^-n
    return len(str(q * 10 ** (-n))) if q else 1


def check_func_types(ref, duck, stats):
    bad = []
    n = 0
    neg = 0
    for (fn, p, s, k), ty in duck.items():
        if fn == "round":
            exp = ref.t_round(p, s, k)
        elif fn in ("floor", "ceil", "trunc"):
            exp = ref.t_floor(p, s)
        else:
            exp = ref.t_neg(p, s)
        n += 1
        got = f"DECIMAL({exp[0]},{exp[1]})"
        if fn == "round" and k < 0:
            neg += 1
            # first-principles: the type is never too narrow, and the scale is 0
            need = round_neg_oracle(p, s, k)
            # at the 38-digit cap the largest result (10^38) needs 39 digits: that is the checked run-time overflow
            # (DuckDB: "Overflow in ROUND of DECIMAL(38)"); everywhere else the type must hold the largest result
            if exp[1] != 0 or not (1 <= exp[0] <= MAXP) or (need > exp[0] and exp[0] < MAXP):
                bad.append(("round_neg_type_too_narrow_or_invalid", (p, s), k, exp, need))
            if need > MAXP:
                stats["ROUND n<0 at the cap: the largest input rounds to 10^38 (39 digits): checked run-time overflow (type, n)"] = stats.get(
                    "ROUND n<0 at the cap: the largest input rounds to 10^38 (39 digits): checked run-time overflow (type, n)", 0) + 1
            if exp[0] > need:
                stats["ROUND n<0: precision wider than the largest result needs (type, n)"] = stats.get(
                    "ROUND n<0: precision wider than the largest result needs (type, n)", 0) + 1
        # decision 2: exactly DuckDB's type (F2 resolved: zero differences are expected)
        if ty != got:
            bad.append((fn, (p, s), k, exp, ty))
    stats["function result types checked"] = n
    stats["  of which ROUND with n < 0 (equal to DuckDB, checked against the oracle)"] = neg
    return bad


# =========================================================================== Decimal64 overflow analysis (2d)
def overflow_analysis():
    """Exhaustive over all type pairs with both operands <= 18 digits. Pure integer arithmetic."""
    ref = Ref()
    I64 = 2 ** 63
    T = [(p, s) for p in range(1, 19) for s in range(p + 1)]
    r = {
        "pairs": 0,
        "addsub_P_le_18": 0, "addsub_max_rescaled_operand": 0, "addsub_max_sum": 0, "addsub_violations": 0,
        "addsub_P_gt_18": 0, "addsub_P_gt_18_needs_i128_rescale": 0,
        "mul_cap_off_P_le_18": 0, "mul_cap_off_max_product": 0, "mul_cap_off_violations": 0,
        "mul_cap_off_P_gt_18": 0, "mul_cap_off_P_gt_18_needs_i128": 0,
        "mul_cap_on": 0, "mul_cap_on_can_exceed_P": 0, "mul_cap_on_never_exceeds": 0,
        "mul_cap_on_only_between_1e18_and_2^63": 0, "mul_cap_on_can_exceed_i64": 0,
        "mul_bind_error": 0, "mul_i64_wrap_with_unchecked_cap": 0, "mul_cap_on_has_value_pair_in_[1e18,2^63)": 0,
        "div_P_le_18": 0, "div_P_le_18_divisor_wider_than_18": 0, "div_max_numerator": 0, "div_max_twice_divisor_plus": 0,
        "div_violations": 0,
        "mod_pairs_both_operands_le_18": 0, "mod_P_le_18": 0, "mod_aligned_operand_exceeds_i64": 0,
        "mod_max_aligned_digits": 0, "mod_P_gt_operand_digits": 0,
    }
    ex = {}
    for t1 in T:
        for t2 in T:
            (p1, s1), (p2, s2) = t1, t2
            A, B = 10 ** p1 - 1, 10 ** p2 - 1
            r["pairs"] += 1
            # ---- + and -
            P, S = ref.t_add(p1, s1, p2, s2)
            ra, rb = A * 10 ** (S - s1), B * 10 ** (S - s2)
            if P <= 18:
                r["addsub_P_le_18"] += 1
                r["addsub_max_rescaled_operand"] = max(r["addsub_max_rescaled_operand"], ra, rb)
                r["addsub_max_sum"] = max(r["addsub_max_sum"], ra + rb)
                if not (ra < I64 and rb < I64 and ra + rb < 10 ** P and ra + rb < I64):
                    r["addsub_violations"] += 1
            else:
                r["addsub_P_gt_18"] += 1
                if max(ra, rb) >= I64 or ra + rb >= I64:
                    r["addsub_P_gt_18_needs_i128_rescale"] += 1
                    ex.setdefault("addsub_needs_i128", (t1, t2, P, S))
            # ---- *
            try:
                P, S = ref.t_mul(p1, s1, p2, s2)
            except BindErr:
                r["mul_bind_error"] += 1
                continue
            cap_on = (p1 + p2 > P)  # precision was cut below p1+p2: width-class cap engaged
            mx = A * B
            if not cap_on:
                if P <= 18:
                    r["mul_cap_off_P_le_18"] += 1
                    r["mul_cap_off_max_product"] = max(r["mul_cap_off_max_product"], mx)
                    if not (mx < 10 ** P and mx < I64):
                        r["mul_cap_off_violations"] += 1
                else:
                    r["mul_cap_off_P_gt_18"] += 1
                    if mx >= I64:
                        r["mul_cap_off_P_gt_18_needs_i128"] += 1
                        ex.setdefault("mul_nocap_needs_i128", (t1, t2, P, S))
            else:
                r["mul_cap_on"] += 1
                if mx >= 10 ** P:
                    r["mul_cap_on_can_exceed_P"] += 1
                else:
                    r["mul_cap_on_never_exceeds"] += 1
                if mx >= I64:
                    r["mul_cap_on_can_exceed_i64"] += 1
                    # an example whose product exceeds i64 (a plain i64 multiply would wrap)
                    ex.setdefault("mul_cap_exceeds_i64", (t1, t2, P, S, A, B))
                    r["mul_i64_wrap_with_unchecked_cap"] += 1
                else:
                    r["mul_cap_on_only_between_1e18_and_2^63"] += 1
                    ex.setdefault("mul_cap_between", (t1, t2, P, S, A, B))
                ex.setdefault("mul_cap_example", (t1, t2, P, S, A, B))
                for a_ in (A, A // 2 + 1, 10 ** (p1 - 1)):
                    b_ = -(-(10 ** 18) // a_)
                    if b_ <= B and 10 ** 18 <= a_ * b_ < I64:
                        r["mul_cap_on_has_value_pair_in_[1e18,2^63)"] += 1
                        break
            # ---- /
            try:
                P, S = ref.t_div(p1, s1, p2, s2)
            except BindErr:
                P = None
            if P is not None and P <= 18:
                r["div_P_le_18"] += 1
                num = A * 10 ** (s2 + S - s1)  # largest |a| * 10^(s2+S-s1)
                r["div_max_numerator"] = max(r["div_max_numerator"], num)
                r["div_max_twice_divisor_plus"] = max(r["div_max_twice_divisor_plus"], 2 * B + 1)
                if not (num < 10 ** P and num + B < I64 and 2 * B + 1 < I64):
                    r["div_violations"] += 1
            # ---- % (decision 3: P = min(p1-s1, p2-s2) + S, so the output is much smaller than the aligned operands;
            # with operands of at most 18 digits the alignment needs at most 36 digits, so no bind error here)
            P, S = ref.t_mod(p1, s1, p2, s2)
            r["mod_pairs_both_operands_le_18"] += 1
            if P <= 18:
                r["mod_P_le_18"] += 1
            ra, rb = A * 10 ** (S - s1), B * 10 ** (S - s2)
            r["mod_max_aligned_digits"] = max(r["mod_max_aligned_digits"], len(str(max(ra, rb))))
            if max(ra, rb) >= I64:
                r["mod_aligned_operand_exceeds_i64"] += 1
                ex.setdefault("mod_aligned_exceeds_i64", (t1, t2, P, S))
            if P > max(p1, p2):
                r["mod_P_gt_operand_digits"] += 1
    # division with P <= 18 and a divisor wider than 18 digits (output Decimal64, input Decimal128)
    wide = 0
    for t1 in TYPES:
        for t2 in TYPES:
            if t2[0] > 18:
                try:
                    P, S = ref.t_div(*t1, *t2)
                except BindErr:
                    continue
                if P <= 18:
                    wide += 1
                    ex.setdefault("div_wide_divisor", (t1, t2, P, S))
    r["div_P_le_18_divisor_wider_than_18"] = wide
    # % over ALL type pairs (decision 3): bind error where the alignment needs > 38 digits; for the others the aligned
    # operand (a * 10^(S-s1)) is at most 38 digits, so it fits i128 but can be wider than the output type or than i64
    I128 = 2 ** 127
    wide_in = bound = bind = over128 = over_i64 = 0
    for t1 in TYPES:
        for t2 in TYPES:
            (p1, s1), (p2, s2) = t1, t2
            try:
                P, S = ref.t_mod(p1, s1, p2, s2)
            except BindErr:
                bind += 1
                continue
            bound += 1
            A, B = 10 ** p1 - 1, 10 ** p2 - 1
            ra, rb = A * 10 ** (S - s1), B * 10 ** (S - s2)
            if P <= 18 and max(p1, p2) > 18:
                wide_in += 1
                ex.setdefault("mod_decimal64_output_decimal128_input", (t1, t2, P, S))
            if max(ra, rb) >= I64:
                over_i64 += 1
            if max(ra, rb) >= I128:
                over128 += 1
    r["mod_pairs_bound (all type pairs)"] = bound
    r["mod_bind_error_pairs: alignment needs more than 38 digits (all type pairs)"] = bind
    r["mod_P_le_18_with_an_operand_wider_than_18_digits (all type pairs)"] = wide_in
    r["mod_bound_pairs_where_aligned_operand_exceeds_i64 (all type pairs)"] = over_i64
    r["mod_bound_pairs_where_aligned_operand_exceeds_i128 (must be 0)"] = over128
    # for each cap-engaged pair show which sub-classes exist: count by p1+p2
    return r, ex


def straddle_example(t1, t2):
    """(a,b) unscaled with 10^18 <= a*b < 2^63 for a cap-engaged pair (None if impossible)."""
    (p1, s1), (p2, s2) = t1, t2
    A, B = 10 ** p1 - 1, 10 ** p2 - 1
    a = A
    b = -(-(10 ** 18) // a)  # smallest b with a*b >= 10^18
    if b <= B and a * b < 2 ** 63:
        return a, b
    return None
