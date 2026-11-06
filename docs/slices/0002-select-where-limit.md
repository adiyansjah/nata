# 0002: Parser and binder for `SELECT ... FROM <parquet> WHERE ... LIMIT`, CLI results, `EXPLAIN`

Status: accepted (by the assistant under the owner's delegation of 2026-11-06; the owner can change it before the baseline spike runs)
Date: 2026-11-06
ADRs: 0001, 0002, 0003, 0006 (relies on); creates ADR 0007 "SQL dialect and strictness" (feature-map open question 5)

Rule: `docs/way-of-working.md`, section 2. Slice 4 of the order in `docs/feature-map.md`, after slice 3 (card 0001, closed 2026-11-06).

## 1. Goal

`nata sql -c "<query>"` runs a single-table query over Parquet files and prints the result (CSV with `--csv`, an aligned table by default):
`SELECT <columns and scalar expressions> FROM '<file or glob>' [WHERE <boolean expression>] [LIMIT n [OFFSET m]]`, plus
`EXPLAIN` and `EXPLAIN ANALYZE` of such a query. Everything outside that subset answers `not supported yet: <what>` with the position in the
query. Example: `SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS net FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01'
AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 LIMIT 10` prints DuckDB's rows (in the order of the file) and
`EXPLAIN` shows the scan with its pushed-down predicate, the remaining filter, the projection and the limit.

The binder pushes `column <op> literal` conjuncts into the slice 3 scan (pruning and the exact row filter) and evaluates every other
expression with the strict kernels of ADR 0003 (checked arithmetic, exact decimals, errors under selection masked). The `differential`
CI job (`nata-check` against DuckDB through `nata sql`) becomes gating.

Not in this slice: aggregation, `GROUP BY`, `ORDER BY` (slice 5 and 6), joins (7), more than one table, subqueries, CTEs, writes,
prepared statements, the Postgres wire protocol (10), Iceberg tables (12).

Decisions it needs: (a) build or reuse the parser (rule R1, baseline spike in part 4), (b) the dialect and strictness rules: written as
`docs/dialect.md` and ADR 0007 BEFORE the parser work starts (feature map, B1 "Dialect spec document": "write before parser work starts").

## 2. Quality targets

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | Same rows as DuckDB for every query of the slice's test set (projections, arithmetic on ints and decimals, comparisons, `AND`/`OR`/`NOT`, `BETWEEN`, `IN (list)`, `IS [NOT] NULL`, `CASE`, casts, `LIMIT`/`OFFSET`, NULLs, NaN, -0.0, empty results), and the same error class where DuckDB errors (overflow, division by zero, bad casts); pushdown never changes a result. | `nata-check` differential against DuckDB (gating in CI); the grammar fuzzer of `spikes/sqlfuzz` pointed at `nata sql` for the subset (DuckDB oracle, 28 known-bad engines must still be caught); **mutants** of the binder and the pushdown (each caught): a conjunct pushed through `OR`, a pushed predicate also dropped from the remaining filter when it is not exact, a `NOT` lost, an alias resolved to the wrong column, a literal typed wrongly (decimal scale, date), `LIMIT` applied before the filter, `OFFSET` off by one. | DuckDB; the slice 3 scan (`nata scan`) for the same pushed-down queries |
| Performance | Parsing and binding of the test-set queries under 1 ms each (median), so that planning is invisible next to the scan; end-to-end time of the example query within 1.2x of `nata scan` with the same columns and pushed filter. | Paired benchmark with a control arm on settled VMs of both CPU types (ADR 0006 rule 7), `nata-bench` | `bench/results/20261106-scan-bench.md` |
| Scalability | Inherits the scan's (5x at 8 threads on SF10, met); the planner adds no serial step that grows with the data. | Same benchmark at 1 and 8 threads, SF10 | slice 3 |
| Reliability | No hang, leak or panic on cancel (`LIMIT`, Ctrl-C, a closed stdout) at any point of parse, bind or run. | The slice 3 leak counters and scenarios extended to `nata sql`; a test that sends SIGINT mid-scan | `bench/results/20261106-scan-parallel.md` |
| Fault tolerance | Every malformed query is an error with a position (line and column) and, where possible, a suggestion ("did you mean ..."); never a panic. | A fuzz of the parser with mutated test-set queries (10,000 cases: no panic, every error has a position) | `spikes/sqlfuzz` shrinker |
| Resource bounds | A query text up to 1 MB and an expression nesting depth up to 1,000 parse within bounded memory; deeper nesting is an error, not a stack overflow. | Tests with generated deep and long queries under a stack limit | - |
| Portability | ARM64 and x86-64, clean VMs; every number on both. | `orchestrate.sh` job on `n4a-highcpu-8` and `c3d-highcpu-8` | slice 3 |
| Compatibility | The dialect subset documented in `docs/dialect.md` and `docs/supported-sql.md`, each construct with an example; Postgres/DuckDB spelling where they agree, and the difference stated where Nata is stricter. | A test per documented construct; the documented "not supported yet" list tested to error | DuckDB |
| Security and privacy | Untrusted query text cannot cause unbounded memory, a stack overflow or a panic; error messages quote the query, never data values. | The parser fuzz above; a test that errors contain no cell values; the engineering checks below | - |
| Operability | `EXPLAIN` shows the plan (scan with pushed predicate and pruning columns, filter, projection, limit); `EXPLAIN ANALYZE` adds rows and time per operator and the scan report (row groups skipped, bytes read). | Golden-output tests of both on known files | slice 3 scan report |

**Engineering checks (verification-plan item 34) start with this slice**, as decided on 2026-11-05: `cargo-deny` (licenses, bans,
duplicates) and `cargo-audit` in CI; the textual unsafe inventory (`scripts/unsafe-inventory.py`) recorded per dependency change; ASan and
TSan on the test suites of `nata-sched`, `nata-parquet` and the new crates, and Miri on `nata-scan` and `nata-kernels` (nightly, a GCP job);
a benchmark regression harness that runs `crates/nata-bench/scan-bench.sh` and this slice's benchmark against stored baselines with a stated
threshold (a regression larger than the twin's spread fails).

## 3. Decision rules (fixed before measuring)

- **R1, parser: reuse the `sqlparser` crate** unless the baseline spike shows that it (i) cannot parse every query of the slice's test set
  and the 22 TPC-H queries in its generic or PostgreSQL dialect, (ii) loses the literal text of numbers (an exact decimal must not pass
  through a float), (iii) cannot give the line and column of an error, or (iv) adds `unsafe` code or a license that `cargo-deny` refuses.
  If any of these fails, write our own recursive-descent parser for the subset behind the same AST boundary, and keep `sqlparser` as a
  test oracle for what it does parse.
- **R2, pushdown:** a conjunct is pushed into the scan only if it is `column <op> literal` (or `IS [NOT] NULL`) on a column of the file,
  with the literal typed exactly to the column; everything else stays in the filter above the scan. A pushed conjunct stays in the scan's
  exact row filter (slice 3), so pushdown can only skip work, never change the result. Measured: the example query must skip at least 60%
  of the row groups of the sorted SF1 file, as slice 3's R2.

## 4. Baseline spike (required: a build-versus-reuse choice)

- Question: does `sqlparser` meet R1 (i) to (iv)?
- Smallest experiment, with its control arm: parse the test set, the 22 TPC-H queries and 10,000 fuzzer queries with `sqlparser` (latest
  release); check that every numeric literal keeps its text, that errors carry positions, and run `cargo-deny` and the unsafe inventory
  on its dependency tree; control: the same corpus through DuckDB's parser (accept/reject agreement), timing of parse per query.
- Time box and cloud spend: half a day, local only (parsing is not a performance-critical number at this size); no GCP needed.
- Result file: `bench/results/<date>-sql-parser-baseline.md`.

## 5. Closeout (fill in when done)

- Results file:
- ADR updated (amendment or revision note), confidence and what changed: ADR 0007 written (dialect and strictness)
- Benchmark added to the regression set, baseline numbers:
- Deviations from this card, and why:
- Spike code deleted:
