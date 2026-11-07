# Wire/catalog slice implementation — 2026-11-08

**Partial implementation; T0 acceptance NOT MET.** This change implements the
read-only COPY forms and corrects the extended statement deadline. Schema
browsing is still the skeleton's explicit failure boundary. No claim that the
51 covered inventory rows are fully implemented or compared with PostgreSQL;
the 36 additions remain explicit refusals. This is not slice-10 closeout.

No GCP, push, merge, new Rust dependency, accepted-ADR change, general SQL
extension, write path, TLS/authentication or streaming optimization.

## Measured local verdicts and client × action matrix

Rule: every mandatory T0 cell passes, or acceptance stays open. Measured:
**47 PASS / 29 FAIL / 9 not run**, matrix exit **1**; verdict **NOT MET**.
Compared with the skeleton, nine export FAILs become PASS. Two additional
probes pass: ADBC GetInfo and the declared suspended-fixture projections.
No formerly failing browse cell was silently omitted or relabelled as passing.
[Complete cells, versions, exceptions and mutation controls](raw/20261108-wire-catalog-slice/local/).

| Client | Connect / registered SELECT | Browse | Positive fetch | Binary COPY | Text / CSV / table COPY | Timeout / cancel / recovery |
|---|---|---|---|---|---|---|
| psql 17.11 | PASS / PASS | FAIL: \d, \dt, \dn, \df, \l | not run: no wire max-rows API | PASS | PASS / PASS / PASS | PASS / PASS / PASS |
| pgjdbc 42.7.7 | PASS / PASS, ten preparations | FAIL: all eight metadata methods | PASS, fetchSize=2 with rollback | PASS | PASS / PASS / PASS | PASS / PASS / PASS |
| psycopg 3.3.8 | PASS / PASS, default preparation | Two simple projections PASS; filtered tables/columns and type join FAIL | not run: SQL ServerCursor is a separate 0A000 boundary | PASS | PASS / PASS / PASS | PASS / PASS / PASS |
| pgcli 4.7.1 | PASS / PASS | Six completion refreshers FAIL; casing PASS; complete refresh FAIL | not run: no wire max-rows API | PASS | not separately probed | PASS / PASS / PASS |
| ADBC PostgreSQL 1.13.0 / Arrow 26.0.1 | PASS / PASS; GetInfo PASS | FAIL: all five GetObjects depths and GetTableSchema | not run: default COPY stream | PASS | not separately probed | PASS / PASS / PASS |
| DBeaver UI | unavailable | unavailable: actual navigator/shipped driver capture absent (R24) | not run | not run | not run | not run |

The nine scripted not-run cells retain the skeleton's client API boundaries
(four positive-fetch probes and five client-specific malformed-frame probes).
The separate raw malformed-frame probe passes. DBeaver UI is outside the 85
scripted cells and remains blocked. JDBC 42.7.14 is pinned in the parent runner;
local 42.7.7 is the available extracted distribution driver, not DBeaver parity.
No client setting disables default preparation, binary results, COPY or SSL
preference. The matrix fixture is one million ordered BIGINT values; exports
validate every value. These are semantic checks, not timings.

Rule: exact PostgreSQL COPY bytes, zero unexplained mismatches. Measured:
**40/40** local PG17.11 comparisons and **3/3** frozen PG15 COPY sequences;
verdict **PASS for the implemented forms**. Text/CSV quoting, NULLs, empty
strings, headers, controls, UTF8, empty results and binary metadata/counts agree.
The reusable COPY wrapper also passes locally. PG16 certification remains open.

Rule: required mutants are CAUGHT, INVALID never counts. The four focused
private-copy controls below are **CAUGHT**, with passing unmutated controls and
zero INVALID/surviving; the **full 75-mutant sweep is NOT RUN** locally.

| Mutant | Focused test | Verdict |
|---|---|---|
| catalog-wrong-oid | exact_adbc_bootstrap_and_unknown_catalog_refusal | CAUGHT |
| refused-catalog-answers-rows | exact_adbc_bootstrap_and_unknown_catalog_refusal | CAUGHT |
| information-schema-column-missing | snapshot_columns_preserve_every_field_in_order | CAUGHT |
| statement-timeout-restart-per-execute | skeleton_fetch_exact_end_cancel_suspended_and_timeout_scope | CAUGHT |

