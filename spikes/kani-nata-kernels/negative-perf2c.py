#!/usr/bin/env python3
"""Run admission-weakenings against the real included source; always restore."""
from pathlib import Path
import subprocess, os, json
root=Path(__file__).resolve().parents[2]; os.chdir(root); raw=root/'bench/results/raw/20261108-hash-aggregate-perf2c'
cases=[
 ('precision-plus-one','bench/results/raw/20261108-hash-aggregate-perf2c/retry-product-decimal.rs','<= u16::from(p)','<= u16::from(p).saturating_add(1)','perf2c_precision'),
 ('drop-scan','crates/nata-kernels/src/decimal.rs','ma < la && mb < lb','true','perf2c_scan'),
 ('aligned-no-spare-digit','crates/nata-kernels/src/decimal.rs','d.checked_sub(1)','d.checked_sub(0)','perf2c_aligned_precision'),
 ('envelope-off-by-one','crates/nata-kernels/src/sum_admission.rs','|v| v < (1u128 << 127)','|v| v <= (1u128 << 127)','perf2c_sum_envelope'),
]
results=[]
for name,file,before,after,harness in cases:
 p=root/file
 assert p.exists(), p
 original=p.read_text(); assert before in original,(name,before)
 try:
  p.write_text(original.replace(before,after))
  cmd=['timeout','180','cargo','kani','--manifest-path','spikes/kani-nata-kernels/Cargo.toml','--harness',harness]
  env=dict(os.environ,CARGO_BUILD_JOBS='4',CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR',str(root/'target/perf2c-kani')))
  r=subprocess.run(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  (raw/f'retry-negative-{name}.txt').write_text(r.stdout)
  caught=r.returncode not in (0,124) and 'VERIFICATION:- FAILED' in r.stdout
  results.append(dict(name=name,command=cmd,exit=r.returncode,caught=caught))
 finally: p.write_text(original)
(raw/'retry-negative-outcomes.json').write_text(json.dumps(results,indent=2)+'\n')
assert all(r['caught'] for r in results),results
