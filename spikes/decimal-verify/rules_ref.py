"""Independent reference implementation of docs/decimal-rules.md (exact integers only).

Derived from the document, not from DuckDB.  A `Ref` can carry deliberate MUTATIONS (see
MUTATIONS) so the verification harness can be shown to fail on known-bad rules.

A decimal value of type (p, s) is represented by its unscaled integer u (value = u / 10**s).
"""

MAXP = 38
CAP64 = 18  # Decimal64 holds precision <= 18

# integer types: name -> bits, and the exact-decimal reading used when an integer meets a decimal (section 2)
INT_BITS = {"TINYINT": 8, "SMALLINT": 16, "INTEGER": 32, "BIGINT": 64}
INT_NAME = {v: k for k, v in INT_BITS.items()}
INT_DEC = {"TINYINT": (3, 0), "SMALLINT": (5, 0), "INTEGER": (10, 0), "BIGINT": (19, 0)}
INT_TYPES = list(INT_BITS)


class BindErr(Exception):
    """Query rejected before it runs."""

    def __init__(self, kind):
        super().__init__(kind)
        self.kind = kind


class RunErr(Exception):
    """Error raised for a selected, valid row."""

    def __init__(self, kind):
        super().__init__(kind)
        self.kind = kind


def fmt(u, s):
    """Exact text of unscaled u at scale s, as DuckDB prints a DECIMAL."""
    neg = u < 0
    d = str(-u if neg else u)
    if s == 0:
        out = d
    else:
        d = d.rjust(s + 1, "0")
        out = d[:-s] + "." + d[-s:]
    return "-" + out if neg else out


def norm(text, S):
    """Normalise DuckDB's decimal text (it prints DECIMAL(p,p) as '.9801', without the leading 0) to fmt() form."""
    if text == "NULL":
        return text
    neg = text.startswith("-")
    body = text[1:] if neg else text
    ip, _, fp = body.partition(".")
    assert len(fp) == S, (text, S)
    u = int((ip + fp) or "0")
    return fmt(-u if neg else u, S)


def digits(x):
    return len(str(abs(x))) if x else 0


def round_div(n, d, mode="away"):
    """n / d rounded to an integer. d != 0. Default: nearest, ties away from zero."""
    if d < 0:
        n, d = -n, -d
    neg = n < 0
    q, r = divmod(-n if neg else n, d)
    twice = 2 * r
    if mode == "away":
        up = twice >= d
    elif mode == "even":
        up = twice > d or (twice == d and q % 2 == 1)
    elif mode == "tie_down":  # ties toward zero
        up = twice > d
    elif mode == "tie_posinf":  # ties toward +infinity
        up = twice > d or (twice == d and not neg)
    elif mode == "trunc":
        up = False
    elif mode == "ceil_abs":
        up = r != 0
    else:
        raise ValueError(mode)
    if up:
        q += 1
    return -q if neg else q


# name -> (operators it affects, description)
# Operator keys: "+", "-", "*", "/", "%" decimal operators; "avg", "unify", "round", "cast";
# "i+", "i-", "i*", "i%", "i//", "i/" integer-with-integer operators; "idec" integer-with-decimal.
MUTATIONS = {
    "div_scale_plus5": ({"/"}, "division scale s1+5 instead of s1+6"),
    "div_scale_plus7": ({"/"}, "division scale s1+7 instead of s1+6"),
    "div_prec_plus1": ({"/"}, "division precision +1 (bind-error threshold shifts)"),
    "div_scale_cap37": ({"/"}, "division scale capped at 37 instead of 38"),
    "div_round_half_even": ({"/"}, "division rounds ties to even"),
    "div_round_half_up_posinf": ({"/"}, "division rounds ties toward +infinity"),
    "div_round_trunc": ({"/"}, "division truncates instead of rounding"),
    "div_by_zero_gives_zero": ({"/"}, "division by zero returns 0 instead of an error"),
    "add_scale_min": ({"+", "-"}, "max(s1,s2) -> min(s1,s2) in + and -"),
    "add_prec_no_plus1": ({"+", "-"}, "+/- precision without the carry digit"),
    "add_cap_cuts_scale": ({"+", "-"}, "+/- at the 38 cap cuts the scale instead of keeping it"),
    "add_prec_max_to_min": ({"+", "-"}, "max(p1-s1,p2-s2) -> min in + and -"),
    "mul_cap_threshold_19": ({"*"}, "width-class cap applies to operands <= 19 instead of <= 18"),
    "mul_cap_scale_le18": ({"*"}, "width-class cap applies when S <= 18 instead of S < 18"),
    "mul_cap_or": ({"*"}, "width-class cap applies when EITHER operand is <= 18"),
    "mul_cap_value_17": ({"*"}, "width-class cap is 17 digits instead of 18"),
    "mul_no_cap": ({"*"}, "no width-class cap: exact min(p1+p2,38)"),
    "mul_scale_error_39": ({"*"}, "multiplication scale bind error above 39 instead of 38"),
    "mul_overflow_i64_only": ({"*", "+", "-"}, "overflow checked against i64, not against 10^P (P<=18)"),
    "mul_overflow_unchecked": ({"*"}, "no overflow check on multiplication"),
    "mod_sign_follows_divisor": ({"%"}, "modulo sign follows the divisor"),
    "mod_scale_min": ({"%"}, "modulo scale min(s1,s2)"),
    "avg_scale_plus5": ({"avg"}, "AVG scale s+5 instead of s+6"),
    "avg_no_cap38": ({"avg"}, "AVG scale min(s+6,38) without the 38-(p-s) limit"),
    "avg_round_trunc": ({"avg"}, "AVG truncates instead of rounding"),
    "avg_round_half_even": ({"avg"}, "AVG rounds ties to even"),
    "unify_prec_max": ({"unify"}, "unification precision max(p) instead of int digits + scale"),
    "round_half_even": ({"round", "cast"}, "ROUND/CAST round ties to even"),
    # ---- decision 1: + and - at the 38-digit cap
    "add_cap_exact_sum": ({"+", "-"}, "D1: + - at the cap compute the exact sum instead of erroring when an operand does not fit"),
    "add_fit_check_first_operand_only": ({"+", "-"}, "D1: only the first operand is tested for fitting the result type"),
    "sub_no_fit_check": ({"-"}, "D1: the fit test is missing for subtraction only"),
    # ---- decision 2: ROUND with n < 0
    "round_neg_prec_old_rule": ({"round"}, "D2: ROUND n<0 precision min(p1+1,38) always (the old rule)"),
    "round_neg_prec_same": ({"round"}, "D2: ROUND n<0 precision p1, never widens (replaces the old 'p1 instead of p1+1')"),
    "round_neg_widen_without_range_test": ({"round"}, "D2: widens whenever s1=0, ignoring -n <= p1"),
    "round_neg_widen_without_scale_test": ({"round"}, "D2: widens whenever -n <= p1, ignoring s1 = 0"),
    "round_neg_range_strict": ({"round"}, "D2: widens only if -n < p1 (off by one)"),
    "round_neg_no_cap38": ({"round"}, "D2: widened precision not capped at 38"),
    # ---- decision 3: modulo (tight type; bind error when aligning the operands needs more than 38 digits)
    "mod_prec_old_conservative": ({"%"}, "D3: modulo precision max(p1-s1,p2-s2)+S (the old, wider type), keeps the alignment bind error"),
    "mod_no_alignment_bind_error": ({"%"}, "D3: modulo without the alignment bind error (tight type for every pair)"),
    "mod_bind_error_threshold_39": ({"%"}, "D3: alignment bind error only above 39 digits instead of 38"),
    "mod_bind_error_threshold_37": ({"%"}, "D3: alignment bind error already above 37 digits"),
    "mod_prec_tight_minus1": ({"%"}, "D3: modulo precision one digit too narrow"),
    "mod_prec_plus1": ({"%"}, "D3: modulo precision one digit too wide (tight+1, capped at 38)"),
    # ---- decision 4: integer with integer
    "int_mul_widen_bigint": ({"i*"}, "D4: integer * integer is always BIGINT"),
    "int_mul_as_decimal": ({"i*"}, "D4: integer * integer follows the decimal reading ((10,0)*(10,0) = (18,0))"),
    "int_add_widen_one_step": ({"i+", "i-"}, "D4: integer +/- integer widens to the next integer type"),
    "int_result_narrower": ({"i+", "i-", "i*", "i%", "i//"}, "D4: the result has the narrower of the two integer types"),
    "int_overflow_wraps": ({"i+", "i-", "i*"}, "D4: integer overflow wraps (two's complement)"),
    "int_overflow_saturates": ({"i+", "i-", "i*"}, "D4: integer overflow saturates"),
    "int_div_min_by_minus1_unchecked": ({"i//"}, "D4: MIN // -1 is not an error"),
    "int_mod_min_by_minus1_returns_zero": ({"i%"}, "D4: MIN % -1 returns 0 (the earlier reading) instead of an overflow error"),
    "int_mod_by_minus1_always_overflows": ({"i%"}, "D4: every x % -1 is an overflow error"),
    "int_div_floor": ({"i//"}, "D4: // floors instead of truncating toward zero"),
    "int_mod_sign_follows_divisor": ({"i%"}, "D4: integer % sign follows the divisor"),
    "int_slash_stays_integer": ({"i/"}, "D4: integer / integer stays integer"),
    "intdec_bigint_as_18": ({"idec"}, "D4: BIGINT read as DECIMAL(18,0) in mixed arithmetic"),
}


