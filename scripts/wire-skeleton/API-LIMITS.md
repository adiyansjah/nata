# Client API limits (slice 10)

`api-limits.json` gives each of the nine unresolved cells its own reason and
primary source. The matrix copies it beside its result and leaves the cell
`not run`; its unchanged acceptance rule still exits 1 for **any** non-PASS row.
These are API assessments, not successful protocol probes.

Versions: psql 17.11, psycopg 3.3.8, pgcli 4.7.1, pgjdbc 42.7.14,
ADBC PostgreSQL 1.13.0 (ADBC libraries release 25). Inspect pinned psycopg
`pq/abc.py`, `_server_cursor_base.py`, pgcli `pgexecute.py`, ADBC
`statement.cc`/`result_reader.cc`, pgjdbc's public PGConnection API, and PostgreSQL
17 `src/bin/psql/common.c` and `src/interfaces/libpq/fe-exec.c`.
libpq's extended query builder writes zero for Execute's row limit. Its row/chunk
result modes change delivery to the caller. They do not suspend the server portal.

The extra `chunked-delivery` / `fetchmany-client-buffer` rows check exact values
and exhaustion separately from `positive-fetch`. The psycopg ServerCursor probe
requires 0A000, rollback and healthy reuse: DECLARE/FETCH remains excluded by
ADR 0015. None substitutes for positive wire fetch. The existing pgjdbc probe is
the supported positive wire-fetch API. The existing raw malformed-frame probe
checks rejection and a healthy second connection, separately from all five
client API cells. Descriptor access, a socket factory, transport interception or
proxy corruption can inject bytes, but cannot certify that a client emission API
supports arbitrary malformed frames.

## Proposed parent-recorded deviation (not applied)

For each impossible cell, record the exact client/version/action, source and API
reason, parent/owner decision and date, substitute evidence path and its limits,
and a revisit trigger (driver change or a newly exposed API). Bind the decision
to the tested commit. For positive-fetch, retain pgjdbc plus raw byte fixtures;
for malformed frames retain raw fault injection and native codec/fuzz evidence.
Keep the original matrix JSON and exit 1 visible. A separate closeout decision
may cite the approved deviations; it must never rewrite a row to PASS, omit it,
or make the matrix exit 0. No deviation is approved by this implementation.

## Resource stop condition

The next item cannot become a complete acceptance gate under the current card:
card 0007 §2/§3 requires reference-derived RSS bounds frozen before new Nata
measurements. The reference record's resource decision leaves RSS and metadata
scratch adequacy unfrozen; its final correction freezes streaming only. The
2026-11-08 item-1/item-2 records explicitly retain this gap. A 16 MiB metadata
cap or 128 KiB queue is not a process RSS ceiling. Supplying one would invent a
new target; accepting without it changes the card rule. Parent must supply a
reference-only RSS/adequacy freeze before resource acceptance proceeds. No ADR,
threshold or heavy-template exit has been changed here.
