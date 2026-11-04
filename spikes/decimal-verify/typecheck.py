"""Result-type checks: invariants over all (p1,s1,p2,s2) and the DuckDB type matrix."""
import os
import pickle
from array import array

from duck import csv_row, parse_type, run_jobs
from rules_ref import MAXP, NT, TYPES, BindErr, Ref

OPS = ["+", "-", "*", "/", "%"]
ERR, DBL, OTHER = 2, 1, 3


def code_of(t):
    if isinstance(t, tuple):
        return 4096 + (t[0] << 6 | t[1])
    return DBL if t == "DOUBLE" else OTHER


def decode(c):
    return ((c - 4096) >> 6, (c - 4096) & 63) if c >= 4096 else c


def expr(op, t1, t2):
    a, b = f"NULL::DECIMAL({t1[0]},{t1[1]})", f"NULL::DECIMAL({t2[0]},{t2[1]})"
    if op == "unify":
        return f"typeof(COALESCE({a}, {b}))"
    return f"typeof({a} {op} {b})"


def duck_type_matrix(op, cache_dir, chunk=3000):
    """array('H') of NT*NT codes: DuckDB's typeof for op over all type pairs (cached)."""
    path = os.path.join(cache_dir, f"types_{op.replace('/', 'div').replace('%', 'mod').replace('*', 'mul')}.pkl")
    if os.path.exists(path):
        with open(path, "rb") as f:
            return pickle.load(f)
    base = Ref()
    safe, risky = [], []
    for i, t1 in enumerate(TYPES):
        for j, t2 in enumerate(TYPES):
            k = i * NT + j
            try:
                if op == "%":
                    pass  # DuckDB never errors for %: DOUBLE where Nata has a bind error (documented)
                elif op != "unify":
                    base.result_type(op, t1, t2)
                else:
                    base.t_unify(t1, t2)
                safe.append(k)
            except BindErr:
                risky.append(k)
    codes = array("H", [0]) * (NT * NT)
    jobs = []
    for c in range(0, len(safe), chunk):
        jobs.append((f"c{c}", "SELECT " + ", ".join(expr(op, TYPES[k // NT], TYPES[k % NT]) for k in safe[c : c + chunk])))
    for k in risky:
        jobs.append((f"s{k}", "SELECT " + expr(op, TYPES[k // NT], TYPES[k % NT])))
    res = run_jobs(jobs)
    redo = []
    for c in range(0, len(safe), chunk):
        data, errs = res[f"c{c}"]
        if errs or not data:
            redo += safe[c : c + chunk]
            continue
        row = csv_row(data[0])
        assert len(row) == len(safe[c : c + chunk]), (len(row), op)
        for k, ty in zip(safe[c : c + chunk], row):
            codes[k] = code_of(parse_type(ty))
    for k in risky:
        data, errs = res[f"s{k}"]
        codes[k] = ERR if errs else code_of(parse_type(csv_row(data[0])[0]))
    if redo:
        res2 = run_jobs([(f"s{k}", "SELECT " + expr(op, TYPES[k // NT], TYPES[k % NT])) for k in redo])
        for k in redo:
            data, errs = res2[f"s{k}"]
            codes[k] = ERR if errs else code_of(parse_type(csv_row(data[0])[0]))
    os.makedirs(cache_dir, exist_ok=True)
    with open(path, "wb") as f:
        pickle.dump(codes, f)
    return codes


def duck_mod_model(t1, t2):
    """What DuckDB 2.0.0 does for decimal % decimal (found by experiment, checked below against all pairs):
    the wider operand type DECIMAL(max(int digits) + S, S), or DOUBLE when that needs more than 38 digits."""
    (p1, s1), (p2, s2) = t1, t2
    S = max(s1, s2)
    P = max(p1 - s1, p2 - s2) + S
    return (P, S) if P <= MAXP else DBL


def compare_types(ref, op, codes):
    """Compare the reference with DuckDB. Returns (counts dict, list of disagreement samples).

    Expectation per operator:
      + - : identical decimal type
      *   : identical decimal type, or both bind errors (DuckDB: out-of-range error)
      /   : DuckDB always DOUBLE (documented difference: Nata is exact decimal or bind error)
      %   : documented differences (section 9). DuckDB's type must equal the model above (else the model, and so the
            classification, is wrong: a disagreement). Where aligning the operands needs more than 38 digits
            (max(i1,i2)+S > 38, written here independently of the reference) Nata has a bind error and DuckDB is
            DOUBLE; elsewhere Nata binds, its scale equals DuckDB's and its precision is <= DuckDB's.
    """
    cnt = {"equal": 0, "both_error": 0, "doc_diff": 0, "disagree": 0}
    if op == "%":
        cnt = {"equal": 0, "doc_diff_tighter_decimal": 0, "doc_diff_bind_error_duckdb_double": 0, "disagree": 0}
    bad = []
    for i, t1 in enumerate(TYPES):
        for j, t2 in enumerate(TYPES):
            c = codes[i * NT + j]
            try:
                r = ref.result_type(op, t1, t2)
            except BindErr:
                r = None
            d = decode(c)
            if op in "+-":
                ok = r is not None and d == r
                kind = "equal"
            elif op == "*":
                if r is None:
                    ok, kind = d == ERR, "both_error"
                else:
                    ok, kind = d == r, "equal"
            elif op == "/":
                ok, kind = d == DBL, "doc_diff"
            else:  # %
                model = duck_mod_model(t1, t2)
                over = max(t1[0] - t1[1], t2[0] - t2[1]) + max(t1[1], t2[1]) > MAXP
                if over:
                    ok, kind = r is None and d == DBL and model == DBL, "doc_diff_bind_error_duckdb_double"
                else:
                    ok = r is not None and d == model and r[1] == model[1] and r[0] <= model[0]
                    kind = "equal" if (ok and r == model) else "doc_diff_tighter_decimal"
            if ok:
                cnt[kind] += 1
            else:
                cnt["disagree"] += 1
                if len(bad) < 5:
                    bad.append((t1, t2, r, d))
    return cnt, bad


def compare_unify(ref, codes):
    """CASE/COALESCE/UNION: equal when it fits 38; DuckDB silently cuts the scale to 38 - intdigits otherwise."""
    cnt = {"equal": 0, "doc_diff": 0, "disagree": 0}
    bad = []
    for i, t1 in enumerate(TYPES):
        for j, t2 in enumerate(TYPES):
            d = decode(codes[i * NT + j])
            try:
                r = ref.t_unify(t1, t2)
                ok, kind = d == r, "equal"
            except BindErr:
                I = max(t1[0] - t1[1], t2[0] - t2[1])
                ok, kind = d == (MAXP, MAXP - I), "doc_diff"
            if ok:
                cnt[kind] += 1
            else:
                cnt["disagree"] += 1
                if len(bad) < 5:
                    bad.append((t1, t2, d))
    return cnt, bad


# --------------------------------------------------------------------------- invariants
def digits(x):
    return len(str(x))


def bound(op, t1, t2, S):
    """Exact max |result| as an unscaled integer at scale S (first principles, no use of Ref's P)."""
    (p1, s1), (p2, s2) = t1, t2
    A, B = 10 ** p1 - 1, 10 ** p2 - 1
    if op in "+-":
        return A * 10 ** (S - s1) + B * 10 ** (S - s2)
    if op == "*":
        return A * B
    if op == "/":  # largest |a| over the smallest nonzero |b| (one unit of 10^-s2)
        return A * 10 ** (s2 + S - s1)
    if op == "%":  # |a % b| <= |a| and < |b| (after aligning to S)
        return min(A * 10 ** (S - s1), B * 10 ** (S - s2) - 1)
    raise ValueError(op)


def check_invariants(ref, op):
    """Exhaustive over all 779^2 type pairs. Returns (n_pairs, violations{name: (count, sample)})."""
    viol = {}

    def bad(name, sample):
        c, s = viol.get(name, (0, None))
        viol[name] = (c + 1, s or sample)

    n = 0
    P = [0] * (NT * NT)  # 0 = bind error
    S_ = [0] * (NT * NT)
    for i, t1 in enumerate(TYPES):
        p1, s1 = t1
        for j, t2 in enumerate(TYPES):
            p2, s2 = t2
            n += 1
            s = (t1, t2)
            try:
                p, sc = ref.result_type(op, t1, t2)
            except BindErr as e:
                # bind error exactly where the document says (written here as digit budgets, not as the formula)
                if op in "+-":
                    bad("bind_error_where_none_expected", s)
                elif op == "*":
                    if s1 + s2 <= MAXP:
                        bad("mul_bind_error_but_scale_fits", s)
                elif op == "/":
                    S = min(s1 + 6, MAXP)
                    if digits(bound("/", t1, t2, S)) <= MAXP:
                        bad("div_bind_error_but_needs_<=38_digits", s)
                else:
                    # decision 3: a bind error exactly when aligning the operands needs more than 38 digits
                    if max(p1 - s1, p2 - s2) + max(s1, s2) <= MAXP:
                        bad("mod_bind_error_but_alignment_fits_38", s)
                continue
            P[i * NT + j], S_[i * NT + j] = p, sc
            if not (1 <= p <= MAXP):
                bad("P_out_of_1..38", s)
            if not (0 <= sc <= p):
                bad("S_not_in_0..P", s)
            # S as specified
            if op in "+-%" and sc != max(s1, s2):
                bad("S_is_max_scale", s)
            if op == "*" and sc != s1 + s2:
                bad("S_is_sum_of_scales", s)
            if op == "/" and not (sc - s1 == 6 or sc == MAXP):
                bad("S_is_s1_plus_6_or_38", s)
            # no bind error where the scale/digits budget says it must be one
            if op == "*" and s1 + s2 > MAXP:
                bad("mul_no_bind_error_above_38", s)
            if op == "/":
                if digits(bound("/", t1, t2, sc)) > MAXP:
                    bad("div_no_bind_error_when_needs_>38", s)
                # tightness: precision is exactly the digits of the largest quotient
                if p != digits(bound("/", t1, t2, sc)):
                    bad("div_P_equals_digits_of_max_quotient", s)
            if op == "%" and max(p1 - s1, p2 - s2) + max(s1, s2) > MAXP:
                bad("mod_no_bind_error_when_alignment_over_38", s)
            if op == "%":
                # decision 3, first principles: the exact largest |a % b| after aligning to S is
                # min(A', B' - 1) (attained: a = B' - 1 gives B' - 1 when B' - 1 <= A', a = A' < B' gives A'
                # otherwise), so P must be exactly its digit count, and never above 38
                if p != digits(max(1, bound("%", t1, t2, sc))):
                    bad("mod_P_is_digits_of_largest_remainder", s)
            # +/-: never too narrow below the cap; exact when the scales are equal, at most one digit
            # wider otherwise (the aligned operand has trailing zeros, so the carry may not happen)
            if op in "+-":
                dg = digits(bound(op, t1, t2, sc))
                if p < MAXP and dg > p:
                    bad("addsub_P_too_narrow", s)
                if p < MAXP and p - dg > 1:
                    bad("addsub_P_more_than_1_wider_than_needed", s)
                if p < MAXP and s1 == s2 and p != dg:
                    bad("addsub_P_exact_for_equal_scales", s)
            # *: P is min(38, p1+p2) unless the 64-bit width-class cap engaged (P=18 < p1+p2)
            if op == "*":
                if p < min(MAXP, p1 + p2):
                    if not (p == 18 and p1 <= 18 and p2 <= 18 and sc < 18):
                        bad("mul_P_below_p1+p2_only_via_64bit_cap", s)
                elif p != min(MAXP, p1 + p2):
                    bad("mul_P_above_min38_p1+p2", s)
            # %: the result always fits (|a % b| <= min(|a|, |b|-1))
            if op == "%" and digits(max(1, bound("%", t1, t2, sc))) > p:
                bad("mod_result_may_exceed_P", s)
    # monotonicity the document implies: a wider operand never gives a narrower result,
    # and a bind error never disappears when an operand gets wider (same scales).
    idx = {t: k for k, t in enumerate(TYPES)}
    for i, (p1, s1) in enumerate(TYPES):
        i_up = idx.get((p1 + 1, s1))
        for j, (p2, s2) in enumerate(TYPES):
            j_up = idx.get((p2 + 1, s2))
            k = i * NT + j
            for k2, which in ((i_up * NT + j if i_up is not None else None, "p1"), (i * NT + j_up if j_up is not None else None, "p2")):
                if k2 is None:
                    continue
                if P[k] and not P[k2]:
                    continue  # error appears when wider: fine
                if not P[k] and P[k2]:
                    bad(f"bind_error_disappears_when_{which}_widens", (TYPES[i], TYPES[j]))
                elif P[k] and P[k2] < P[k]:
                    bad(f"P_shrinks_when_{which}_widens", (TYPES[i], TYPES[j]))
    for i, (p1, s1) in enumerate(TYPES):
        i_up = idx.get((p1 + 1, s1 + 1))
        for j, (p2, s2) in enumerate(TYPES):
            k = i * NT + j
            if i_up is not None:
                k2 = i_up * NT + j
                if P[k] and P[k2] and S_[k2] < S_[k]:
                    bad("S_shrinks_when_s1_grows", (TYPES[i], TYPES[j]))
    return n, viol


def check_mod_bound_bruteforce(limit=40):
    """The oracle behind decision 3, itself checked: for all aligned maxima A', B' in 1..limit the exact largest
    a % b over a in 0..A', b in 1..B' is min(A', B' - 1) (what bound('%') returns). Pure integers, no Ref."""
    n = bad = 0
    for A in range(1, limit + 1):
        for B in range(1, limit + 1):
            exact = max(a % b for a in range(A + 1) for b in range(1, B + 1))
            n += 1
            if exact != min(A, B - 1):
                bad += 1
    return n, bad


def check_mod_witnesses(ref):
    """Decision 3, value level, over every type pair: evaluate the reference's checked `%` at the extreme witnesses
    (both maxima, and the exact-supremum pair a = B' - 1, b = B when the dividend needs no rescaling) and require that
    no overflow is raised and the remainder fits P. Returns (pairs, attained, failures):
    `attained` = pairs where some witness remainder has exactly P digits (P is not just an upper bound)."""
    from rules_ref import RunErr

    pairs = attained = bad = 0
    for t1 in TYPES:
        for t2 in TYPES:
            (p1, s1), (p2, s2) = t1, t2
            try:
                P, S = ref.t_mod(p1, s1, p2, s2)
            except BindErr:
                continue  # the invariant check owns the bind-error set
            A, B = 10 ** p1 - 1, 10 ** p2 - 1
            k1, k2 = S - s1, S - s2
            Bal = B * 10 ** k2
            ws = [(A, B)]
            if k1 == 0 and Bal - 1 <= A:
                ws.append((Bal - 1, B))
            if k1 == 0 and Bal - 1 > A:
                ws.append((A, B))
            pairs += 1
            hit = False
            for a, b in ws:
                for sa in (1, -1):
                    try:
                        r = ref.v_mod(sa * a, t1, b, t2)
                    except RunErr:
                        bad += 1
                        continue
                    hit = hit or len(str(abs(r))) == P
            attained += hit
    return pairs, attained, bad


def monotonicity_report(ref, op):
    """Descriptive: where is P non-monotonic in a scale (not asserted; the document does not promise it)."""
    idx = {t: k for k, t in enumerate(TYPES)}
    cnt = 0
    first = None
    for i, (p1, s1) in enumerate(TYPES):
        i_up = idx.get((p1, s1 + 1))
        if i_up is None:
            continue
        for j, t2 in enumerate(TYPES):
            try:
                a = ref.result_type(op, TYPES[i], t2)
                b = ref.result_type(op, TYPES[i_up], t2)
            except BindErr:
                continue
            if b[0] < a[0]:
                cnt += 1
                first = first or (TYPES[i], t2, a, b)
    return cnt, first


DOC_EXAMPLES = [
    # operands, +/-, *, /, %    (None = bind error)   -- transcribed verbatim from docs/decimal-rules.md section 3
    ((15, 2), (15, 2), (16, 2), (18, 4), (23, 8), (15, 2)),
    ((15, 2), (5, 3), (17, 3), (18, 5), (24, 8), (5, 3)),
    ((18, 2), (18, 2), (19, 2), (18, 4), (26, 8), (18, 2)),
    ((38, 2), (38, 2), (38, 2), (38, 4), None, (38, 2)),
    ((10, 2), (10, 2), (11, 2), (18, 4), (18, 8), (10, 2)),
    ((10, 9), (10, 9), (11, 9), (20, 18), (25, 15), (10, 9)),
    ((15, 2), (20, 2), (21, 2), (35, 4), (23, 8), (15, 2)),
    ((20, 20), (20, 20), (21, 20), None, None, (20, 20)),
    ((10, 0), (10, 0), (11, 0), (18, 0), (16, 6), (10, 0)),
    ((19, 0), (19, 0), (20, 0), (38, 0), (25, 6), (19, 0)),
    ((38, 0), (38, 0), (38, 0), (38, 0), None, (38, 0)),
    ((10, 8), (10, 8), (11, 8), (18, 16), (24, 14), (10, 8)),
    ((38, 38), (1, 0), (38, 38), (38, 38), (38, 38), None),
    ((38, 0), (38, 38), (38, 38), (38, 38), None, None),
]
# Cells where an example contradicts the document's own formula would be listed here (the reference follows the
# formula; such a cell is reported as a finding, any other mismatch is a failure). Empty since the document's
# `DECIMAL(15,2) % DECIMAL(20,2)` row was corrected to (15,2).
DOC_EXAMPLE_ERRORS = {}
DOC_AVG = [((15, 2), (21, 8)), ((18, 2), (24, 8)), ((10, 8), (16, 14)), ((38, 2), (38, 2)), ((38, 0), (38, 0)), ((5, 5), (11, 11))]


def check_doc_examples(ref):
    """Returns (checked, failures[list of str], findings{str: count}). A cell where the reference differs from the
    document is a failure unless it is listed in DOC_EXAMPLE_ERRORS with exactly the reference's value."""
    fails, n, findings = [], 0, {}

    def get(op, a, b):
        try:
            return ref.result_type(op, a, b)
        except BindErr:
            return None

    for a, b, add, mul, div, mod in DOC_EXAMPLES:
        for op, exp in (("+", add), ("-", add), ("*", mul), ("/", div), ("%", mod)):
            n += 1
            got = get(op, a, b)
            if got != exp:
                if (a, b, op) in DOC_EXAMPLE_ERRORS and DOC_EXAMPLE_ERRORS[(a, b, op)] == got:
                    key = f"doc example {a} {op} {b} prints {exp}, the document's formula gives {got}"
                    findings[key] = findings.get(key, 0) + 1
                else:
                    fails.append(f"{a} {op} {b}: doc {exp}, ref {got}")
    for t, exp in DOC_AVG:
        n += 1
        got = ref.t_avg(*t)
        if got != exp:
            fails.append(f"AVG{t}: doc {exp}, ref {got}")
    # Q1 chain from the notes: price * (1 - discount) * (1 + tax) on DECIMAL(15,2) is DECIMAL(18,6)
    n += 1
    one = (10, 0)  # the literal 1 is INTEGER
    try:
        one_minus = ref.result_type("-", one, (15, 2))
        one_plus = ref.result_type("+", one, (15, 2))
        step = ref.result_type("*", (15, 2), one_minus)
        got = ref.result_type("*", step, one_plus)
    except BindErr:
        got = None
    if got != (18, 6):
        fails.append(f"Q1 chain: doc (18,6), ref {got}")
    n += 1
    got = ref.result_type("/", (10, 0), (10, 0))
    if got != (16, 6):
        fails.append(f"7/2 type: doc (16,6), ref {got}")
    return n, fails, findings
