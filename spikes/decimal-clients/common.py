"""Shared test table and helpers for the decimal client probes."""
from decimal import Decimal as D

import pyarrow as pa

# Decimal(15,2): 13 integer digits max. Decimal(18,0): 18 digits max.
D64_VALUES = [None, D("0.00"), D("0.01"), D("-0.01"), D("1.00"), D("10000.00"),
              D("9999999999999.99"), D("-9999999999999.99"), D("123456789012.34")]
WIDE_VALUES = [999_999_999_999_999_999, None, -999_999_999_999_999_999, 1, 0, -1, 10_000, 100_000_000, 12_345_678_901_234_567]
assert len(D64_VALUES) == len(WIDE_VALUES)

T_D64 = pa.decimal64(15, 2)
T_D128 = pa.decimal128(15, 2)
T_WIDE = pa.decimal64(18, 0)


def make_table() -> pa.Table:
    return pa.table({
        "id": pa.array(range(len(D64_VALUES)), pa.int32()),
        "d64": pa.array(D64_VALUES, T_D64),
        "d128": pa.array(D64_VALUES, T_D128),
        "d64_wide": pa.array([None if v is None else D(v) for v in WIDE_VALUES], T_WIDE),
    })


def expected():
    return {"d64": D64_VALUES, "d128": D64_VALUES,
            "d64_wide": [None if v is None else D(v) for v in WIDE_VALUES]}


rows = []  # (probe, outcome, detail)


def probe(label, fn):
    """Run fn(); record 'ok' with the printable result or 'ERROR' with the exact exception text."""
    try:
        out = fn()
        print(f"[ok ] {label}\n      {out}", flush=True)
        rows.append((label, "ok", out))
        return out
    except BaseException as e:  # noqa: BLE001 - every failure text is data
        msg = f"{type(e).__name__}: {e}".replace("\n", " | ")
        print(f"[ERR] {label}\n      {msg[:900]}", flush=True)
        rows.append((label, "ERROR", msg[:900]))
        return None
