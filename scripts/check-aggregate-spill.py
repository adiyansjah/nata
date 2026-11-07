#!/usr/bin/env python3
"""SF1 HAVING spill gate and indicative CLI timings; no target verdicts.

Run with a release candidate, disk TMPDIR and --out outside the source tree.
Checks exact numeric multisets (including multiplicities) before timing, asserts
real spill at 64 MiB and zero spill in the default-budget control. Timed runs
alternate the controls, include startup/planning/CSV, and write CSV to /dev/null.
"""
import argparse
import collections
import csv
import decimal
import json
import os
from pathlib import Path
import statistics
import subprocess
import time


def multiset(path):
    with path.open(newline="") as stream:
        reader = csv.reader(stream)
        next(reader)
        return collections.Counter(tuple(decimal.Decimal(x) for x in row) for row in reader)


def counters(stderr):
    line = next(line for line in stderr.splitlines() if line.startswith("aggregate:"))
    return {k: int(v) for k, v in (s.split("=", 1) for s in line.split()[1:])}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=3)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parents[1]
    sql = (source / "check/queries/aggregate/high_cardinality_tpch.sql").read_text().replace("{data}", str(args.data.resolve()))
    env = dict(os.environ)
    env.pop("AI_AGENT", None)
    env["TMPDIR"] = str(args.out.resolve())
    reference = args.out / "reference.csv"
    with reference.open("w") as output:
        subprocess.run(["duckdb", "-csv", "-c", "SET threads=1; " + sql], stdout=output, check=True, env=env, timeout=600)
    expected = multiset(reference)
    reference.unlink()
    observations = []
    for threads in [1, 8]:
        commands = {}
        for mode in ["memory", "64MiB"]:
            command = [str(args.candidate.resolve()), "sql", "--csv", "--report", "--threads", str(threads), "--spill-directory", str(args.out.resolve()), "--spill-disk-bytes", str(1 << 30), "-c", sql]
            if mode == "64MiB":
                command += ["--aggregate-memory-bytes", str(64 << 20)]
            commands[mode] = command
            actual = args.out / "candidate.csv"
            with actual.open("w") as output:
                result = subprocess.run(command, stdout=output, stderr=subprocess.PIPE, check=True, env=env, text=True, timeout=600)
            assert multiset(actual) == expected, (threads, mode, "multiset mismatch")
            actual.unlink()
            metrics = counters(result.stderr)
            assert (metrics["spill_bytes_written"] > 0) == (mode == "64MiB"), metrics
            if mode == "64MiB":
                assert metrics["peak_reservation"] <= 64 << 20, metrics
            observations.append(dict(kind="gate", threads=threads, mode=mode, rows=sum(expected.values()), counters=metrics))
        for repeat in range(args.rounds):
            for mode in ["memory", "64MiB"]:
                start = time.perf_counter()
                with open(os.devnull, "w") as output:
                    result = subprocess.run(commands[mode], stdout=output, stderr=subprocess.PIPE, check=True, env=env, text=True, timeout=600)
                elapsed = time.perf_counter() - start
                observations.append(dict(kind="timing", threads=threads, mode=mode, repeat=repeat, seconds=elapsed, counters=counters(result.stderr)))
    (args.out / "observations.json").write_text(json.dumps(observations, indent=2) + "\n")
    for threads in [1, 8]:
        for mode in ["memory", "64MiB"]:
            samples = [o["seconds"] for o in observations if o["kind"] == "timing" and o["threads"] == threads and o["mode"] == mode]
            if samples:
                print(threads, mode, "seconds min/median/max:", *(f"{n:.3f}" for n in [min(samples), statistics.median(samples), max(samples)]))
    print("PASS: exact multiset gate at 1 and 8 threads, default and 64 MiB;", sum(expected.values()), "rows")


if __name__ == "__main__":
    main()
