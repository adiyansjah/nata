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
MUTANTS = {
 'aggregate-numeric-wrong-oid': (CODEC, 'Decimal { precision, scale } => (\n            1700,', 'Decimal { precision, scale } => (\n            25,', 'protocol'),
 'aggregate-cancel-token-lost': (SERVER, 'self.token.cancel();', '// lost executor cancellation', 'protocol'),
 'aggregate-admission-pool-multiplied': (SERVER, 'Some(pool / config.max_queries)', 'Some(pool)', 'protocol'),
 'date-epoch-off': ('crates/nata-pgwire/src/date.rs','const EPOCH: i64 = 10_957;', 'const EPOCH: i64 = 10_958;', 'protocol'),
 'date-range-unchecked': ('crates/nata-pgwire/src/date.rs','if !(MIN..END).contains(&days) {','if false {','protocol'),
 'date-bc-spelling': ('crates/nata-pgwire/src/date.rs','format!("{:04}-{m:02}-{d:02} BC", 1 - y)','format!("{:04}-{m:02}-{d:02} (BC)", 1 - y)','protocol'),
 'date-wrong-sqlstate': ('crates/nata-pgwire/src/server.rs','if message.starts_with("date outside PostgreSQL range")','if false','protocol'),
'decoder-guard-removed': ('crates/nata-pgwire/src/codec.rs', 'if n > self.0.len() {', 'if false {', 'codec'),
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
 'unbounded-bridge': ('crates/nata-pgwire/src/server.rs', 'mpsc::channel(BRIDGE_BUFFERS)', 'mpsc::channel(1_100_000)', 'protocol'),
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
 'execute-row-limit-ignored': ('crates/nata-pgwire/src/server/fetch.rs', 'n == 1', 'false', 'protocol'),
 'binary-formats-accepted': ('crates/nata-pgwire/src/extended.rs',
                             'if formats.iter().any(|f| !matches!(*f, 0 | 1)) {',
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
                                'protocol'),
 'boolean-text-case-sensitive': ('crates/nata-pgwire/src/extended.rs',
                                 '16 => match v.to_ascii_lowercase().as_str() {',
                                 '16 => match v {',
                                 'protocol')}

MUTANTS.update({
 'rowless-limit-refused': (SERVER, 'if column_count(&p.statement.description) != 0 {', 'if true {', 'protocol'),
 'session-cancellation-disabled': (SERVER, '            a.cancel();', '            let _ = a;', 'reliability'),
 'unbounded-bridge-slow-reader': (SERVER, 'mpsc::channel(BRIDGE_BUFFERS)', 'mpsc::channel(1_100_000)', 'reliability'),
 'live-decoder-guard-removed': (CODEC, 'if n > self.0.len() {', 'if false {', 'reliability'),
 'session-registry-leaked-after-kill': (SERVER, 'registry.remove(&self.id);', 'registry.get(&self.id);', 'reliability'),
 'named-statement-not-stored': (SERVER, 'self.map.insert(name, value).is_none()', 'name.is_empty() && self.map.insert(name, value).is_none()', 'protocol'),
 'named-overwrite-allowed': (SERVER, '!name.is_empty() && self.statements.get(name).is_some()', 'false && !name.is_empty() && self.statements.get(name).is_some()', 'protocol'),
 'portal-survives-sync': (SERVER, "if state.transaction == b'I' {\n                objects.portals.clear();", "if false {\n                objects.portals.clear();", 'protocol'),
 'statement-count-cap-removed': (SERVER, 'self.statements.map.len() >= 256', 'false', 'protocol'),
 'statement-byte-cap-removed': (SERVER, 'bytes + name.len() + sql.len() > 4 * codec::MAX_SQL', 'false', 'protocol'),
 'portal-byte-cap-removed': (SERVER, 'bytes + name.len() + size(portal) > 16 * codec::MAX_SQL', 'false', 'protocol'),
 'deallocate-does-not-clear': (SERVER, 'objects.statements.clear();', 'objects.statements.remove("nonexistent");', 'protocol'),
 'binary-integer-wrong-endian': ('crates/nata-pgwire/src/binary.rs', '23 => i32::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)', '23 => i32::from_le_bytes(b.as_slice().try_into().map_err(|_| invalid())?)', 'protocol'),
 'binary-date-wrong-epoch': ('crates/nata-pgwire/src/date.rs', 'const EPOCH: i64 = 10_957;', 'const EPOCH: i64 = 0;', 'protocol'),
 'admission-not-released-after-kill': (SERVER, 'let _permit = permit;', 'let _permit = permit; std::mem::forget(_permit);', 'reliability'),
})