The four controls use `python3 crates/nata-pgwire/mutate.py NAME --filter TEST
--work "$HOME/tmp-s10/mutants"`, with disk TMPDIR. The parent command omits
`--filter` and names to run every inherited/new mutant and its full suite.

## Checks and reproduction

All required local checks pass: fmt, workspace/all-target clippy with warnings
denied, workspace tests (**553 passed, zero failed/ignored, 82 summaries**),
deny (inherited unmatched license allowance warnings only), and nata-check
scan **5/5**, sql **252/252**, sql-differences **55/55**, aggregate **65/65**.
The 36 protocol tests and metadata/COPY fuzz smokes are included in workspace
tests. Wirecapture Python tests **3/3**, Python/shell syntax, job-chain stubs,
and **13** offline ARM/x86 fixture-pair equality checks pass. These are local
x86-64 results, not clean-VM portability, heavy/deep or performance passes.
[Check summaries and raw output](raw/20261108-wire-catalog-slice/local/checks.txt).

```sh
export TMPDIR="$HOME/tmp-s10/tmp"
# Build or use the binaries produced by the workspace checks, with the clients
# installed in a disposable venv as pinned in scripts/wire-skeleton/run.sh.
"$HOME/tmp-s10/venv/bin/python" scripts/wire-skeleton/matrix.py \
  --nata "$PWD/target/debug/nata" --work "$HOME/tmp-s10/matrix" \
  --jdk "$HOME/tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin" \
  --jar "$HOME/tmp-4b/clients/root/usr/share/java/postgresql-42.7.7.jar"
# Matrix exit 1 is the reported incomplete T0 verdict, not a runner failure.
"$HOME/tmp-s10/venv/bin/python" scripts/wire-skeleton/compare-copy.py \
  --nata "$PWD/target/debug/nata" --work "$HOME/tmp-s10/copy-oracle"
```

Scratch `~/tmp-s10` is removed after preserving the evidence. The worktree
build cache remains for the parent's normal cleanup. No Rust crate/version or
unsafe-code inventory change; Cargo.lock is unchanged.

## Protocol changes

COPY option names must be unquoted words; a regression rejects string-literal
option names and fails on the earlier implementation before the guard.

COPY accepts query and registered-table TO STDOUT in default text, explicit
text, CSV and binary; WITH is optional, format values can be quoted. CSV supports
HEADER / HEADER true / HEADER false. Other options, table column lists, FROM,
files and PROGRAM return 0A000. All formats use the existing admission and two
64 KiB byte buffers, cancellation and frame-safe send deadline. There is no
whole-result collection or spool. Text escapes PostgreSQL control characters;
CSV preserves NULL/empty distinctions, quotes embedded quotes/commas/CR/LF and
the one-column end marker. Empty exports retain the CSV header when requested.
Original-query errors retain COPY query offsets; errors in the generated SELECT
for a table form omit the generated offset.

The statement deadline starts at the first Parse/Bind/Describe/Execute and lasts until
Sync, including Parse/Describe planning, admission and suspension. Execute uses
the remaining time, never a fresh duration. Expiry wakes the idle session and
releases suspended producers even before another frontend message or Sync.
Error recovery discards messages until Sync; transaction failure/rollback and
inert suspended CancelRequest retain their existing contracts. Absolute deadline
checks precede completion so a late timer task cannot select success after the
wall deadline. Simple Query keeps its own deadline; timeout errors identify
statement timeout rather than user cancellation.

## Catalog status and proposed next implementation boundary

The existing adapter still serves only the documented information_schema
projections and exact ADBC type bootstrap. A new snapshot regression asserts
that every information_schema.columns field is retained, in ordinal order,
including quoted names, exact numeric precision/scale and database filtering.
It is a regression control, not schema-browser implementation.

[The frozen inventory](20261107-wire-catalog-reference.md) and its 65 captured
client/SQL pairs remain the source for the next change. The unresolved work is
concrete:

- psql describe needs pg_class/namespace/attribute joins, format_type, correlated
  attrdef/collation subqueries, regclass/regnamespace casts and the typed policy,
  publication, statistics and inheritance probes. Listing also needs database,
  namespace and function projections, regex patterns and visibility helpers.
- JDBC metadata needs database/schema/table projections, getColumns' derived
  table and row_number shape, type-name/search-path resolution, primary/index
  metadata and type/function enumeration. Its generated-column/default helpers
  must bind even when the supported fixture has no defaults or constraints.
