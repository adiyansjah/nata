"""Check pg_numeric.encode_numeric against a REAL PostgreSQL (the pgserver wheel, PostgreSQL 16.2) and against mutants.

    .venv-pg/bin/python verify_numeric.py          # needs pgserver + psycopg (see README)
"""
import random
import struct
import sys
from decimal import Decimal

import pgserver
import psycopg
from psycopg import pq

import pg_numeric as N

srv = pgserver.get_server("/tmp/nata-dc-pg", cleanup_mode="stop")
conn = psycopg.connect(srv.get_uri(), autocommit=True)
pgc = conn.pgconn
print("server:", conn.execute("select version()").fetchone()[0])

NUMERIC_OID = 1700


def pg_binary(text_value: str, typ="numeric") -> bytes:
    """PostgreSQL's own numeric_send output for the value written as text."""
    r = pgc.exec_params(f"SELECT $1::{typ}".encode(), [text_value.encode()], param_formats=[0], result_format=1)
    assert r.status == pq.ExecStatus.TUPLES_OK, r.error_message
    return r.get_value(0, 0)


def pg_text_from_binary(b: bytes) -> str:
    """Send OUR bytes to PostgreSQL as a binary numeric parameter (numeric_recv) and read the text it prints."""
    r = pgc.exec_params(b"SELECT $1::numeric::text", [b], param_types=[NUMERIC_OID], param_formats=[1], result_format=0)
    assert r.status == pq.ExecStatus.TUPLES_OK, r.error_message.decode()
    return r.get_value(0, 0).decode()


def cases():
    """(unscaled, scale, label)"""
    out = []
    for scale in (0, 2, 15, 18, 38):
        out += [(0, scale, f"zero scale={scale}"), (1, scale, f"one unit scale={scale}"), (-1, scale, f"minus one unit scale={scale}")]
    out += [(100, 2, "1.00"), (1, 2, "0.01"), (-1, 2, "-0.01"), (1_000_000, 2, "10000.00"), (10_000, 0, "10000"), (9_999, 0, "9999"),
            (10_001, 0, "10001"), (100_000_000, 0, "10^8"), (10**12, 0, "10^12"), (10**12, 8, "10^4 at scale 8"), (123456, 4, "12.3456 (one fractional group)"),
            (12345, 4, "1.2345"), (1, 4, "0.0001"), (1, 5, "0.00001"), (10001, 8, "0.00010001"), (50000, 4, "5.0000 (trailing zero group)"),
            (99_999_999_999_999_999, 0, "17 digits"), (10**17, 0, "10^17"),
            # precision 15 scale 2 boundaries (the test table)
            (999_999_999_999_999, 2, "9999999999999.99"), (-999_999_999_999_999, 2, "-9999999999999.99"), (12_345_678_901_234, 2, "123456789012.34"),
            # Decimal64(18,0) and (18,18) boundaries
            (999_999_999_999_999_999, 0, "18 digits max"), (-999_999_999_999_999_999, 0, "-18 digits max"), (999_999_999_999_999_999, 18, "0.999999999999999999 (scale = precision = 18)"),
            (1, 18, "1e-18"), (-999_999_999_999_999_999, 18, "-0.999999999999999999"), (123_456_789_012_345_678, 9, "9+9 digits"),
            # Decimal128(38,*) boundaries
            (10**38 - 1, 0, "38 nines scale 0"), (-(10**38 - 1), 0, "-38 nines"), (10**38 - 1, 38, "scale = precision = 38 (0.99..9)"), (1, 38, "1e-38"), (-1, 38, "-1e-38"),
            (10**38 - 1, 19, "38 digits, scale 19"), (10**37, 37, "1.0000.. scale 37"), (10**37, 0, "10^37"),
            (2**63 - 1, 0, "i64 max"), (-(2**63), 0, "i64 min"), (2**63 - 1, 18, "i64 max at scale 18 (9.22..)"), (2**127 - 1 if 2**127 - 1 < 10**38 else 10**38 - 1, 0, "i128-ish max"),
            ]
    return out


