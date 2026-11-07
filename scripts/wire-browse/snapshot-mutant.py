#!/usr/bin/env python3
"""Actual cap-removal RSS control, built in a private source tree (heavy only)."""
import argparse
from pathlib import Path
import os
import shutil
import subprocess
import time
import pyarrow as pa
import pyarrow.parquet as pq
from resources import budget,proc
ap=argparse.ArgumentParser(description=__doc__)
ap.add_argument('--work',type=Path,required=True);ap.add_argument('--nata',type=Path,required=True)
a=ap.parse_args();root=Path(__file__).resolve().parents[2];a.work.mkdir(parents=True,exist_ok=True)
fixture=a.work.resolve()/'wide.parquet'
pq.write_table(pa.table({f'column{j}':pa.array([1],type=pa.int64()) for j in range(512)}),fixture)
registry=a.work.resolve()/'oversize.tsv'
registry.write_text(''.join(f'nata\tpublic\tt{j}\t{fixture}\n' for j in range(20000)))
# Empty-catalog idle control, same executable and configured engine pool.
log=(a.work/'control.log').open('w')
base=subprocess.Popen([str(a.nata.resolve()),'serve','--listen','127.0.0.1:55439','--threads','1','--max-queries','1','--aggregate-memory-bytes',str(64<<20)],stdout=log,stderr=log)
try:
    time.sleep(.5);assert base.poll() is None
    idle=proc(base.pid)[0]
finally:base.terminate();base.wait(timeout=10);log.close()
ceiling=budget(idle,1,64<<20)
with (a.work/'bounded.log').open('w') as out:
    good=subprocess.run([str(a.nata.resolve()),'serve','--catalog',str(registry),'--listen','127.0.0.1:55439'],stdout=out,stderr=out,timeout=60)
assert good.returncode!=0,'bounded control accepted oversized snapshot'
private=a.work/'source'
private.mkdir()
for filename in ['Cargo.toml','Cargo.lock','rust-toolchain.toml']:
    shutil.copy(root/filename,private/filename)
shutil.copytree(root/'crates',private/'crates',ignore=shutil.ignore_patterns('target','__pycache__'))
p=private/'crates/nata-pgwire/src/catalog.rs'
s=p.read_text();old='pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;'
assert s.count(old)==1;p.write_text(s.replace(old,'pub const MAX_SNAPSHOT_BYTES: usize = 1usize << 40;'))
env=dict(os.environ,CARGO_TARGET_DIR=str(a.work.resolve()/'target'))
subprocess.run(['cargo','build','--locked','-p','nata','-j','4'],cwd=private,env=env,check=True)
with (a.work/'mutant.log').open('w') as out:
    mutant=subprocess.Popen([str(a.work.resolve()/'target/debug/nata'),'serve','--catalog',str(registry),'--listen','127.0.0.1:55439','--threads','1','--max-queries','1','--aggregate-memory-bytes',str(64<<20)],stdout=out,stderr=out)
    peak=0
    try:
        deadline=time.monotonic()+1200
        while time.monotonic()<deadline:
            assert mutant.poll() is None,'INVALID: mutant exited before RSS breach'
            peak=max(peak,proc(mutant.pid)[1])
            if peak>ceiling:break
            time.sleep(.02)
        assert peak>ceiling,'MISSED: snapshot-cap removal did not exceed frozen budget'
        print(f'CAUGHT actual snapshot-cap removal: idle={idle}, S=1, snapshot=16777216, wire=524288, engine=67108864, allowance=67108864, budget={ceiling}, peak={peak}')
    finally:mutant.terminate();mutant.wait(timeout=10)
