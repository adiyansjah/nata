#!/usr/bin/env python3
"""Short, sequential SF1 reservation diagnostic; no timing acceptance verdict.

Build the same instrumented release source before and after chunking, keep both
binaries, then alternate them on the same fixture. Check results separately with
nata-check before interpreting these local, indicative timings.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=5)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    # SQL quotes, not shell quoting; subprocess never invokes a shell.
    source = str((args.data / 'lineitem.parquet').resolve()).replace("'", "''")
    query = ("SELECT l_orderkey, count(*) AS n, sum(l_extendedprice) AS price, "
             "sum(l_extendedprice * (1 - l_discount)) AS net "
             f"FROM '{source}' WHERE l_shipdate <= DATE '1998-09-02' "
             "GROUP BY l_orderkey HAVING count(*) > 1")
    variants = [('before', args.before)]
    if args.after:
        variants.append(('after', args.after))
    env = dict(os.environ)
    env.pop('AI_AGENT', None)
    results = []
    with open(os.devnull, 'wb') as sink:
        for round_number in range(args.rounds):
            for threads in (1, 8):
                # Alternate order too, so neither arm always runs first.
                order = variants if round_number % 2 == 0 else list(reversed(variants))
                for variant, binary in order:
                    start = time.monotonic()
                    run = subprocess.run([str(binary.resolve()), 'sql', '--csv',
                                          '--threads', str(threads), '--report', '-c', query],
                                         env=env, stdout=sink, stderr=subprocess.PIPE,
                                         text=True, timeout=120, check=True)
                    row = dict(round=round_number, variant=variant, threads=threads,
                               elapsed_seconds=time.monotonic() - start)
                    for line in run.stderr.splitlines():
                        if line.startswith('aggregate: '):
                            for field in line[len('aggregate: '):].split():
                                name, value = field.split('=', 1)
                                row[name] = int(value)
                    if 'reservation_calls' not in row:
                        raise RuntimeError('candidate lacks aggregate diagnostic counters')
                    print(json.dumps(row), flush=True)
                    results.append(row)
                    args.out.write_text(json.dumps(dict(verdict='local, indicative only',
                                                        results=results), indent=2) + '\n')


if __name__ == '__main__':
    main()
