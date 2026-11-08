#!/usr/bin/env python3
"""Four focused ADR 0015 controls, serial -j4 builds in a private source copy.
Build errors, timeouts and unrelated failures are INVALID, never catches.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
FETCH = 'crates/nata-pgwire/src/server/fetch.rs'
TEST = 'admission_pressure_preserves_young_portals_and_yields_only_oldest_once'
EXEC = 'admission_pressure_never_yields_executing_work_and_keeps_five_second_wait'
MUTANTS = {
    'skip-yield': ('_ = pressure => unreachable!(),', '_ = std::future::pending::<()>() => { let _ = pressure; unreachable!() },', None),
    'yield-young': ('at + Duration::from_millis(250)', 'at + Duration::ZERO', TEST),
    'yield-two': ('if yielded {\n                    break;', 'if yielded {\n                    self.yield_oldest();\n                    break;', TEST),
    # Lie about eligibility through a separate candidate lease. Mutating the
    # real Run phase also double-suspends it on completion, aborting in Drop
    # before libtest can report the intended assertion (INVALID).
    'yield-executing': ('let idle = Arc::new(Idle { phase: Arc::clone(&self.phase), token: token.clone(), cancelled: Arc::clone(&cancelled), resume: Arc::clone(&resume), join: Arc::clone(&join) });',
        'let idle = Arc::new(Idle { phase: Arc::new(Lease::new()), token: token.clone(), cancelled: Arc::clone(&cancelled), resume: Arc::clone(&resume), join: Arc::clone(&join) }); idle.phase.enter(); idle.phase.suspend(Instant::now() - Duration::from_secs(1)); queries.publish(&idle);', EXEC),
}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--work', type=Path, required=True)
    ap.add_argument('--target', type=Path, required=True)
    ap.add_argument('--only', choices=list(MUTANTS), action='append')
    a = ap.parse_args()
    a.work = a.work.resolve(); a.work.mkdir(parents=True, exist_ok=True)
    ws = a.work/'source'
    assert not ws.exists(), 'use a fresh work directory'
    ws.mkdir()
    for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']:
        shutil.copy(ROOT/name, ws/name)
    shutil.copytree(ROOT/'crates', ws/'crates', ignore=shutil.ignore_patterns('target', '__pycache__'))
    # copy2 preserves old source mtimes: a shared target may still hold a newer
    # mutant binary. Force Cargo to validate the fresh control source.
    for source in ws.rglob('*.rs'): os.utime(source, None)
    env = dict(os.environ, CARGO_TARGET_DIR=str(a.target.resolve()))
    def run(cmd, log):
        with (a.work/log).open('w') as out:
            try:
                rc = subprocess.run(cmd, cwd=ws, env=env, stdout=out, stderr=subprocess.STDOUT, timeout=600).returncode
            except subprocess.TimeoutExpired:
                rc = 99
        return rc, (a.work/log).read_text()
    cargo = ['cargo', 'test', '--locked', '-p', 'nata-pgwire', '-j', '4', '--test', 'protocol']
    rc, _ = run(cargo+['admission_pressure', '--', '--nocapture'], 'control.log')
    assert rc == 0, 'unmutated protocol control fails'
    results = {}
    path = ws/FETCH
    original = path.read_text()
    # Run the fairness control with the ordinary binary before mutating it.
    def fairness(name):
        rc, out = run(['cargo', 'build', '--locked', '-p', 'nata', '-j', '4'], name+'-build.log')
        if rc: return 99, out
        return run([sys.executable, str(ROOT/'scripts/wire-browse/resources.py'), '--nata', str(a.target.resolve()/'debug/nata'), '--smoke', '--work', str(a.work/name)], name+'.log')
    rc, _ = fairness('control-fairness')
    assert rc == 0, 'unmutated fairness control fails'
    for name, (old, new, test) in MUTANTS.items():
        if a.only and name not in a.only: continue
        assert original.count(old) == 1, (name, 'anchor count', original.count(old))
        path.write_text(original.replace(old, new))
        try:
            rc, out = run(cargo+[test, '--', '--exact', '--nocapture'], name+'.log') if test else fairness(name)
            expected = ('test result: FAILED' in out and test in out) if test else ('query admission timeout' in out or 'admission deadline exceeded' in out)
            results[name] = 'CAUGHT' if rc not in [0,99] and expected and 'could not compile' not in out else ('MISSED' if rc == 0 else 'INVALID')
            print(name, results[name], 'rc='+str(rc), flush=True)
            if results[name] == 'INVALID': print(out[-6000:], flush=True)
        finally:
            path.write_text(original)
    rc, _ = run(cargo+['admission_pressure', '--', '--nocapture'], 'restored-protocol.log')
    assert rc == 0, 'restored protocol control fails'
    rc, _ = fairness('restored-fairness')
    assert rc == 0, 'restored fairness control fails'
    (a.work/'results.json').write_text(json.dumps(results, indent=2)+'\n')
    return int(any(v != 'CAUGHT' for v in results.values()))

if __name__ == '__main__':
    raise SystemExit(main())
