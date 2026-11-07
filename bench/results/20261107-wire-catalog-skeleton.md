# Wire/catalog skeleton — 2026-11-07

**Working thin implementation; walking-skeleton acceptance NOT MET.** Registered
local tables and bounded data portals run end to end. Every installed client
passes registered SELECT, binary COPY, timeout, cancel and error recovery;
JDBC positive fetch passes. Five PostgreSQL fetch fixtures match byte for byte.
All five requested mutants are CAUGHT (zero INVALID/surviving). Full default
schema browsing still FAILS explicitly. The full lifecycle model, deep/resource
checks and remaining COPY/settings/catalog surface are open; do not treat this
as slice-10 closeout or a complete card-0007 walking skeleton.

No GCP, push, merge to main, new dependency or aggregate-internal change. The
required skeleton branch was merged first. The original preflight is preserved
in commit 517a081 and its [baseline probes](raw/20261107-wire-catalog-skeleton/local/).
The two parent amendments are implemented, not reopened.

## Implemented boundary

- `nata serve --catalog registry.tsv` loads one bounded, immutable snapshot with
  catalogs, schemas, tables, columns/Nata types and explicit local file locations.
  One/two/four tab-separated fields declare a catalog, schema, or table/file.
  No discovery, refresh or registration SQL. Generated OIDs are collision-free
  checked allocations, stable within this server lifetime only. Empty catalogs
  and schemas can be declared. Unsupported source types fail startup.
- Named one/two/three-component SQL sources resolve through that snapshot into
  the existing nata-sql planner and executor. Quoted components are exact.
  The source-plan equality test includes all scalar columns in the existing
  mixed-type fixture. Five real clients SELECT the registered BIGINT fixture.
- Bounded information_schema projections and the existing exact ADBC bootstrap
  read the snapshot. Additional objects/columns/helpers/syntax return explicit
  0A000, never invented empty catalogs. Wildcards on incomplete views are
  refused, rather than silently narrowing the standard view. No general join/window/aggregate engine
  was added. See [serve.md](../../docs/serve.md) for exact supported columns.
- Positive data Execute retains one producer, admission and bounded byte bridge;
  no rerun or collect-all. Exact-end suspension, per-Execute counts, Execute(0)
  resume, Close/replacement/error/disconnect cleanup and five-second idle expiry
  have live tests. Expiry releases the admission and next Execute reports 34000.
  An independent timer wakes and reaps the producer even while the session is
  executing another statement. The lease remains a reasoned provisional policy,
  not a measured resource gate.
- Cancel has no target while suspended; removal precedes publication of the
  suspension marker. statement_timeout is checked integer milliseconds (ms/s/min
  suffixes), fresh per Execute, with idle time excluded. Timers pin their own
  active token so a stale deadline cannot cancel a replacement. Metadata CPU
  uses bounded input/result limits and an elapsed check; stalled/partial output
  closes the socket under the statement/send deadline. Failed transactions reject
  work with 25P02 until rollback. DECLARE/FETCH/CLOSE are tested 0A000 boundaries.

## Client × action: measured local x86-64

Defaults retained: no preparation/binary/COPY/SSL preference workarounds, no -X.
Psql -qAt is used only to check values in script recovery/cancel probes; schema
commands use its normal presentation. SQL fixture: BIGINT 0..999999, registered
as nata.public.all_types; small SELECT/COPY validate 0,1,2. No timings or
performance verdicts. JVM 21.0.13+9-1-deb13u2; Rust 1.99.1. Client versions below.

