"""PostgreSQL `numeric` wire encoders for an unscaled integer + scale (what an Arrow Decimal64/Decimal128 value is).

Layout of the binary format (PostgreSQL 16 src/backend/utils/adt/numeric.c, numeric_send / numeric_recv):
    int16  ndigits     number of base-10000 digits that follow
    int16  weight      power of 10000 of the FIRST digit (value = sum digits[i] * 10000^(weight - i)); weight < 0 allowed
    uint16 sign        0x0000 positive, 0x4000 negative, 0xC000 NaN, 0xD000 +Inf, 0xF000 -Inf  (Inf: PG 14+)
    int16  dscale      decimal digits after the point to DISPLAY (so 1.50 and 1.5 differ), 0..16383
    int16  digits[ndigits]  each 0..9999, big-endian on the wire (network order)
PostgreSQL stores no leading or trailing zero digits; zero is ndigits=0, weight=0, sign=0, dscale kept.
typmod in RowDescription for numeric(p,s): ((p << 16) | (s & 0x7ff)) + 4   (make_numeric_typmod)
"""
import struct
from decimal import Decimal

NBASE = 10000
POS, NEG = 0x0000, 0x4000


def typmod(precision: int, scale: int) -> int:
    return ((precision << 16) | (scale & 0x7FF)) + 4


def encode_numeric(unscaled: int, scale: int) -> bytes:
    """Binary `numeric` for value = unscaled / 10**scale, scale >= 0, dscale = scale."""
    assert scale >= 0
    sign = NEG if unscaled < 0 else POS
    n = abs(unscaled)
    if n == 0:
        return struct.pack("!hhHh", 0, 0, POS, scale)
    s = str(n).rjust(scale + 1, "0")            # at least one integer digit
    ip, fp = s[: len(s) - scale], s[len(s) - scale:]
    ip = ip.rjust(-(-len(ip) // 4) * 4, "0")    # integer part: groups of 4 aligned at the decimal point (from the right)
    fp = fp.ljust(-(-len(fp) // 4) * 4, "0")    # fractional part: groups of 4 aligned at the decimal point (from the left)
    groups = [int(ip[i:i + 4]) for i in range(0, len(ip), 4)] + [int(fp[i:i + 4]) for i in range(0, len(fp), 4)]
    weight = len(ip) // 4 - 1
    while groups and groups[0] == 0:            # PostgreSQL never stores leading zero digits
        groups.pop(0)
        weight -= 1
    while groups and groups[-1] == 0:           # ... nor trailing zero digits (dscale carries them)
        groups.pop()
    return struct.pack("!hhHh", len(groups), weight, sign, scale) + struct.pack(f"!{len(groups)}h", *groups)


def decode_numeric(b: bytes) -> Decimal:
    """Independent decoder (straight from the layout, exact integer arithmetic) used to check the encoder without PostgreSQL."""
    nd, weight, sign, dscale = struct.unpack("!hhHh", b[:8])
    assert sign in (POS, NEG), f"special value {sign:#x}"
    digits = struct.unpack(f"!{nd}h", b[8:8 + 2 * nd])
    assert len(b) == 8 + 2 * nd and all(0 <= d < NBASE for d in digits)
    # value * 10^dscale = sum digits[i] * 10^(4*(weight-i) + dscale); all exponents must be >= 0 for a value that fits dscale
    unscaled = 0
    for i, d in enumerate(digits):
        e = 4 * (weight - i) + dscale
        assert e >= 0 or d == 0 or (d % 10 ** -e == 0), "digits hidden by dscale"
        unscaled += d * 10 ** e if e >= 0 else d // 10 ** -e
    return Decimal((1 if sign == NEG else 0, tuple(int(c) for c in str(unscaled)), -dscale))


def text_numeric(unscaled: int, scale: int) -> str:
    """Text form (what numeric_out prints): exactly `scale` decimals, no exponent, '-' only for non-zero."""
    n = abs(unscaled)
    s = str(n).rjust(scale + 1, "0")
    out = s[: len(s) - scale] + ("." + s[len(s) - scale:] if scale else "")
    return ("-" if unscaled < 0 else "") + out
