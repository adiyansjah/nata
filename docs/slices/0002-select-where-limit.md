# 0002: Parser and binder for `SELECT ... FROM <parquet> WHERE ... LIMIT`, CLI results, `EXPLAIN`

Status: accepted (by the assistant under the owner's delegation of 2026-11-06; the owner can change it before the baseline spike runs). Amended the same day after a check against `docs/way-of-working.md`: the edge cases of item 2, contention and injected delays, the failures that reach the query from below, clean-VM resource limits, the leak scan, replay seeds, timing competitors alike, and a real bake-off in the spike. Each GCP run of this slice gets a cost estimate and the owner's yes first (section 8).
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
| Correctness | Same rows as DuckDB for every query of the slice's test set (projections, arithmetic on ints and decimals, comparisons, `AND`/`OR`/`NOT`, `BETWEEN`, `IN (list)`, `IS [NOT] NULL`, `CASE`, casts, `LIMIT`/`OFFSET`, NULLs, NaN, -0.0), at the edges of `way-of-working.md` item 2: an empty file, a one-row file, an all-NULL column, duplicate rows, a filter that keeps nothing and one that keeps everything (larger than memory: N/A, because every operator of this slice streams; the scan's memory bound is slice 3's), and the same error class where DuckDB errors (overflow, division by zero, bad casts); pushdown never changes a result. | `nata-check` differential against DuckDB (gating in CI); the grammar fuzzer of `spikes/sqlfuzz` pointed at `nata sql` for the subset (DuckDB oracle, 28 known-bad engines must still be caught); **mutants** of the binder and the pushdown (each caught): a conjunct pushed through `OR`, a pushed predicate also dropped from the remaining filter when it is not exact, a `NOT` lost, an alias resolved to the wrong column, a literal typed wrongly (decimal scale, date), `LIMIT` applied before the filter, `OFFSET` off by one. | DuckDB; the slice 3 scan (`nata scan`) for the same pushed-down queries |
| Performance | Parsing and binding of the test-set queries under 1 ms each (median), so that planning is invisible next to the scan; end-to-end time of the example query within 1.2x of `nata scan` with the same columns and pushed filter. | Paired benchmark with a control arm on settled VMs of both CPU types (ADR 0006 rule 7), `nata-bench`; competitors timed the same way (`way-of-working.md` section 6, rule 4): parse and bind timed in-process for us and through DuckDB's `EXPLAIN` timing for DuckDB, end-to-end both as processes and with each side's start-up time measured and stated | `bench/results/20261106-scan-bench.md` |
| Scalability | Inherits the scan's (5x at 8 threads on SF10, met); the planner adds no serial step that grows with the data. | Same benchmark at 1 and 8 threads, SF10 | slice 3 |
| Reliability | No hang, leak or panic on cancel (`LIMIT`, Ctrl-C, a closed stdout) at any point of parse, bind or run. | The slice 3 leak counters and scenarios extended to `nata sql`; a test that sends SIGINT mid-scan; the cancel scenarios also run under CPU contention (a busy-loop competitor on every core) and with injected delays in the file opener, as `way-of-working.md` item 2 requires for concurrent code | `bench/results/20261106-scan-parallel.md` |
| Fault tolerance | Every malformed query is an error with a position (line and column) and, where possible, a suggestion ("did you mean ..."); never a panic. Every failure that reaches the query from below keeps slice 3's rule: a missing, unreadable, truncated or corrupt file, an I/O error mid-scan, or a dependency panic on a corrupt page is an error naming the file and row group, with a non-zero exit, never a partial result presented as complete. | A fuzz of the parser with mutated test-set queries (10,000 cases: no panic, every error has a position); the slice 3 fault opener (injected I/O errors and delays) and damaged files driven through `nata sql` | `spikes/sqlfuzz` shrinker |
| Resource bounds | A query text up to 1 MB and an expression nesting depth up to 1,000 parse within bounded memory; deeper nesting is an error, not a stack overflow. A query over 10,000 small files and over SF10 stays within the scan's memory and open-file bounds. | Tests with generated deep and long queries under a stack limit; the 10,000-file and memory tests of slice 3 through `nata sql`, on a clean VM with its default limits (`ulimit -n` of a fresh machine) | slice 3 bounds |
| Portability | ARM64 and x86-64, clean VMs; every number on both. | `orchestrate.sh` job on `n4a-highcpu-8` and `c3d-highcpu-8` | slice 3 |
| Compatibility | The dialect subset documented in `docs/dialect.md` and `docs/supported-sql.md`, each construct with an example; Postgres/DuckDB spelling where they agree, and the difference stated where Nata is stricter. | A test per documented construct; the documented "not supported yet" list tested to error | DuckDB |
| Security and privacy | Untrusted query text cannot cause unbounded memory, a stack overflow or a panic; error messages quote the query, never data values; no secret, key or project identifier in logs, results or errors. | The parser fuzz above; a test that errors contain no cell values; `scripts/check-leaks.sh` in CI and on every result copied into the repository; the engineering checks below | - |
| Operability | `EXPLAIN` shows the plan (scan with pushed predicate and pruning columns, filter, projection, limit); `EXPLAIN ANALYZE` adds rows and time per operator and the scan report (row groups skipped, bytes read). Every fuzz or differential failure prints a seed (and the shrunk query) that replays it. | Golden-output tests of both on known files; a test that a planted failure replays from its printed seed | slice 3 scan report |

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
- Smallest experiment, with its control arm (a bake-off, `way-of-working.md` section 6): a minimal hand-written recursive-descent parser
  for exactly the slice's subset (a few hundred lines, throwaway, in `spikes/sql-parser/`) against `sqlparser` (latest release) on the same
  corpus: the test set, the 22 TPC-H queries (subset parser: those in the subset) and 10,000 fuzzer queries; compare acceptance, error
  positions and messages, literal fidelity and parse time; check that every numeric literal keeps its text, that errors carry positions, and run `cargo-deny` and the unsafe inventory
  on its dependency tree; control: the same corpus through DuckDB's parser (accept/reject agreement), timing of parse per query.
- Time box and cloud spend: one day, local only (parsing is not a performance-critical number at this size); no GCP needed.
- Result file: `bench/results/20261106-sql-parser-baseline.md`. Result (2026-11-06): R1 is not triggered, so reuse `sqlparser` 0.63.0 (PostgreSQL dialect, `default-features = false`, behind a conversion layer and a depth guard, parsing on a stack of at least 16 MB): all four criteria passed (22 of 22 TPC-H and 230 of 230 test-set queries parse to the hand parser's AST, 47 of 47 literals keep their text, every error has line and column, no `unsafe` and no refused license without the crate's `recursive-protection` feature), at the price of 10 times the parse time of a hand-written parser (11 us against 1.1 us median, both far under 1 ms) and the close calls listed in the file.

## 4b. Decisions after the baseline spike (2026-11-06, by the assistant under the owner's delegation)

- **R1: reuse `sqlparser`** (`bench/results/20261106-sql-parser-baseline.md`: none of the four criteria failed), with
  `default-features = false, features = ["std"]` (no `recursive-protection`: its `stacker`/`psm` dependencies bring `unsafe` and assembly)
  and the **PostgreSQL dialect** (the generic one accepts `SELECT a, FROM`).
- **Stack:** the parser runs on a dedicated thread with a 16 MB stack (the crate needs 6 to 8 MB for 1,000 nested levels without
  `stacker`); **a token-level guard runs before the parser** and refuses nesting deeper than 1,000 and an operator chain longer than 10,000
  terms with a positioned error (a 250,000-term chain aborted the process, even with `stacker`). Both limits are in `docs/dialect.md`.
- **What the AST hides:** `LIMIT ALL` is dropped by the crate; the layer detects it from the token stream and refuses it (`not supported
  yet`), with a test; any further construct found to vanish in the AST gets the same treatment and a test.
- **Errors at the end of the input** get the position of the end of the text (the crate gives none there).
- The hand-written parser and its corpus stay as a test oracle for the subset until the slice closes; the spike is then deleted.

## 5. Carried-in improvements (from `docs/improvements.md`)

The open register items on this slice's path (the scan under `nata sql`, its scheduler, CI). Together about a quarter of the slice's work.

| Register item | Taken in or deferred | Decision rule (fixed before measuring) / reason | Check |
|---|---|---|---|
| P1 late materialization | taken in | Keep it only if, on SF1 and SF10 and both CPU types, it is at least 1.2x faster than decoding every projected column at 10% selectivity and no slower than 1.05x at 100%; otherwise leave it off (default) and record why | paired benchmark, late against early, twin control, settled; same rows as DuckDB in both modes; mutants: a projected column decoded for the wrong rows, a selection applied twice |
| P2 footers read ahead within the window | taken in | Keep it if a scan of 1,000 small files is at least 1.3x faster at 8 threads and the open-file and footer bounds of slice 3 still hold under the `ulimit` test | the 10,000-file test and a many-small-files benchmark; peak open files and footers from the report |
| R4 Parquet page checksums | taken in | Verify a page's CRC when the file has one (on by default; a corrupt page is an error naming the file and row group); its cost must stay under 5% on a file with checksums, or it becomes an option | the fuzz test on files written WITH checksums: its "different rows" class must be 0 for them; the cost measured with a control arm |
| R9 nightly CI for the deep checks | taken in | - | the nightly job must go red on one known-bad scheduler mutant (it can fail) and green on main |
| R6a report the delta-byte-array panic upstream | done 2026-11-06 (apache/arrow-rs#12097) | the panics stay caught until a release fixes it | - |
| R6b upgrade past 60.1.0 (#11691 fixed upstream) | taken in if the next arrow-rs release lands during the slice | the whole test, differential and fuzz suite must pass on the new version | the existing suites |
| P5 overflow-check elision | deferred to slice 5 | this slice's arithmetic is per-row projection, where the checks cost about 0-5% | - |
| R1 memory manager | deferred, written before slice 5 | this slice's operators stream; no operator holds memory beyond the scan's window | - |

## 6. Closeout (fill in when done)

- Progress, 2026-11-06: step 2, the SQL front end, is built as `crates/nata-sql` (parse behind the token guard on a 16 MB thread, bind with the exact types of the dialect, the logical plan with the R2 pushdown, the plan text); checks and mutants in `bench/results/20261106-sql-frontend.md`.
- Progress, 2026-11-06: step 3, execution, is built as `crates/nata-exec` and `nata sql` (filter and projection inside the scan tasks on strict, checked, selection-aware evaluation, offset and limit that stop the scan, `EXPLAIN` / `EXPLAIN ANALYZE`, cancellation by limit, closed stdout, Ctrl-C and errors, CSV and table output); the `differential` CI job is gating (`nata-check` on 284 new queries plus the existing sets with a PENDING list, and 20,000 fuzzed queries); checks, differences by class, mutants and the readings decided are in `bench/results/20261106-sql-execution.md`. Step 4 (benchmarks on GCP, ARM64 and x86-64, the closeout) is not started.
- Results file:
- ADR updated (amendment or revision note), confidence and what changed: ADR 0007 written and accepted 2026-11-06 (step 0, before the parser work); `docs/dialect.md`
- Benchmark added to the regression set, baseline numbers:
- Deviations from this card, and why:
- Spike code deleted:
- Improvement register updated (items done, new findings added):