class Ref:
    def __init__(self, mutations=()):
        self.m = frozenset(mutations)
        for x in self.m:
            assert x in MUTATIONS, x

    # ---------------------------------------------------------------- result types
    def t_add(self, p1, s1, p2, s2):
        m = self.m
        S = min(s1, s2) if "add_scale_min" in m else max(s1, s2)
        ip = (min if "add_prec_max_to_min" in m else max)(p1 - s1, p2 - s2)
        raw = ip + S + (0 if "add_prec_no_plus1" in m else 1)
        if "add_cap_cuts_scale" in m and raw > MAXP:
            S = max(0, S - (raw - MAXP))
        return min(MAXP, raw), S

    t_sub = t_add

    def t_mul(self, p1, s1, p2, s2):
        m = self.m
        S = s1 + s2
        if S > (39 if "mul_scale_error_39" in m else MAXP):
            raise BindErr("scale")
        lim = 19 if "mul_cap_threshold_19" in m else CAP64
        cap_val = 17 if "mul_cap_value_17" in m else CAP64
        sl = (S <= 18) if "mul_cap_scale_le18" in m else (S < 18)
        both = (p1 <= lim or p2 <= lim) if "mul_cap_or" in m else (p1 <= lim and p2 <= lim)
        if "mul_no_cap" not in m and both and sl:
            P = min(p1 + p2, cap_val)
        else:
            P = min(p1 + p2, MAXP)
        return min(P, MAXP), S

    def t_div(self, p1, s1, p2, s2):
        m = self.m
        extra = 5 if "div_scale_plus5" in m else 7 if "div_scale_plus7" in m else 6
        S = min(s1 + extra, 37 if "div_scale_cap37" in m else MAXP)
        P = (p1 - s1) + s2 + S + (1 if "div_prec_plus1" in m else 0)
        if P > MAXP:
            raise BindErr("precision")
        return P, S

    def t_mod(self, p1, s1, p2, s2):
        m = self.m
        S = min(s1, s2) if "mod_scale_min" in m else max(s1, s2)
        i1, i2 = p1 - s1, p2 - s2
        # decision 3: aligning both operands to scale S needs max(i1, i2) + S digits; above 38 is a bind error
        limit = 39 if "mod_bind_error_threshold_39" in m else 37 if "mod_bind_error_threshold_37" in m else MAXP
        if max(i1, i2) + S > limit and "mod_no_alignment_bind_error" not in m:
            raise BindErr("alignment")
        if "mod_prec_old_conservative" in m:
            P = max(i1, i2) + S
        else:
            P = min(i1, i2) + S
            if "mod_prec_tight_minus1" in m:
                P = max(1, S, P - 1)
            if "mod_prec_plus1" in m:
                P = min(MAXP, P + 1)
        return P, S

    def t_avg(self, p, s):
        m = self.m
        extra = 5 if "avg_scale_plus5" in m else 6
        S = s + extra if "avg_no_cap38" in m else min(s + extra, MAXP - (p - s))
        S = min(S, MAXP)
        return (p - s) + S, S

    def t_unify(self, *types):
        S = max(s for _, s in types)
        I = max(p - s for p, s in types)
        P = I + S
        if "unify_prec_max" in self.m:
            P = max(p for p, _ in types)
        if P > MAXP:
            raise BindErr("precision")
        return P, S

    def t_round(self, p, s, n):
        if n >= s:
            return p, s
        if n >= 0:
            return p, n
        m = self.m
        if "round_neg_prec_old_rule" in m:
            P = min(p + 1, MAXP)
        elif "round_neg_prec_same" in m:
            P = p
        elif "round_neg_widen_without_range_test" in m:
            P = min(p + 1, MAXP) if s == 0 else p
        elif "round_neg_widen_without_scale_test" in m:
            P = min(p + 1, MAXP) if -n <= p else p
        elif "round_neg_range_strict" in m:
            P = min(p + 1, MAXP) if (s == 0 and -n < p) else p
        elif "round_neg_no_cap38" in m:
            P = p + 1 if (s == 0 and -n <= p) else p
        else:
            P = min(p + 1, MAXP) if (s == 0 and -n <= p) else p
        return P, 0

    def t_floor(self, p, s):
        return p, 0

    def t_neg(self, p, s):
        return p, s

    def result_type(self, op, t1, t2):
        f = {"+": self.t_add, "-": self.t_sub, "*": self.t_mul, "/": self.t_div, "%": self.t_mod}[op]
        return f(t1[0], t1[1], t2[0], t2[1])

    # ---------------------------------------------------------------- overflow limit
    def limit(self, P):
        if "mul_overflow_i64_only" in self.m and P <= CAP64:
            return 2 ** 63  # |u| must be < limit
        return 10 ** P

    def _check(self, u, P):
        if abs(u) >= self.limit(P):
            raise RunErr("overflow")
        return u

    # ---------------------------------------------------------------- values
    def v_add(self, a, t1, b, t2, sign=1):
        (p1, s1), (p2, s2) = t1, t2
        P, S = self.t_add(p1, s1, p2, s2)
        Sa = max(s1, s2)  # alignment scale (mutation changes only the declared type)
        ra, rb = a * 10 ** (Sa - s1), sign * b * 10 ** (Sa - s2)
        m = self.m
        # decision 1: an operand that does not fit the result type once rescaled is an overflow,
        # even when the exact sum would fit
        if S == Sa and "add_cap_exact_sum" not in m and not (sign == -1 and "sub_no_fit_check" in m):
            lim = self.limit(P)
            if abs(ra) >= lim or (abs(rb) >= lim and "add_fit_check_first_operand_only" not in m):
                raise RunErr("overflow")
        r = ra + rb
        if S != Sa:
            r = round_div(r, 10 ** (Sa - S)) if Sa > S else r * 10 ** (S - Sa)
        return self._check(r, P)

    def v_sub(self, a, t1, b, t2):
        return self.v_add(a, t1, b, t2, sign=-1)

    def v_mul(self, a, t1, b, t2):
        P, S = self.t_mul(*t1, *t2)
        r = a * b
        if "mul_overflow_unchecked" in self.m:
            return r
        return self._check(r, P)

    def v_div(self, a, t1, b, t2):
        (p1, s1), (p2, s2) = t1, t2
        P, S = self.t_div(p1, s1, p2, s2)
        if b == 0:
            if "div_by_zero_gives_zero" in self.m:
                return 0
            raise RunErr("div0")
        mode = "away"
        if "div_round_half_even" in self.m:
            mode = "even"
        elif "div_round_half_up_posinf" in self.m:
            mode = "tie_posinf"
        elif "div_round_trunc" in self.m:
            mode = "trunc"
        # (a/10^s1) / (b/10^s2) * 10^S = a * 10^(s2 + S - s1) / b
        k = s2 + S - s1
        q = round_div(a * 10 ** k, b, mode) if k >= 0 else round_div(a, b * 10 ** (-k), mode)
        return self._check(q, P)

    def v_mod(self, a, t1, b, t2):
        (p1, s1), (p2, s2) = t1, t2
        P, S = self.t_mod(p1, s1, p2, s2)
        Sa = max(s1, s2)
        a2, b2 = a * 10 ** (Sa - s1), b * 10 ** (Sa - s2)
        if b2 == 0:
            raise RunErr("div0")
        r = abs(a2) % abs(b2)
        if "mod_sign_follows_divisor" in self.m:
            r = r if b2 > 0 else -r
        else:
            r = -r if a2 < 0 else r
        if S != Sa:
            r = round_div(r, 10 ** (Sa - S)) if Sa > S else r * 10 ** (S - Sa)
        return self._check(r, P)

    # ---------------------------------------------------------------- integer with integer (decision 4)
    def as_dec(self, t):
        """The exact-decimal reading of an integer type name; decimals pass through."""
        if isinstance(t, tuple):
            return t
        if t == "BIGINT" and "intdec_bigint_as_18" in self.m:
            return (18, 0)
        return INT_DEC[t]

    def t_int(self, op, n1, n2):
        """Result type of integer op integer: ('int', name) or ('dec', (P, S)) (only '/' is a decimal)."""
        m = self.m
        w1, w2 = INT_BITS[n1], INT_BITS[n2]
        if op == "/":
            if "int_slash_stays_integer" in m:
                return ("int", INT_NAME[max(w1, w2)])
            return ("dec", self.t_div(*INT_DEC[n1], *INT_DEC[n2]))
        if op == "*" and "int_mul_as_decimal" in m:
            return ("dec", self.t_mul(*INT_DEC[n1], *INT_DEC[n2]))
        w = max(w1, w2)
        if "int_result_narrower" in m:
            w = min(w1, w2)
        if op == "*" and "int_mul_widen_bigint" in m:
            w = 64
        if op in ("+", "-") and "int_add_widen_one_step" in m:
            w = min(64, w * 2)
        return ("int", INT_NAME[w])

    def v_int(self, op, a, n1, b, n2):
        """Value of integer op integer: the Python int for an integer result, the unscaled int for '/'."""
        m = self.m
        kind, t = self.t_int(op, n1, n2)
        if op == "/":
            if kind == "int":
                if b == 0:
                    raise RunErr("div0")
                q = abs(a) // abs(b)
                return q if (a < 0) == (b < 0) else -q
            return self.v_div(a, INT_DEC[n1], b, INT_DEC[n2])
        if op in ("%", "//") and b == 0:
            raise RunErr("div0")
        if op == "%" and b == -1 and kind == "int":
            # decision N2: MIN % -1 is an overflow error (the checked remainder overflows), like MIN // -1
            if "int_mod_by_minus1_always_overflows" in m:
                raise RunErr("overflow")
            wres = INT_BITS[t]
            if a == -(1 << (wres - 1)) and "int_mod_min_by_minus1_returns_zero" not in m:
                raise RunErr("overflow")
        if op == "+":
            r = a + b
        elif op == "-":
            r = a - b
        elif op == "*":
            r = a * b
        elif op == "%":
            r = abs(a) % abs(b)
            neg = (b < 0) if "int_mod_sign_follows_divisor" in m else (a < 0)
            r = -r if neg else r
        else:  # //
            if "int_div_floor" in m:
                r = a // b
            else:
                q = abs(a) // abs(b)
                r = q if (a < 0) == (b < 0) else -q
        if kind == "dec":
            return self._check(r, t[0])
        w = INT_BITS[t]
        lo, hi = -(1 << (w - 1)), (1 << (w - 1)) - 1
        if lo <= r <= hi:
            return r
        if op == "//" and "int_div_min_by_minus1_unchecked" in m:
            return r
        if "int_overflow_wraps" in m and op in "+-*":
            return ((r - lo) % (1 << w)) + lo
        if "int_overflow_saturates" in m and op in "+-*":
            return max(lo, min(hi, r))
        raise RunErr("overflow")

    def v_op(self, op, a, t1, b, t2):
        return {"+": self.v_add, "-": self.v_sub, "*": self.v_mul, "/": self.v_div, "%": self.v_mod}[op](
            a, t1, b, t2
        )

    def _rmode(self):
        return "even" if "round_half_even" in self.m else "away"

    def v_round(self, u, t, n):
        p, s = t
        P, S = self.t_round(p, s, n)
        if n >= s:
            return u
        if n >= 0:
            return self._check(round_div(u, 10 ** (s - n), self._rmode()), P)
        # n < 0: round to a multiple of 10^-n, scale 0
        q = round_div(u, 10 ** (s - n), self._rmode())  # in units of 10^-n
        return self._check(q * 10 ** (-n), P)

    def v_cast(self, u, t, t2):
        """DECIMAL(p,s) -> DECIMAL(p2,s2): round at s2, error if it does not fit p2."""
        (p, s), (p2, s2) = t, t2
        if s2 >= s:
            r = u * 10 ** (s2 - s)
        else:
            r = round_div(u, 10 ** (s - s2), self._rmode())
        if abs(r) >= 10 ** p2:
            raise RunErr("overflow")
        return r

    def v_floor(self, u, t):
        return u // 10 ** t[1]

    def v_ceil(self, u, t):
        return -((-u) // 10 ** t[1])

    def v_trunc(self, u, t):
        d = 10 ** t[1]
        return u // d if u >= 0 else -((-u) // d)

    def v_avg(self, us, t):
        p, s = t
        P, S = self.t_avg(p, s)
        n = len(us)
        mode = "trunc" if "avg_round_trunc" in self.m else "even" if "avg_round_half_even" in self.m else "away"
        return self._check(round_div(sum(us) * 10 ** (S - s), n, mode), P)


# Profile: "full" = all 779 types (the real verification, run on a quiet machine); "quick" = a 29-type sample that
# keeps every boundary (1,0) ... (38,38), used to check that the harness itself works in a minute or two.
# verify.py sets NATA_DV_PROFILE before it imports anything.
import os

PROFILE = os.environ.get("NATA_DV_PROFILE", "full")
ALL_TYPES = [(p, s) for p in range(1, MAXP + 1) for s in range(p + 1)]
QUICK_TYPES = sorted({
    (1, 0), (1, 1), (2, 1), (2, 2), (3, 0), (3, 2), (5, 0), (5, 2), (9, 4), (10, 0), (10, 2), (10, 9), (15, 2), (17, 8),
    (18, 0), (18, 2), (18, 18), (19, 0), (19, 9), (20, 10), (27, 13), (30, 2), (37, 36), (38, 0), (38, 2), (38, 19),
    (38, 38), (20, 20), (19, 18)})
TYPES = ALL_TYPES if PROFILE == "full" else QUICK_TYPES
NT = len(TYPES)  # 779 in the full profile
TIDX = {t: i for i, t in enumerate(TYPES)}
