#!/usr/bin/env python3
"""Failing byte-oracle and native same-run attribution controls (no timing verdict)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from bench_verdict import REQUIRED_STAGES, reconcile_stats


def check(binary, data, out=None):
    evidence = []
    if out is not None:
        out.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    for key in ('AI_AGENT', 'CODEX_CI', 'NATA_QUERY_STATS', 'NATA_STATS_JSON', 'NATA_QUERY_TRACE'):
        env.pop(key, None)
    with tempfile.TemporaryDirectory() as temporary:
        sidecar = Path(temporary)/'stats.json'
        for query in ('having_overflow', 'limit_overflow', 'q1'):
            sql = (root/f'check/queries/aggregate/{query}.sql').read_text().replace('{data}', str(data.resolve()))
            for spill in (False, True):
                # Q1 has no ORDER BY; parallel resident partition delivery is
                # already unordered with stats off. Use a serial success-byte
                # control; overflow diagnostics keep the parallel execution.
                threads = '1' if query == 'q1' else '2'
                argv = [str(binary.resolve()), 'sql', '--csv', '--threads', threads, '--command='+sql]
                if 'overflow' in query:
                    argv += ['--report']
                if spill:
                    argv += ['--force-aggregate-spill', '--aggregate-memory-bytes', str(16 << 20)]
                off = subprocess.run(argv, env=env, capture_output=True)
                sidecar.unlink(missing_ok=True)
                on = subprocess.run(argv, env=dict(env, NATA_STATS_JSON=str(sidecar), NATA_QUERY_TRACE='1' if spill else '0'), capture_output=True)
                assert (off.returncode, off.stdout, off.stderr) == (on.returncode, on.stdout, on.stderr), (query, spill, 'OUTPUT CHANGED')
                assert on.returncode == (1 if 'overflow' in query else 0), (query, on.stderr)
                record = json.loads(sidecar.read_text())
                assert set(REQUIRED_STAGES) <= record['stages'].keys()
                assert record['query_id'] > 0
                evidence.append(dict(query=query,spill=spill,exit=on.returncode,
                                     stdout_sha256=hashlib.sha256(on.stdout).hexdigest(),
                                     stderr_sha256=hashlib.sha256(on.stderr).hexdigest()))
                if out is not None:
                    (out/f'{query}-{spill}.stats.json').write_text(json.dumps(record)+'\n')
                if on.returncode == 0:
                    assert not reconcile_stats(record), reconcile_stats(record)
                    for stage in ('scan_decode', 'filter', 'expression', 'group_id', 'state_update',
                                  'merge', 'finalize', 'output_sink', 'teardown', 'bind_metadata'):
                        assert record['stages'][stage]['calls'] > 0, stage
                    counters = dict(zip(record['counter_names'], record['counter_values']))
                    assert counters['grants'] > 0 and counters['grant_bytes'] > 0
                    assert record['admission']['tasks'] > 0
                    assert record['scheduler'][0] > 0 and record['scheduler'][1] > 0
                    if spill:
                        for stage in ('spill_write', 'spill_read'):
                            assert record['stages'][stage]['calls'] > 0, stage
                        assert record['io'][0][0] > 0 and record['io'][1][0] > 0
                        assert sum(level[2] for level in record['spill_levels']) > 0
                print('PASS', query, 'spill' if spill else 'normal', 'stdout/stderr identical, separate record')
        for sql in ("SELECT FROM", "SELECT nope FROM '"+str(data.resolve()/'lineitem.parquet')+"'"):
            argv = [str(binary.resolve()), 'sql', '--command='+sql]
            off = subprocess.run(argv, env=env, capture_output=True)
            sidecar.unlink(missing_ok=True)
            on = subprocess.run(argv, env=dict(env, NATA_STATS_JSON=str(sidecar)), capture_output=True)
            assert (off.returncode, off.stdout, off.stderr) == (on.returncode, on.stdout, on.stderr)
            assert on.returncode == 2
            assert json.loads(sidecar.read_text())['stages']['bind_metadata']['calls'] == 1
        print('PASS syntax/bind error stats published after diagnostics')
        sql = "EXPLAIN ANALYZE SELECT count(*) FROM '"+str(data.resolve()/ 'lineitem.parquet')+"'"
        run = subprocess.run([str(binary.resolve()), 'sql', '--threads', '2', '--command='+sql],
                             env=dict(env, NATA_STATS_JSON=str(sidecar)), capture_output=True, check=True)
        line = next(line for line in run.stdout.decode().splitlines() if line.startswith('query_stats: '))
        shown = json.loads(line.removeprefix('query_stats: '))
        final = json.loads(sidecar.read_text())
        assert shown['query_id'] == final['query_id']
        assert shown['stages']['state_update'] == final['stages']['state_update']
        assert b'query_stats' not in run.stderr
        print('PASS EXPLAIN ANALYZE same execution owner')
        if out is not None:
            with binary.open('rb') as stream:
                binary_sha256=hashlib.file_digest(stream,'sha256').hexdigest()
            (out/'byte-oracle.json').write_text(json.dumps(dict(binary_sha256=binary_sha256,cases=evidence),indent=2)+'\n')
            (out/'explain.stats.json').write_text(json.dumps(final)+'\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--data', type=Path, required=True)
    p.add_argument('--out', type=Path, help='retain native records and byte-oracle hashes')
    a = p.parse_args()
    check(a.binary, a.data, a.out)
