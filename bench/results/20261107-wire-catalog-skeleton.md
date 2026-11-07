# Wire/catalog skeleton preflight — 2026-11-07

**STOPPED for parent scope resolution; walking skeleton NOT IMPLEMENTED.**
This is a preflight record, not a passing skeleton or slice-10 closeout. No
runtime code, dependencies, accepted decisions, or cloud resources changed.
Source inspected: `7d9f4a7c59e8e9d125f6ef0fee74f9325b2e0cd4`.

## Required resolution

- Card 0007 requires the parent to name the immutable registered-catalog
  interface and its owner before implementation. There is no such interface
  in `nata-pgwire`, `nata-sql`, or `nata`: `SourceProvider` supplies path expansion
  and Parquet leaves, not catalog identity, schemas, nullability, snapshot
  generations, or stable object identities. The card gives registration and
  CREATE/DROP CATALOG to track B; this task asks track C to add registration.
  Proposed seam: a parent-approved immutable local-file snapshot, populated at
  startup from explicit configuration, owned by track B and consumed by track C.
  No filesystem discovery or SQL CREATE CATALOG is proposed here.
- The frozen reference has 65 client/SQL pairs, including catalog joins,
  correlated subqueries, arrays, aggregate and window operations. The current
  metadata path is two exact ADBC bootstrap queries in `session.rs`, not a
  bounded relational adapter. Before building one, scope its independently
  supported operations and per-query refusals. In particular JDBC getColumns
  uses row_number and ADBC GetObjects uses array aggregation. Implementing these
  through general aggregate execution would cross the track-A boundary.
  Refusing them is allowed, but those client actions must remain failures.
- ADR 0015 requires oracle-frozen cancellation/deadline behavior and a
  fail-capable lifecycle model. The reference explicitly leaves suspended-idle
  cancel delivery and Parse/Bind/pipelined deadline reset points uncaptured.
  Existing production `Portal` has only statement/parameters/result formats;
  `Session.active` holds one active query, and execution unconditionally joins
  its producer. Retention changes this ownership model. A model and reference
  capture must precede that change; neither is supplied in this checkout.
- PostgreSQL fixtures are 15.19 while the server advertises 16.0. The reference
  labels the major-version comparison unresolved. Do not change the advertised
  major or claim complete fixture compatibility without that comparison.

These are card part-1/part-3/part-4 scope and contract questions, not a request
for a new dependency, TLS/authentication, SQL cursors, or performance work.
The task authorizes implementation; this record does not interpret the old
documents-only wording as withholding that authorization.

## Client × action (unchanged baseline, not skeleton certification)

The installed clients were run against this worktree's freshly built unchanged
server. SELECT is over a quoted local Parquet path, **not a registered table**;
that required action remains NR for every client. The fixture contains BIGINT
0..999999; SELECT/COPY check the first three rows. No timings are claimed.
NR = not run; unimplemented skeleton paths remain open. These are fresh local
observations, not inherited results relabeled as skeleton passes.

| Client | Default connect | Local-file SELECT | Schema browsing | Positive fetch | Binary query COPY | Timeout | Cancel | Error/recovery | Malformed frame |
|---|---|---|---|---|---|---|---|---|---|
| psql 17.11 | PASS | PASS | FAIL: \\d, \\dt, \\dn, \\df, \\l each exit 1 | NR: no wire probe | PASS | FAIL: SET exits 1 | NR: no client cancel probe | NR: no psql recovery probe | NR: shared raw probe below |
| pgjdbc 42.7.7 | PASS | PASS: 10 PreparedStatement executions | FAIL: getCatalogs/getTypeInfo 0A000; getSchemas/getTables/getColumns/getPrimaryKeys/getIndexInfo 42883 | FAIL: 0A000 at fetchSize=2 | PASS | FAIL: SET 0A000 | NR: no JDBC cancel probe | PASS: 22012, rollback, reuse | NR: shared raw probe below |
| DBeaver | NR: unavailable | NR | NR: actual UI/init capture absent | NR | NR | NR | NR | NR | NR |
| psycopg 3.3.8 | PASS | PASS: 10 executions cross default preparation threshold | FAIL: information_schema tables/columns and pg_type/namespace join 0A000 | NR: no wire probe | PASS | FAIL: SET 0A000 | PASS: 57014, rollback, reuse | PASS: 22012, rollback, reuse | NR: shared raw probe below |
| pgcli | NR: absent from supplied environment and PATH | NR | NR: completion refresh unavailable | NR | NR | NR | NR | NR | NR |
| ADBC PostgreSQL | NR: absent from supplied environment | NR | NR: get_objects/get_table_schema unavailable | NR | NR | NR | NR | NR | NR |

The shared raw probe sends a length-3 Query frame: **PASS**, bounded 08P01 and
a healthy psycopg connection still queries successfully. Binary COPY in all
three installed clients matches the complete 63-byte PostgreSQL binary payload
for BIGINT 0,1,2, including header/trailer, not just its signature. This is a
small generated payload check, not replay of the committed COPY fixture. Text,
CSV, table-form COPY and the full scalar-type corpus were not probed here.
Local pgjdbc differs from the reference's 42.7.14; it is not version parity.

[Probe sources and output](raw/20261107-wire-catalog-skeleton/local/) are retained
as diagnostic evidence. They deliberately print individual failures and do not
turn them into a failing orchestration exit status; **do not use their exit code
as a compatibility gate**. Reproduce with the supplied client installation:

