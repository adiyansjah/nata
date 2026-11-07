#!/usr/bin/env python3
"""Independent exact Decimal -> Python float oracle, all 779 (p,s) pairs.
Usage: decimal-oracle.py path/to/decimal-bits WORK [N=1000000]
"""
import decimal, random, struct, subprocess, sys
from pathlib import Path
exe, work = sys.argv[1:3]; n=int(sys.argv[3]) if len(sys.argv)>3 else 1_000_000
work=Path(work);work.mkdir(parents=True,exist_ok=True)
rng=random.Random(17179869189)
pairs=[(p,s) for p in range(1,39) for s in range(p+1)]
with (work/'input').open('w') as f:
    for i in range(n):
        p,s=pairs[i%len(pairs)]; cap=10**p-1
        v=([0,cap,-cap,1,-1][i//len(pairs)%5] if i<5*len(pairs) else rng.randint(-cap,cap))
        f.write(f'{v} {s}\n')
with (work/'input').open() as f, (work/'output').open('w') as out:
    subprocess.run([exe],stdin=f,stdout=out,check=True)
diffs=0
with (work/'input').open() as inp, (work/'output').open() as out:
    for i,line in enumerate(inp):
        v,s=line.split(); v=int(v);s=int(s)
        # Tuple construction is exact, independent of Decimal's context precision.
        d=decimal.Decimal((int(v<0),tuple(map(int,str(abs(v)))),-s))
        expected=struct.pack('>d',float(d)).hex(); actual=out.readline().strip()
        if expected!=actual:
            if diffs<8:print(v,s,expected,actual)
            diffs+=1
    assert out.readline()==''
print(f'seed=17179869189 pairs={len(pairs)} decimals={n} differences={diffs}')
assert diffs==0