- psycopg needs complete filtered information_schema tables/columns, parameter
  AST binding and the pg_type/namespace join. pgcli adds array/subscript/helper
  operations for completion. ADBC needs every GetObjects depth, table-schema
  lookup and the captured constraint CTE/UNION/unnest/array_agg shape.

Choose typed metadata operations for those frozen AST shapes in nata-sql and
snapshot-backed evaluation in the session adapter, preserving explicit refusal
for other shapes. Do not add a second general engine or raw-query canned values.
No choice for these shapes has been implemented in this change. A complete
view-by-view PostgreSQL fixture comparison is still required. R24 tracks every
remaining schema-browse FAIL, including actual DBeaver UI capture.

## Fixtures and mutants

The local matrix replays both architectures' suspended fixtures. The cancel
fixture uses an equivalent registered BIGINT projection instead of
`generate_series`; all backend bytes are compared. The timeout fixture replaces
`pg_sleep` with the same empty text values, projects its void result descriptor
to text, and excludes PostgreSQL implementation-source F/L/R error fields.
All remaining payload bytes, SQLSTATE/message, suspension and Sync/rollback
markers are compared. **This projected comparison is not full byte-for-byte
parity of the original timeout fixture.** R25 retains that unmet requirement;
Nata does not fabricate PostgreSQL source-file/line/function diagnostics.

Proposed clarification for the parent, not an accepted amendment: define the
cross-server timeout oracle over S/V/C/M and any position/hint fields, excluding
PG implementation-source F/L/R, and freeze an equivalent supported projection
for the sleep-only result column. Keep literal full-fixture replay as a distinct
PostgreSQL-reference check. Until decided, the requested full-byte gate stays
unmet; this change does not redefine it.

`compare-copy.py` uses the capture's synthetic all_types schema/rows plus a
second control phase for backslashes, the end marker, quotes, UTF8 and controls.
It compares complete backend messages with local PostgreSQL, for table/query,
text/CSV/binary, empty results and CSV headers; three matching frozen PG15 COPY
sequences are also checked. The measured 40/40 and 3/3 results are recorded above.

New mutants are information-schema-column-missing, refused-catalog-answers-rows
and statement-timeout-restart-per-execute; catalog-wrong-oid is retained. All 75
mutation anchors apply exactly once. A focused-test option permits short local
controls; the default command still runs the full required suites. INVALID and
SURVIVED fail the runner. The inherited full sweep belongs on the parent's heavy
VMs and is not inferred from baseline results.

## Parent commands and remaining slice 10

No command below has been run against GCP. Parent supplies approved run ids and
budgets, uses the current worktree tree, and reads every results/*.exit file.
The matrix remains red until schema browsing is implemented; a heavy wrapper
must preserve that failure.

```sh
cp scripts/wire-skeleton/heavy.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --max-vm-hours 2 --run-id "$S10_ARM_RUN" --approved-usd "$S10_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --max-vm-hours 2 --run-id "$S10_X86_RUN" --approved-usd "$S10_X86_BUDGET"
```

The runner installs the PostgreSQL oracle and pinned clients, runs the complete
matrix, COPY oracle, protocol tests and all inherited/new mutants. Heavy fuzz,
loaded reliability and damaged-file steps are retained. Record the installed
PostgreSQL version: local PG17 evidence does not close advertised PG16 parity.

Still required: full schema browsing and view comparison (R24), complete timeout
fixture parity (R25), real DBeaver UI/shipped driver, PG16 comparison, both-CPU
full mutants/deep fuzz/sanitizers and real portal loom bound 3, fresh-machine
RSS/FD/metadata scratch/admission fairness, and performance against P16's frozen
per-client ≤1.10x target (plus the paired ≤1.20x small-query regression gate).
P14 optimization and R22's original driver failure remain visible; none is
claimed closed by these semantic changes. TLS/passwords, writes, SQL cursors,
object-store and aggregate internals remain outside this work.

Tested code-content identity (`scripts/code-tree.sh`):
`6a68104ebf6931f119b28735a32618d987f38f4e00a5bc84b122d52d7127f6f6`.
Branch `t3/catalog-browsing-timeout-copy`, base `fa3d4dc`. The implementation
commit includes this record; no push or merge is performed.
