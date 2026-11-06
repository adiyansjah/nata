# Local Postgres-wire server

```
nata serve --listen 127.0.0.1:5432 --threads 4 --max-queries 4
```

Trust authentication, protocol 3.0, no TLS. The default listener is loopback;
non-loopback needs `--unsafe-listen` and is unsafe until TLS/auth exist. Startup
accepts SSL/GSS probes with `N`, UTF8 only, and supplies BackendKeyData for cancel.
This is a local lakehouse query interface, not a PostgreSQL database or catalog.
SQL continues to follow [dialect.md](dialect.md).

Simple Query and unnamed extended Parse/Bind/Describe/Execute/Sync/Close/Flush
work, including empty queries and EXPLAIN/ANALYZE. Describe statement sends
ParameterDescription then RowDescription/NoData; Describe portal sends only
RowDescription/NoData. Execute sends rows and CommandComplete, with no duplicate
RowDescription. After an extended error, all messages except Terminate are ignored
until Sync; Sync restores ReadyForQuery. Close releases the unnamed object; Sync
releases the portal. A simple Query replaces the unnamed statement and portal.

Named objects, binary parameters/results and **nonzero Execute max-rows** return
`0A000`; set fetch size to zero. Suspended portals are not implemented. Keeping
an admission and scan window parked indefinitely would undermine the reliability
bound; implementing suspension needs a separate bounded portal lifecycle.

Parameters are substituted as literal AST nodes, never pasted into the query.
Text/string OIDs 25/1043 hold raw UTF8; numeric OIDs 20/21/23/700/701/1700 accept
numeric dialect literals; OID 16 accepts true/false, t/f or 1/0; OID 1082 takes ISO
date text. Boolean words are case-insensitive, including pgjdbc's TRUE/FALSE.
NULL is supported. Other OIDs return `0A000`. SMALLINT parameters widen
to INTEGER and FLOAT to DOUBLE, the dialect's available CAST types. Unspecified
OID zero resolves to text (reported as 25); use `CAST($1 AS INTEGER)` for numeric
contexts. Numeric OID 1700 does not specify precision/scale: cast it explicitly,
e.g. `CAST($1 AS DECIMAL(10,2))`, when it is a result expression. A bound value
that changes the described result type returns `0A000` before rows, rather than
sending rows with false metadata. LIMIT/OFFSET accept nonnegative integer
parameters. Up to 1,024 parameters, 1 MiB SQL, and a conservative 1 MiB aggregate
parameter-expansion budget apply. Parameters cannot supply identifiers or paths.

Driver settings for this text-only, unnamed surface:

- pgjdbc: `prepareThreshold=0&binaryTransfer=false`, default fetch size zero.
- psycopg 3: `prepare_threshold=None`; use `%t` placeholders for text parameters
  (default `%s` chooses binary for integers and other types); keep `binary=False`.
- psql: `psql -X -h 127.0.0.1 -d nata -c "SELECT ... FROM 'file.parquet'"`.

Defaults connect and read text results, but are **not** sufficient for repeated
prepared/typed parameter use: pgjdbc names statements on execution five; psycopg
automatically prepares on execution six. Named and binary requests fail clearly.
See [ADR 0008](decisions/0008-client-protocol.md) and the
[local client results](../bench/results/20261107-pgwire-extended.md).

## Session compatibility statements

A separate explicit session layer handles these before nata-sql. It does not
change the standalone SQL dialect. One statement per message, at most 16 KiB;
multi-statement messages return `0A000`.

