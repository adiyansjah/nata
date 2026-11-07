#!/usr/bin/env python3
"""Loopback PostgreSQL oracle for COPY: compare every backend byte, not only values.
Uses the capture's all_types rows, plus escaping controls and an empty result.
Requires distribution PostgreSQL binaries, psycopg and pyarrow. No timing claims.
"""
import argparse
from datetime import date
from decimal import Decimal
import json
from pathlib import Path
import socket
import subprocess
import sys
import time

import psycopg
import pyarrow as pa
import pyarrow.parquet as pq

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/wirecapture'))
from protocol import Conn


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def run(args):
    subprocess.run([str(x) for x in args], check=True, stdout=subprocess.DEVNULL)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--nata', type=Path, required=True)
    ap.add_argument('--work', type=Path, required=True)
    ap.add_argument('--pg-bin', type=Path, default=Path('/usr/lib/postgresql/17/bin'))
    a = ap.parse_args()
    a.work = a.work.resolve()
    a.work.mkdir(parents=True, exist_ok=True)
    pgdata = a.work / 'postgres'
    pgport, nataport = port(), port()
    while nataport == pgport:
        nataport = port()
    run([a.pg_bin / 'initdb', '-D', pgdata, '-A', 'trust', '-U', 'nata', '--no-locale', '--encoding=UTF8'])
    server = None
    report = {}
    try:
        run([a.pg_bin / 'pg_ctl', '-D', pgdata, '-l', a.work / 'postgres.log', '-o', f"-h 127.0.0.1 -p {pgport} -k ''", '-w', 'start'])
        with psycopg.connect(host='127.0.0.1', port=pgport, user='nata', dbname='postgres', autocommit=True) as c:
            c.execute('CREATE DATABASE nata')
        text = 'quote" comma, tab\t newline\n'
        controls = ['\\N', '\\.', '\r\b\v\f\\"é']
        names = ['tiny', 'b', 's', 'i', 'l', 'f', 'd', 'n', 'day', 'txt']
        rows = [('-128', True, -32768, -2147483648, -9223372036854775808, 1.25, 1.25, Decimal('123.450'), date(2000, 1, 1), text),
                (None,) * 10,
                ('', False, 0, 0, 0, 0., 0., Decimal('0.000'), date(1, 1, 1), '')]
        types = [pa.string(), pa.bool_(), pa.int16(), pa.int32(), pa.int64(), pa.float32(), pa.float64(), pa.decimal128(18, 3), pa.date32(), pa.string()]
        pq.write_table(pa.Table.from_arrays([pa.array([r[i] for r in rows], type=t) for i, t in enumerate(types)], names=names), a.work / 'types.parquet')
        registry = a.work / 'registry.tsv'
        registry.write_text(f'nata\tpublic\tall_types\t{a.work / "types.parquet"}\n')
        with psycopg.connect(host='127.0.0.1', port=pgport, user='nata', dbname='nata', autocommit=True) as c:
            c.execute('CREATE TABLE all_types(tiny text,b boolean,s smallint,i integer,l bigint,f real,d double precision,n numeric(18,3),day date,txt text)')
            with c.cursor() as cur:
                cur.executemany('INSERT INTO all_types VALUES (' + ','.join(['%s'] * 10) + ')', rows)
        with (a.work / 'nata.log').open('w') as log:
            server = subprocess.Popen([str(a.nata.resolve()), 'serve', '--listen', f'127.0.0.1:{nataport}', '--catalog', str(registry), '--threads', '2'], stdout=log, stderr=log)
        for _ in range(200):
            try:
                n = Conn(nataport)
                break
            except OSError:
                if server.poll() is not None:
                    raise RuntimeError('nata exited')
                time.sleep(.02)
        else:
            raise RuntimeError('nata startup timed out')
        p = Conn(pgport)
        try:
            fixture = json.loads((ROOT / 'fixtures/wirecapture/postgresql-15.19-x86/copy.json').read_text())
            frozen = {}
            for step in fixture['steps']:
                sql = bytes.fromhex(step['frontend'][1]).rstrip(b'\0').decode()
                frozen[' '.join(sql.split())] = step['backend']
            for phase in ('capture', 'controls'):
                if phase == 'controls':
                    extra = [(x, None, None, None, None, None, None, None, None, x) for x in controls]
                    rows.extend(extra)
                    pq.write_table(pa.Table.from_arrays([pa.array([r[i] for r in rows], type=t) for i, t in enumerate(types)], names=names), a.work / 'types.parquet')
                    with psycopg.connect(host='127.0.0.1', port=pgport, user='nata', dbname='nata', autocommit=True) as c:
                        with c.cursor() as cur:
                            cur.executemany('INSERT INTO all_types VALUES (' + ','.join(['%s'] * 10) + ')', extra)
                for source in ['public.all_types', '(SELECT * FROM public.all_types)', '(SELECT * FROM public.all_types LIMIT 0)', '(SELECT txt FROM public.all_types)']:
                    for options in ['', '(FORMAT text)', '(FORMAT csv)', 'WITH (FORMAT csv, HEADER true)', '(FORMAT binary)']:
                        sql = f'COPY {source} TO STDOUT {options}'
                        expected, actual = p.query(sql), n.query(sql)
                        assert actual == expected, (phase, sql, expected, actual)
                        # Every tag, format code, CopyData, CopyDone, count and RFQ.
                        report[phase + ' ' + sql] = 'PASS: exact backend bytes'
                        if phase == 'capture' and ' '.join(sql.split()) in frozen:
                            assert actual == frozen[' '.join(sql.split())], (sql, actual, frozen[' '.join(sql.split())])
                            report['PG15 fixture ' + sql] = 'PASS: exact backend bytes'
            p.close()
            n.close()
        finally:
            p.s.close()
            n.s.close()
        report['reference'] = subprocess.check_output([str(a.pg_bin / 'postgres'), '--version'], text=True).strip()
        (a.work / 'copy-comparison.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    finally:
        try:
            if server is not None:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
        finally:
            if (pgdata / 'postmaster.pid').exists():
                run([a.pg_bin / 'pg_ctl', '-D', pgdata, '-m', 'immediate', '-w', 'stop'])


if __name__ == '__main__':
    main()
