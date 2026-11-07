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
