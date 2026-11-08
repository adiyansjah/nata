# Wire/catalog slice implementation — 2026-11-08

**Scripted schema browsing PASS; slice-10 T0 acceptance remains NOT MET.**
Both local pgjdbc versions produce **76 PASS / 0 FAIL / 9 not run** in the full
client matrix. Every scripted browsing cell passes. The matrix still exits 1
for its nine inherited not-run API cells; actual DBeaver UI is unavailable and
outside those 85 cells. **56/56 projected PostgreSQL statement comparisons**
and **36/36 amendment classifications** pass. R24 stays open for the actual
DBeaver capture, PostgreSQL 16 certification and clean-VM/full acceptance gates.

## Browsing matrix (local x86-64)

| Client | Browsing actions | Verdict |
|---|---|---|
| psql 17.11 | `\d`, `\dt`, `\dn`, `\df`, `\l`; reference replay also covers qualified/pattern, empty and quoted-schema lists | PASS |
| pgjdbc 42.7.14 and 42.7.7, JDK 21.0.13 | getCatalogs, getSchemas, getTables, getColumns, getPrimaryKeys, getIndexInfo, getTypeInfo, getFunctions | all eight PASS in both matrices |
| psycopg 3.3.8 | simple projections, filtered tables/columns stars, type/namespace join; default preparation | PASS |
| pgcli 4.7.1 | schemata, tables, views, types, databases, casing, functions and complete refresh | PASS |
| ADBC PostgreSQL 1.13.0 / Arrow 26.0.1 | GetInfo; all five GetObjects depths; GetTableSchema; real type resolver | PASS |
| DBeaver UI | actual navigator/shipped-driver/init capture | unavailable; remains R24 |

[Final matrices, versions and JDBC transcripts](raw/20261108-wire-catalog-slice/browsing/)
retain every cell, including not-run cells. No preparation, binary, COPY or
SSL-preference workaround; psql runs without `-X`. Existing query, export,
timeout, cancellation, recovery and protocol-fixture cells remain PASS.
These are semantic checks, not performance measurements.

## Query-shape choices and PostgreSQL comparison

Choose **recognised normalized shapes**, option (a), for every captured shape.
The adapter lowers them to typed operations over the connection's immutable
snapshot. It does not execute general catalog SQL or widen nata-sql/user-table
SQL. **45 browsing shapes plus two bootstrap operations cover 56 statements**:
54 metadata statements from the frozen corpus plus two locally captured client
variants. Nine original statements differ only in predicate values. The other
11 original corpus statements use existing session/data/COPY paths.

[Choice, source and reason for every shape](raw/20261108-wire-catalog-slice/browsing/query-shapes.json).
Joins are named snapshot relationships; CASE/type formatting/row ordinals are
fixed typed operations. LIKE and psql identifier patterns apply only to marked
predicate slots. Relation-kind arrays accept the narrow scalar spelling the
clients send and refuse ambiguous syntax. All other tokens, literal meanings,
projections and ordering must match. Parameters remain literal AST values;
SQL/cell text is never pasted into executable SQL. This avoids implementing
array, window, correlated-subquery and join operators for ordinary data.

| Captured statement IDs | Operation and reason |
|---|---|
| psql 0–4, 12, 20, 22; local 65 | table identity/list, schema and catalog operations; only registered objects and explicit compatibility identities |
| psql 5–6, 13–14 | table properties and source attributes; exact OIDs, type lengths, decimal typmods and ordinals |
| psql 7–11, 15–19, 21 | typed empty policy/statistics/publication/inheritance/function queries: these capabilities are absent |
| JDBC 26–32, 35–36 | catalogs, namespaces, tables, columns and supported scalar type operations; ordinals replace the captured row_number construct |
| JDBC 33–34, 37; local 66 | typed empty keys/indexes/functions; 42.7.7 reads indexes for primary keys, 42.7.14 reads constraints |
| psycopg 40–42 | complete captured table/column stars and the supported type/namespace join |
| pgcli 45–48, 51 | effective schema path, namespaces, registered tables/columns and catalogs |
| pgcli 49–50, 52 | typed empty foreign keys, user-defined non-table types and SQL-callable functions |
| ADBC 54–55, 57–62 | attribute/type bootstrap and catalog/schema/table/column/schema operations; constraint query binds to truthful absence |

[Statement-by-statement rows and descriptors](raw/20261108-wire-catalog-slice/browsing/comparison.json)
compare the exact SQL against local PostgreSQL **17.11**, using
`scripts/wirecapture/fixture.sql` and equivalent registered Parquet schemas
(including empty/quoted schemas, duplicate table names and all mapped types).
The reference retains its three synthetic data rows; the Nata oracle files can
be empty because these statements read schema, not user data or table statistics.
The normal client matrix separately validates million-row data/export fixtures.
No unavailable PostgreSQL run is presented as measured. The frozen reference
is PG15.19; the advertised compatibility marker remains 16.0. Live PG16 and
actual DBeaver version parity remain open.

Declared projections, applied **only to PostgreSQL**:

- Restrict PostgreSQL storage/system objects, built-in functions and types to
  Nata's registered objects and ten scalar wire types. Nata has no hidden heap
  attributes, SQL-callable functions, user-defined types, keys or indexes.
  Unexpected extra Nata rows are never filtered out.
- Map generated relation OIDs by `(catalog, schema, table)` identity. Built-in
  type/array/receive identities and all descriptor names/type OIDs/sizes/typmods
  are exact. Table/attribute origin descriptors retain ADR 0008's zeros. Attribute bootstrap groups may move under identity mapping; ordering
  within each group is preserved. Other captured ORDER BY sequences are exact;
  only statements without ORDER BY use multiset comparison.
- Map the reference owner to the documented local compatibility owner `nata`.
  Read-only insert/update flags become NO. PostgreSQL heap access method and
  logical replica identity project to Nata's absent method / `n`. Locale-provider
  is NULL: no libc/ICU implementation is claimed; C reports bytewise ordering.
- PG17 renamed PG15/16's `daticulocale`; replace that reference expression with
  typed NULL for this C-locale fixture. ICU locale/rules and ACLs are absent.
  The psql 17 variant adds `array_length` and ICU-rules projection; it was captured
  against Nata's unchanged 16.0 marker and checked against PG17 with that rename.

The oracle caught and repaired yes/no varchar(3) typmods, domain/UDT positions,
text octet length and identity-cycle metadata. Its tightened reference-only
filter also exposed a quoted-table projection error in the harness, now fixed;
that row is compared, not discarded. The new primary-key variant is captured
with pgjdbc 42.7.7 through the existing framed proxy against PostgreSQL.

ADR 0014 permits the provider transition after the default resolver passes:
bootstrap attributes now reflect registered columns; receive-function **names**
are retained, while built-in array identities and metadata-only oid/name/char/
regproc result descriptors match PostgreSQL. The binary metadata encoders have
an exact receive-function/OID vector control. This adds no general array or SQL
type capability and edits no accepted ADR.

[All 36 original client/object additions](raw/20261108-wire-catalog-slice/browsing/inventory-36.json)
are classified, linked to their compared statements and exercised. Captured
queries are served from the snapshot or statically bound to a typed absent
capability. Standalone/other shapes remain 0A000; the inherited direct 36-probe
protocol test still verifies refusal before rows. No unknown object receives
an invented empty success. The two new local variants add only the captured
`daticurules`/`array_length` and `indnkeyatts` uses inside their exact shapes.

## Mutants and checks

Rule: required mutants must fail assertions; INVALID never counts. Measured:
**6/6 focused controls CAUGHT**, passing unmutated lib/protocol controls, zero
survivors/INVALID. All 75 inherited mutants remain registered and all **78**
edit anchors apply exactly once. The full sweep is **NOT RUN locally** under
way-of-working §8's heavy-run rule; it remains a parent gate, not a claimed pass.

