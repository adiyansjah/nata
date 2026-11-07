#!/usr/bin/env python3
"""Independent arbitrary-precision integer/rational oracle. No floating point.
Regenerate: python3 crates/nata-kernels/tests/aggregate_oracle.py > crates/nata-kernels/tests/fixtures/aggregate.txt
"""
import random

rng = random.Random(0x000520261107)
print('# p s comma-separated unscaled inputs | exact sum | rounded unscaled AVG')
for p in range(1, 39):
    for s in sorted({0, p // 2, p}):
        limit = 10**p - 1
        for values in ([limit, limit, -limit], [-limit, -limit, limit],
                       [rng.randint(-limit, limit) for _ in range(7)]):
            total = sum(values)
            scale = min(s + 6, 38 - (p - s))
            numerator = total * 10**(scale - s)
            q, r = divmod(abs(numerator), len(values))
            if 2*r >= len(values):
                q += 1
            rounded = q if numerator >= 0 else -q
            print(p, s, ','.join(map(str, values)), '|', total, '|', rounded)
# Exact +/- halfway cases when scale cannot grow (p=38).
for sign in [1, -1]:
    for value in [1, 3, 5, 7]:
        values = [sign*value, 0]
        q = sign*((value+1)//2)
        print(38, 0, ','.join(map(str, values)), '|', sum(values), '|', q)