| Statement | Supported values / answer |
|---|---|
| `SET extra_float_digits = ...` (or `TO`) | 0, 1, 2, 3; formatting remains shortest round-trip |
| `SET application_name = '...'` | store for SHOW and report ParameterStatus |
| `SET DateStyle = 'ISO'` | ISO / ISO, MDY / ISO, DMY accepted; report canonical ISO, MDY |
| `SET client_encoding = 'UTF8'` | UTF8 / UTF-8 only; report UTF8 |
| `SET TimeZone = '...'`, `SET TIME ZONE '...'` | store only; Date32 has no timezone; report TimeZone |
| `SET search_path = ...` | store only; SQL still resolves quoted file paths |
| `SET statement_timeout = 0` | zero only; nonzero refused rather than silently ignored |
| `SET standard_conforming_strings = on` | on only, ParameterStatus |
| `SET default_transaction_read_only = off` | off only |
| `SET default_transaction_isolation = 'read committed'` | read committed only |
| `SET default_transaction_deferrable = off` | off only |
| `SHOW <setting>` | all settings above, server_version, server_version_num, server_encoding, integer_datetimes; `SHOW TIME ZONE` also works |
| `SELECT 1` | integer 1 (pgcli connection probe) |
| `SELECT version()` | PostgreSQL 16.0 (Nata compatibility) |
| `SELECT current_setting('setting')` | same setting table; literal argument only |
| `SELECT current_schema()` | public (compatibility marker, not a catalog) |
| `BEGIN [WORK/TRANSACTION]`, `COMMIT [WORK]`, `ROLLBACK [WORK]` | auto-commit no-ops; command tags BEGIN/COMMIT/ROLLBACK |

ReadyForQuery reports I outside a compatibility transaction, T after BEGIN,
E after an error inside one, I after COMMIT/ROLLBACK. COMMIT after E reports
ROLLBACK. These markers do not provide isolation, snapshots, rollback of work,
or transaction-local settings. Unknown settings, non-UTF8 encoding, startup
`options`, replication and other transaction commands fail explicitly. Startup
user/database are routing labels; accepted setting keys use the same allowlist.
Startup reports server/client encoding, version, DateStyle, TimeZone,
application_name, integer_datetimes and standard_conforming_strings.

pgcli's catalog/completion queries and psql catalog backslash commands remain
unsupported; no information_schema/pg_catalog emulation is claimed.

## Bounds, cancellation and errors

Default: 64 authenticated sessions, eight reserved startup/cancel slots, at most
72 sockets, five-second total startup deadline. Startup cap 10,000 bytes; other
frontend cap 1,114,112 including length, excluding tag. Headers are checked before
body allocation. `--max-connections` accepts 1..=64.

`--threads` is the **total** CPU worker budget (default available parallelism).
`--max-queries` defaults to that budget, must be positive and no larger than it.
Each admitted query uses at most floor(threads/max-queries) existing scheduler
workers, plus its bounded coordinator/planning/evaluation helpers; no new pool or
spawn_blocking executor. Admission waits expire after five seconds with `53300`.
Two tokio workers service sockets; file access, planning, decode and evaluation
stay off them. One 32-row bridge per query retains backpressure. These are row
bounds, not byte bounds for arbitrary-width cells or an SF10 RSS certification.

A backend frame that cannot finish writing within **two seconds** cancels the
query and closes its socket, releasing the admission after workers join. Closing
is necessary because an unfinished frame cannot be followed safely by an error.
The log records `57014`. Ordinary CancelRequest matches the session id **and**
random secret, cancels only that session, and returns `57014`. Idle/stale/wrong
keys do nothing. Cancellation preserves the same in-progress frame for up to one
second before ErrorResponse; if still blocked, close instead. Disconnect closes
the bridge and cancels/joins workers. Sessions install no process signal handler;
SIGINT remains CLI-only.

Nata-owned syntax/bind/evaluation messages and query positions reach ErrorResponse;
closest-column suggestions are Hint fields. Parameter diagnostic text is generic
where substitution could otherwise quote parameter data. Foreign library/IO text
is generic, never a Detail field that could carry cell data. SQLSTATEs:

| Code | Meaning |
|---|---|
| 42601 | syntax |
| 42703 | unknown column |
| 42P01 | unknown/unreadable relation or file |
| 42883 | unknown function (function calls outside the dialect) |
| 42804 | type/rule mismatch |
| 22012 | division by zero |
| 22003 | numeric overflow/range |
| 22P02 | bad cast text / malformed text parameter |
| 0A000 | unsupported feature |
| 57014 | cancel / send timeout (socket closed on incomplete frame) |
| 08P01 | protocol violation |
| 53300 | too many connections / admission timeout |
| 22000 | other evaluation / unrepresentable wire text |
| XX000 | internal error |

Position is a 1-based character offset in the original query, including newlines.
RowDescription follows ADR 0008; NULL length -1 differs from empty string length
zero. Date32 outside years 1–9999 and text with embedded zero fail explicitly.
Query-worker logs contain session id, duration, rows and SQLSTATE, never SQL,
cell values or cancel secrets. No TLS/auth, COPY, binary or catalog compatibility.
