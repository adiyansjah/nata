# Deep-check closeout: SQL items C and D (2026-11-07)

C: replay PASS (both engines error). D: Nata follows §9b; a tested listed difference. Smoke PASS (20,000 queries, no BUG?); 6/6 mutants CAUGHT. Workspace checks PASS.
All observations below are measured locally (x86-64, Rust 1.99.1, DuckDB 2.0.0), without GCP. Raw logs: `~/tmp-0004c/`.

## C

- **Rule as written:** Nata raises the overflow error, as its dialect and ADR 0003 require. A double negation is never folded into a different literal type.
- **Numbers:** before, the two minima negated again returned 9223372036854775808 and 2147483648. `Binder::negate` folded the already typed `Value::Int` through `int_literal(-i128::from(i), pos)`, retyping INTEGER minimum as BIGINT and BIGINT minimum as DECIMAL(19,0). Only a direct AST number now forms a signed literal; an outer minus stays a checked Neg node of the operand's type. Tests cover spaced, parenthesized and triple negations of both minima, ordinary double/triple negations of 5, decimal 1.25, -0, and unreachable rows/CASE branches.
- **Oracle correction:** local PostgreSQL 17.11 and DuckDB 2.0.0 both return 9223372036854775808 for the bare double-negation spellings (and 2147483648 for INTEGER minimum). PostgreSQL returns `bigint out of range` / `integer out of range` for `-CAST('-9223372036854775808' AS BIGINT)` / `-CAST('-2147483648' AS INTEGER)`. The card's claim about bare literals does not match these measurements; Nata's fixed value-typing/overflow rule remains the decision rule. The generated fuzz replay is a different expression, containing multiplication as well as triple negation.
- **Additional replay:** seed 20261107, batch 2, query 13 now legitimately errors in Nata where DuckDB folds a wider literal. It shrinks to `SELECT true FROM t2 WHERE ((- -9223372036854775808) <= b)` with one row (`b=-1`, 22 shrink checks). The new listed classifier requires the exact unary-minus overflow type and the error's position at the generated `(- -MIN)` operation; it reconstructs the adapter's temporary FROM path for WHERE positions. Another operation's error, another type, a nonminimum, a decimal or compound operand, or a wrong/missing position is not excused. Reproduce with `--batch 2 --query 13` in the command below.
- **Verdict:** PASS: the new binder regression fails on the original code (DECIMAL(19,0) versus required BIGINT). The final replay reports BIGINT unary-minus overflow at column 91; DuckDB reports INT64 multiplication overflow; comparison: `same` (both errors). EXPLAIN now shows `Project (--9223372036854775808) AS x: BIGINT`.
- **Reproduce:** from `spikes/sqlfuzz`, with `NATA_BIN` pointing to this worktree's release binary and `TMPDIR=$HOME/tmp-0004c`:
  `env -u AI_AGENT python3 -I fuzz.py replay-subset --seed 20261107 --batch 1816 --query 15 --target-cmd 'python3 -I adapters/nata_adapter.py'`.
  Regression: `cargo test -p nata-sql s8b_nested_negations` and `cargo test -p nata-exec nested_negations`.

## D

- **Rule as written:** dialect §9b: the upper bound is evaluated only for rows that the lower bound did not decide as FALSE. If the failing row's lower bound was FALSE, fix evaluation; if NULL, add a listed difference class with a test.
- **Numbers and row values:** seed 20261107, batch 2202, query 31. Every t1 row below; lower is `(a + id) >= 2147483647`, reaches is `ti >= ti` (WHERE). NULL is written `NULL`.

| a | id | b | lower | reaches first CASE condition |
|---|---|---|---|---|
| 4 | 1 | -9223372036854775808 | FALSE | TRUE |
| NULL | 2 | NULL | NULL | TRUE |
| 0 | 3 | 1 | FALSE | NULL |
| 5 | 4 | 9223372036854775807 | FALSE | TRUE |
| 2 | 5 | 0 | FALSE | TRUE |
| 5 | 6 | 0 | FALSE | TRUE |
| -1 | 7 | -12 | FALSE | TRUE |
| -2 | 8 | NULL | FALSE | TRUE |
| NULL | 9 | 9223372036854775807 | NULL | TRUE |
| 5 | 10 | 1 | FALSE | TRUE |
| 5 | 11 | 9223372036854775807 | FALSE | TRUE |
| NULL | 12 | -4294967296 | NULL | TRUE |

  Shrinking leaves one row (id 12) and `CASE WHEN a BETWEEN 2147483647 AND CAST(b AS INTEGER) THEN r WHEN true THEN r ELSE r END` (46 checks). Isolating ids 1, 4, 11 succeeds (FALSE skips the invalid upper cast). Isolating 9 or 12 raises the BIGINT-to-INTEGER error; 9 is the first offending row in file order. The full query errors at line 1, column 52, its first CASE condition's upper cast. Row 3 never reaches a projection; all other 11 rows reach this first WHEN, with no earlier branch. In the independent c2 CASE, the first WHEN is TRUE for ids 1, 2, 5, 6, 8, 11, 12 and NULL for 4, 7, 9, 10; only the latter reach its second condition. None of c2's conditions contains the failing cast.
- **Verdict:** Nata follows the written rule. Execution is unchanged. The listed classifier requires the shrunk AST, actual rows, an exact BIGINT-to-INTEGER error, a single unfiltered/unlimited CASE projection whose first WHEN is integer BETWEEN, safe column results/constant later conditions, and an invalid upper cast on a NULL lower comparison. TRUE-lower invalid casts and unproved shapes remain BUG?. Regression tests distinguish FALSE, NULL and WHERE selection, and guard later WHEN conditions with an earlier taken branch.
- **Reproduce:** the replay command above with `--batch 2202 --query 31`. Row diagnostics from repo root:

```sh
TMPDIR=$HOME/tmp-0004c python3 - <<'PY'
import sys
sys.path.insert(0, 'spikes/sqlfuzz')
from sqlfuzz.subset_cli import build
from sqlfuzz.subset import setup_sql
from sqlfuzz.engine import DuckDB
t, _ = build(20261107, 2202, 40, 25)
q = 'SELECT a, id, b, (a + id) >= 2147483647 AS lower_bound, ti >= ti AS reaches_case FROM t1'
for row in DuckDB('duckdb').run(setup_sql(t), [(0, q)])[0].rows:
    print(row)
PY
```

  Regression: `cargo test -p nata-exec between_in_a_case_condition` and
  `python3 -I spikes/sqlfuzz/tests/test_subset.py DifferenceClasses`.

## Final fuzz runs

Local smoke: seed **20261107**, **20,000 queries**, jobs 2, final release binary: **exit 0, no BUG?**. 3,526 oracle errors, 47,776 rows compared, 483 optimizer-dependent queries, 22 not-supported CASEs. All 26 differences are listed:

| class | count |
|---|---|
| signed minimum literal negation (8b.10 / 9), exact operation/type/position | 8 |
| CASE decimal results beyond 38 digits | 4 |
| AND/OR projection evaluation | 2 |
| an operand next to NULL is still evaluated | 4 |
| constant select list error with no selected rows | 7 |
| tight decimal % result type | 1 |

Command (from `spikes/sqlfuzz`, `NATA_BIN` set): `env -u AI_AGENT python3 -I fuzz.py run-subset --seed 20261107 --cases 20000 --jobs 2 --target-cmd 'python3 -I adapters/nata_adapter.py' --out ~/tmp-0004c/fuzz-smoke`.
Raw summary/classes: `~/tmp-0004c/fuzz-smoke-20261107.log`.

The two local nightly-size attempts were stopped before completion. Per the parent's updated brief, the following are **to be run by the parent on GCP** (generic `heavy` job); no local nightly result is claimed. From `spikes/sqlfuzz`, with `NATA_BIN` set to the final release binary:

```sh
python3 -I fuzz.py run-subset --seed 20261107 --cases 100000 --jobs 2 --target-cmd 'python3 -I adapters/nata_adapter.py' --out ~/tmp-0004c/fuzz-out
python3 -I fuzz.py run-subset --seed 20261108 --cases 100000 --jobs 2 --target-cmd 'python3 -I adapters/nata_adapter.py' --out ~/tmp-0004c/fuzz-out
```

**Run by the parent on GCP (2026-11-07):** Spot c3d-highcpu-16, run `card0004-heavy3`, main `21c7e37` (code tree `d2131a21`), `--jobs 6`, both seeds
in parallel with the damaged-file fuzz. Raw outputs: `raw/20261107-card0004-heavy/`.

| Seed | Queries | Rows compared | Differences | `BUG?` | Exit | Wall time |
|---|---:|---:|---|---:|---:|---:|
| 20261107 | 100,000 | 243,221 | 83, all in listed classes | **0** | 0 | 399 s |
| 20261108 | 100,000 | 244,694 | 94, all in listed classes | **0** | 0 | 399 s |

## Mutants

| mutant | result |
|---|---|
| SQL: nested-negation-retyped-as-a-literal | CAUGHT: s8b_nested_negations_preserve_the_negative_literals_type |
| exec: between-null-lower-bound-skips-upper-bound | CAUGHT: new CASE/BETWEEN regression |
| exec: between-upper-bound-evaluated-for-every-row | CAUGHT: new CASE/BETWEEN regression |
| exec: where-between-evaluates-the-upper-bound-for-every-row | CAUGHT: new CASE/BETWEEN regression |
| exec: unary-minus-of-the-minimum-allowed | CAUGHT: new nested-negations regression |
| exec: case-without-else-takes-the-first-branch | CAUGHT: new CASE/BETWEEN regression |

**6 CAUGHT, 0 SURVIVED, 0 INVALID**; both unmutated suites passed.

Commands: `python3 -I crates/nata-sql/mutate.py --work ~/tmp-0004c/mut-work nested-negation-retyped-as-a-literal`;
`python3 -I crates/nata-exec/mutate.py --work ~/tmp-0004c/mut-work between-null-lower-bound-skips-upper-bound between-upper-bound-evaluated-for-every-row where-between-evaluates-the-upper-bound-for-every-row unary-minus-of-the-minimum-allowed case-without-else-takes-the-first-branch`.

## Checks

Cargo/check/mutant commands use `TMPDIR=$HOME/tmp-0004c` on disk; Cargo also uses `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, and `CARGO_TARGET_DIR=$HOME/tmp-0004c/target` (mutate.py uses its private target directory). DuckDB calls unset AI_AGENT.


- `cargo fmt --check`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test --workspace` (TMPDIR on disk): PASS (exit 0; 418 passed, 0 failed, 0 ignored). The first invocation was externally terminated with SIGTERM during compilation (exit 143); the resumed full run passed.
- Release `nata-check`: scan 5/5, sql 252/252, sql-differences 42/42 PASS; 0 mismatches/errors/unsupported/pending/pending-wrong. Existing PENDING handling is enabled (these directories have no PENDING entries).
- `python3 -I spikes/sqlfuzz/tests/test_subset.py`: 8 tests, PASS (one optional prove test skipped); includes both classifiers, mixed NULL/TRUE-row rejection and positioned literal-negation checks.
- `scripts/check-leaks.sh --staged`: PASS.
- No dependencies, GCP, push or merge.