| Mutant | Focused control | Verdict |
|---|---|---|
| catalog-wrong-oid (pg_type) | exact_adbc_bootstrap_and_unknown_catalog_refusal | CAUGHT |
| browse-wrong-attribute-oid | snapshot_filters_identity_types_and_ordering | CAUGHT |
| information-schema-column-missing (thin adapter) | snapshot_columns_preserve_every_field_in_order | CAUGHT |
| browse-column-missing (captured star) | snapshot_filters_identity_types_and_ordering | CAUGHT |
| refused-catalog-answers-rows | exact_adbc_bootstrap_and_unknown_catalog_refusal | CAUGHT |
| browse-wrong-column-order (JDBC's ORDER BY) | snapshot_filters_identity_types_and_ordering | CAUGHT |

Reproduce with disk TMPDIR and a private dependency cache:

```sh
python3 crates/nata-pgwire/mutate.py information-schema-column-missing \
  browse-wrong-attribute-oid browse-column-missing browse-wrong-column-order \
  --filter snapshot_ --work "$HOME/tmp-browse/mutants"
python3 crates/nata-pgwire/mutate.py catalog-wrong-oid refused-catalog-answers-rows \
  --filter exact_adbc_bootstrap_and_unknown_catalog_refusal \
  --work "$HOME/tmp-browse/mutants"
```

Required local checks PASS, serial with TMPDIR on disk: fmt; workspace/all-target
clippy with warnings denied; workspace tests **556 passed, zero failed/ignored,
82 summaries**, including all **36 protocol tests**; deny (inherited unmatched
license-allowance warnings only); nata-check scan **5/5**, SQL **252/252**,
differences **55/55**, aggregate **65/65**. Wirecapture harness tests **3/3**,
Python/shell syntax and job-chain stubs PASS. [Checks and mutation logs](raw/20261108-wire-catalog-slice/browsing/checks.txt)
record the commands and limitations. [Binary controls](raw/20261108-wire-catalog-slice/browsing/binary-comparison.json)
compare type descriptors and every payload byte for all ten bootstrap types and
both captured attribute lists: **3/3 PASS**, including empty internal-char values.
Staged leak scan and whitespace checks PASS before the commit. Whitespace-only
normalization of the captured templates was followed by fmt/clippy, all three
shape tests and the full 56-statement / three-binary-control oracle.

Tested code-content SHA-256 (`scripts/code-tree.sh`):
`441f2e3dc3761a828236467dac25ef9a3cfd8675ee05e13ed03b12c98a20cb29`.

## Reproduction and parent GCP command

```sh
mkdir -p "$HOME/tmp-browse/tmp"
export TMPDIR="$HOME/tmp-browse/tmp"
cargo build -p nata -p nata-check
# Install the same pinned clients as scripts/wire-skeleton/run.sh in a venv.
"$HOME/tmp-browse/venv/bin/python" scripts/wire-skeleton/matrix.py \
  --nata "$PWD/target/debug/nata" --work "$HOME/tmp-browse/matrix" \
  --jdk "$HOME/tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin" \
  --jar "$HOME/tmp-browse/pgjdbc-42.7.14.jar"
# Against a loopback PostgreSQL initialized with scripts/wirecapture/fixture.sql:
"$HOME/tmp-browse/venv/bin/python" scripts/wire-browse/compare.py \
  --nata "$PWD/target/debug/nata" --work "$HOME/tmp-browse/comparison" \
  --postgres 'host=127.0.0.1 port=55432 dbname=nata'
```

No GCP command below was run. After the parent's cost approval, use the same
full matrix/oracle/protocol/fuzz/**all 78 mutants** on both CPUs. The first step
queries instance metadata for the actual machine type from inside each VM;
retain it and every step's `.exit` file. Matrix exit 1 for the nine disclosed
not-run API cells remains visible. The wrapper is not slice-10 acceptance.

```sh
cp scripts/wire-browse/heavy.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --max-vm-hours 1.5
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 1.5
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --max-vm-hours 1.5 --run-id "$BROWSE_ARM_RUN" --approved-usd "$BROWSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --max-vm-hours 1.5 --run-id "$BROWSE_X86_RUN" --approved-usd "$BROWSE_X86_BUDGET"
```

## Remaining slice 10

Actual DBeaver navigator/shipped-driver/init capture; PG16/version certification;
clean ARM64/x86-64 replay and the full inherited/new mutant sweep; SHOW/DESCRIBE
and the remaining settings/RESET/pg_settings helpers; real portal lifecycle
model/loom bound 3 and race mutants; loaded metadata scratch/RSS/FD/fairness and
large-catalog gates; deep/sanitizer/fuzz campaigns and frozen paired performance
gates. The supported surface is the frozen corpus, not general PostgreSQL
catalog emulation. R24 remains open for the unverified browsing certification.
TLS/passwords, writes, Iceberg/object-store providers, general user SQL and
streaming optimization remain outside this change.

Owned `~/tmp-browse` scratch and its local PostgreSQL cluster are removed after
preserving evidence. The worktree build cache remains for the parent's normal
post-merge cleanup. No push, merge, GCP, new Rust dependency/version or unsafe
code. Branch: `t3/catalog-schema-browsing`; implementation and evidence are in
one atomic commit following the checks below.

## Earlier COPY/deadline handoff (historical browsing verdicts)

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

## Clean-VM checks on both CPU types (parent, runs `wbrowse-n4a` and `wbrowse-c3d`, 2026-11-08)

Commit `838e394` (schema browsing merged), `scripts/wire-browse/heavy.steps.example`, n4a-highcpu-16 (ARM64) and c3d-highcpu-16
(x86-64), Spot, approved $0.34 and $0.23; both VMs and disks deleted and verified. Raw results stay in the gitignored
`bench/gcp-runs/`.

| Step | ARM64 | x86-64 |
|---|---|---|
| Client matrix | 76 PASS / 0 FAIL / 9 not run (exit 1 by design: "not run" is not PASS) | same |
| Protocol tests | 36/36 | 36/36 |
| Metadata fuzz, 1,000,000 cases | PASS | PASS |
| Wire mutant sweep | 78/78 CAUGHT, 4 unmutated controls PASS | 78/78 CAUGHT, 4 controls PASS |
| Browse oracle (PostgreSQL **15.19**, Debian 12) | 3 FAIL: statements 8, 16, 65 | same 3 |

The three oracle failures were version gaps in the comparison script, not Nata: PG15 has no `pg_database.daticurules`
(PG16+), so statement 65's reference query failed; PG15 types `pg_statistic_ext.stxstattarget` as int4 where PG17 (Nata's
target) uses int2, and statements 8 and 16 return no rows either way. Fixed in `a956048` and `ba2df5a`; the PG17 local run
still passes 56/56 plus the three binary cases. The fixed oracle is rerun on both CPU types as the last steps of the step 2b
timing job (runs `perf2b-c3d`, `perf2b-n4a`). PG15 is not PG16: PG16 parity stays open.

## Slice-close implementation handoff: item 1 (2026-11-08)

**Partial; slice 10 remains OPEN.** Item 1 is implemented and checked locally.
Stop under the brief's one-turn limit before item 2: a production lifecycle model
requires extracting the real `server/fetch.rs` Gate/Run/Deadline transitions and
`server.rs` cancel-slot/Sync deadline ownership behind a loom-compatible seam,
then demonstrating reachable races and mutant failures. A separate toy model
would not satisfy the card. Items 2–7 are recorded below, not silently omitted.
No GCP, push, merge, timing run, new dependency, Cargo.lock change, unsafe code,
ADR amendment or target change. R22, R24, R25, P14 and P16 remain open.

| Item | Rule | Numbers / verdict | Command / remaining implementation |
|---|---|---|---|
| 1. SHOW/DESCRIBE/settings | Snapshot-backed values and exact typed metadata; one validated session registry; unsupported settings fail clearly; zero unexplained PG17 mismatches | **PASS for the documented bounded grammar**: 95/95 new cases (85 PG pairs, including 9 binary cases; 1 registry-consistency check; 9 explicit refusals). Existing corpus 56/56, binary controls 3/3, missing-inventory classifications 36/36 remain PASS. Oracle PostgreSQL 17.11 from `/usr/lib/postgresql/17`. | `scripts/wire-browse/compare.py` command below; raw `oracle/` |
| 2. Real portal loom | Bound 2 local / 3 heavy; inert suspended cancel, timeout through Sync, exactly-once cleanup, token replacement, lost-wake/double-release/stale-token mutants | **NOT RUN / NOT PREPARED**; existing 36 protocol tests remain passing evidence, not a loom substitute | Heavy `portal-loom-bound-3` is an explicit exit-1 blocker. Implement the production seam and race mutants before replacing it. Loom is already in Cargo.lock; no dependency approval was assumed. |
| 3. Loaded resources / large catalog | 16 MiB metadata cap; inherited two 64 KiB queued buffers, 5 s idle expiry and 2 s cleanup/send bounds; 64 sessions, 1/8 workers; 64/1,024/10,000 tables, many schemas/columns; fresh/default and forced FD limits; healthy-query admission progress | **NOT RUN / NOT PREPARED**. Reference record explicitly leaves new RSS/scratch adequacy and full-client contention budgets unfrozen; no invented RSS threshold | Heavy `resource-gates` exits 1. Add independent scratch/RSS/FD/admission sampling, cap-removal controls and large-catalog reference controls; settle any required reference-derived budget with the parent before Nata measurements. |
| 4. Nine API cells | Matrix exits 1 unless every row is PASS; raw protocol probes never relabel a client-specific gap | **76 PASS / 0 FAIL / 9 not run; exit 1, acceptance NOT MET**. No client-specific new probes in this turn | `WORK=... RESULTS=... bash scripts/wire-skeleton/run.sh` below. Remaining: positive-fetch for psql/psycopg/pgcli/ADBC; malformed-frame for all five clients. Determine APIs/probe feasibility individually, record precise inability reasons where applicable; keep exit rule. Heavy `client-api-gaps` exits 1. |
| 5. PG16 | Official PGDG apt repository on Debian 12; record actual major/version and compare the same corpus, no PG15 substitution | **NOT RUN / NOT PREPARED**, including installer dry-run | Heavy `pg16-parity` exits 1. Add explicit version-selected installation/cluster mode to the oracle wrapper, package/source verification, local dry-run controls and both-CPU replay. PG17 evidence does not certify the advertised PG16 marker. |
| 6. DBeaver / R24 | Shipped driver/version plus connection-init and navigator SQL captured faithfully; actual UI mandatory for UI certification | **NOT RUN / NOT PREPARED**; real UI unavailable, R24 OPEN | Heavy `dbeaver-r24` exits 1. Identify the actual shipped pgjdbc and DBeaver source version, replay its documented init/navigator queries headlessly, disclose the lack of UI capture; generic JDBC remains separate evidence. |
| 7. P16 | Frozen streaming median ≤1.10x matched PG per client/arm/CPU at 1/8 workers; paired pre-change small-query regression ≤1.20x, planted slowdown fails | **NOT RUN / NOT PREPARED**; no local timing, no threshold changes | Heavy `p16-streaming-and-regression` exits 1. Build the same-client/fixture/sink alternating primary/twin harness, correct-output controls, min/median/spread and miss profiles; run only on the parent's two heavy VMs. |

The item-1 grammar adds the card's SHOW forms and DESCRIBE with qualified/quoted
registered names, search-path resolution and positioned missing-object errors.
The same live registry supplies SET, RESET name/ALL, SET name=DEFAULT, SHOW ALL,
current_setting's missing_ok and literal set_config(...,false), plus explicit
pg_settings projections with optional `WHERE name='literal'` / `ORDER BY name`.
Transaction-local settings and unsupported shapes stay 0A000. pg_settings reports
Nata's restricted domains, defaults, descriptions and version marker, rather
than pretending to expose PostgreSQL's entire configuration. Startup values are
retained for RESET. Timeout SHOW/current_setting uses PostgreSQL's display units;
pg_settings retains milliseconds. ISO DateStyle retains MDY/DMY reporting and
ISO alone preserves the current order. `current_schema()` no longer invents
public without a registered schema; current_schemas uses the effective path,
including exact PostgreSQL name-array text/binary output.

**Checks:** `cargo test --locked -p nata-pgwire -j 4`: **59 PASS**
(16 library, 3 codec, 36 protocol, 4 reliability; zero failed/ignored).
`cargo clippy --locked -p nata-pgwire --all-targets -j 4 -- -D warnings`,
workspace fmt check, Python compilation, all 12 heavy-step shell syntax checks,
whitespace and staged leak scan PASS. Metadata fuzz smoke now also evaluates
session helpers; the million-case heavy command is retained, not run locally.
The local matrix exit **1** is a real remaining acceptance failure.

**Mutants:** **7 CAUGHT / 0 MISSED / 0 INVALID**, with the unmutated focused
three-test control PASS: settings-name-array-wrong-element,
settings-reset-loses-startup, settings-missing-ok-invented-empty,
settings-timeout-display-units, settings-datestyle-ignores-dmy,
show-describe-loses-column, settings-pg-settings-wrong-value.
All **85/85** mutation anchors apply once. Full 85-mutant and both-CPU runs are
NOT RUN; the parent's prior 78/78 CPU results do not certify these additions.
The oracle also rejected three incorrectly quoted harness setup cases; corrected
identifier syntax passes. That red control is retained separately. Sharing a
local target cache with the private-copy mutant run left a mutant test artifact;
package-specific clean and final real-source tests/clippy remove that cache
contamination. Sources were unchanged by the private-copy runner. Raw commands,
logs and the distinction between controls and final passes are in
[raw evidence](raw/20261108-wire-catalog-close/).

Reproduce serially (Python 3.12 here because system Python lacked ensurepip;
extracted local JDK 21, pgjdbc 42.7.14 and the script's pinned clients):

```sh
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_TARGET_DIR="$HOME/tmp-slice-close/target"
export TMPDIR="$HOME/tmp-slice-close/tmp"
export WORK="$HOME/tmp-slice-close/clients"
export RESULTS="$HOME/tmp-slice-close/matrix"
# PATH must contain a Python with venv/ensurepip, JDK 21, psql and pg_config.
bash scripts/wire-skeleton/run.sh  # expected exit 1: nine non-PASS cells
cargo test --locked -p nata-pgwire -j 4
cargo clippy --locked -p nata-pgwire --all-targets -j 4 -- -D warnings
cargo fmt --all --check
# Initialize a local PG17 trust cluster with scripts/wirecapture/fixture.sql first.
"$WORK/wire-skeleton-venv/bin/python" scripts/wire-browse/compare.py \
  --nata "$CARGO_TARGET_DIR/debug/nata" \
  --work "$HOME/tmp-slice-close/oracle-replay" \
  --postgres 'host=localhost port=55432 user=nata dbname=nata'
CARGO_BUILD_JOBS=4 python3 crates/nata-pgwire/mutate.py \
  settings-name-array-wrong-element settings-reset-loses-startup \
  settings-missing-ok-invented-empty settings-timeout-display-units \
  settings-datestyle-ignores-dmy show-describe-loses-column \
  settings-pg-settings-wrong-value --filter helpers \
  --work "$HOME/tmp-slice-close/private-mutants"
# Keep private-mutants/target separate from the real target cache.
```

All GCP preparation for this handoff is in
`scripts/wire-browse/slice-close.steps.example`. Its first six steps are runnable;
the six remaining steps deliberately exit 1 and state what is unprepared. Every
`.exit` is a gate, including matrix exit 1. **Do not launch this partial template
as slice-close acceptance.** After implementing the missing gates, the parent's
exact plan/run commands are below (not executed here). n4a-highcpu-16 and
c3d-highcpu-16 retain the prior clean-VM CPU classes with room for eight query
workers and four build jobs. The two-hour cap per CPU gives the prepared prefix
margin over the earlier 1.5-hour allowance for seven more mutants/new oracle
cases; it is a spend ceiling, not a guarantee for unimplemented deep checks.
A 60 GB data disk gives the real/private build caches and retained logs headroom.
The first heavy step records actual machine type from instance metadata; do not
infer it from a run-id. Parent obtains the cost bound from `plan` and supplies
approved budgets; this handoff authorizes no spend.

```sh
cp scripts/wire-browse/slice-close.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_ARM_RUN" --approved-usd "$S10_CLOSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_X86_RUN" --approved-usd "$S10_CLOSE_X86_BUDGET"
```

In addition to items 2–7, closeout still needs closing-commit deep/fuzz/sanitizer
checks and the unchanged R25 full timeout-fixture decision. This is the item-1
implementation/evidence commit on `t3/wire-catalog-slice-close`, base `3e413b8`;
its SHA is supplied in the handoff. The local PostgreSQL cluster is stopped after
preserving evidence; scratch/build caches are local and contain no cloud data.

Item-1 tested code-content SHA-256 (`scripts/code-tree.sh`):
`224dfaa80ac330b2dfab67ef125a45706c3414b96b26564b0cbd885c5ff05aa9`.

## Slice-close continuation: item 2 (2026-11-08)

**Partial; slice 10 remains OPEN.** This continuation from `30cbbda` finishes
item 2's implementation and local controls, then hands back before item 4.
Items 4/3/5/7/6 are not implemented by this commit. The heavy template now has
**five**, rather than six, explicit unprepared exit-1 gates. Do not launch it as
complete slice-close acceptance. No GCP, timing benchmark, push, merge, new
locked package, unsafe production code, ADR amendment or target change.

| Item | Rule | Numbers / verdict | Command / remaining work |
|---|---|---|---|
| 2. Production portal lifecycle | Real production transitions; local loom bound 2 / heavy bound 3; suspended cancel inert, timeout through Sync, wake and exactly-once cleanup, replacement isolation, error drain | **Local PASS:** 7 models, **328 executions** at bound 2; **11/11 focused mutants CAUGHT**, 0 missed, 0 INVALID. Bound 3 and both-CPU replay **NOT RUN**, prepared as a heavy gate | `LOOM_MAX_PREEMPTIONS=2 bash crates/nata-pgwire/run_portal_loom.sh`; focused commands in raw evidence below |
| 4. Nine client API cells | Every mandatory matrix row must be PASS; impossible APIs require a proposed parent-approved deviation, never a silent pass | **Acceptance FAIL:** 76 PASS / 0 FAIL / **9 not run**, matrix exits **1**. No new API probe or source-backed impossibility decision in this continuation | `WORK=... RESULTS=... bash scripts/wire-skeleton/run.sh`; next investigate positive-fetch for psql/psycopg/pgcli/ADBC and malformed-frame for all five clients, individually. Propose, do not apply, any deviation mechanism |
| 3. Loaded resources / large catalog | 16 MiB metadata cap, two 64 KiB queued buffers, 5 s idle expiry, 2 s cleanup/send; 64 sessions, 1/8 workers, 64/1,024/10,000 tables, default/forced FD limits and healthy admission progress | **NOT PREPARED / NOT RUN** beyond inherited native controls. New metadata RSS/scratch adequacy and full-client contention budgets remain unfrozen in the reference record; no threshold invented | Replace `resource-gates` blocker with independent scratch/RSS/FD/admission sampling, cap-removal controls and matched large-catalog reference controls. Parent must resolve any missing reference-derived budget before Nata resource acceptance |
| 5. PG16 parity | Official PGDG apt source on Debian 12; verify actual major/version and compare identical corpus on both CPUs | **NOT PREPARED / NOT RUN**. This turn's oracle is PG17.11, not PG16 | Replace `pg16-parity` with an explicit PG16 installer/cluster/oracle mode, source/package verification and dry-run/failure controls; retain default PG15 distinction |
| 7. P16 | Frozen per-client/arm/CPU streaming median ≤1.10x matched PostgreSQL, 1/8 workers; paired small-query/prepare/reuse regression ≤1.20x pre-change Nata; planted slowdown fails | **NOT PREPARED / NOT RUN**. No local timing or threshold changes | Replace `p16-streaming-and-regression` with correctness-first alternating primary/twin harness, exact same clients/fixtures/sinks, min/median/spread, first-row check and miss profiles; run timing only on parent's heavy VMs |
| 6. DBeaver / R24 | Actual shipped pgjdbc and source-derived connection-init/navigator capture; real UI required for UI certification | **NOT PREPARED / NOT RUN; R24 OPEN** | Replace `dbeaver-r24` with version-pinned DBeaver source/JAR evidence and faithful headless replay; disclose that headless replay cannot certify actual navigator UI |

### Production seam and model limits

`crates/nata-pgwire/src/server/lifecycle.rs` is included directly by the model
and compiled into the server. `fetch::Gate` uses its row-credit transition and
sticky close/wake; `Run` uses its phase/terminal Execute decision; `Session` uses
its cancel slot; expiry and Drop share its take-once join owner. The event loop
uses its first-message deadline, error-drain and Sync state. Terminal Execute
and deadline-start decisions live in the core itself, so an outer production
guard cannot mask those core mutants. The credit slot holds one Execute credit,
not an unbounded resume queue; cancel selection stays locked through the cancel
operation, before suspension publication.

The crate-specific `nata_portal_loom` cfg swaps only that seam's Mutex/Condvar.
Reusing the scheduler's `nata_loom` cfg made downstream Parquet's std/loom Arc
APIs incompatible: that build failure is retained as **INVALID infrastructure**,
not a mutant catch. A script edit overlapping that first running shell also
failed; the finalized script subsequently passed. Both errors are retained in
raw evidence, separate from the final passing commands.

The seven cases explore **4, 3, 11, 3, 4, 302 and 1** executions respectively:
cancel versus suspension publication; close versus park; four rows/two exact-end
fetches, inert suspended cancel and resume; expiry credit versus park; pinned
timer versus replacement; resume/expiry/cleanup ownership; and timeout across
suspension followed by ignore-until-Sync and transaction/idle Sync cleanup.
Completed/expired Execute decisions are terminal. Clock ticks, I/O and rollback
cleanup are supplied boundary events: this does **not** instrument Tokio,
sockets, scan internals, engine admissions/reservations or actual wall-clock
delivery, nor prove unbounded sessions/fetches. Native tests check that wiring;
item 3 still owes loaded resource/cleanup evidence. See
`crates/nata-pgwire/PORTAL_MODEL.md` for bounds and reproduction.

**Mutants:** all ten model controls CAUGHT: terminal resume, cancel selected
outside the publication lock, lost credit notification, lost sticky close,
non-idempotent cleanup, stale installed target, suspended cancel target retained,
timeout restarted, error drain lost, and Sync error reset lost. The additional
real-Tokio `portal-deadline-follows-replacement` mutant is CAUGHT by the native
pinning test. The double-release control makes repeated cleanup try to own an
already-taken join and panic; safe Rust cannot copy that non-Clone join handle.
Lost wakes fail with loom deadlocks, not wall-clock timeouts. Every unmutated
control passes; source fingerprints remain unchanged by private-copy mutation.
All **96/96** registered edit anchors apply once. The full 96-mutant sweep is
**NOT RUN locally**; the heavy wire-mutant step now also selects bound 3.

**Final checks, after centralizing both decisions:** `cargo test --locked -p
nata-pgwire -j 4`: **66 passed**, 0 failed/ignored (16 lib, 3 codec, 7 native
model cases, 36 protocol, 4 reliability). Workspace fmt check, targeted all-target
clippy with warnings denied, cargo-deny, Python/shell syntax and job-chain stubs
PASS. Deny retains only existing unmatched allowance warnings. Local matrix
acceptance **FAILS** as above. Local `/usr/lib/postgresql/17` oracle **PASS**:
56/56 browse statements, 3/3 binary controls, 95/95 helper cases and 36/36 inventory
classifications. PostgreSQL **17.11** / Nata marker **16.0**, x86-64, rustc
**1.99.1**, local JDK **21.0.13**; no clean-VM/performance claim. R22, R24, R25,
P14 and P16 remain open, including R25's full timeout-fixture decision.

[Raw evidence and commands](raw/20261108-wire-catalog-close/portal-model/)
include all focused failures, final model counts, final test/clippy/deny and
matrix/oracle outputs. Text logs have trailing whitespace normalized only; JSON
oracle payloads are unchanged. Builds/tests ran serially at `-j 4`. Private pgwire
artifacts were package-cleaned before final real-source tests; obsolete build
outputs and this run's stopped oracle cluster were reclaimed as disk approached
20 GB. No package/version was added to Cargo.lock: only the conditional edge to
already-locked **loom 0.7.2**. Its existing model-only unsafe inventory (lexical,
including docs/comments) is recorded separately; no new unsafe code was added.

Tested code-content SHA-256 (`scripts/code-tree.sh`):
`d082db8f7edb3a1f520e5d3aac338ba63dee86117101c9719fdfa6ef892a1d03`.
Commit SHA is supplied in the implementation handoff, avoiding self-reference.

### Parent heavy commands (not executed; template still partial)

The previous n4a-highcpu-16 / c3d-highcpu-16 pairing retains reference CPU
classes and capacity for eight query workers, four build jobs and load/sampling
actors. The inherited **2-hour cap per CPU** and **60 GB data disk** remain a
reasoned ceiling/headroom for the prepared prefix, not a certified estimate for
the five unbuilt gates. Re-plan sizing/hours after implementing them and obtain
the real spend bound from `plan`. The first step records actual machine type
from instance metadata; every `.exit`, including matrix exit 1, is a gate.
Closing-commit bound-3, full mutants, deep/fuzz/sanitizer and both-CPU runs remain
mandatory, in addition to the five unfinished items above.

```sh
cp scripts/wire-browse/slice-close.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_ARM_RUN" --approved-usd "$S10_CLOSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_X86_RUN" --approved-usd "$S10_CLOSE_X86_BUDGET"
```

## Slice-close continuation: item 4 and resource-rule stop (2026-11-08)

**Slice 10 remains OPEN; stopped at item 3 under the brief's rule-change stop
condition.** This continuation starts at main `7719aca` on
`t3/slice-10-wire-gates`. Item 4's API investigation is complete, but acceptance
still fails: no client chunking or raw frame injection is relabelled as a positive
wire-fetch/client malformed-frame PASS. No ADR, target, dependency, production
Rust code or matrix exit rule changed. No GCP, timing benchmark, push or merge.

| Item | Rule | Numbers / verdict | Command / remaining work |
|---|---|---|---|
| 4. Client API cells | All mandatory rows PASS, or owner-recorded explicit deviations; matrix exits 1 for any non-PASS | **81 PASS / 0 FAIL / 9 not run; exit 1 (acceptance FAIL)**. Four positive-fetch APIs cannot set wire Execute.max_rows; five client APIs cannot emit invalid framing. All nine have individual source-backed reasons in `scripts/wire-skeleton/api-limits.json`. Extra chunk/buffer/refusal checks are separate evidence | `WORK=... RESULTS=... bash scripts/wire-skeleton/run.sh`; propose deviations below, not applied |
| 3. Resources | Freeze reference-derived RSS budget before new Nata measurements (card §2/§3); retain 16 MiB metadata, bounded bridge, 5 s expiry, 2 s cleanup, 64 sessions and 1/8 workers | **STOP / NOT PREPARED.** Reference resource decision explicitly leaves RSS and metadata scratch adequacy unfrozen; later correction freezes streaming only. Both item-1 and item-2 handoffs retain the gap. No numerical process RSS ceiling is supplied | Parent must supply the reference-only RSS/adequacy freeze. A scratch/queue cap is not an RSS bound; inventing a ceiling or accepting without it would change the card rule |
| 5. PG16 | Official PGDG bookworm-pgdg install; verify major and replay | **NOT STARTED after stop**; PG17 check below is not PG16 certification | Existing `pg16-parity` blocker remains |
| 7. P16 | Per-client matched PG ≤1.10x and paired pre-change small-query ≤1.20x, primary/twin alternating, 1/8 workers, settled VMs | **NOT STARTED after stop**; no timings run | Existing performance blocker remains; no target relaxation |
| 6. DBeaver / R24 | Shipped pgjdbc/init/navigator source replay, real UI for UI certification | **NOT STARTED after stop; R24 OPEN** | Existing DBeaver blocker remains; generic JDBC is separate evidence |

### Client API evidence and proposed deviation

psql **17.11** uses `PQsetChunkedRowsMode` for FETCH_COUNT; older psql used
SQL DECLARE/FETCH. Neither exposes a wire Execute row-limit option. psycopg
**3.3.8** and pgcli **4.7.1** use libpq (its extended builder emits Execute with
row limit zero); ServerCursor uses the explicitly excluded SQL cursor path.
ADBC PostgreSQL **1.13.0**, libraries release **25**, defaults to COPY; Arrow
batch sizes are not portal row credits. PGConnection/JDBC **42.7.14** has the
existing positive-fetch probe, but no public malformed-header API. psycopg
socket descriptors and JDBC socket factories can support separate transport
fault injection, not a client framing-emission claim. The nine precise reasons
and primary-source links are preserved in each matrix artifact, with API details
and limits in `scripts/wire-skeleton/API-LIMITS.md`.

New checks verify psql FETCH_COUNT=2 output, psycopg/pgcli/ADBC fetchmany(2)
values/count/exhaustion, and psycopg ServerCursor's 0A000 followed by rollback
and healthy reuse. They do not pass the nine unresolved cells. Raw malformed
framing and the byte-exact positive-fetch fixtures remain independent rows.

**Proposed mechanism for parent approval, not implemented as an acceptance
bypass:** a per-cell record with exact client/version/action, source/API reason,
substitute evidence and its limits, decision owner/date, tested commit and
revisit trigger. Retain original matrix JSON/exit 1. The separate owner closeout
verdict may cite accepted deviations; never rewrite rows to PASS, omit them or
make the matrix return zero. Suggested substitutes: existing pgjdbc/raw wire
fixtures for positive-fetch, raw/native codec/fuzz controls for malformed frames.

### Checks and controls

**PASS:** `cargo test --locked -p nata-pgwire -j 4`: **66 passed**, zero
failed/ignored (16 lib, 3 codec, 7 model/native, 36 protocol, 4 reliability).
`cargo fmt --all --check`, all-target pgwire clippy with warnings denied, Python
compilation and whitespace check PASS. Local matrix exit **1**, as required:
**81 PASS / 0 FAIL / 9 not run**. Local PostgreSQL **17.11** oracle exit **0**:
**56/56** browse statements, **3/3** binary controls, **95/95** helper cases and
**36/36** inventory classifications PASS. Nata marker remains **16.0**; no PG16
certification. x86-64, rustc **1.99.1**, JDK **21.0.13**, pyarrow **26.0.1**;
client versions in raw evidence. All local builds serial at four jobs with a
separate target. Disk fell to 19.78 GiB: after checking active cargo/rustc
commands, removed only the inactive old worktree's incremental cache, restoring
25.41 GiB. No active target or source was removed. Staged leak scan PASS.

Focused acceptance controls: **12/12 PASS**, covering every unresolved cell
individually and a failed cell alongside PASS. **2 CAUGHT / 0 MISSED** gate
mutants (`any`→`all`, ignore `not run`) fail those controls. These are harness
acceptance mutants, not production concurrency/codec mutants. No production
code changed; full 96-mutant/bound-3 and closing-commit deep campaigns are NOT
RUN and remain in the heavy template. Raw evidence stays small in
[client-api evidence](raw/20261108-wire-catalog-close/client-api/), including
exact commands, versions, matrix outcomes, oracle summaries and deciding logs.

### GCP preparation remains blocked

The five explicit blocked gates in `slice-close.steps.example` are intentionally
retained. Do not run the partial template as acceptance. Making every gate
prepared would require resolving item 3's missing reference freeze first.
The inherited **n4a-highcpu-16 / c3d-highcpu-16**, **60 GB data disk**, **2-hour
cap per CPU** are only the prior prefix's reasoned sizing: eight query workers,
four build jobs and actors, with separate native/private caches. They are not a
validated duration estimate for all remaining heavy work. Parent must re-plan
hours after preparing those gates; obtain the cost bound from plan, not from an
invented dollar estimate. Machine type still comes from instance metadata;
every `.exit`, matrix exit 1, bound-3 loom and full mutant sweep remain gates.

Exact inherited commands below are **not executed and not runnable closeout
acceptance until blockers are resolved**:

```sh
cp scripts/wire-browse/slice-close.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_ARM_RUN" --approved-usd "$S10_CLOSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 60 --max-vm-hours 2 --run-id "$S10_CLOSE_X86_RUN" --approved-usd "$S10_CLOSE_X86_BUDGET"
```

Closeout still requires approved API deviations (or supported probes for all
cells), item 3's reference freeze and loaded gates, items 5/7/6, real DBeaver UI,
R25's full timeout-fixture decision, both-CPU and closing-commit deep evidence.
Commit SHA is supplied in the handoff, avoiding self-reference.

## Parent decisions, 2026-11-08 (after item 4)

- **The nine client-API cells are accepted as recorded deviations**, with the mechanism proposed above: one record per cell (client,
  version, action, the API reason with its source link, the substitute evidence and its limits, decision owner and date, tested
  commit, revisit trigger) in `scripts/wire-skeleton/deviations.json`. Substitutes: the byte-exact pgjdbc and raw-wire positive-fetch
  fixtures for the four positive-fetch cells; the raw codec, fuzz and malformed-frame protocol tests for the five malformed-frame
  cells. The matrix JSON keeps "not run" and the matrix still exits 1; only the closeout verdict cites the deviations. Revisit
  trigger: any listed client version change, or a client API that exposes the action.
- **Item 3's RSS budget is frozen** in the card (part 3, dated amendment): derived from the card's own caps before measuring.
- **P16** is made runnable as reference-first on the same VM (card part 3).

## Closeout preparation A, 2026-11-08

Parent's nine accepted API deviations are now individual records in
`scripts/wire-skeleton/deviations.json`, bound to the starting tested commit.
`python3 scripts/wire-skeleton/test_deviations.py` PASS: all nine missing-record
and failed-row controls reject; an unknown not-run row rejects. The separate
`check-deviations.py matrix.json` verdict never rewrites the matrix or its exit 1.
Client-version changes require revisiting these decisions. No GCP run.

### Closeout preparation C: PG16 oracle mode

Official PGDG Debian 12 installer pins signing-key fingerprint and bookworm-pgdg
package provenance, verifies major 16 and retains package/version evidence.
`PG_MAJOR=16 bash scripts/wire-browse/run-oracle.sh` replays `compare.py` in a
fresh cluster; PG15 remains an explicit separate arm. Shell syntax PASS.
No PG16 behavior normalization added without a measurement. Installer NOT RUN
locally (heavy only); local PG17 and both-CPU PG16 certification remain distinct.

### Closeout preparation B: loaded resource scripts

`resources.py` prints the frozen formula verbatim and each term, checks VmHWM,
RSS within idle + 64 MiB and exact idle FD count within ten seconds of close.
Default heavy grid: 64 sessions; 1/8 workers with competing CPU actors;
64/1,024/10,000 tables, narrow and the reference's 64-column-every-sixteenth
wide distribution; native and forced 256 FD limits. Park every admission with
positive Execute then demand healthy progress within lease + admission deadline.
Snapshot adequacy/startup failure is a gate, never silently reduced fixture size.
`snapshot-mutant.py` privately removes the actual cap, checks the bounded control
rejects, and demands VmHWM exceed the same S=1 frozen budget. An early exit,
build failure or timeout cannot count as a catch. Heavy mutant NOT RUN locally.

Tiny local smoke: 4 sessions, 4 tables, 2 columns, 1 worker, default/256 FD;
peak 19,750,912 / 20,041,728 bytes against 219,369,472 bytes; final FDs 7/7,
healthy completion 5.0043 / 5.0049 s: PASS. This first smoke preceded adding CPU
actors; loaded/full-size certification is NOT RUN. Arithmetic boundaries PASS
(`test_resource_gates.py`). Resource smoke command:
`python resources.py --nata <binary> --work <scratch> --smoke` using matrix venv.

### Closeout preparation D: reference-first P16 harness

`p16.py` persists all PostgreSQL medians/twin noise before any Nata timing,
then alternates paired baseline/current runs with identical clients, fixtures,
startup inclusion, validation and sinks. Stream gate ≤1.10x PostgreSQL; small
query gate ≤1.20x baseline; reference twin noise >5% => UNRESOLVED. Every client,
format and 1/8-worker arm retains its own verdict. Arm set: psql, std Rust raw
text/binary, JDBC, psycopg, ADBC COPY, plus pgcli. Five primary/twin rounds,
min/median/spread and samples retained. `test_p16.py` PASS at exact boundaries,
1.101x/1.201x slowdown and 5.1% reference noise controls. Smoke uses 100 rows,
one round and 1 worker, always labelled SMOKE ONLY; baseline=current is a harness
control only. No performance acceptance inferred locally.

### Closeout preparation E: DBeaver R24 source limit

DBeaver 25.2.0 sources and exact file/line citations in
`scripts/wire-skeleton/dbeaver.json`; headless JDBC replay is integrated in the
matrix via `DBEAVER_JAR`. Local declared-fallback pgjdbc 42.7.2 replay:
**1 PASS / 5 FAIL**: version passes; current_database reports 42883;
database/schema/table/column native navigator shapes report 0A000. These shapes
are beyond the frozen corpus, so no catalog emulation was added. `RELEASE[42.7.2]`
is a dynamic dependency declaration, not evidence of an installed UI's shipped
JAR. R24 remains OPEN; this source replay does not complete shipped-driver/UI or
optional keys/indexes/properties certification. Matrix/checker must retain these
as FAIL; the parent's nine deviations cannot waive new DBeaver failures.

### Closeout preparation F and resource-policy stop, 2026-11-08

**STOP under the brief's ADR/frozen-target rule; slice remains OPEN.** The
stronger local resource smoke retains all sessions, parks every worker admission,
then requires every remaining healthy client to progress under CPU contention.
Both repetitions fail with `53300 query admission timeout`, including the
PostgreSQL-context run. ADR 0015 requires suspended work to retain admission;
the five-second portal lease and five-second admission timeout race. The card
requires healthy completion within that existing deadline. This is not a choice
of RSS budget. No lease, timeout, admission-retention rule or frozen target was
changed. Parent must resolve the fairness policy before resource acceptance.

**Correction to the initial B smoke:** the first harness checked one newly
connected healthy client and allowed lease + admission (10 s); its 5.004 s
numbers establish RSS/FD cleanup only, not the card's five-second healthy-query
completion target. The final gate uses the existing **5 s**, keeps all sessions
and fails rather than widening it. The later loaded single-client numbers in
`resource-loaded-*.json` are likewise superseded for fairness by the failure
logs. RSS formula and cleanup allowances are unchanged.

Prepared heavy commands now cover explicit PG15 and PG16 oracles, official PGDG
installation, native slow-reader/contention tests, independent scratch tests,
portal/scheduler/memory/bridge loom at bound 3, full **98** registered mutants,
actual snapshot-cap removal, resources and reference-first P16. `/usr/bin/time`
is explicitly installed (`sudo apt-get install -y time linux-perf`); machine type
comes from metadata. P16 misses collect server perf profiles while the server is
alive. These are prepared commands, not a claim that all acceptance gates pass.
DBeaver source failures and fairness failure remain failing gates. Full cloud
execution and deep checks have NOT RUN. No GCP, push, merge or new dependency.

GCP uploads exclude `.git`: `package-close.sh` packages the stated starting main
`a3e8e3b` as a source-only archive plus SHA-256 manifest in ignored `.slice-close/`,
then copies the template. Baseline/current release builds use four jobs serially;
the cap mutant copies source files, requiring no VM Git repository. Matrix exit
1 is retained in `matrix.exit`; the separate deviation checker rejects every
unapproved non-PASS row and now rejects changed client versions. PG16 behavior
has no additional unmeasured normalization.

Sizing, **reasoned, NOT RUN**: n4a-highcpu-16 and c3d-highcpu-16 preserve the
frozen ARM/x86 classes, accommodate eight engine workers, four build jobs and
load actors. **100 GB disk** gives separate native/loom/mutant/baseline targets
headroom. **4 hours per CPU** is a ceiling allowing release builds, 98 mutants,
bound-3 explorations, fresh PG installations, catalog DDL and all per-client
primary/twin pairs; it is not a measured completion-time prediction. Obtain the
spend bound from `plan`. These commands spend nothing until explicitly approved
`run` is executed by the parent; do not launch as acceptance while policy is open.

```sh
bash scripts/wire-browse/package-close.sh
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 100 --max-vm-hours 4
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 100 --max-vm-hours 4
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 100 --max-vm-hours 4 --run-id "$S10_CLOSE_ARM_RUN" --approved-usd "$S10_CLOSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 100 --max-vm-hours 4 --run-id "$S10_CLOSE_X86_RUN" --approved-usd "$S10_CLOSE_X86_BUDGET"
```

Remaining beyond the policy stop: real DBeaver UI/resolved shipped driver and
optional property/key/index replay; resource full-size adequacy and PostgreSQL
loaded-memory context (current script records only a catalog backend, not the
same loaded server workload); long-stream first-row/source-completion and
wide/NULL/decimal P16 controls; full-size cap-removal control, both-CPU medians,
R25 full timeout-fixture decision and closing-commit deep/fuzz/sanitizer evidence.
No preparation script or tiny smoke closes R24/R25/P14/P16.

### Final local checks and handoff commits

| Item | Rule | Numbers / verdict | Command |
|---|---|---|---|
| A / deviations | Exactly nine parent decisions; reject every unexplained not-run or FAIL | **PASS** coverage/version checker; matrix **81 PASS / 0 FAIL / 9 not run**, original exit **1** | `WORK=... RESULTS=... bash scripts/wire-skeleton/run.sh`; `python3 scripts/wire-skeleton/check-deviations.py <matrix.json>` |
| B / resources | Frozen printed RSS terms; cleanup idle + 64 MiB within 10 s; idle FDs; healthy completion within existing 5 s | Initial RSS/FD controls PASS (peaks 19.7–20.0 MB; idle FDs 7). **Final fairness FAIL / policy STOP**, reproduced against fresh current binary. Full-size cap-removal and loaded PostgreSQL context unmeasured | `resources.py --nata <binary> --work <scratch> --smoke`; failure logs in raw gates directory |
| C / PG16 | Official PGDG bookworm-pgdg, verified major; no unmeasured PG16 handling | Installer/oracle **prepared, NOT RUN**; local **PG17** only | `PG_MAJOR=16 bash scripts/wire-browse/run-oracle.sh` after installer |
| D / P16 | Reference first, same VM; ≤1.10x streaming, ≤1.20x paired small query; >5% PG twin noise UNRESOLVED | **14 tiny smoke arms** (7 clients/formats × stream/small), no performance verdict; 3 verdict mutants CAUGHT. Full first-row/wide controls remain | `p16.py --smoke --nata ... --baseline ... --jar ... --postgres ... --work ...` |
| E / R24 | Source replay exposes native navigator limits; installed-driver/UI certification remains separate | Fallback 42.7.2 / DBeaver source 25.2.0: **1 PASS / 5 FAIL**, **R24 OPEN**; no shipped-driver claim | `DBEAVER_JAR=... matrix.py --nata ... --jar ... --work ...` |
| F / heavy | Every exit gated, both CPU classes, source-only baseline packaging | Commands prepared; **not complete acceptance / not launched**, policy stop and stated remaining controls | `bash scripts/wire-browse/package-close.sh`; plan/run commands above |

Final Rust tests: **68 PASS**, 0 failed/ignored; fmt and all-target pgwire clippy
with warnings denied PASS. Python/shell syntax and diff whitespace PASS. Fresh
local PG17 oracle: **56 browse / 3 binary / 95 helpers / 36 inventory PASS**.
Focused new source-gate mutants: **7 Python + 2 native scratch CAUGHT**, **0
MISSED / 0 INVALID**, restored native scratch control PASS. All 98 full-sweep
anchors apply once; sweep NOT RUN. No new Cargo.lock package or production Rust
behavior change. Rust changes are two private scratch-boundary tests only.
All builds/tests serialized at four jobs. Stale inactive pgwire package artifacts
were reclaimed with `cargo clean` when free space reached 19.84 GiB, restoring
21.75 GiB; active cache untouched. The first matrix-script attempt found local
system Python lacked ensurepip; final script used installed Python 3.12 and JDK
17.0.21. The earlier JDBC timeout failures during exploratory concurrent runs
were not accepted: final fresh-binary matrix passes every supported cell.

Small evidence: [raw gates](raw/20261108-wire-catalog-close/gates/), including
checks JSON, original final matrix JSON, unchanged failure controls and smoke
numbers. The final staged leak scan is required before the handoff commit.

Per-item commits: A `1a8e3d6`; B `98a7c5b`; C `6973bbd`; D `1d28f3e`;
E `37c4ffb`. Final preparation/check/policy-stop commit SHA is supplied in the
handoff rather than self-referenced. No push or merge.

Final code-content SHA-256: `73f10a2722e9e9a20117a0c38c741af10a0b6ffa4388a55a9549577a5d0cacbb`.
Staged leak scan PASS before the final preparation/stop commit.

## ADR 0015 admission-pressure yield implementation, 2026-11-08

**Measured verdict: the local fairness policy stop is resolved; slice closeout remains OPEN.**
Implementation commit **`84f93dc84260c5204ed55852c317978aa8b1b494`**, based on main
`72edc6f`. No dependency, Cargo.lock, lifetime, admission wait, RSS formula or
frozen target changed. No GCP plan/run, push or merge was executed.
Small evidence: [yield gates](raw/20261108-wire-catalog-close/yield/).

**Rule implemented:** with admission exhausted, a waiting query expires the
oldest suspended portal eligible at 250 ms after its PortalSuspended flush.
Each acquisition can expire one portal; executing work is excluded. Both query
admission paths use the same helper. Suspension publication wakes waiters; an
eligible-time timer avoids polling or waiting for the five-second lease. A
shared production/loom lease lock arbitrates resume versus expiry. Selection
rechecks the captured suspension time under that lock; a winning resume survives
and the waiter reselects. Close invalidates a candidate before releasing its
producer. Timer and pressure expiry use the same pinned cancel/credit-wake/join
path and terminal state; the existing once-only join owner releases reservations
and admission. Next Execute reports `34000 suspended portal expired`; Sync and
ROLLBACK permit normal session reuse. Weak registry entries are pruned to the
admission count and never retain a separate parked-work pool. CancelRequest
selection and statement deadline tokens retain their existing rules.

Local scope: x86-64 KVM, eight visible CPUs (Intel i3-1220P), debug builds;
all Cargo builds/tests serial at `-j 4`, alongside the other child's independent
work. Frozen-budget checks use Linux VmHWM. Fairness uses 64 retained sessions,
100 source rows, four two-column tables, every admission parked, and one CPU
actor per worker. Catalog smoke uses 16 retained sessions, 100 source rows,
64/1,024 tables, widths 1/64 (wide every sixteenth table), 1/8 workers and native/
256 FD limits. These are local smoke measurements, not both-CPU certification.

| Check / rule | Numbers and verdict | Command |
|---|---|---|
| Every admission parked; every remaining healthy client completes within existing 5 s | **PASS 12/12 arms**: three repetitions × 1/8 workers × native/256 FD. 1-worker min/median/max **0.342868 / 0.354629 / 0.373944 s**; 8-worker **0.251680 / 0.293180 / 0.327030 s** | `$PY scripts/wire-browse/resources.py --nata "$CARGO_TARGET_DIR/debug/nata" --work "$RESULTS/fairness" --smoke --sessions 64 --workers 1 8 --repeat 3` |
| Frozen RSS formula; return to idle + 64 MiB and idle FD count within 10 s | **PASS 12/12** fairness arms. VmHWM **22,724,608–24,203,264 bytes**, individual budgets **1,257,848,832–1,258,360,832 bytes**. Final FDs **7/7** in every arm. Each raw arm prints all unchanged formula terms | Same fairness command; `fairness.jsonl` |
| Portal protocol: under-250 ms survives; oldest only; expiry explanation and healthy reuse; executing work survives and waiter gets 53300 after 5 s | **PASS 2 new tests**, included in **73 total PASS** (18 lib / 3 codec / 10 model-native / 38 protocol / 4 reliability). Young test uses 100 ms statement deadline then successfully resumes both portals; mature check waits 275 ms. Executing test holds both producers before suspension, asserts **5 ≤ wait < 6 s**, 53300 and both producers' later success | `cargo test --locked -p nata-pgwire -j 4` |
| Real lease arbitration under loom: waiter/resume, waiter/close, two waiters/one portal | **PASS all 10 cases, bound 2, 340 executions total**; each of the three new cases explores **4** executions. Bound 3 remains heavy | `LOOM_MAX_PREEMPTIONS=2 bash crates/nata-pgwire/run_portal_loom.sh` |
| Metadata scratch boundary, including unbounded controls already in the suite | **PASS 2 metadata_scratch_cap tests** in the 18 lib tests; no cap change | `cargo test --locked -p nata-pgwire -j 4` |
| Loaded native lifecycle / no leaks or hangs under competing CPU work | **PASS 4/4**, 8 CPU actors | `NATA_PGWIRE_CONTENTION=8 cargo test --locked -p nata-pgwire -j 4 --test reliability` |
| Large catalog correctness, RSS, FD cleanup | **PASS 16/16 resource smoke arms**, PostgreSQL typed tables/columns equality; peak **22,446,080–32,858,112 bytes**, budgets **427,499,520–429,084,672 bytes**, final FDs **7/7**; healthy completion **0.171888–0.291538 s** | `PG_MAJOR=17 ORACLE_EXTRA=resources-smoke bash scripts/wire-browse/run-oracle.sh` |
| Large catalog browse ≤1.20× PostgreSQL | **NOT CERTIFIED**: one round per smoke arm, ratios **0.321635–2.973248×**, **8/16 above 1.20×**. Raw verdict remains `SMOKE ONLY`; resource PASS does not waive the timing target | Same PG17/resource smoke command |
| Client matrix plus exact nine deviations | Original matrix **81 PASS / 0 FAIL / 9 not run**, original exit **1** preserved; separate deviation checker **PASS** | `bash scripts/wire-skeleton/run.sh`; `python3 scripts/wire-skeleton/check-deviations.py "$RESULTS/wire-skeleton/matrix.json"` |
| PG17 browse oracle | **PASS 56 browse / 3 binary / 95 helpers / 36 inventory**, PostgreSQL **17.11**; advertised compatibility major 16 remains a separate arm | Same PG17 oracle command |
| DBeaver source replay (out of implementation scope) | Reproduced **1 PASS / 5 FAIL**: current_database **42883**; database/schema/table/column navigators **0A000**. Source 25.2.0 / fallback pgjdbc **42.7.2**, original exit **1**; R24 stays OPEN | `bash scripts/wire-browse/dbeaver-gate.sh` |
| Clean / harness controls | **PASS** fmt, all-target pgwire clippy with warnings denied, Python/shell syntax, 2 resource arithmetic tests, deviation controls, local fake-provider job-chain tests, diff whitespace; all **98** full-sweep anchors still apply once. Full 98-mutant sweep **NOT RUN** | `cargo fmt --all -- --check`; `cargo clippy --locked -p nata-pgwire --all-targets -j 4 -- -D warnings`; `$PY scripts/wire-browse/test_resource_gates.py`; `python3 scripts/wire-skeleton/test_deviations.py`; `bash scripts/gcp/test/test-job-chains.sh`; `git diff --check` |

Focused mutants, **4 CAUGHT / 0 MISSED / 0 INVALID**, private source copy,
serial four-job builds; unmutated protocol and fairness controls pass, and the
restored fairness control passes. Build errors/timeouts cannot count as catches.
Reproduce: `$PY scripts/wire-browse/yield-mutants.py --target "$CARGO_TARGET_DIR" --work "$RESULTS/yield-mutants"`
(use a fresh work directory).

| Mutant | Failure observed / verdict |
|---|---|
| skip yield | Loaded fairness rejects `53300 query admission timeout`: **CAUGHT** |
| yield idle under 250 ms | Healthy query succeeds instead of its 100 ms timeout: protocol expected-error assertion fails: **CAUGHT** |
| yield two portals per waiter | Second old portal returns ErrorResponse instead of DataRow + PortalSuspended: **CAUGHT** |
| yield executing work | Publishes the running producer as idle before PortalSuspended; executing portal returns ErrorResponse after release instead of rows/suspension: **CAUGHT** |

Reproduction environment: the matrix script needs Python with venv support and
JDK 17 on PATH (local run used CPython 3.12.16 and JDK 17.0.21), plus installed
PostgreSQL 17 tools for the oracle. Export `CARGO_TARGET_DIR` to a build cache,
`WORK` and `RESULTS` to scratch directories, and after matrix setup set
`PY="$WORK/wire-skeleton-venv/bin/python"`. Oracle clusters and mutant-copy work
directories must be fresh. Local raw evidence omits startup/socket/key logs.
The initial loaded attempt exposed a harness-only bytes-versus-hex-string error
in the new expiry explanation assertion; fixed before all twelve accepted arms.
The first clippy run requested collapsing a nested if; final clippy passes.
Inactive stale Parquet mutant build outputs were reclaimed for disk headroom;
free disk was **24.78 GiB** at final validation (above the 20 GB cleanup floor). No active cache removed.

### Prepared heavy handoff, not executed

`scripts/wire-browse/slice-close.steps.example` now includes all ten portal loom
cases at **bound 3**, the four focused yield mutants, and the repeated loaded
fairness gate with **64 sessions / 1 and 8 workers / three repetitions**. It also
retains the full resource grid, 98 existing mutants and all prior closing checks.
The gate script's full-size defaults and every frozen threshold are unchanged.

**Reasoned sizing:** retain **n4a-highcpu-16** (ARM64) and **c3d-highcpu-16**
(x86-64), 100 GB scratch disk and **4 VM-hours per CPU** as ceilings. Sixteen
vCPUs accommodate eight engine workers plus load actors; four-job builds remain
serial. Disk accommodates separate native/loom/mutant/baseline targets. The
four-hour cap allows clean builds, 98 + 4 mutants, bound-3 models, catalog DDL,
reference installation and per-client primary/twin timing pairs. The new fairness
smoke adds seconds locally; the cap is a conservative allowance, **not a measured
heavy duration**. Obtain the current spend bound from each plan before approval.
Exact commands (prepared only; no project identifiers or credentials):

```sh
bash scripts/wire-browse/package-close.sh
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 100 --max-vm-hours 4
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 100 --max-vm-hours 4
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 100 --max-vm-hours 4 --run-id "$S10_CLOSE_ARM_RUN" --approved-usd "$S10_CLOSE_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 100 --max-vm-hours 4 --run-id "$S10_CLOSE_X86_RUN" --approved-usd "$S10_CLOSE_X86_BUDGET"
```

Remaining closeout: both-CPU heavy/full-size evidence (including 10,000 tables,
actual snapshot-cap removal, all mutants, bound-3 models, deep/fuzz/sanitizers),
PG15/PG16 parity, the full loaded PostgreSQL memory context (this smoke records a
catalog backend only), and reference-first P16 including long-stream first-row,
wide/NULL/decimal controls. The **8 above-budget catalog smoke timings** require
full paired investigation; no performance acceptance or target relaxation is
inferred. DBeaver shipped-driver/UI and optional property/key/index evidence,
and R25's timeout fixture decision, remain open. This implementation resolves the
admission fairness failure only. The evidence commit SHA is provided in the
handoff to avoid a self-referential commit hash. Staged leak scans pass before
both commits.

## 2026-11-08 closing-run repairs from e4d7a5f (implementation handoff)

**Verdict: local repairs pass their focused checks; slice 10 remains OPEN.** No GCP run, push or merge in this task. The 1.20× browse target, 16 MiB snapshot cap and frozen RSS formula are unchanged. Clean-VM both-CPU reruns and nightly evidence on the closing commit remain required. R24 stays open.

### Original closing run: every step

Read directly from the gitignored `bench/gcp-runs/s10close3-n4a/results/` and `bench/gcp-runs/s10close3-c3d/results/` in the primary checkout. ARM64 completed all 28 steps; x86 completed 23 and was cut off by parent tooling during the next step. “Not reached” is never a pass. Compact, non-identifying evidence: [closing repairs JSON](raw/20261108-s10-close-fixes.json).

| Step | ARM64, n4a-highcpu-16 | x86-64, c3d-highcpu-16 |
|---|---|---|
| machine-type | PASS (0) | PASS (0) |
| machine-context | PASS (0) | PASS (0) |
| apt-index | PASS (0) | PASS (0) |
| timing-tools | PASS (0) | PASS (0) |
| matrix-deps | PASS (0) | PASS (0) |
| pg16-install | PASS (0) | PASS (0) |
| matrix-psql-version | PASS (0) | PASS (0) |
| matrix-and-deviations | PASS (0) | PASS (0) |
| browse-pg15 | PASS (0) | PASS (0) |
| protocol | PASS (0) | PASS (0) |
| metadata-scratch | PASS (0) | PASS (0) |
| loaded-native | PASS (0) | PASS (0) |
| metadata-fuzz | PASS (0) | PASS (0) |
| wire-mutants | PASS (0) | PASS (0) |
| portal-loom-bound-3 | PASS (0) | PASS (0) |
| scheduler-loom-bound-3 | FAIL: model timeouts (1) | FAIL: model timeouts (1) |
| memory-loom-bound-3 | PASS (0) | PASS (0) |
| bridge-loom-bound-3 | TIMEOUT (124) | TIMEOUT (124) |
| resource-gate-mutants | PASS (0) | PASS (0) |
| p16-gate-controls | PASS (0) | PASS (0) |
| admission-yield-mutants | FAIL: executing INVALID (1) | PASS (0) |
| loaded-fairness | PASS (0) | PASS (0) |
| resource-gates | FAIL: 1.37665× (1) | FAIL: 1.31717× (1) |
| snapshot-cap-mutant | PASS (0) | Interrupted; no exit/verdict |
| pg16-parity | PASS (0) | Not reached |
| dbeaver-r24 | Known replay failures (1), R24 open | Not reached |
| p16-builds | FAIL: package missing (1) | Not reached |
| p16-streaming-and-regression | FAIL: PG raw order (1) | Not reached |

### 1. Browse performance: local encoding/projection fix; heavy verdict pending

**Cause, measured:** the resource wrapper selected `debug/nata` for timing. This is still a real miss of the frozen gate: ARM median Nata/PostgreSQL **3.2935/2.3924 ms = 1.37665×** at 64 tables / width 64; x86 **9.4303/7.1595 ms = 1.31717×** at 1,024 tables / width 1. Earlier cells passed, then each resource process stopped on this miss; later sizes and memory checks were not certified.

Release stage profiling on the shared local x86 host (Intel i3-1220P, 8 logical CPUs; warm filesystem; no network/disk transmission in the stage probe): snapshot load **1,638 µs once at startup**, not per query (sessions share the immutable Arc). Tables/columns shape matching **16/2 µs**; generation + sorting + projection **15/188 µs**; encoding **7/117 µs**. Reusing the response description in the same probe reduced encoding to **4/49 µs**. The description and result formats had been rebuilt/cloned for every row. The generic adapter also cloned every selected String while discarding the original relation.

**Fix:** retain one bounded row description and borrow formats per response; move a cell on its last projection use (clone only repeated columns). No snapshot/cache expansion, result spooling, dependency or SQL-surface change. The release profile test is opt-in; it adds no serving-path clocks. Retry and full closeout now explicitly build/select release for the resource gate; client correctness still runs before timing. The generic adapter still materializes the bounded relation; no broader executor change is proposed.

The expanded stage probe separately measures relation construction. Under concurrent checks it gave tables/columns **22/175 µs** relation, **34/270 µs** whole adapter, **28/5 µs** shape, and **14/173 → 5/68 µs** original/reused-description encoding. A 10,000-table mixed-width snapshot (49,375 columns) also loaded within the unchanged cap; full stage results are in JSON. These probes have different host load and are **not paired speed claims**.

**Local end-to-end check, UNSETTLED:** release old/new binaries, alternating A/B then B/A in the same session, exact rows first and during timing, three rounds of 40 pairs; includes planning, row generation, encoding and loopback psycopg materialization; excludes startup/snapshot load. Old/new medians (ms):

| Tables / width | Round 1 | Round 2 | Round 3 |
|---|---|---|---|
| 64 / 64 | 1.081 / 1.086 | 1.473 / 1.135 | 1.675 / 1.347 |
| 1024 / 1 | 4.948 / 4.127 | 5.157 / 4.104 | 5.663 / 4.221 |
| 10000 / 1 | 49.370 / 41.566 | 44.416 / 35.742 | 39.015 / 33.950 |

All min/median/max values are in JSON. Large ranges overlap; other local checks and loaded actors ran concurrently. Do not infer settled gains or ARM portability from these samples. Release resource gates ran **24 arms**: 64/1,024 tables × width 1/64 × one worker × default/256 FD limit × three repetitions, **64 sessions**, million-row fixture, five alternating PG/Nata browse pairs per arm. Every typed-value, browse, peak-RSS, FD and 10-second cleanup check passed: ratio **0.1277–0.5244×**, peak RSS **≤36.53 MiB**, frozen per-arm ceilings about **1.25 GB**. Original release also passed six 64-table/width-64 arms locally (**0.1640–0.3452×**), so both the unoptimized build selection and local per-row work matter. PG17 locally is not the PG15 clean-VM closing oracle. Eight-worker/full-10,000-table resource grids remain for GCP.

**Check can fail:** browse boundary 1.20 passes, 1.201 fails; bypass mutant CAUGHT. Repeated-projection cell-loss mutant fails the exact snapshot row assertion (CAUGHT); the unchanged cap test passes and the inherited actual cap-removal mutant remains in the x86 retry. Original cap-removal passed ARM. No budget was re-derived.

### 2. Executing-yield mutant: valid assertion failure

**Cause, reproduced:** the old mutation suspended the real Run lease before execution finished. A later normal `suspend`/`complete` asserts that the phase is Running; depending on the race, cleanup poisons the lease and aborts in a destructor before libtest emits `test result: FAILED`. Locally the intended portal-survival assertion fired, then SIGABRT made the result **INVALID** under the strict classifier. The ARM retained log has only INVALID, not its underlying stderr, so the precise historical signal is unavailable; this is a reproduced architecture-independent invalidity, not an ARM compile failure.

**Fix:** publish an old/idle candidate with an independent valid lease but the executing producer’s cancellation/join handles. This tests wrongful executing cancellation without corrupting the real phase transitions. Preserve INVALID for build errors, timeouts and aborts; print return code and invalid log tail. Touch copied sources before a control build: otherwise copy2’s old mtimes can reuse a newer mutant artifact from a shared Cargo target (also reproduced). Restored protocol and fairness controls now both run.

**Verdict:** skip-yield **CAUGHT (rc 1)**; yield-young, yield-two, yield-executing **CAUGHT (rc 101)**; unmutated/restored protocol and fairness controls PASS; **0 MISSED / 0 INVALID** in the corrected sweep. ARM validity is explicitly rerun; x86 already caught the original but also needs future closing-commit deep evidence.

### 3. Loom budgets: no timeout relabelled as success

**Cause:** scheduler default is 600 s/model. ARM timed out m09/m21; x86 m09/m14/m21. The nightly allows **3,000 s/model**, except **19,800 s for m21** (the prior bound-3 nightly on 84e9c83 is historical evidence, not evidence for this commit). Full steps now use those exact boxes, with m21 selected separately. Retry filters select only the timed-out scheduler models.

Bridge default was **180 s**, not 600 s: x86 timed out in `io_completion_before_or_after_park_is_not_lost`; ARM passed that in **153.91 s** then timed out in `two_workers_two_waiters_keep_the_window_bound`. Full/retry steps use release bridge exploration, **bound 3 / 1,200 s per ordinary model** (nightly box). The combined five-thread model runs **bound 2 / 3,600 s here**. A local release bound-2 probe was deliberately stopped after **180 s**, INCONCLUSIVE, not passed.

**Bound-3 obligation:** the previous nightly explicitly excluded the combined model. A separate nightly job now selects it at **bound 3 / 19,800 s**, inside a 350-minute job. That job, plus the scheduler nightly, **must pass on the final closing commit**; neither was triggered here. If the combined model also exceeds that box, record a deviation for parent resolution; do not silently lower its nightly bound. The ordinary IO bridge model passed locally in release at **bound 2 / 7.59 s** and **bound 3 / 192.30 s**, exceeding its old 180-second box. A deliberately too-short **0.001 s** model control exited **124** as required. Nightly YAML bound/profile/time-box assertions and shell checks pass; scheduler and combined explorations remain PENDING.

### 4. Packaging: checked before any expensive step

**Cause:** steps were copied without the ignored `.slice-close` baseline archive/checksum/commit. `p16-builds` therefore failed at cd. The first full/retry step now calls `check-close-package.sh`, which names the missing file and exact packaging command, validates checksums and commit syntax. `prepare-p16.sh` checks it again. `package-close.sh --steps FILE` packages the same immutable **a3e8e3b** P16 baseline and selects the CPU retry file. This baseline is unchanged; e4d7a5f is this repair’s implementation base. Missing-input and corrupted-checksum controls exit 1; valid package and byte-for-byte selected steps checks pass. Actual two-release-build preparation **PASS**: archived a3e8e3b baseline cold build **6m09s**, current build **21.59s** with a copy of the existing release cache, serial `--locked -j 4`.

### 5. Five unreached x86 steps: readiness and limits

- **snapshot-cap-mutant:** unchanged actual cap-removal check; passed ARM in the closing run, x86 interrupted before an exit. Fresh private target, 16 MiB bounded refusal versus frozen RSS breach required; selected for x86 retry. Not rerun locally (large clean build/RSS probe).
- **pg16-parity:** version-selected installer and oracle commands parse; original ARM PG16 replay passed. Local PG17 replay passed again, **not PG16 certification**. Selected for x86 retry.
- **dbeaver-r24:** step unchanged, selected on both CPUs (ARM failed; x86 never reached). Five known replay failures remain R24; no acceptance claim.
- **p16-builds:** missing/corrupt/valid package controls and real paired release builds checked; selected on both CPUs.
- **p16-streaming-and-regression:** ARM exposed an additional reference failure independent of the missing builds: raw text expected 0 but got **998016**. PostgreSQL synchronized scans may join the heap mid-scan; the fixture’s value oracle requires physical insertion order. Disable `synchronize_seqscans` for fresh connections to the isolated fixture database before any reference client. A **14-arm / seven-client** reference-first smoke passed, plus **12 million-value ordered PG17 raw scans** (text/binary × 1/8 worker settings × three rounds). Smoke remains SMOKE ONLY; P16 targets/noise limits and all-reference-before-Nata ordering are unchanged. Full PG16 timing, long-stream first-row and wide/NULL/decimal acceptance remain open; this repair does not certify those missing closeout controls.

### Local checks and reproduce commands

All Cargo builds/tests were serial with **-j 4** (including the mutant harness). No new lockfile dependencies. Inactive old test/binary artifacts were reclaimed for disk headroom; no active cache was deleted. One initial local resource attempt hit a full **tmpfs** while writing its fixture; discarded it and reran on disk before accepting samples. One development profile compile needed non-Debug WireError handling; final checks below pass. A later paired-build smoke attempt found its private PostgreSQL shared-memory segment had disappeared; that attempt failed before any reference timing. Restarted that private cluster with file-backed dynamic shared memory and reran the smoke; no product fix was inferred from this local environment failure.

- `cargo test --locked -p nata-pgwire -j 4`: **73 passed**, one opt-in profile ignored.
- `cargo fmt --all -- --check`; `cargo clippy --locked -p nata-pgwire -j 4 --all-targets -- -D warnings`: **PASS**.
- Local matrix: raw exit **1**, deviation validator **PASS**, same nine accepted API deviations; no DBeaver acceptance implied. JDK17 / pinned existing Python client environment.
- PG17.11 browse oracle: **PASS**, 56 frozen shape replays, three binary and 95 helper cases (154 PASS lines).
- Focused gates: **eight bypass/threshold/noise mutants CAUGHT**; four yield mutants CAUGHT; repeated-projection loss CAUGHT. No misses in accepted corrected checks.
- Python compilation; all full/retry step command syntax; packaging missing/corrupt/valid controls: **PASS**.

Use a writable scratch disk, a fresh WORK/RESULTS per oracle, the existing pinned matrix venv, JDK17 on PATH, and a shared CARGO_TARGET_DIR only while no other Cargo build/test is running. PROFILE_REGISTRY is a resource fixture catalog.tsv; PGDSN names a private local oracle cluster. Commands actually exercised (paths parameterized to avoid committing machine-specific data):

```sh
cargo test --locked -p nata-pgwire -j 4
cargo fmt --all -- --check
cargo clippy --locked -p nata-pgwire -j 4 --all-targets -- -D warnings
cargo build --locked --release -p nata -j 4
NATA_CATALOG_PROFILE="$PROFILE_REGISTRY" cargo test --locked --release -p nata-pgwire -j 4 --lib catalog_profile -- --ignored --nocapture
"$PY" scripts/wire-browse/resources.py --nata "$RELEASE_NATA" --work "$SCRATCH/resources" --postgres "$PGDSN" --tables 64 1024 --widths 1 64 --workers 1 --repeat 3
"$PY" scripts/wire-browse/yield-mutants.py --target "$CARGO_TARGET_DIR" --work "$SCRATCH/yield"
"$PY" scripts/wire-browse/check-gate-mutants.py
"$PY" scripts/wire-skeleton/matrix.py --nata "$RELEASE_NATA" --work "$SCRATCH/matrix" --jar "$JAR"
python3 scripts/wire-skeleton/check-deviations.py "$SCRATCH/matrix/matrix.json"
PG_MAJOR=17 NATA_BROWSE_BIN="$RELEASE_NATA" bash scripts/wire-browse/run-oracle.sh
"$PY" scripts/wire-browse/p16.py --nata "$RELEASE_NATA" --baseline "$BEFORE_NATA" --jar "$JAR" --work "$SCRATCH/p16-smoke" --postgres "$PGDSN" --smoke
bash scripts/wire-browse/package-close.sh
WORK="$PREPARE_WORK" bash scripts/wire-browse/prepare-p16.sh
```

### Prepared retry: CPU-specific failed/unreached gates only

The two `slice-close-rerun-{arm,x86}.steps.example` files include prerequisite installation/builds for a **fresh VM**, without rerunning previously accepted matrix/fuzz/full-mutant gates. ARM retries its invalid admission sweep; x86 retries snapshot cap and PG16 parity. Both retry failed resources, scheduler/bridge, DBeaver and P16. The resource gate runs first after setup, so its result is captured before lengthy model exploration.

**Reasoned sizing, not measured duration:** **n4a-highcpu-16 / c3d-highcpu-16**, **150 GB** scratch, **20 VM-hours per CPU ceiling**. Sixteen vCPUs support the full eight-worker + eight-load-actor grid; 32 GiB-class memory is sufficient for the existing bounded workload. Serial four-job builds and separate native/loom/mutant/baseline targets need disk headroom. The larger cap replaces the disproven four-hour allowance: scheduler ceilings alone total about **6.34 h ARM / 7.17 h x86**, bridge permits another **1 h combined** plus ordinary models, and P16 has up to **56 × 600 s** settle boxes plus client/validation/profile work and paired clean builds. Twenty hours is a conservative maximum, **not expected runtime or spending approval**. Parent obtains each current plan cost and starts long runs detached; no split at two hours is needed. Package immediately before each matching upload (the second command overwrites heavy.steps):

```sh
bash scripts/wire-browse/package-close.sh --steps scripts/wire-browse/slice-close-rerun-arm.steps.example
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --data-gb 150 --max-vm-hours 20
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --data-gb 150 --max-vm-hours 20 --run-id "$S10_RETRY_ARM_RUN" --approved-usd "$S10_RETRY_ARM_BUDGET"

bash scripts/wire-browse/package-close.sh --steps scripts/wire-browse/slice-close-rerun-x86.steps.example
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --data-gb 150 --max-vm-hours 20
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --data-gb 150 --max-vm-hours 20 --run-id "$S10_RETRY_X86_RUN" --approved-usd "$S10_RETRY_X86_BUDGET"
```

Implementation commit: **fa988c9ba33224998b26f4b28367b2c9fdd88c4b**. Its staged leak scan passed. The evidence-only commit is reported in the handoff to avoid a self-referential hash.

After the private projection-mutant check, touched the root metadata source to force fresh Cargo validation of the shared target, then reran the complete pgwire suite, clippy and fmt: **PASS again** (73 tests, one opt-in profile ignored).

The fresh **actual packaged-baseline/current-release P16 smoke also PASS** (14 arms, all seven clients; still SMOKE ONLY). The earlier smoke used the repair’s pre-change release control; this second smoke checks the intended a3e8e3b paired binary. Final disk headroom was **21.46 GiB**, above the 20 GiB cleanup floor. Both staged leak scans pass; no Cargo.lock change.


## Clean-VM retry and P16 encoding follow-up (2026-11-08)

The retry measured source tree `fd7a8b970201b7a4ad7e63d1e5089933d688e167590ffd5b32983dfc2bd0697e`,
PostgreSQL 16.15 for P16, and the packaged pre-slice baseline `a3e8e3b`.
This follow-up starts at main `506c9ff`. Targets remain **streaming ≤1.10x
PostgreSQL per client/worker/CPU**, paired small queries **≤1.20x pre-slice
Nata**; PostgreSQL twin noise >5% means **UNRESOLVED**. The original coarse-timer
results below remain recorded, not retroactively passed. R24 remains open.

Raw inputs were read directly from the two local `s10retry-{arm,x86}/pulled/results`
directories. [Compact evidence](raw/20261108-wire-catalog-close/retry/) contains
all per-arm samples, resource records, step exits and hashes of every input
`.txt`/`.exit` and P16 server log. No full perf traces, fixtures or VM identities
are copied into git.

| Retry step | ARM | x86 | Meaning |
|---|---|---|---|
| package-inputs / rerun-prerequisites / p16-builds | 0 / 0 / 0 | 0 / 0 / 0 | PASS prerequisites, no acceptance claim |
| admission-yield-mutants | 0; all four CAUGHT | not scheduled in retry | skip-yield, yield-young, yield-two, yield-executing |
| snapshot-cap-mutant | not scheduled in retry | 0; CAUGHT | real cap removal peak 163,766,272 > budget 163,364,864 bytes |
| pg16-parity | not scheduled in retry | 0; PASS | projected catalog/helper parity |
| bridge-combined-bound-2 | 0; 1/1, 2255.55 s | 0; 1/1, 3324.08 s | bound 2 only; closing bound 3 remains separate |
| resource-gates | 1 | 1 | all **17/17 Nata verdicts per CPU** PASS (34 total); reference lock exhaustion stops remaining grid |
| dbeaver-r24 | 1 | 1 | same five known failures; outside this work |
| p16-streaming-and-regression | 1 | 1 | misses/noisy cells below; remains NOT MET |

Resource failure is PostgreSQL `out of shared memory`, with the hint to increase
`max_locks_per_transaction`, during the next schema drop. Main `506c9ff` already
sizes the reference locks for the grid; this follow-up does not call the aborted
full resource grid PASS. Large-catalog browse ratios are 0.24–0.40x PostgreSQL;
RSS/return-to-idle/FD and admission-yield checks pass in each completed cell.
The two retries split the focused controls; absent per-CPU steps are not inferred
passes.

Every P16 arm (ratio shown as **Nata/PG; Nata/baseline; verdict**):

| Client / kind | ARM w1 | ARM w8 | x86 w1 | x86 w8 |
|---|---|---|---|---|
| psql / stream | 1.2416; 0.9997; NOT MET | 1.1074; 0.9998; NOT MET | 1.1946; 1.0001; NOT MET | 1.0975; 0.9185; MET |
| psql / small | 1.3065; 1.0002; MET | 1.0001; 0.9997; MET | 0.9999; 1.0000; MET | 1.0001; 1.0000; MET |
| raw-text / stream | 1.1903; 1.0000; NOT MET | 1.3774; 1.1593; NOT MET | 1.3795; 0.9999; UNRESOLVED | 1.3794; 1.0000; UNRESOLVED |
| raw-text / small | 0.9997; 0.9998; MET | 1.0001; 1.0002; MET | 1.3057; 1.3054; NOT MET | 1.0003; 0.9997; MET |
| raw-binary / stream | 1.7591; 1.0001; NOT MET | 1.7583; 1.0000; NOT MET | 1.7590; 0.9999; NOT MET | 1.9482; 1.1075; NOT MET |
| raw-binary / small | 1.0002; 0.9997; MET | 0.9997; 0.9999; MET | 1.3057; 1.3053; NOT MET | 1.3059; 1.3055; NOT MET |
| jdbc / stream | 1.4312; 1.0000; NOT MET | 1.4312; 1.0002; NOT MET | 1.3549; 1.0000; NOT MET | 1.3552; 0.9387; UNRESOLVED |
| jdbc / small | 1.1601; 0.8797; MET | 1.3189; 1.0000; MET | 1.2751; 1.1208; MET | 1.1375; 1.0000; MET |
| psycopg / stream | 1.2431; 1.0000; NOT MET | 1.2429; 1.0001; NOT MET | 1.0973; 0.9185; MET | 1.0972; 0.9186; MET |
| psycopg / small | 1.6126; 1.0005; MET | 1.6112; 0.9999; MET | 1.6114; 1.0000; MET | 1.6113; 1.0000; MET |
| adbc-copy / stream | 1.3626; 0.9994; NOT MET | 1.3638; 0.9194; NOT MET | 1.3235; 0.9999; NOT MET | 1.3235; 1.0003; NOT MET |
| adbc-copy / small | 1.0002; 1.0001; MET | 1.0007; 0.9999; MET | 0.8104; 1.0000; MET | 0.8102; 1.0000; MET |
| pgcli / stream | 1.0657; 1.0002; MET | 1.0662; 1.0001; MET | 1.0616; 1.0001; MET | 1.1230; 1.0578; NOT MET |
| pgcli / small | 1.3058; 1.0001; MET | 1.3062; 1.0003; MET | 0.9999; 1.0000; MET | 1.0002; 0.9998; MET |

### Small-arm timer diagnosis

Cause: `perf_counter()` was already monotonic/high resolution. The deadline wait,
not clock resolution, quantized completion: Python `Popen.wait(timeout)` with
uncaptured output polls with delays up to 50 ms. Retry x86 small raw medians
jump from about 163.9 to 214.0 ms (214/163.9 ≈ 1.3057). The baseline and current
samples both occupy those same modes. It cannot establish a 30.5% code regression.

Change: time `subprocess.run` with captured stdout/stderr, so `communicate()`
waits for pipe readiness/EOF, and use `perf_counter_ns()` explicitly. Keep the
180 s deadline and expose child output on failure. A deterministic test checks
the pipe/deadline/clock path; the threshold and noise tests retain the frozen
rules and planted >1.20 regression. A 120 ms subprocess control measures
~165 ms on the old path versus 122–125 ms on the corrected path. This establishes
the timer mechanism; corrected paired server samples below test for a residual
real regression. No historical verdict is overwritten.

### Encoding design and profiling boundary

The retry binary profiles identify allocator/row reconstruction work on both
CPUs (compact top-self summaries are alongside the retry evidence). PostgreSQL
16's [`printtup.c`](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/backend/access/common/printtup.c)
caches each column's output/send function and reuses a per-destination message
buffer. [`pqcomm.c`](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/backend/libpq/pqcomm.c)
coalesces messages in its normally 8 KiB send buffer. It does not issue a socket
write for each DataRow. Nata already batches at 64 KiB, but was formatting typed
values as strings, allocating a values vector, building a text DataRow,
cloning/parsing its description for binary types, reparsing values and rebuilding
the final frame for every row. Binary COPY then copied that row into another
body/frame. This is the on-path P14 allocation work, distinct from engine time.

Candidate 1: encode binary BOOL/INT2/INT4/INT8/DATE and borrowed VARCHAR directly
from Arrow into reusable current-row scratch; build binary COPY header/tuple in
the same scratch. Float, decimal and TINYINT retain the existing exact conversion
at this stage. Candidate 2: extend reused framing to text and mixed formats;
format integers into a 20-byte stack buffer, write bool and borrowed UTF8 directly.
Keep established float/decimal/date text spellings. Text/CSV COPY keeps its
existing quoting/escaping encoder. The first two candidates add no dependency; the final locked-itoa amendment is below.

Bounds and delivery stay unchanged: signed-i32 backend frame lengths checked
before scratch growth, frontend frame/session/object caps, two queued 64 KiB
buffers, producer/writer ownership and 64 KiB socket writer. Current-row scratch
is reused, not a result spool. Backend wide rows retain their original framing;
frontend MAX_FRAME is not applied to them. Batch boundaries still flush, as do
suspension and protocol response boundaries. No modification to cancellation,
admission, fetch credit, deadlines, source execution, or the socket flush policy.
The new row code has byte comparisons against the old path for every mapped
scalar, text/binary/mixed formats, dictionaries, NULL/empty/controls, decimal
extremes, exceptional dates and first/subsequent COPY tuples. Existing live wide
row/cancellation and client fixtures remain required checks.

### Prepared P16-only clean-VM rerun (NOT RUN)

Use the same `n4a-highcpu-16` / `c3d-highcpu-16` sizes as the retry: 16 vCPUs
leave room for the measured eight-worker server and the client; 32 GiB-class
memory covers the existing limits and serial `-j 4` builds. Retain the default
30 GB data disk and **1.5 VM-hour cap per CPU**. Retry reference settling consumed
470 s ARM / 605 s x86; the two release builds took about 5 minutes total per CPU.
The cap leaves setup, serial debug/client preparation, all reference-first arms,
paired baseline arms and miss profiles room to finish. If settling cannot finish
within that cap, the run remains incomplete; no automatic extension or relaxed
noise threshold. Parent must inspect the plan's cost bound before supplying its
approved budget. No GCP plan/run was executed by this child.

Each CPU's steps header requires `package-close.sh`: this creates the archived
pre-slice baseline, checksum and commit files in `.slice-close/`, checks them,
and copies the chosen steps to `heavy.steps`. The first line is the runner’s required `# gate-exits` sentinel: every nonzero
step exit fails its resumable job step, rather than becoming a successful wrapper.
Every step exit is a gate;
`DEBIAN_FRONTEND=noninteractive` is explicit for all apt commands. There are no
R24, full mutant/resource or loom jobs in these reruns. `run-oracle.sh` retains its
small PG16 browse sanity precheck before the P16-only performance extra. P16 runs
all seven clients, both kinds and worker counts, five alternating rounds; the
full acceptance invocation refuses local-only filters or changed round counts.

Run the ARM group, then package/run the x86 group so `heavy.steps` is not
accidentally overwritten before upload. These are exact commands with the
parent's run IDs and approved budget supplied only through environment variables:

```sh
bash scripts/wire-browse/package-close.sh --steps scripts/wire-browse/p16-rerun-arm.steps.example
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --max-vm-hours 1.5
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --max-vm-hours 1.5 --run-id "$P16_ARM_RUN" --approved-usd "$P16_ARM_BUDGET"

bash scripts/wire-browse/package-close.sh --steps scripts/wire-browse/p16-rerun-x86.steps.example
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 1.5
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --max-vm-hours 1.5 --run-id "$P16_X86_RUN" --approved-usd "$P16_X86_BUDGET"
```

### Local measurements and limits

All local timings use the existing P16 fixture/reader/client validation and
reference-first order, alternate primary/twin rounds and paired current/baseline
order, and are **LOCAL UNSETTLED**. This host is an eight-vCPU Intel i3-1220P KVM
x86-64 environment, PostgreSQL 17.11, Python 3.12.16, the pinned matrix clients and
JDK 21; it is neither clean-VM CPU and cannot close P16 or prove a gain on ARM.
Early comparisons overlap local compilation and are especially noisy. No local
run's MET suffix is an acceptance verdict. The `--local` switch skips settling
and VM perf-on-miss collection, not correctness checks, alternation or the
reference-noise rule. Filters/round overrides are forbidden in acceptance mode.

[Local samples and counters](raw/20261108-wire-catalog-close/encoding/):

| Isolated change | w1 current/previous | w8 current/previous | Verdict |
|---|---:|---:|---|
| typed binary/framing, raw-binary stream vs main 506c9ff | 0.5621x | 0.6014x | large local diagnostic gain; reference twins noisy, not acceptance |
| text framing/stack integers, raw-text stream vs binary-only candidate | 0.9616x | 0.7794x | local diagnostic; w1 gain small, both-CPU confirmation pending |

Counter probe: one validated 10,000-row raw query, one worker, same million-row
Parquet fixture. Counts include process startup, planning and scan, so these are
**whole-process totals**, not an allocator interposer inside the encoder:

| Probe | main allocations | final allocations | Difference |
|---|---:|---:|---:|
| binary | 134,303 | 4,402 | −129,901 (−96.7%; about 13 eliminated per result row) |
| text | 64,037 | 4,139 | −59,898 (−93.5%; about 6 eliminated per result row) |

Both baseline and final Valgrind probes report zero memory errors. Native perf
samples (five separately validated million-row reads per format) move from
`malloc`/`realloc`/`cfree`, text vectors and binary packet reconstruction to
`row::encode`/checked append, memcpy, frame-boundary accounting, integer formatting
and Parquet decompression. Sampling percentages are not wall-clock ratios.
The raw syscall probes retain **380 binary / 368 text `sendto` calls** for a
million rows in both builds. Neither had per-row socket writes; the flush policy
and socket buffer are deliberately unchanged. Remaining per-row primitive work
is framing/type dispatch, length checks, writing network-endian fields and the
single row-to-bridge copy. Float/decimal fallback still allocates current-cell
conversion scratch, with exact byte behavior retained.

Reproduce the isolated timings (save a release binary for each source variant;
`BASE`, `BINARY`, `FINAL` below are those absolute paths, `PG` is a local synthetic
reference DSN, and `PY`/`JAR` select the pinned client environment):

```sh
"$PY" scripts/wire-browse/p16.py --local --clients raw-binary,raw-text --workers 1,8 --rounds 5 --nata "$BINARY" --baseline "$BASE" --jar "$JAR" --work "$WORK/binary" --postgres "$PG"
"$PY" scripts/wire-browse/p16.py --local --clients raw-text --kinds stream --workers 1,8 --rounds 5 --nata "$FINAL" --baseline "$BINARY" --jar "$JAR" --work "$WORK/text" --postgres "$PG"
python3 scripts/wire-browse/profile-local.py --nata "$BASE" --work "$WORK/binary" --out "$WORK/base-perf" --tool perf
python3 scripts/wire-browse/profile-local.py --nata "$FINAL" --work "$WORK/binary" --out "$WORK/final-perf" --tool perf
python3 scripts/wire-browse/profile-local.py --nata "$BASE" --work "$WORK/binary" --out "$WORK/base-counts" --tool counts
python3 scripts/wire-browse/profile-local.py --nata "$FINAL" --work "$WORK/binary" --out "$WORK/final-counts" --tool counts
P16_ENCODER_PROFILE=1 cargo test --locked --release -p nata-pgwire -j 4 --lib profile_typed_rows -- --nocapture
```

The raw probe script validates every value; its heap arm reads 10,000 rows and
its perf/syscall arms one million. Perf/Valgrind/strace data remain local; only
compact sample/counter summaries are committed. No custom allocator, new-to-lock crate, unsafe code, engine/source optimization
or client timing-boundary change.

Additional timer evidence from the **original retry**, not the optimized code:
[three-row server timings](raw/20261108-wire-catalog-close/retry/small-server-times.json)
aggregate successful three-row executions across clients. On x86, current vs
baseline medians are **13.045 / 12.908 ms (w1, 1.0107x)** and **12.932 / 12.919 ms
(w8, 1.0010x)**; maximum current duration is 13.897 ms. ARM medians are
9.846 / 10.157 ms and 10.068 / 10.113 ms. This cannot account for a 50 ms client
wait-mode jump. Server durations exclude connection/client startup and are not
a replacement denominator for the frozen end-to-end gate.

The corrected-timer repeat comparing **unchanged main 506c9ff against a3e8e3b**
(no new encoding in either arm) yields raw-binary small ratios **0.9781x / 1.0005x**
and raw-text **0.9446x / 1.1558x** at w1/w8. The fixed 50 ms modes disappear;
reference noise still makes three of four cells UNRESOLVED on this busy host.
The w8 raw-binary MET suffix remains LOCAL UNSETTLED. Earlier more contended
repeats are also retained, including a 1.3403x unresolved small arm; none is
silently discarded or presented as a clean regression pass. The polling artifact
is established; final small-query acceptance awaits the corrected clean-VM run.

A third optimization candidate added inlining hints to the checked append/length
helpers after they appeared in native profiles. It was **discarded**: three
alternating local repeats did not establish a consistent gain beyond paired
noise (last repeat current/previous: binary 1.0140x / 0.9621x, text 1.0248x /
1.0259x at w1/w8). The inlining hints are absent from the final code; the later whole-buffer
counting and locked-formatter changes below are separate measurements. No
unmeasured compiler-hint speedup is claimed.

### Final local checks and focused mutants

- `cargo test --locked -p nata-pgwire -j 4 -- --test-threads=1`: **78 passed,
  zero failed**, including all **38 protocol tests**. One inherited ignored
  catalog profiling hook requires its own registry and is unrelated to these
  tests. The new release row/profile run also passes all lib controls (23 passed; one inherited profiling hook ignored). No timeout or assertion was weakened.
- `cargo fmt --all -- --check` and
  `cargo clippy --locked --workspace --all-targets -j 4 -- -D warnings`: PASS.
- `scripts/wire-skeleton/run.sh` through `matrix-verdict.sh`: **76 PASS, zero
  FAIL, nine accepted API not-run cells**; original matrix exit **1** is retained,
  exact deviation check PASS. Pinned pgjdbc **42.7.14**. No R24/UI claim.
- PG17.11 browse oracle: **56/56** statements, **36/36** inventory additions,
  **95/95** helper comparisons and **3/3** binary metadata controls PASS.
  COPY oracle: **40/40 exact backend-byte comparisons** PASS. Raw frozen
  execute-limit byte fixtures and declared suspended-fixture projections PASS
  in the matrix; the existing projection limitations remain unchanged.
- **11/11 focused Rust mutants CAUGHT**, zero SURVIVED/INVALID, with passing
  unmutated lib/protocol controls: `typed-text-wrong-integer`,
  `typed-row-little-endian`, `typed-row-null-as-empty`, `typed-row-ignore-format`,
  `typed-row-copy-header-every-row`, `typed-row-nul-unchecked`,
  `typed-row-length-wrap`, `typed-row-date-unchecked`, the retargeted inherited
  `copy-null-length-zero`, `frame-count-split-header-missed`, and
  `frame-count-non-row-as-row`. **108/108 registered edit anchors** apply exactly
  once; the full sweep is not run locally.
- Python harness tests **2/2 PASS**. Planted small-threshold bypass and
  reference-noise bypass both fail their assertions (**2/2 CAUGHT**).
  Python syntax, staged whitespace checks and local job-chain stubs PASS,
  including strict exit propagation. Both CPU packages/checks validated locally;
  no cloud calls. Only the nata-pgwire dependency list in Cargo.lock changes, adding the already
  locked **itoa 1.0.18**; package/version/checksum identities are unchanged. No new
  unsafe code. Disk stayed above 20 GB throughout (minimum observed over 45 GB).

Validation tooling notes: sharing a target directory between private mutant
copies and the main tree reused a stale NULL-mutant protocol executable in an
initial post-check. Sources were unchanged. The shared target link was removed,
`cargo clean -p nata-pgwire` purged affected artifacts, and main checks rebuilt.
A concurrent post-clean run then hit the inherited 80 ms extended-timeout test
under the other child's compilation; unchanged assertions pass in the complete
serial test run above. These failed runs are not counted as passing checks.
The copied release benchmark binaries and earlier byte oracles preceded the
mutant cache and were unaffected. Subsequent final row/profile tests rebuilt
from main after dropping the inlining candidate.

Reproduce using a **private** mutant target directory (do not share target
outputs with the main build):

```sh
python3 crates/nata-pgwire/mutate.py typed-text-wrong-integer typed-row-little-endian typed-row-null-as-empty typed-row-ignore-format typed-row-copy-header-every-row typed-row-nul-unchecked typed-row-length-wrap typed-row-date-unchecked --filter row --work "$WORK/mutants"
python3 crates/nata-pgwire/mutate.py copy-null-length-zero --filter copy_ --work "$WORK/mutants"
WORK="$WORK/clients" RESULTS="$WORK/matrix-results" CARGO_BUILD_JOBS=4 bash scripts/wire-browse/matrix-verdict.sh
"$PY" scripts/wire-browse/compare.py --nata "$FINAL" --work "$WORK/pg17-browse" --postgres "$PG"
"$PY" scripts/wire-skeleton/compare-copy.py --nata "$FINAL" --work "$WORK/pg17-copy"
bash scripts/gcp/test/test-job-chains.sh
```

[Compact check outcomes, versions and original matrix](raw/20261108-wire-catalog-close/encoding/checks.json).
Full logs, native perf files, generated Parquet, binaries and build outputs remain
local. The prepared GCP jobs are P16-only; full deep-check/loom/fuzz/R24/resource
closeout obligations remain with the parent.

### Residual JDBC profile and two final send/text changes

The encoding-only full local sweep still missed JDBC: **1.2814x PostgreSQL w1**,
**1.3639x w8** (w8 UNRESOLVED, reference noise 7.70%). Other streaming ratios were
below 1.10x, with several noisy references still UNRESOLVED. Its raw counters and
all small arms are retained, not promoted to final-code or clean-VM acceptance.

The follow-up JDBC profile identifies a concrete cost: `fetch.rs` called
`FrameBoundary::consume` on **each byte**, solely to count DataRow headers for
CommandComplete. It consumed **22.39% self CPU** in the JDBC sample, versus
4.48% in the raw text sample. A validated million-row JDBC query made **3,144
sendto / 7,581 futex calls**, versus 368 / 1,126 for the raw text counter probe;
JDBC's fixed fetchSize=1000 causes repeated suspend/resume and terminal flushes.
These counts explain workload shape, not a causal allocation of all wall time.

Change 3: the existing incremental boundary scan returns the number of completed
DataRow **headers** while skipping bodies by length. Fetch uses that one whole-
buffer scan, rather than calling it once per byte. Counting at header completion
matches the prior implementation exactly, even when a row body spans buffers.
The parser state, cancellation/partial-frame rules, flushes, queue bounds and
portal transitions do not change. A permanent test covers **every split point**,
byte-at-a-time delivery, non-row frames containing `D`, split headers and a row
larger than 64 KiB; both new mutants are CAUGHT.

Its isolated P16 comparison against the encoding-only binary improves JDBC
**0.9392x w1 / 0.8480x w8**, while raw controls and all small arms remain recorded
alongside it. PostgreSQL ratios are **1.4528x / 1.1149x**, still NOT MET locally.
Absolute PG ratios shift substantially across unsettled repeats; same-round
Nata/baseline gains are diagnostic, not a new denominator or accepted deviation.

Change 4: text integers use **itoa =1.0.18**, already in Cargo.lock, in place of
`std::fmt` through a stack Cursor. Native text profiles show the formatting
callbacks as remaining work. This adds one direct dependency edge only:
old/new lockfiles have identical `(name, version, source, checksum)` sets. The
explicit user authorization permits an already-locked crate; no new package,
version, parser, unsafe block or formatting domain is introduced. Integer minima,
maxima, NULL/mixed formats and complete wire bytes remain under the same tests.
The wrong-integer mutant is retargeted and rerun against the new implementation.

Reproduce these isolated changes and the residual profile:

```sh
"$PY" scripts/wire-browse/p16.py --local --clients jdbc,raw-binary,raw-text --workers 1,8 --rounds 5 --nata "$BULK_COUNT" --baseline "$ENCODING_ONLY" --jar "$JAR" --work "$WORK/frame-count" --postgres "$PG"
"$PY" scripts/wire-browse/p16.py --local --clients jdbc,raw-text --workers 1,8 --rounds 5 --nata "$FINAL" --baseline "$BULK_COUNT" --jar "$JAR" --work "$WORK/itoa" --postgres "$PG"
python3 scripts/wire-browse/profile-local.py --nata "$ENCODING_ONLY" --work "$WORK/frame-count" --out "$WORK/jdbc-perf" --jdbc --jar "$JAR" --tool perf
python3 scripts/wire-browse/profile-local.py --nata "$ENCODING_ONLY" --work "$WORK/frame-count" --out "$WORK/jdbc-counts" --jdbc --jar "$JAR" --tool counts
python3 crates/nata-pgwire/mutate.py frame-count-split-header-missed frame-count-non-row-as-row --filter frame_boundary_counts --work "$WORK/frame-mutants"
```


### Closing-code local P16 readout (all seven clients)

This final full sweep uses the **exact closing release binary**, PostgreSQL
17.11, five alternating rounds, both configured worker/admission settings, and
all value checks. It is **LOCAL UNSETTLED**, not the required clean ARM/x86 PG16
acceptance. Labels below are the frozen-rule diagnostic suffixes: M=MET,
N=NOT MET, U=UNRESOLVED. No averaging across arms. Small-query ratios use the
paired archived **a3e8e3b** baseline, not PostgreSQL.

| Client | Stream / PG w1 | Stream / PG w8 | Stream / baseline w1 / w8 | Small / baseline w1 / w8 |
|---|---:|---:|---:|---:|
| psql | 0.9758 M | 0.9204 U | 0.7933 / 0.8242 | 1.1099 / 1.0065 |
| raw-text | 0.9004 M | 0.8708 M | 0.6929 / 0.6554 | 1.0010 / 0.9715 |
| raw-binary | 0.8144 M | 0.8315 M | 0.5107 / 0.5212 | 1.0135 / 0.9556 |
| jdbc | 1.1596 N | 1.2937 N | 0.7807 / 0.6370 | 1.0063 / 0.9904 |
| psycopg | 0.9834 M | 1.0213 U | 0.8458 / 0.8075 | 0.9871 / 0.9736 |
| adbc-copy | 0.9825 U | 0.9842 M | 0.6546 / 0.7280 | 0.9982 / 0.9898 |
| pgcli | 0.9192 M | 0.9729 M | 0.9476 / 0.8929 | 1.0234 / 0.9795 |

All **14 small-query arms** have a MET diagnostic suffix; maximum paired ratio
is **1.1099x**, below 1.20x. Streaming: **9 MET / 2 NOT MET / 3 UNRESOLVED**.
The three unresolved streams are ADBC w1, psql w8 and psycopg w8. Their reference
noise is preserved in the raw JSON, including psql w8 slightly above 5%.
Every arm remains LOCAL UNSETTLED, including those with low measured twin noise.

**Remaining gap after all four fixes:** JDBC is **1.1596x / 1.2937x PG**, with
reference twin noise **1.8% / 2.3%**. That is 0.0596x / 0.1937x over the allowed
ratio; another **5.1% / 15.0% reduction of current time** would be needed locally.
It is nevertheless **0.7807x / 0.6370x the paired old Nata build**. The final
profile reduces frame-boundary self CPU from **22.39% to 4.34%**. Remaining work
includes row/frame copies, typed-column dispatch, Parquet decode, and kernel/
scheduler wakeups around the fixed 1,000-row fetch cycle. Context-switch and
kernel-lock symbols now lead the JDBC sample. These samples establish remaining
costs, not a complete causal attribution of the PG ratio or proof that 1.10x is
unattainable. Fetch-response coalescing would need its own cancellation and
partial-frame validation before retention. No fetch-size/client workaround,
portal policy, cap or target is changed here.

Other clients show no numerical gap in this closing local sweep, but the three
noisy cells and every ARM cell still require the prepared clean-VM run. **P16
remains open / NOT MET.** Parent should use those runs to decide further work or
a recorded deviation; this child approves none. Earlier misses, unresolved
repeats and discarded hints remain in the evidence.

The final whole-process allocation probe reports **4,403 binary / 4,139 text
allocations** for 10,000 validated rows, versus **134,303 / 64,037** on main
506c9ff (the earlier encoding-only binary count was 4,402). Both final probes
have zero Valgrind memory errors; million-row raw send counts remain **380 / 368**.
The final release per-type profile also runs all lib controls: **23 passed**, one
inherited catalog profiling hook ignored. Every mapped type/format's profile is
retained in `encoder-types-final.txt`; for example BIGINT text is 14.835→4.462 ms
and binary 27.852→3.804 ms per 100,000 mixed NULL/non-NULL rows.

Reproduce the complete final local sweep:

```sh
"$PY" scripts/wire-browse/p16.py --local --rounds 5 --nata "$FINAL" --baseline "$PRE_SLICE" --jar "$JAR" --work "$WORK/closing-p16" --postgres "$PG"
```

Final source content identity (`scripts/code-tree.sh`):
`f6157b4847e064d63d0c41a699c8584aceed2d6c47206e2c31dcf22bd437ae91`.
The closing context records this same identity, client/JVM/Python versions, jar
and binary hashes. Exact commit SHA is supplied in the implementation handoff.
Staged leak scan is required immediately before the commit. No push, merge or
GCP operation was performed.

The final JDBC counter repeat retains **3,142 sendto calls** for a million rows
and records 8,514 futex calls (kernel wake counts vary with scheduling). Its
10,000-row heap probe has zero Valgrind errors. The output/terminal flush policy
has not changed; the CPU reduction removes the byte-at-a-time accounting, not
the fetch exchanges. All closing PG17 browse/helper/binary and **40/40 COPY-byte
oracles** were rerun against the final release binary and pass.

Final checked commands remain `cargo test --locked -p nata-pgwire -j 4 --
--test-threads=1`, workspace/all-target clippy with warnings denied, fmt, the
matrix/deviation wrapper, PG17 oracles, both harness gate mutants and all eleven
focused Rust mutations (wrong-integer rerun after switching to itoa). **108**
registered anchors still apply once. No required focused mutant is missed or
INVALID; the full sweep and both-CPU deep campaigns are not claimed.