MUTANTS.update({
 'per-row-write-restored': (SERVER, '                rows += 1;', '                if !output.is_empty() { send_buffer(std::mem::replace(&mut output, Vec::with_capacity(OUTPUT_BYTES)))?; }\n                rows += 1;', 'protocol'),
 'bridge-byte-bound-removed': (SERVER, 'const OUTPUT_BYTES: usize = 64 * 1024;', 'const OUTPUT_BYTES: usize = 64 * 1024 * 1024;', 'protocol'),
 'nodelay-not-set': (SERVER, 'socket.set_nodelay(true)?;', 'socket.set_nodelay(false)?;', 'protocol'),
 'catalog-wrong-oid': (SESSION, '(23, "int4", "int4recv")', '(24, "int4", "int4recv")', 'protocol'),
 'partial-row-cancel-appends-error': (SERVER, 'self.used == 0 && self.remaining == 0', 'true', 'protocol'),
 'decimal-leading-zero-lost': (SERVER, 'format!("0{s}")', 's', 'protocol'),
})

MUTANTS.update({
 'copy-wrong-signature': ('crates/nata-pgwire/src/copy.rs', 'PGCOPY', 'PGCOPX', 'protocol'),
 'copy-missing-trailer': (SERVER, 'tail.extend_from_slice(crate::copy::TRAILER);', 'tail.extend_from_slice(b"");', 'protocol'),
 'copy-null-length-zero': (CODEC, 'None => b.extend((-1i32).to_be_bytes())', 'None => b.extend(0i32.to_be_bytes())', 'protocol'),
 'numeric-weight-plus-one': ('crates/nata-pgwire/src/binary.rs', 'let mut weight = (whole.len().div_ceil(4) as i16) - 1;', 'let mut weight = whole.len().div_ceil(4) as i16;', 'protocol'),
 'copy-date-epoch-1970': ('crates/nata-pgwire/src/date.rs', 'const EPOCH: i64 = 10_957;', 'const EPOCH: i64 = 0;', 'protocol'),
 'copy-without-backpressure': (SERVER, 'mpsc::channel(BRIDGE_BUFFERS)', 'mpsc::channel(if copying { 1_100_000 } else { BRIDGE_BUFFERS })', 'protocol'),
 'float-threshold-plus-one': ('crates/nata-pgwire/src/float.rs', 'e >= precision', 'e > precision', 'protocol'),
 'float-trailing-dot-zero': ('crates/nata-pgwire/src/float.rs', 'out.push_str(&"0".repeat(n - digits.len()));', 'out.push_str(&"0".repeat(n - digits.len())); out.push_str(".0");', 'protocol'),
 'float-closed-midpoint': ('crates/nata-pgwire/src/float.rs', '!midpoint(d, q, lower, e2) && !midpoint(d, q, upper, e2)', 'true || (!midpoint(d, q, lower, e2) && !midpoint(d, q, upper, e2))', 'protocol'),
})

MUTANTS.update({
 'information-schema-column-missing': ('crates/nata-pgwire/src/metadata.rs', 'for (i, c) in t.source.columns.iter().enumerate() {', 'for (i, c) in t.source.columns.iter().enumerate().skip(1) {', 'lib'),
 'refused-catalog-answers-rows': (SESSION, 'return Err(WireError::unsupported(\n            "catalog query outside the frozen browsing and ADBC bootstrap shapes",\n        ));', 'out.rows.push(vec![Some("invented".into())]); return Ok(Some(out));', 'protocol'),
 'statement-timeout-restart-per-execute': (SERVER, '// Suspension preserves the statement deadline until Sync.', 'if result.is_ok() { statement_deadline = None; }', 'protocol'),
 'portal-idle-wake-missing': ('crates/nata-pgwire/src/server/fetch.rs', 'let _ = resume.send(0);', 'let _ = resume;', 'lib'),
 'exact-end-suspension-missing': ('crates/nata-pgwire/src/server/fetch.rs', "codec::packet(b's', b\"\")", "codec::packet(b'C', b\"SELECT 0\\0\")", 'protocol'),
 'statement-timeout-not-enforced': ('crates/nata-pgwire/src/session.rs', '(ms != 0).then(', 'false.then(', 'protocol'),
 'refused-catalog-silent-empty': (SESSION, 'return Err(WireError::unsupported(\n            "catalog query outside the frozen browsing and ADBC bootstrap shapes",\n        ));', 'return Ok(Some(out));', 'protocol'),
 'cancel-applied-while-suspended': ('crates/nata-pgwire/src/server/fetch.rs', '.take(); // suspension transition', '.clone(); // suspension transition', 'protocol'),
})