| Client | Connect / registered SELECT | Browse | Positive Execute | Binary COPY | Timeout | Cancel | Error / recovery |
|---|---|---|---|---|---|---|---|
| psql 17.11 | PASS / PASS | FAIL: each of `\d`, `\dt`, `\dn`, `\df`, `\l`, explicit catalog refusal (exit 1) | not run: no wire max-rows API | PASS | PASS | PASS: SIGINT after startup | PASS |
| pgjdbc 42.7.7 | PASS / PASS: ten PreparedStatement executions | FAIL 0A000: getCatalogs/getSchemas/getTables/getColumns/getPrimaryKeys/getTypeInfo/getIndexInfo/getFunctions | PASS: fetchSize=2, autocommit off, rollback | PASS | PASS | PASS | PASS |
| psycopg 3.3.8 | PASS / PASS: crosses default preparation threshold | FAIL 0A000: captured filtered tables/columns and type join; supported simple projections PASS | not run: ServerCursor is SQL DECLARE, tested separately as refused | PASS | PASS | PASS | PASS: 22012 → 25P02 → rollback → SELECT |
| pgcli 4.7.1 | PASS / PASS | FAIL 0A000: schemata/tables/views/types/databases/functions completion; casing-only refresh PASS | not run: no wire max-rows API | PASS | PASS | PASS | PASS |
| ADBC PostgreSQL 1.13.0 / Arrow 26.0.1 | PASS / PASS: default COPY Arrow fetch | FAIL 0A000: all five get_objects depths and get_table_schema | not run: default stream is COPY, not Execute max-rows | PASS: default Arrow stream | PASS | PASS: adbc_cancel | PASS |
| DBeaver UI | not run: unavailable | not run: actual UI/driver/init capture absent | not run | not run | not run | not run | not run |

Shared raw malformed-frame cell: **PASS**, length-3 Query gives bounded 08P01,
then a new healthy connection succeeds; no panic. Client APIs do not expose
malformed frames; those per-client cells are explicitly not run, not duplicated
raw passes. Full [matrix JSON, versions, JVM output, refusals and transcript](raw/20261107-wire-catalog-skeleton/implementation/)
record 36 PASS / 38 FAIL / 9 not run. Matrix exits **1**, as required for its
failures/open cells; generic heavy-job completion must not disguise this exit.

Text, CSV and table COPY are independently attempted through psql, pgjdbc and
psycopg: all nine cells FAIL with explicit refusals (psql exit 1; others 0A000).
Binary COPY remains a utility stream, including positive Execute as in the
inherited byte tests; it does not use row-portal fetch limits. Positive Execute
of EXPLAIN/ANALYZE and metadata/session rows is refused rather than ignoring the
row limit.

The pgcli actions use PGExecute's actual connection/cursor factory and default
completion refresher functions, without swallowing background refresh errors.
Its ProtocolSafeCursor.execute returns None, so the harness fetches from the
cursor explicitly. The psql SIGINT probe waits for a startup echo before firing;
a pre-startup signal was not a valid cancel measurement. Earlier harness-only
failures were corrected before this recorded run. JDBC differs from the captured
42.7.14; that version parity remains open. The cloud runner pins 42.7.14.

## Fixture comparisons and inventory

- **PASS:** execute-limit-0/1/2/3/4 setup and each Execute backend tag/payload,
  byte for byte against PostgreSQL 15.19 x86 fixtures. The source is the equivalent
  `SELECT v + 1 AS i ... LIMIT n` registered Parquet query, replacing unsupported
  generate_series only. Exact two/four rows suspend; next Execute is SELECT 0.
  Completion counts and repeated completed Execute agree. All 11 ARM/x86 fixture
  pairs remain identical offline.
- **PASS:** all installed clients' small binary COPY payload/default Arrow values;
  this does not replay the full mixed-type COPY fixture. SQLSTATE/transaction
  recovery, timeout, suspended cancel/timeout and cursor refusal have live tests.
- **Not run:** full byte comparisons for SQL-cursor, timeout, extended-timeout,
  rollback, concurrent-cancel and mixed-type COPY fixtures. PostgreSQL source
  error fields and the advertised PG16 versus captured PG15 difference remain
  outside this byte claim.
- **Pending capture:** suspended-cancel and suspended-timeout-restart are now
  generated by scripts/wirecapture/protocol.py and replayed with recorded idle
  delays. No PostgreSQL bytes for these two have been fabricated or committed.
  Nata tests follow the parent's rule until its wirecapture run records them.
- **36/36 additions refused with tested effect; zero unclassified additions.**
  Each client/object row has a direct live 0A000-before-rows probe, held in
  `skeleton_all_36_inventory_additions_refuse_before_rows`. The actual psql/JDBC/
  pgcli/ADBC browsing operations above fail. The [36-row inventory](raw/20261107-wire-catalog-skeleton/implementation/inventory.json)
  gives each object's columns/client/effect. This covers array/window helpers,
  pg_depend/policy/publication/statistics catalogs and the extra columns listed
  in the reference. The original 51 rows are **not** claimed fully served: only
  the documented projections and exact type bootstrap pass; full relational
  catalog lowering remains absent. Refusal is an implementation boundary, not
  an owner-approved passing compatibility deviation.

