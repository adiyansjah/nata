#!/usr/bin/env python3
"""Private-copy mutants: compilation failures/timeouts are INVALID, never CAUGHT.
Step 3 must add the extended Sync-does-not-clear-error-state mutant.
"""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
CODEC = 'crates/nata-pgwire/src/codec.rs'
SERVER = 'crates/nata-pgwire/src/server.rs'
MUTANTS = {
    'decoder-guard-removed': (CODEC, 'if n > self.0.len() {', 'if false {', 'codec'),
    'wrong-type-oid': (CODEC, 'Integer => (23, 4, -1)', 'Integer => (25, 4, -1)', 'codec'),
    'null-as-empty-string': (CODEC, 'None => b.extend((-1i32).to_be_bytes())', 'None => b.extend(0i32.to_be_bytes())', 'codec'),
    'lost-command-complete': (SERVER, "b'C', format!(", "b'N', format!(", 'protocol'),
    'rows-from-another-session': (SERVER, 'let q = sql.clone();', 'let q = { static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None); let mut previous = LAST.lock().expect("lock"); previous.replace(sql.clone()).unwrap_or_else(|| sql.clone()) };', 'protocol'),
    'wrong-cancel-key-accepted': (SERVER, 's.secret == secret', 's.secret == secret || s.secret != secret', 'protocol'),
    'generic-cancel-error': (SERVER, 'canceling statement due to user request', 'spike query failed', 'protocol'),
    'unbounded-bridge': (SERVER, 'mpsc::channel(32)', 'mpsc::channel(1_100_000)', 'protocol'),
    'max-connections-ignored': (SERVER, 'Semaphore::new(config.max_connections)', 'Semaphore::new(1024)', 'protocol'),
    'startup-cap-removed': (CODEC, 'let max = if startup { MAX_STARTUP } else { MAX_FRAME };', 'let max = MAX_FRAME;', 'codec'),
    'error-position-in-bytes': (CODEC, 'return i + 1;', 'return sql.chars().take(i).map(char::len_utf8).sum::<usize>() + 1;', 'codec'),

}


def fingerprint():
    h = hashlib.sha256()
    for p in sorted((ROOT/'crates').rglob('*.rs')):
        h.update(p.read_bytes())
    return h.digest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('names', nargs='*')
    ap.add_argument('--work', type=Path, default=ROOT/'target/mutwork-pgwire')
    a = ap.parse_args()
    before = fingerprint()
    ws = a.work/'ws'
    shutil.rmtree(ws, ignore_errors=True)
    ws.mkdir(parents=True)
    for f in ['Cargo.toml', 'Cargo.lock']:
        shutil.copy(ROOT/f, ws/f)
    shutil.copytree(ROOT/'crates', ws/'crates', ignore=shutil.ignore_patterns('target', '__pycache__'), copy_function=shutil.copy)
    env = dict(os.environ, CARGO_TARGET_DIR=str(a.work/'target'), CARGO_TERM_COLOR='never')
    def test(suite):
        try:
            p = subprocess.run(['cargo','test','-p','nata-pgwire','--test',suite], cwd=ws, env=env, capture_output=True, text=True, timeout=1200)
            return p.returncode, p.stdout+p.stderr
        except subprocess.TimeoutExpired:
            return 99, 'timeout'
    names = a.names or list(MUTANTS)
    for suite in sorted({MUTANTS[n][3] for n in names}):
        rc, out = test(suite)
        if rc:
            print('unmutated copy FAILS\n'+out[-4000:]); return 2
        print('unmutated '+suite+': PASS', flush=True)
    bad = 0
    for name in names:
        file, old, new, suite = MUTANTS[name]
        p = ws/file
        original = p.read_text()
        if original.count(old) != 1:
            status, out = 'INVALID', 'edit does not apply exactly once'
        else:
            p.write_text(original.replace(old,new))
            rc, out = test(suite)
            status = 'SURVIVED' if rc == 0 else ('CAUGHT' if rc != 99 and ('test result: FAILED' in out or 'process didn\'t exit successfully' in out) and 'could not compile' not in out else 'INVALID')
        p.write_text(original)
        (a.work/(name+'.log')).write_text(out)
        print(name+': '+status, flush=True)
        bad += status != 'CAUGHT'
    assert fingerprint() == before, 'real sources changed'
    return int(bad > 0)

if __name__ == '__main__':
    raise SystemExit(main())
