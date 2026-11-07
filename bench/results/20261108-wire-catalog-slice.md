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