def fuzz(n, seed):
    rnd = random.Random(seed)
    out = []
    for _ in range(n):
        nd = rnd.randint(1, 38)
        scale = rnd.randint(0, 38)
        digits = "".join(rnd.choice("0123456789" if rnd.random() < .5 else "09") for _ in range(nd))   # also runs of 0 and 9
        u = int(digits) * rnd.choice((1, -1))
        out.append((u, scale, f"fuzz nd={nd} scale={scale}"))
    return out


def run_checks(enc, items, label, quiet=False):
    """Compare enc(u, scale) with PostgreSQL's numeric_send for the same value; return the list of mismatches."""
    bad = []
    for u, scale, name in items:
        text = N.text_numeric(u, scale)
        want = pg_binary(text)
        got = enc(u, scale)
        if got != want:
            bad.append((name, u, scale, text, want.hex(), got.hex()))
    if not quiet:
        print(f"{label}: {len(items) - len(bad)}/{len(items)} identical to PostgreSQL's numeric_send" + ("" if not bad else f"; first mismatch: {bad[0]}"))
    return bad


ok = True
items = cases()
# 1) boundary cases + fuzz: byte-for-byte equal to PostgreSQL's own output
b1 = run_checks(N.encode_numeric, items, "boundary cases")
b2 = run_checks(N.encode_numeric, fuzz(20000, 1), "fuzz 20000 random values (38 digits, scale 0..38, runs of 0/9)")
ok &= not b1 and not b2

# 2) PostgreSQL accepts our bytes (numeric_recv) and prints the same text, including dscale
bad = []
for u, scale, name in items + fuzz(5000, 2):
    got = pg_text_from_binary(N.encode_numeric(u, scale))
    if got != N.text_numeric(u, scale):
        bad.append((name, u, scale, got, N.text_numeric(u, scale)))
print(f"numeric_recv round trip (our bytes -> PostgreSQL -> text): {len(items) + 5000 - len(bad)}/{len(items) + 5000} equal to our text" + (f"; first mismatch {bad[0]}" if bad else ""))
ok &= not bad

# 3) independent decode of PostgreSQL's own bytes with the layout (no PG involved in the decoder)
bad = []
for u, scale, name in items + fuzz(2000, 3):
    d = N.decode_numeric(pg_binary(N.text_numeric(u, scale)))
    if d != Decimal(N.text_numeric(u, scale)) or d.as_tuple().exponent != -scale:
        bad.append((name, u, scale, str(d)))
print(f"layout decoder vs PostgreSQL bytes (value and dscale): {len(items) + 2000 - len(bad)}/{len(items) + 2000}" + (f"; first mismatch {bad[0]}" if bad else ""))
ok &= not bad

# 4) column typmod and a numeric(15,2) / numeric(18,0) / numeric(38,38) column as the server would describe it
for p, s in ((15, 2), (18, 0), (18, 18), (38, 0), (38, 38), (1, 0)):
    conn.execute("DROP TABLE IF EXISTS tm")
    conn.execute(f"CREATE TABLE tm (a numeric({p},{s}))")
    r = pgc.exec_params(b"SELECT a FROM tm", [], result_format=1)
    t = r.fmod(0)
    print(f"RowDescription typmod for numeric({p},{s}): PostgreSQL {t}, ours {N.typmod(p, s)}", "OK" if t == N.typmod(p, s) else "MISMATCH")
    ok &= t == N.typmod(p, s)
    r2 = pgc.exec_params(b"SELECT a FROM tm", [], result_format=1)
    assert r2.ftype(0) == NUMERIC_OID

# 5) a numeric(15,2) COLUMN read back in binary: dscale is the declared scale even for 0 and 10000
conn.execute("DROP TABLE IF EXISTS tm"); conn.execute("CREATE TABLE tm (id int, a numeric(15,2))")
vals = [None, (0, 2), (1, 2), (-1, 2), (100, 2), (1_000_000, 2), (999_999_999_999_999, 2), (-999_999_999_999_999, 2), (12_345_678_901_234, 2)]
for i, v in enumerate(vals):
    conn.execute("INSERT INTO tm VALUES (%s, %s)", (i, None if v is None else Decimal(N.text_numeric(*v))))