```sh
mkdir -p "$HOME/tmp-wcskel/tmp" "$HOME/tmp-wcskel/logs"
export TMPDIR="$HOME/tmp-wcskel/tmp" CARGO_TARGET_DIR="$HOME/tmp-wcskel/target"
cargo build -p nata
duckdb -c "COPY (SELECT range AS v FROM range(1000000)) TO '$HOME/tmp-wcskel/data.parquet' (FORMAT parquet)"
cp bench/results/raw/20261107-wire-catalog-skeleton/local/Baseline.java "$HOME/tmp-wcskel/"
JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR" "$HOME/tmp-4b/py/bin/python" bench/results/raw/20261107-wire-catalog-skeleton/local/probe.py
# Remove this scratch directory after inspecting its logs.
rm -rf "$HOME/tmp-wcskel"
```

Named SQL DECLARE/FETCH/CLOSE stays outside T0 per card 0007 and ADR 0015.
The psycopg ServerCursor attempt returns 0A000 and is rolled back: **PASS** for
that negative boundary, not SQL-cursor support. Individual FETCH/CLOSE refusal
probes remain unrun. No cursor amendment is proposed. No preparation, binary,
COPY or SSL preference workaround was used.

## Fixtures, inventory, and mutants

Measured offline: all **11 ARM/x86 fixture pairs are byte-identical**; both
inventories agree: **87 client/object rows, 51 named by the original ADR,
36 missing**, and **65 client/SQL pairs**. These counts reproduce the reference,
not Nata compatibility. No fixture was replayed against Nata in this task.

Inventory implementation coverage is **unverified**: none of the 36 additions
has a new served or refused-with-tested-client-effect verdict. All 36 remain
missing from this task's implementation evidence. The two old bootstrap answers
cannot certify registered-table attributes. An inventory row's `covered` flag
means the ADR names it, not that production serves it.

The five requested new mutants were **not added or run**: wrong catalog-row OID,
missing exact-end PortalSuspended, ineffective statement_timeout, silent wrong
rows for a refused object, and cancel missing a suspended portal. Zero catches
are claimed; INVALID would never count. Existing sweeps do not certify these
missing production paths.

## Parent run commands

No GCP command was executed. Existing `wirecapture`, `item35`, and `pgwirecheck`
jobs are reference/inherited-regression tools, **not a new skeleton matrix**.
After resolving the contracts, implement the new matrix/job steps before using
these commands as skeleton evidence. For a separately approved inherited check:

```sh
bash scripts/gcp/orchestrate.sh plan --job pgwirecheck --machines n4a-highcpu-8 --max-vm-hours 1
bash scripts/gcp/orchestrate.sh plan --job pgwirecheck --machines c3d-highcpu-8 --max-vm-hours 1
# Supply the parent's unique run ids and approved worst-case budgets from plan.
bash scripts/gcp/orchestrate.sh run --job pgwirecheck --machines n4a-highcpu-8 --max-vm-hours 1 --run-id "$WC_ARM_RUN" --approved-usd "$WC_ARM_BUDGET"
bash scripts/gcp/orchestrate.sh run --job pgwirecheck --machines c3d-highcpu-8 --max-vm-hours 1 --run-id "$WC_X86_RUN" --approved-usd "$WC_X86_BUDGET"
```

The future `heavy.steps` must contain the full default-client matrix, PostgreSQL
fixture replay and inventory verdicts, the five new mutants, portal lifecycle
model/real-code loom bound 3, metadata-SQL fuzz (1,000,000 cases), inherited
wire/COPY/state fuzz, SQL subset fuzz, damaged-file fuzz, sanitizer checks and
fresh-machine resource gates. There are no runnable new portal/model/metadata
targets yet; inventing commands for them would be a stub. Existing nightly
sizes/seeds are in `.github/workflows/nightly.yml`; retain them. This task
prepared no compliant new heavy job and makes no timing or resource verdict.

## Remaining slice-10 work

Resolve the seam and oracle gaps, then implement the registered catalog and
bounded typed metadata lowering with explicit per-object client effects;
model and implement retained execution, expiry, cancellation and statement
deadlines; expand COPY only to the frozen forms; run every default-client
action independently so a failed bootstrap cannot hide later cells. Recheck
ordinary SQL/aggregate ownership and preserve DATE domains. Then run both-CPU
deep/resource checks and frozen paired performance gates, pay down P14 from
profiles, update contracts and the register, and close only on the tested SHA.

## Local checks

PASS, one heavy command at a time, with TMPDIR on disk under the home directory:

- `cargo fmt --check`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test --workspace`: 479 passed, zero failed/ignored across 70 summaries.
- `cargo deny check`: advisories/bans/licenses/sources OK; existing unmatched
  license-allowance warnings remain. No dependency/unsafe inventory change.
- Fresh SF0.01 plus SQL fixtures generated with `scripts/gen-tpch.sh` and
  `scripts/gen-sql-check-data.sh`; freshly built `nata-check` against freshly
  built `nata`: scan 5/5, sql 252/252, sql-differences 54/54, aggregate 65/65;
  zero mismatch/error/unsupported/pending.
- `diff -rq fixtures/wirecapture/postgresql-15.19-arm fixtures/wirecapture/postgresql-15.19-x86`:
  no differences; JSON inventory counts/architecture equality checked offline.
- Installed-client probes above completed; cell failures remain failures.
- `scripts/check-leaks.sh --staged` and `git diff --cached --check`: PASS.

The scratch target, fixtures, logs and JVM temporary directory used
`~/tmp-wcskel`; that directory was deleted after preserving these synthetic probes.
No new fuzz/model/mutation campaign or resource/performance gate was run.