## Mutants and checks

The cached, private-copy local sweep stayed below fifteen minutes, sequential.
Unmutated lib and protocol suites PASS; all five required mutants **CAUGHT by failing
assertions**, no compile failure/timeout/INVALID counted. [Sweep output](raw/20261107-wire-catalog-skeleton/implementation/mutants.log):

| Mutant | Result |
|---|---|
| catalog-wrong-oid (existing mutation, now exercises the new coverage too) | CAUGHT |
| exact-end-suspension-missing | CAUGHT |
| statement-timeout-not-enforced | CAUGHT |
| refused-catalog-silent-empty | CAUGHT |
| cancel-applied-while-suspended | CAUGHT |
| portal-idle-wake-missing (additional independent-lease control) | CAUGHT |

Final local checks **PASS**, sequential with TMPDIR on disk:

- `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test --workspace`: **488 passed, zero failed/ignored, 70 summaries**.
- `cargo deny check`: advisories/bans/licenses/sources OK; inherited unmatched
  license allowance warnings only.
- Fresh SF0.01 TPC-H and SQL fixtures, freshly built nata/nata-check: scan **5/5**,
  sql **252/252**, sql-differences **54/54**, aggregate **65/65**; zero mismatch,
  error, unsupported, reference-error or pending.
- Python syntax and wirecapture unit tests **3 PASS**; job-chain suite **PASS**;
  fixture architecture equality PASS. See [check summary](raw/20261107-wire-catalog-skeleton/implementation/checks.txt).
- Staged leak scan and whitespace check PASS before commit.

No crate/version or unsafe-code inventory change; Cargo.lock is unchanged. Heavy nightly-size campaigns, real
portal loom bound 3, loaded fresh-machine RSS/FD/resource gates and ARM64 are not
local passes. The new bounded metadata tokenizer fuzzer has a short default
smoke and a million-case nightly/heavy command.

## Reproduction and parent commands

Local client smoke (expected exit 1 while browsing/open API cells remain):

```sh
mkdir -p "$HOME/tmp-wcskel2/tmp"
export TMPDIR="$HOME/tmp-wcskel2/tmp"
cargo build -p nata -p nata-check
uv venv "$HOME/tmp-wcskel2/venv" --python 3.12
uv pip install --python "$HOME/tmp-wcskel2/venv/bin/python" 'psycopg[binary]==3.3.8' 'pgcli==4.7.1' 'adbc-driver-postgresql==1.13.0' 'pyarrow==26.0.1'
"$HOME/tmp-wcskel2/venv/bin/python" scripts/wire-skeleton/matrix.py --nata "$PWD/target/debug/nata" --work "$HOME/tmp-wcskel2/matrix" --jdk "$HOME/tmp-4b/clients/root/usr/lib/jvm/java-21-openjdk-amd64/bin" --jar "$HOME/tmp-4b/clients/root/usr/share/java/postgresql-42.7.7.jar"
```

No command below was run against GCP. The parent plans/approves each budget and
supplies unique run ids; commands contain no project identifiers. Copy the
runnable new-path matrix/fuzz/mutant steps, then run separately on each CPU:

```sh
cp scripts/wire-skeleton/heavy.steps.example heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --max-vm-hours 1
bash scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 1
bash scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-16 --max-vm-hours 1 --run-id "$WC_ARM_RUN" --approved-usd "$WC_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --max-vm-hours 1 --run-id "$WC_X86_RUN" --approved-usd "$WC_X86_BUDGET"
# PostgreSQL reference, including the two new suspended cases, both CPUs:
bash scripts/gcp/orchestrate.sh plan --job wirecapture --machines n4a-highcpu-8 --max-vm-hours 2.75
bash scripts/gcp/orchestrate.sh plan --job wirecapture --machines c3d-highcpu-8 --max-vm-hours 2.75
bash scripts/gcp/orchestrate.sh run --job wirecapture --machines n4a-highcpu-8 --max-vm-hours 2.75 --run-id "$CAP_ARM_RUN" --approved-usd "$CAP_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job wirecapture --machines c3d-highcpu-8 --max-vm-hours 2.75 --run-id "$CAP_X86_RUN" --approved-usd "$CAP_X86_BUDGET"
```

