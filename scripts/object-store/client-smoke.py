#!/usr/bin/env python3
"""Default-client smoke for object-store tables (card 0006, walking skeleton): `nata serve` with one registered table whose location is an `s3://` object and one
local twin of the same bytes; psql, psycopg and pgjdbc, each with default settings, must get the same rows from both. A client whose tooling is missing is
reported NOT-RUN (never PASS), and the exit code is nonzero if any client FAILED or NOT-RUN is not allowed (--allow-not-run).

  client-smoke.py --nata target/debug/nata --objfake target/debug/nata-objfake --parquet <a lineitem-like .parquet> --work <scratch dir>
                  [--store fake | --store s3://bucket/key-of-the-same-file]   (a real store needs its credentials in the environment and the file already uploaded)
                  [--jar pgjdbc.jar] [--allow-not-run]
Nothing here prints a credential; the fake endpoint's dummy keys are in the child environment only."""
import argparse, os, shutil, signal, socket, subprocess, sys, time
from pathlib import Path

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('--nata', type=Path, required=True)
ap.add_argument('--objfake', type=Path)
ap.add_argument('--parquet', type=Path, required=True)
ap.add_argument('--work', type=Path, required=True)
ap.add_argument('--store', default='fake')
ap.add_argument('--jar', type=Path)
ap.add_argument('--allow-not-run', action='store_true')
a = ap.parse_args()
os.environ.pop('AI_AGENT', None)
a.work.mkdir(parents=True, exist_ok=True)
results, procs = [], []

def record(name, fn):
    try:
        fn(); results.append((name, 'PASS', ''))
    except NotImplementedError as e:
        results.append((name, 'NOT-RUN', str(e)))
    except Exception as e:  # noqa: BLE001 - a smoke reports every failure
        results.append((name, 'FAIL', f'{type(e).__name__}: {str(e)[:200]}'))

def free_port():
    s = socket.socket(); s.bind(('127.0.0.1', 0)); p = s.getsockname()[1]; s.close(); return p

env = dict(os.environ)
if a.store == 'fake':
    assert a.objfake, '--objfake is required with --store fake'
    d = a.work / 'bucket'; shutil.rmtree(d, ignore_errors=True); d.mkdir()
    shutil.copy(a.parquet, d / 'table.parquet')
    p = subprocess.Popen([str(a.objfake), '--dir', str(d), '--bucket', 'bkt'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    procs.append(p)
    endpoint = p.stdout.readline().strip().split('=', 1)[1]
    p.stdout.readline()
    env.update(AWS_ENDPOINT=endpoint, AWS_ALLOW_HTTP='true', AWS_ACCESS_KEY_ID='AKIDSMOKE', AWS_SECRET_ACCESS_KEY='smoke-secret', AWS_REGION='us-east-1')
    remote = 's3://bkt/table.parquet'
else:
    remote = a.store
registry = a.work / 'catalog.tsv'
registry.write_text(f'nata\tpublic\tremote_table\t{remote}\nnata\tpublic\tlocal_table\t{a.parquet.resolve()}\n')
port = free_port()
log = (a.work / 'server.log').open('w')
server = subprocess.Popen([str(a.nata.resolve()), 'serve', '--catalog', str(registry), '--listen', f'127.0.0.1:{port}', '--threads', '4', '--max-queries', '2'], stdout=log, stderr=log, env=env)
procs.append(server)
for _ in range(100):
    try:
        socket.create_connection(('127.0.0.1', port), 0.2).close(); break
    except OSError:
        if server.poll() is not None: sys.exit('nata serve exited; see ' + str(a.work / 'server.log'))
        time.sleep(0.1)
dsn = f'host=127.0.0.1 port={port} dbname=nata user=nata'
Q = ["SELECT count(*), sum(l_quantity) FROM nata.public.{t}",
     "SELECT l_orderkey, l_shipdate, l_comment FROM nata.public.{t} WHERE l_orderkey < 40 LIMIT 25",
     "SELECT l_returnflag, count(*) FROM nata.public.{t} GROUP BY l_returnflag"]

def psql():
    if not shutil.which('psql'): raise NotImplementedError('psql is not installed')
    for q in Q:
        out = {}
        for t in ('remote_table', 'local_table'):
            r = subprocess.run(['psql', dsn, '-At', '-c', q.format(t=t)], capture_output=True, text=True, timeout=60)
            assert r.returncode == 0, r.stderr[:200]
            out[t] = sorted(r.stdout.splitlines()) if 'GROUP BY' in q else r.stdout.splitlines()
        assert out['remote_table'] and out['remote_table'] == out['local_table'], (q, out['remote_table'][:3], out['local_table'][:3])

def psycopg_check():
    try:
        import psycopg
    except ImportError:
        raise NotImplementedError('psycopg is not installed')
    with psycopg.connect(dsn) as c:
        for q in Q:
            got = {}
            for t in ('remote_table', 'local_table'):
                for _ in range(2):  # the second run is a named/prepared statement for psycopg's default prepare_threshold paths
                    rows = c.execute(q.format(t=t)).fetchall()
                got[t] = sorted(map(repr, rows)) if 'GROUP BY' in q else list(map(repr, rows))
            assert got['remote_table'] and got['remote_table'] == got['local_table'], q

def pgjdbc():
    if not (shutil.which('javac') and a.jar and a.jar.exists()): raise NotImplementedError('a JDK and --jar <pgjdbc.jar> are required')
    src = Path(__file__).with_name('ObjectClients.java')
    cls = a.work / 'cls'; cls.mkdir(exist_ok=True)
    subprocess.run(['javac', '-d', str(cls), str(src)], check=True, capture_output=True)
    r = subprocess.run(['java', '-cp', f'{cls}:{a.jar}', 'ObjectClients', str(port)], capture_output=True, text=True, timeout=120)
    sys.stdout.write(r.stdout)
    assert r.returncode == 0, (r.stdout + r.stderr)[-300:]

record('psql', psql); record('psycopg', psycopg_check); record('pgjdbc', pgjdbc)
for p in procs:
    p.send_signal(signal.SIGTERM)
for p in procs:
    try: p.wait(10)
    except subprocess.TimeoutExpired: p.kill()
leak = [w for w in ('smoke-secret', 'AKIDSMOKE') if w in (a.work / 'server.log').read_text(errors='replace')]
results.append(('server log holds no credential', 'FAIL' if leak else 'PASS', ','.join(leak)))
for name, verdict, why in results:
    print(f'{verdict:8} {name} {why}')
bad = [r for r in results if r[1] == 'FAIL' or (r[1] == 'NOT-RUN' and not a.allow_not_run)]
sys.exit(1 if bad else 0)
