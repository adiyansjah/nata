# sqlfuzz (spike, verification-plan item 33)

A differential SQL fuzzer with **DuckDB as the oracle**: it generates random tables (adversarial data) and random queries, runs them on the
oracle and on a target, and reports every difference, shrunk to a small standalone reproducer. Python standard library only; it drives the
`duckdb` command line. "Build once, use on every later slice": the target is any engine behind a tiny command protocol (below), so the same
fuzzer will test Nata's SQL front end when it exists. Where it ends up (this spike, or the generator moved into `nata-check`, which today runs
fixed `.sql` files) is a decision for when Nata has a SQL front end.

```bash
python3 -I fuzz.py run   --seed 1 --cases 20000            # oracle vs a second DuckDB: must find nothing (a control)
python3 -I fuzz.py run   --seed 1 --cases 5000 --fault nan-is-ieee   # a known-bad target: finds, shrinks, writes sqlfuzz-out/diff-*.sql
python3 -I fuzz.py run   --target-cmd "python3 adapters/my_engine.py" # a real target
python3 -I fuzz.py prove --seed 1 --cases 6000             # control clean AND every one of the 28 known-bad engines caught
python3 -I tests/test_sqlfuzz.py                           # 14 tests of the fuzzer itself
```

## What it generates

- **Data** (`gen.py`): two tables of 0 to 25 rows with INTEGER, BIGINT, DOUBLE, DECIMAL(10,2), VARCHAR, BOOLEAN, DATE, TIMESTAMP, TIMESTAMPTZ;
  NULLs; NaN, +-Infinity, -0.0, 1e308, 0.1 + 0.2; integer and bigint extremes; empty and blank strings, case variants, `Straße`/`STRASSE`, `İstanbul`,
  composed and decomposed `é`, CJK, emoji, `%`, `_`, `\`, leading and trailing blanks; leap days, 1900, 2038, the two DST days of 2021 in
  `America/New_York` (the session time zone is set explicitly).
- **Queries**: projection with filter, order and limit (a unique tie-breaker, so the order is compared), grouped and global aggregates
  (count, count distinct, sum, avg, min, max, HAVING), inner/left/right/full joins with equality and residual predicates, EXISTS / NOT EXISTS,
  windows (row_number, rank, dense_rank, lag, lead, sum), UNION / INTERSECT / EXCEPT with and without ALL, CASE, COALESCE, casts, LIKE, BETWEEN,
  IN, IS [NOT] DISTINCT FROM, string, date and interval functions.
- A query the oracle rejects (overflow, division by zero, a bad cast) is kept on purpose: **the target must also fail** ("error expected"). About 7% of queries.

## How results are compared (`compare.py`)

NaN equals NaN, -0.0 equals 0.0, doubles agree within a relative 1e-9, integers and decimals agree exactly (decimals by value), NULL equals only NULL,
text and temporal values as the oracle prints them; rows as a multiset unless the query's order is total, then row by row. A target answer of
`not supported yet: ...` is counted as a coverage gap, not a difference (way-of-working, section 2.6). With `--strict-types` column types must match too.

## The target protocol (`engine.Command`)

`CMD in.json out.json`, where `in.json = {"setup": [sql...], "cases": [{"id": 0, "sql": "..."}]}` and
`out.json = {"results": {"0": {"cols": [...], "types": [...], "rows": [[...]]} | {"error": "message"}}}`. `adapters/duckdb_adapter.py` is the
reference implementation and the template for Nata's; with `SQLFUZZ_ADAPTER_BUG=droprow|nan-null|unsupported|crash` it is a known-bad target.

## Shown to catch known-bad engines (`prove`)

28 faults, each a classic engine bug applied while rendering the query for the target (`faults.py`, `render.py`): LEFT/RIGHT/FULL join handling,
NULL keys matching, EXISTS and NOT EXISTS (NOT IN) null semantics, NULLS FIRST/LAST, DESC ignored, LIMIT off by one, COUNT/AVG/SUM with NULLs,
integer division, case-insensitive text comparison, length in bytes, ASCII-only upper, trim, substr base, IEEE NaN comparisons, BETWEEN bounds,
LIKE case, UNION/INTERSECT/EXCEPT duplicates, rank vs row_number, lag default, silent integer overflow, DECIMAL via DOUBLE, a wrong session time zone.

Results on the dev box (DuckDB 2.0.0): the control (a faithful rendering against the oracle) finds 0 differences in 4,000 queries per seed;
**28 of 28 faults are caught on each of seeds 1, 2, 3** (4,000 to 6,000 queries allowed; the slowest catch took 2,767 queries on seed 1).
Each is shrunk automatically to a one- or two-line reproducer with 0 to 3 rows. A 20,000-query clean run takes about 75 seconds on 4 jobs.

GCP (job `sqlfuzz`, 2026-11-04, clean VMs, 8 vCPU, DuckDB 2.0.0): **ARM64 (`n4a-highcpu-8`) and x86-64 (`c3d-highcpu-8`) both PASS everything**: the 14 tests,
`prove` on seeds 1, 2 and 3 (28 of 28 faults caught each time, control clean), and two 60,000-query control campaigns (seeds 101 and 102) with
0 differences. The two architectures produced the same counts of oracle errors (4,993 and 5,030 of 60,000), so DuckDB itself behaved identically on both
for these queries. VM cost $0.009 and $0.007.

Things the checks of the fuzzer itself found: the `trim` fault SURVIVED until the string pool contained values with trailing blanks; no
INTEGER column ever held 2147483647 (so overflow was reachable only through literals) until the join key `a` got extreme values; one fault
rendered invalid SQL and was "caught" by the parse error (now a fault whose rendering the engine rejects is reported INVALID, and the
shrinker keeps to the kind of difference it started with).

## Not covered (yet)

DuckDB is the oracle, so a bug DuckDB shares with the target is invisible, and the oracle's own bugs are inherited. The grammar has no
subqueries in FROM, CTEs, scalar subqueries, `ORDER BY` on expressions with `LIMIT` over joins, nested types, `COLLATE`, `GROUP BY ROLLUP`,
`DISTINCT ON`, or multi-key joins on strings with collation other than the default. Data is small (at most 25 rows); large-data effects (spill,
parallel summation order) are not what this finds. No time-zone database other than DuckDB's own is tested (the target uses its own). Floating
point comparison is tolerant by design, so a wrong 10th significant digit passes.
