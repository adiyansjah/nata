# PostgreSQL wire/catalog reference

`wirecapture` in `scripts/gcp/job.sh` is an independent reference job. It builds
only the std-only Rust client and Java metadata client; it never builds Nata.
The distribution server binds loopback, uses trust and UTF8, and logs every
statement during capture. `protocol.py` proxies the actual client frames;
startup, passwords and BackendKeyData are omitted. Only synthetic fixtures
are used. Timing bypasses the proxy and disables statement logging.

Clients are pinned to the slice-4b observed versions. JDBC metadata calls
exercise DBeaver's driver APIs, not its navigator or its shipped driver version.
Psycopg has no schema browser: its metadata SQL is explicit harness input.
Pgcli uses CompletionRefresher's real default background-refresh implementation.
ADBC uses every exposed GetObjects depth and GetTableSchema, then default COPY.

Local checks, one command at a time (TMPDIR must be on disk):

```
python3 scripts/wirecapture/test_protocol.py
python3 -m py_compile scripts/wirecapture/*.py
rustc --edition=2021 -O scripts/wirecapture/raw.rs -o "$TMPDIR/wcap-raw"
bash -n scripts/wirecapture/run.sh
bash scripts/gcp/test/test-job-chains.sh
```

Plan both machines independently, keep each worst case below the approved cap,
and run detached as described in `scripts/gcp/README.md`. There are 15 settle
blocks, each limited to ten minutes, plus setup and the small catalog sweep;
the 2.75-hour bound includes that worst case. A failed settle is recorded and
its timing omitted, never silently treated as a valid reference. Each timing
arm has one warm validation followed by five alternating primary/twin pairs.
Psql and JVM process startup are included; Python interpreter startup is
excluded, but Python connection/bootstrap is included. Raw includes process,
connection and per-value validation. No cross-client ratios are meaningful.
PostgreSQL's per-gather worker limit is configuration, not an enforced Nata-style
worker/admission budget. No inference about eight-worker utilization follows.

Results: `wirecapture/` contains versions, SQL log, frontend/backend frames,
action-to-connection map, timing samples, browse samples and fixtures. The
backend fixtures contain tag and exact payload hex (length = payload + 4).
Frontend groups explicitly include Flush/Sync. On a matching server build,
load `fixture.sql` and run `python3 scripts/wirecapture/replay.py FIXTURE.json`.
`concurrent` inserts a fresh CancelRequest using the live session key; secrets
are never stored. Error source file/line fields are version-specific. Generated
object OIDs must be mapped when adapting COPY/catalog fixtures to another server.

Offline inventory (pglast 8.7):

```
python3 scripts/wirecapture/inventory.py RESULTS/wirecapture \
  docs/decisions/0014-read-only-catalog-views.md OUTPUT
```

This is a conservative AST reducer, not proof of metadata-lowering sufficiency.
It resolves captured aliases using the reference column inventory; stars expand
against that inventory. It keeps separate SELECT scopes and expands base-relation stars, while
ignoring derived output/CTE names. Correlated/ambiguous unqualified references
still require manual audit. Review `sql.json` alongside the table before
freezing. Functions include generic aggregate/array operations, which need a
bounded implementation even when they are not catalog-object providers. A helper
marked covered means named by ADR 0014, not implemented by Nata. Missing functions
inside predicates against empty objects still require query binding/typing.

The reference does not freeze PostgreSQL 16 compatibility from PostgreSQL 15
or 17; does not certify actual DBeaver UI; does not measure 64-session contention,
parked portal expiry, Nata RSS/scratch, wide streaming controls or pre-change
Nata small-query regression. Those remain explicit parent decisions/gates.

The recorded subprocess medians used timeout polling with no pipe. The final
helper drains stderr through a pipe to remove that delay; it has a local control
but no permitted third cloud run. Do not use the old psql/raw/JVM samples to
freeze P16. Python in-process psycopg/ADBC samples do not use that wait path.
