#!/usr/bin/env python3
"""Local release send-path probes using p16.py's validated raw reader/fixture.
No builds, cloud calls or acceptance verdicts. Counters are process totals,
including startup and the Parquet scan, not isolated encoder allocations.
"""
import argparse
import os
from pathlib import Path
import signal
import socket
import subprocess as sp
import time


def probe(a, kind, tool):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    stem = a.out / f'{kind}-{tool}'
    command = [str(a.nata), 'serve', '--catalog', str(a.work / 'catalog.tsv'),
               '--listen', f'127.0.0.1:{port}', '--threads', '1', '--max-queries', '1']
    if tool == 'heap':
        command = ['valgrind', '--tool=memcheck', '--leak-check=no',
                   f'--log-file={stem}.txt', *command]
    elif tool == 'syscalls':
        command = ['strace', '-f', '-c', '-o', f'{stem}.txt', *command]
    with Path(f'{stem}-server.log').open('w') as log:
        server = sp.Popen(command, stdout=log, stderr=log, start_new_session=True)
        try:
            for _ in range(2000):
                assert server.poll() is None, 'server/profiler stopped'
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        break
                except OSError:
                    time.sleep(.02)
            else:
                raise RuntimeError('server startup timeout')
            rows = '10000' if tool == 'heap' else '1000000'
            reader = (['java', '-cp', str(a.work) + ':' + str(a.jar), 'P16', str(port), rows]
                      if kind == 'jdbc' else [str(a.work / 'raw'), kind, str(port), rows])
            if tool == 'perf':
                sp.run(reader, check=True, stdout=sp.DEVNULL, timeout=120)
                perf = sp.Popen(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '999',
                                 '-g', '-o', f'{stem}.data', '-p', str(server.pid)])
                try:
                    time.sleep(.1)
                    for _ in range(5):
                        sp.run(reader, check=True, stdout=sp.DEVNULL, timeout=120)
                finally:
                    perf.send_signal(signal.SIGINT)
                    assert perf.wait(timeout=10) in [0, -2, 130], 'perf failed'
                with Path(f'{stem}.txt').open('w') as report:
                    sp.run(['perf', 'report', '--stdio', '--no-children', '--sort',
                            'symbol', '-i', f'{stem}.data'], check=True, stdout=report)
            else:
                sp.run(reader, check=True, stdout=sp.DEVNULL, timeout=120)
        finally:
            if server.poll() is None:
                os.killpg(server.pid, signal.SIGTERM)
            server.wait(timeout=10)


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ['nata', 'work', 'out']:
        ap.add_argument('--' + name, type=Path, required=True)
    ap.add_argument('--tool', choices=['perf', 'counts'], default='perf')
    ap.add_argument('--jdbc', action='store_true', help='profile the frozen JDBC fetchSize=1000 client instead of raw')
    ap.add_argument('--jar', type=Path)
    a = ap.parse_args()
    a.nata, a.work, a.out = a.nata.resolve(), a.work.resolve(), a.out.resolve()
    assert not a.jdbc or a.jar, '--jdbc requires --jar'
    if a.jar: a.jar = a.jar.resolve()
    a.out.mkdir(parents=True, exist_ok=True)
    for tool in (['perf'] if a.tool == 'perf' else ['heap', 'syscalls']):
        for kind in (['jdbc'] if a.jdbc else ['binary', 'text']):
            probe(a, kind, tool)
