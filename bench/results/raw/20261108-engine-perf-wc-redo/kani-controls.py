#!/usr/bin/env python3
"""Run serially, with no other Cargo process. Mutate the real included source."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[4]
source = root/'crates/nata-kernels/src/native.rs'
original = source.read_text()
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
controls = [
    ('precision', 'v.unsigned_abs() < self.limit', 'v.unsigned_abs() <= self.limit', 'wc1_proofs::wc1_add'),
    ('selection', 'if bits == u64::MAX {', 'if true {', 'wc1_proofs::wc1_numbers_and_selection'),
    ('input-row', 'values[row as usize].into()', 'values[0].into()', 'wc1_proofs::wc1_numbers_and_selection'),
    ('product', 'self.check(x.checked_mul(y))', 'self.check(x.checked_add(y))', 'wc1_full_product::wc1_mul_full_i64'),
]
results = []
try:
    for name, before, after, harness in controls:
        assert original.count(before) == 1
        source.write_text(original.replace(before, after))
        p = subprocess.run(['timeout', '240', 'cargo', 'kani', '-Z', 'stubbing', '--exact', '--harness', harness],
                           cwd=root/'spikes/kani-nata-kernels', capture_output=True, text=True)
        (out/f'{name}.txt').write_text(p.stdout+p.stderr)
        caught = (p.returncode not in (0, 124) and 'Status: FAILURE' in p.stdout+p.stderr
                  and 'VERIFICATION:- FAILED' in p.stdout+p.stderr)
        results.append(dict(control=name,harness=harness,exit=p.returncode,caught=caught))
        assert caught, (name, p.returncode)
        source.write_text(original)
finally:
    source.write_text(original)
    (out/'controls.json').write_text(json.dumps(dict(source_sha256=hashlib.sha256(original.encode()).hexdigest(),results=results),indent=2)+'\n')
