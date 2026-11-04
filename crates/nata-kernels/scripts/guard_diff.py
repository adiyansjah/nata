#!/usr/bin/env python3
"""Differential check of the guarded-expression kernels against DuckDB.

Runs examples/guard_diff (our kernels, three executors) and the same SQL in DuckDB, compares the outcome
of every (table, query, executor), then probes how DuckDB treats division on its own and prints the table of
differences from the Nata rule. Exit status is non-zero if any compared case differs.
Usage: guard_diff.py   (from anywhere; needs `cargo` and `duckdb` on PATH)
"""
import csv, io, os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
ENV = {k: v for k, v in os.environ.items() if k != "AI_AGENT"}


def duckdb(sql):
    r = subprocess.run(["duckdb", "-no-agent", "-csv", "-c", sql], capture_output=True, text=True, env=ENV)
    return r.returncode, r.stdout, r.stderr


def canonical(rc, out, err):
    if rc != 0:
        e = err.lower()
        if "division by zero" in e:
            return "error:division_by_zero"
        if "overflow in division" in e or "overflow" in e:
            return "error:overflow"
        return "error:OTHER " + err.strip().splitlines()[0]
    rows = list(csv.reader(io.StringIO(out)))[1:]
    return "rows:" + ";".join(f"{r[0]}={'NULL' if r[1] == 'NULL' else r[1]}" for r in rows)


def main():
    nata = subprocess.run(["cargo", "run", "-q", "-p", "nata-kernels", "--example", "guard_diff"], cwd=ROOT, capture_output=True, text=True)
    if nata.returncode != 0:
        print(nata.stderr)
        sys.exit(2)
    total = same = 0
    errors = oks = 0
    bad = []
    for line in nata.stdout.splitlines():
        table, query, ex, setup, sql, want = line.split("\t")
        got = canonical(*duckdb(setup + " " + sql))
        total += 1
        if got == want:
            same += 1
            errors += want.startswith("error")
            oks += want.startswith("rows")
        else:
            bad.append((table, query, ex, want, got))
    print(f"compared {total} cases (3 tables x 9 queries x 3 executors): {same} identical ({oks} result sets, {errors} errors), {len(bad)} different")
    for b in bad:
        print("DIFFERENT:", b)

    print("\nDuckDB 2.0.0 probes (not compared with our kernels; the Nata rule is the second column):")
    probes = [
        ("BIGINT 1 / 0", "SELECT 1::BIGINT / 0::BIGINT AS v"),
        ("BIGINT 1 // 0", "SELECT 1::BIGINT // 0::BIGINT AS v"),
        ("BIGINT MIN // -1", "SELECT (-9223372036854775807::BIGINT - 1) // (-1)::BIGINT AS v"),
        ("BIGINT MIN / -1", "SELECT (-9223372036854775807::BIGINT - 1) / (-1)::BIGINT AS v"),
        ("BIGINT 1 % 0", "SELECT 1::BIGINT % 0::BIGINT AS v"),
        ("BIGINT MIN % -1", "SELECT (-9223372036854775807::BIGINT - 1) % (-1)::BIGINT AS v"),
        ("TRY(1 // 0)", "SELECT TRY(1::BIGINT // 0::BIGINT) AS v"),
        ("typeof(7 / 2)", "SELECT typeof(7::BIGINT / 2::BIGINT) AS v"),
        ("7 / 2", "SELECT 7::BIGINT / 2::BIGINT AS v"),
        ("-7 // 2", "SELECT (-7)::BIGINT // 2::BIGINT AS v"),
        ("SET error_on_division_by_zero=false; 1 // 0", "SET error_on_division_by_zero=false; SELECT 1::BIGINT // 0::BIGINT AS v"),
    ]
    print(f"{'expression':<46} | DuckDB 2.0.0")
    print(f"{'-' * 46}-+-{'-' * 60}")
    for name, sql in probes:
        rc, out, err = duckdb(sql)
        if rc != 0:
            res = "ERROR: " + err.strip().splitlines()[0][:80]
        else:
            res = out.strip().splitlines()[-1]
        print(f"{name:<46} | {res}")
    sys.exit(1 if bad else 0)


main()