Read every results/*.exit file. The checked-in heavy example exercises the new
matrix, protocol, metadata/wire fuzz, live regression, the requested mutants plus an idle-wakeup mutant and damaged
files; it does **not** manufacture commands for missing portal-model/resource
checks. It is not a full closeout job. The matrix script has passed locally as
an executable probe with the disclosed failing cells, not as a green matrix.

## Remaining slice 10

Build the bounded relational metadata surface needed for real default browsing,
SHOW/DESCRIBE and the shared settings helpers/RESET/pg_settings; preserve the
36 explicit refusals until their implementations and client replays pass. Extend
COPY to frozen text/CSV/table forms without taking aggregate internals. Complete
the actual retained-portal lifecycle model/loom bound 3, race mutants, metadata
scratch charging and fresh-machine loaded resource/fairness gates. Add remaining
client API probes where applicable, actual DBeaver UI capture and version parity;
record/replay both new PostgreSQL fixtures. Then both-CPU deep/sanitizer/fuzz
and frozen paired performance gates, register/ADR closeout. TLS/passwords,
writes, SQL cursors and Iceberg/object-store providers remain outside this task.

Tested code-content hash (`scripts/code-tree.sh`):
`a91914fe69d2d62e2c130c80c1bd42794f7b1b75d13204cfc74213f2bb2a03f1`.
Branch: `t3/wire-catalog-portals`; preflight merge: `f797e21`, from main `7e1fac4`.
The implementation commit includes this record. The owned `~/tmp-wcskel2` scratch
is removed after preserving these results; the worktree target remains for the
parent's normal worktree cleanup.

## Rebased onto main — 2026-11-07

Rebased the two track C commits onto `c0bb960`, including step D (`b67cbd1`,
`e198d08`). Conflicts were confined to `server.rs`, `tests/protocol.rs` and
`tests/support/mod.rs`: retain both appended test blocks, combine catalog and
execution configuration in the server/test harness, and pass the admitted
`ExecConfig` through bounded portals as well as ordinary execution. No test was
dropped. An additional forced-spill bounded aggregate test checks the divided
budget, suspended cancel, resume and grant cleanup. No behavioral contradiction
or feature extension was needed.

Both client matrices still hold **with no changed cells**: wire/catalog remains
36 PASS / 38 explicit FAIL / 9 not run (expected exit 1), including five exact
PostgreSQL Execute fixture comparisons; all 50 step D aggregate cells PASS,
including forced spill and the existing ADBC R22 outcome. Client versions match
the recorded local versions. Fmt, clippy, workspace tests (497 passed), deny,
scan 5/5, SQL 252/252, differences **55/55** (current main's extra case), aggregate
65/65 and forced aggregate 65/65 PASS. All nine focused wire mutants are CAUGHT
(track C's five plus idle wakeup, and step D's three), with passing private-copy
controls, zero survivors/INVALID and a 160-second sequential sweep. Wirecapture
unit tests, Python syntax, 11 offline fixture pairs and local job-chain stubs
also PASS. Missing Python dependencies were installed only in disposable
scratch; all heavy work used disk TMPDIR and ran one job at a time.

[Rebase check summary, matrices and mutant results](raw/20261107-wire-catalog-skeleton/rebased-on-main/)
retain the rerun evidence. Original acceptance gaps and explicit refusals above
remain open; no GCP, push or merge was performed.

## Heavy checks on clean VMs (added by the parent, 2026-11-07)

Runs `wcskel-arm64` (n4a-highcpu-16, $0.054) and `wcskel-x86-r2` (c3d-highcpu-16, $0.044), generic `heavy` job with
`scripts/wire-skeleton/heavy.steps.example`. A first x86-64 attempt (`wcskel-x86`) failed at once with "no heavy.steps in the shipped
tree": the parent removed the file before the tree was shipped. Nothing was measured in it.

| Step | ARM64 | x86-64 |
|---|---|---|
| protocol tests | pass | pass |
| wire-codec fuzz, 1,000,000 cases | pass | pass |
| metadata fuzz, 1,000,000 cases | pass | pass |
| live reliability (4,000 scenarios, 20,000 cases, contention 8) | pass | pass |
| skeleton mutants | all CAUGHT | all CAUGHT |
| damaged-Parquet fuzz, 30,000 per shape | pass | pass |
| client matrix | exit 1: the known skeleton FAIL cells (schema browsing, text/CSV/table COPY with 0A000) | identical cells |

The client matrix is identical on both CPU types. Its FAIL cells are the rest of slice 10, not regressions.
