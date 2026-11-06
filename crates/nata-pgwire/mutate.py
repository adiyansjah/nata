#!/usr/bin/env python3
"""Private-copy mutants: compilation failures/timeouts are INVALID, never CAUGHT.
Includes extended recovery, session compatibility, diagnostics and stalled-reader isolation.
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
SESSION = 'crates/nata-pgwire/src/session.rs'
EXTENDED = 'crates/nata-pgwire/src/extended.rs'
SQLERROR = 'crates/nata-sql/src/error.rs'
MUTANTS = {'decoder-guard-removed': ('crates/nata-pgwire/src/codec.rs', 'if n > self.0.len() {', 'if false {', 'codec'),
 'wrong-type-oid': ('crates/nata-pgwire/src/codec.rs', 'Integer => (23, 4, -1)', 'Integer => (25, 4, -1)', 'codec'),
 'null-as-empty-string': ('crates/nata-pgwire/src/codec.rs',
                          'None => b.extend((-1i32).to_be_bytes())',
                          'None => b.extend(0i32.to_be_bytes())',
                          'codec'),
 'lost-command-complete': ('crates/nata-pgwire/src/server.rs',
                           'b\'C\', format!("SELECT {n}\\0")',
                           'b\'N\', format!("SELECT {n}\\0")',
                           'protocol'),
 'rows-from-another-session': ('crates/nata-pgwire/src/server.rs',
                               'let q = sql.clone();',
                               'let q = { static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None); let mut '
                               'previous = LAST.lock().expect("lock"); previous.replace(sql.clone()).unwrap_or_else(|| '
                               'sql.clone()) };',
                               'protocol'),
 'wrong-cancel-key-accepted': ('crates/nata-pgwire/src/server.rs',
                               's.secret == secret',
                               's.secret == secret || s.secret != secret',
                               'protocol'),
 'generic-cancel-error': ('crates/nata-pgwire/src/server.rs',
                          'canceling statement due to user request',
                          'spike query failed',
                          'protocol'),
 'unbounded-bridge': ('crates/nata-pgwire/src/server.rs', 'mpsc::channel(32)', 'mpsc::channel(1_100_000)', 'protocol'),
 'max-connections-ignored': ('crates/nata-pgwire/src/server.rs',
                             'Semaphore::new(config.max_connections)',
                             'Semaphore::new(1024)',
                             'protocol'),
 'startup-cap-removed': ('crates/nata-pgwire/src/codec.rs',
                         'let max = if startup { MAX_STARTUP } else { MAX_FRAME };',
                         'let max = MAX_FRAME;',
                         'codec'),
 'error-position-in-bytes': ('crates/nata-pgwire/src/codec.rs',
                             'return i + 1;',
                             'return sql.chars().take(i).map(char::len_utf8).sum::<usize>() + 1;',
                             'codec'),
 'sync-keeps-error-state': ('crates/nata-pgwire/src/server.rs',
                            '            failed = false;',
                            '            // mutant: never clear failed',
                            'protocol'),
 'extended-ignores-error-state': ('crates/nata-pgwire/src/server.rs', '        if failed {', '        if false {', 'protocol'),
 'lost-bind-complete': ('crates/nata-pgwire/src/server.rs',
                        'codec::packet(b\'2\', b"")',
                        'codec::packet(b\'4\', b"")',
                        'protocol'),
 'lost-close-complete': ('crates/nata-pgwire/src/server.rs',
                         'codec::packet(b\'3\', b"")',
                         'codec::packet(b\'4\', b"")',
                         'protocol'),
 'lost-parameter-description': ('crates/nata-pgwire/src/server.rs',
                                "codec::packet(b't', &body)",
                                "codec::packet(b'n', &body)",
                                'protocol'),
 'portal-described-as-statement': ('crates/nata-pgwire/src/server.rs',
                                   "if target == b'S' { 2 } else { 3 }",
                                   "if target == b'S' { 2 } else { 2 }",
                                   'protocol'),
 'execute-row-limit-ignored': ('crates/nata-pgwire/src/server.rs', 'if limit != 0 {', 'if false {', 'protocol'),
 'binary-formats-accepted': ('crates/nata-pgwire/src/extended.rs',
                             'if formats.iter().any(|f| *f != 0) {',
                             'if false {',
                             'protocol'),
 'parameter-text-lost': ('crates/nata-pgwire/src/extended.rs',
                         '25 | 1043 => mk(ExprKind::Str(v.into()))',
                         '25 | 1043 => mk(ExprKind::Str(String::new()))',
                         'protocol'),
 'parameter-expansion-unbounded': ('crates/nata-pgwire/src/extended.rs',
                                   'if expansion.saturating_add(self.sql.len()) > codec::MAX_SQL {',
                                   'if false {',
                                   'protocol'),
 'parameter-result-type-drift': ('crates/nata-pgwire/src/server.rs',
                                 '!extended::same_types(e,&p)',
                                 'false && !extended::same_types(e,&p)',
                                 'protocol'),
 'session-status-lost': ('crates/nata-pgwire/src/session.rs', 'out.status.push(status);', 'let _ = status;', 'protocol'),
 'transaction-status-idle': ('crates/nata-pgwire/src/session.rs',
                             "self.transaction = b'T';",
                             "self.transaction = b'I';",
                             'protocol'),
 'generic-sql-error-message': ('crates/nata-pgwire/src/server.rs',
                               '            e.message\n',
                               '            String::from("bind error")\n',
                               'protocol'),
 'sql-error-hint-lost': ('crates/nata-pgwire/src/server.rs',
                         'if let Some(h) = &e.hint {',
                         'if let Some(h) = &None::<String> {',
                         'protocol'),
 'unknown-column-wrong-code': ('crates/nata-sql/src/error.rs', '=> "42703",', '=> "42804",', 'protocol'),
 'single-query-admission': ('crates/nata-pgwire/src/server.rs',
                            'Semaphore::new(config.max_queries)',
                            'Semaphore::new(1)',
                            'protocol'),
 'stalled-send-never-expires': ('crates/nata-pgwire/src/server.rs',
                                'tokio::time::sleep(Duration::from_secs(2))',
                                'tokio::time::sleep(Duration::from_secs(200))',
                                'protocol')}

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