MUTANTS.update({
 'browse-wrong-attribute-oid': ('crates/nata-pgwire/src/browse.rs', 'let (oid, len, modif) = crate::codec::type_info(ty);', 'let (_, len, modif) = crate::codec::type_info(ty); let oid = 24;', 'lib'),
 'browse-column-missing': ('crates/nata-pgwire/src/browse.rs', 'for (i, c) in t.source.columns.iter().enumerate() {\n                        let ty = column_type(t, i)?;', 'for (i, c) in t.source.columns.iter().enumerate().skip(1) {\n                        let ty = column_type(t, i)?;', 'lib'),
 'browse-wrong-column-order': ('crates/nata-pgwire/src/browse.rs', 'ordered.sort_by_key(|t| (&t.schema, &t.name));', 'ordered.sort_by_key(|t| std::cmp::Reverse((&t.schema, &t.name)));', 'lib'),
})

MUTANTS.update({
 'settings-name-array-wrong-element': ('crates/nata-pgwire/src/binary.rs', 'for n in [dims, 0, 19]', 'for n in [dims, 0, 25]', 'lib'),
 'settings-reset-loses-startup': ('crates/nata-pgwire/src/session/helpers.rs', '.reset_settings\n            .get(key)', '.settings\n            .get(key)', 'lib'),
 'settings-missing-ok-invented-empty': ('crates/nata-pgwire/src/session/helpers.rs', 'else if missing_ok {\n                        None', 'else if missing_ok {\n                        Some(String::new())', 'lib'),
 'settings-timeout-display-units': ('crates/nata-pgwire/src/session/helpers.rs', 'if key == "statement_timeout" {', 'if false {', 'lib'),
 'settings-datestyle-ignores-dmy': (SESSION, 'l.contains("dmy")', 'false', 'lib'),
 'show-describe-loses-column': ('crates/nata-pgwire/src/session/helpers.rs', 'let selected = [3, 6, 5, 4, 7, 8];', 'let selected = [3, 6, 5, 4, 8, 7];', 'lib'),
 'settings-pg-settings-wrong-value': ('crates/nata-pgwire/src/session/helpers.rs', 'Some(value.clone()),', 'Some("invented".into()),', 'lib'),
})

def fingerprint():
    h = hashlib.sha256()
    for p in sorted((ROOT/'crates').rglob('*.rs')):
        h.update(p.read_bytes())
    return h.digest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('names', nargs='*')
    ap.add_argument('--filter', help='Run one focused test control; omitted for the full required sweep')
    ap.add_argument('--work', type=Path, default=ROOT/'target/mutwork-pgwire')
    a = ap.parse_args()
    a.work = a.work.resolve()
    before = fingerprint()
    ws = a.work/'ws'
    shutil.rmtree(ws, ignore_errors=True)
    ws.mkdir(parents=True)
    for f in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml']:
        shutil.copy(ROOT/f, ws/f)
    shutil.copytree(ROOT/'crates', ws/'crates', ignore=shutil.ignore_patterns('target', '__pycache__'), copy_function=shutil.copy)
    env = dict(os.environ, CARGO_TARGET_DIR=str(a.work/'target'), CARGO_TERM_COLOR='never')
    def test(suite):
        try:
            p = subprocess.run(['cargo','test','--locked','-j','4','-p','nata-pgwire']+(['--lib'] if suite == 'lib' else ['--test',suite])+([a.filter] if a.filter else []), cwd=ws, env=env, capture_output=True, text=True, timeout=1200)
            return p.returncode, p.stdout+p.stderr
        except subprocess.TimeoutExpired:
            return 99, 'timeout'
    names = a.names or list(MUTANTS)
    for suite in sorted({MUTANTS[n][3] for n in names}):
        rc, out = test(suite)
        if rc:
            (a.work/('unmutated-'+suite+'.log')).write_text(out)
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