r = pgc.exec_params(b"SELECT a FROM tm ORDER BY id", [], result_format=1)
col_bad = 0
for i, v in enumerate(vals):
    got = r.get_value(i, 0)
    want = None if v is None else N.encode_numeric(*v)
    if got != want:
        col_bad += 1; print("column mismatch", i, v, got and got.hex(), want and want.hex())
print(f"numeric(15,2) column, {len(vals)} rows incl. NULL, binary result: {len(vals) - col_bad}/{len(vals)} identical")
ok &= col_bad == 0

# 6) mutants: the checks must be able to fail
def mut_weight_lt1(u, s):
    b = bytearray(N.encode_numeric(u, s)); nd, w = struct.unpack("!hh", b[:4])
    if w < 0 and nd:
        struct.pack_into("!h", b, 2, w + 1)
    return bytes(b)
def mut_no_trailing_strip(u, s):
    nd, w, sg, ds = struct.unpack("!hhHh", N.encode_numeric(u, s)[:8]); g = list(struct.unpack(f"!{nd}h", N.encode_numeric(u, s)[8:]))
    # pad one trailing zero group when the fractional part has room
    return struct.pack("!hhHh", nd + 1, w, sg, ds) + struct.pack(f"!{nd + 1}h", *g, 0) if nd else N.encode_numeric(u, s)
def mut_frac_from_right(u, s):
    # align the fractional digits to the right instead of the left (a classic off-by-padding bug)
    if s % 4 == 0 or u == 0:
        return N.encode_numeric(u, s)
    sign = N.NEG if u < 0 else 0; t = str(abs(u)).rjust(s + 1, "0"); ip, fp = t[:len(t) - s], t[len(t) - s:]
    ip = ip.rjust(-(-len(ip) // 4) * 4, "0"); fp = fp.rjust(-(-len(fp) // 4) * 4, "0")
    g = [int(ip[i:i + 4]) for i in range(0, len(ip), 4)] + [int(fp[i:i + 4]) for i in range(0, len(fp), 4)]; w = len(ip) // 4 - 1
    while g and g[0] == 0: g.pop(0); w -= 1
    while g and g[-1] == 0: g.pop()
    return struct.pack("!hhHh", len(g), w, sign, s) + struct.pack(f"!{len(g)}h", *g)
def mut_dscale_minimal(u, s):
    b = bytearray(N.encode_numeric(u, s)); nd = struct.unpack("!h", b[:2])[0]; w = struct.unpack("!h", b[2:4])[0]
    struct.pack_into("!h", b, 6, max(0, (nd - w - 1) * 4)); return bytes(b)   # dscale from the digits: loses trailing zeros like 1.50
def mut_neg_sign(u, s):
    b = bytearray(N.encode_numeric(u, s));
    if u < 0: struct.pack_into("!H", b, 4, 0x8000)
    return bytes(b)
def mut_zero_weight(u, s):
    b = bytearray(N.encode_numeric(u, s))
    if u == 0: struct.pack_into("!h", b, 2, -1)
    return bytes(b)
mutants = {"weight not decremented for values < 1": mut_weight_lt1, "extra trailing zero digit kept": mut_no_trailing_strip, "fraction groups aligned right": mut_frac_from_right,
           "dscale derived from digits (drops trailing zeros)": mut_dscale_minimal, "negative sign 0x8000": mut_neg_sign, "zero with weight -1": mut_zero_weight}
survivors = []
corpus = items + fuzz(2000, 4)
for name, m in mutants.items():
    bad = run_checks(m, corpus, name, quiet=True)
    print(f"mutant '{name}': caught ({len(bad)} of {len(corpus)} cases differ from PostgreSQL)" if bad else f"mutant '{name}': SURVIVED")
    if not bad: survivors.append(name)
ok &= not survivors
print("RESULT:", "all checks passed, all mutants caught" if ok else "FAILED")
sys.exit(0 if ok else 1)
