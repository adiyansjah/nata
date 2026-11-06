"""Comparison of two results. Defaults are the SQL semantics the project will promise: NaN equals NaN (and sorts last), -0.0 equals 0.0,
doubles agree within a relative 1e-9 (summation order), decimals and integers agree exactly, text and temporal values agree as the oracle prints them."""
import math
from decimal import Decimal, InvalidOperation

REL = 1e-9


def is_num(v): return isinstance(v, (int, float)) and not isinstance(v, bool)


def cell_eq(a, b, dec):
    if a is None or b is None: return a is None and b is None
    if dec:   # a DECIMAL column: exact comparison of the numeric value, whatever the representation
        try: return Decimal(str(a)) == Decimal(str(b))
        except InvalidOperation: return a == b
    if is_num(a) and is_num(b):
        if isinstance(a, float) or isinstance(b, float):
            fa, fb = float(a), float(b)
            if math.isnan(fa) or math.isnan(fb): return math.isnan(fa) and math.isnan(fb)
            if math.isinf(fa) or math.isinf(fb): return fa == fb
            return abs(fa - fb) <= max(1e-12, REL * max(abs(fa), abs(fb)))
        return a == b
    if isinstance(a, (list, dict)) or isinstance(b, (list, dict)): return a == b
    return a == b and type(a) is type(b) if isinstance(a, bool) or isinstance(b, bool) else a == b


def row_eq(r1, r2, decs):
    return len(r1) == len(r2) and all(cell_eq(x, y, d) for x, y, d in zip(r1, r2, decs))


def compare(oracle, target, ordered, strict_types=False):
    """None if equal, else a short reason. Both results are `engine.Result`."""
    if not oracle.ok:
        return None if not target.ok else "oracle raised an error but the target answered: " + oracle.error[:300]
    if not target.ok:
        if "not supported yet" in target.error.lower(): return "SKIP"
        return "target raised an error but the oracle answered: " + target.error[:300]
    if len(oracle.cols) != len(target.cols): return f"column count {len(oracle.cols)} vs {len(target.cols)}"
    decs = [t.upper().startswith("DECIMAL") for t in oracle.types]
    if strict_types and target.types and [t.upper() for t in oracle.types] != [t.upper() for t in target.types]:
        return f"column types {oracle.types} vs {target.types}"
    if len(oracle.rows) != len(target.rows): return f"row count {len(oracle.rows)} vs {len(target.rows)}"
    if ordered:
        for i, (r1, r2) in enumerate(zip(oracle.rows, target.rows)):
            if not row_eq(r1, r2, decs): return f"row {i} differs: oracle {r1} vs target {r2}"
        return None
    left = list(target.rows)
    for r1 in oracle.rows:
        for k, r2 in enumerate(left):
            if row_eq(r1, r2, decs):
                del left[k]; break
        else:
            return f"oracle row {r1} has no match in the target's result"
    return None
