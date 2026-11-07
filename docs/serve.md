# Local Postgres-wire server

```
nata serve --listen 127.0.0.1:5432 --threads 4 --max-queries 4
```

Trust authentication, protocol 3.0, no TLS. The default listener is loopback;
non-loopback needs `--unsafe-listen` and is unsafe until TLS/auth exist. Startup
accepts SSL/GSS probes with `N`, UTF8 only, and supplies BackendKeyData for cancel.
This is a local lakehouse query interface, not a PostgreSQL database or catalog.
SQL continues to follow [dialect.md](dialect.md).

Simple Query and named/unnamed extended Parse/Bind/Describe/Execute/Sync/Close/Flush
work, including empty queries and EXPLAIN/ANALYZE. Describe statement sends
ParameterDescription then RowDescription/NoData; Describe portal sends only
RowDescription/NoData. Execute sends rows and CommandComplete, with no duplicate
RowDescription. After an extended error, all messages except Terminate are ignored
until Sync; Sync restores ReadyForQuery. Named statements survive Sync and simple
Query until Close, DEALLOCATE, or disconnect. Unnamed Parse/Bind silently replace
the corresponding object; a duplicate named statement returns `42P05`, a duplicate
named portal `42P03`. Unknown statement lookup returns `26000`, unknown portal
lookup `34000`. Close is idempotent even for unknown names, matching PostgreSQL.
A simple Query destroys the unnamed statement and portal. Portals are destroyed
at Sync in auto-commit, or at COMMIT/ROLLBACK in a compatibility transaction.

**Nonzero Execute max-rows on a row-producing query** returns `0A000`; set fetch
size to zero. Rowless commands (SET, COMMIT/ROLLBACK) accept a limit, since they
cannot suspend; this permits pgjdbc rollback after a fetch refusal. Suspended
portals are not implemented. Keeping an admission, worker share and scan window
parked between fetches needs an explicit idle-portal admission/expiry policy.
Named wire portals work with unlimited Execute; SQL DECLARE/FETCH cursors remain
unsupported, including psycopg server-side cursors. pgjdbc `fetchSize=100` with
autocommit off is tested to return `0A000`, with Sync/rollback recovery.

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

Default pgjdbc 42.7.7 and psycopg 3.3.8 preparation thresholds (both five) work;
no `prepareThreshold=0`, `%t`, or `binaryTransfer=false` workaround is required.
Binary input supports bool, int2/int4/int8, float4/float8, text/varchar and date;
text numeric parameters support exact Decimal values (cast the result explicitly
as above). Binary numeric **input** and float-special parameter literals remain
`0A000`/`22P02`; psycopg's default Decimal adapter uses text. Binary output supports
every OID in ADR 0008's slice-4 map, including numeric; NULLs stay length -1.
Malformed binary scalar lengths/values return `22P03`; format codes other than
0/1 return `0A000`. Smallint/float inputs retain the dialect widening above.

The [step 4 client results](../bench/results/20261107-pgwire-reliability.md) record
ten executions, types/values/NULLs, positioned errors, cancel and fetch refusal.
psql: `psql -X -h 127.0.0.1 -d nata -c "SELECT ... FROM 'file.parquet'"`.

## Session compatibility statements

A separate explicit session layer handles these before nata-sql. It does not
change the standalone SQL dialect. One statement per message, at most 16 KiB;
multi-statement messages return `0A000`.

| Statement | Supported values / answer |
|---|---|
| `SET extra_float_digits = ...` (or `TO`) | 1, 2, 3; PostgreSQL shortest round-trip; values below 1 return 0A000 |
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
| `DEALLOCATE [PREPARE] name`, `DEALLOCATE ALL` | release prepared statements; quoted names preserve case |
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

Per session: **256 prepared statements including unnamed**, with **4 MiB total
stored SQL plus names**. Up to 256 portals, additionally limited by a **16 MiB
conservative byte charge**: each portal charges its name, cloned SQL/description
and 4 MiB for bounded parameter AST storage. This deliberately limits even small
portals to at most three; these are protective initial limits, not capacity
measurements. Statement/portal budget exhaustion is `54000` (program limit).
Closing or replacing an object releases its charge immediately. A failed named
Parse preserves the old statement; failed unnamed Parse discards its predecessor.

`--threads` is the **total** CPU worker budget (default available parallelism).
`--max-queries` defaults to that budget, must be positive and no larger than it.
Each admitted query uses at most floor(threads/max-queries) existing scheduler
workers, plus its bounded coordinator/planning/evaluation helpers; no new pool or
spawn_blocking executor. Admission waits expire after five seconds with `53300`.
Two tokio workers service sockets; file access, planning, decode and evaluation
stay off them. DataRows cross a two-buffer bridge with at most **64 KiB per
buffer: 128 KiB queued bytes**, or 256 KiB including a producer buffer and a
writer buffer, plus a 64 KiB socket response buffer. Flush at executor batch
boundaries, ReadyForQuery, frontend Flush and before waiting for input. Row
encoding scratch and the existing executor scan window are separate; this is
not an arbitrary-width-cell or SF10 RSS certification. Metadata/EXPLAIN packets
over 64 KiB return 54000. Accepted sockets enable TCP_NODELAY.

A streamed buffer that cannot finish writing and flushing within **two seconds** cancels the
query and closes its socket, releasing the admission after workers join. Closing
is necessary because an unfinished frame cannot be followed safely by an error.
The log records `57014`. Ordinary CancelRequest matches the session id **and**
random secret, cancels only that session, and returns `57014`. Idle/stale/wrong
keys do nothing. Cancellation preserves the same in-progress frame for up to one
second before ErrorResponse; if still blocked, close instead. A completed byte
buffer may still end inside a wide DataRow: cancellation then closes the socket
rather than inserting an error into its unfinished body. Disconnect closes
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
| 22P03 | malformed binary parameter |
| 42P05 / 42P03 | duplicate named statement / portal |
| 26000 / 34000 | unknown statement / portal lookup |
| 54000 | per-session statement/portal budget |
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
cell values or cancel secrets. No TLS/auth, general COPY, SQL cursors or general catalog compatibility.

## Item 35 client notes

The [slice closeout](../bench/results/20261107-pgwire-close.md) records clean-VM
versions, the full successful SQL corpus, native types/NULLs, ten executions,
positioned errors, cancellation and frontend session chatter. Ordinary default
pgjdbc/psycopg and noninteractive pgcli use the existing protocol surface; pgcli's
DATE loader deliberately returns strings. psql returns display text and SQL
`PREPARE` remains `0A000` (wire preparation is a different interface).

ADBC PostgreSQL 1.13.0 connects using two exact, read-only type-bootstrap
answers (empty pg_attribute; built-in pg_type rows). Other catalog/completion SQL
returns 0A000. Default Arrow fetch now uses bounded binary COPY:

```sql
COPY (SELECT * FROM 'data/example.parquet') TO STDOUT (FORMAT binary);
```

Only that read-only query wrapper is supported, with optional `WITH` and quoted
`'binary'`. COPY FROM, files, text/CSV and other options return a clear 0A000.
Simple and extended execution share the DataRow admission, byte buffers,
backpressure, send deadline and cancellation path. Numeric remains the driver's
opaque Arrow extension with exact string storage; TINYINT maps to text.
The optional `adbc.postgresql.use_copy=false` path also remains supported.
ADBC 1.13.0 itself generates PostgreSQL-invalid COPY for SQL ending in a
semicolon followed by whitespace. Use `cursor.execute(query.rstrip())` with
that driver; this preserves its default COPY mode and all query semantics.
The original failure and normalization check remain recorded (R22).

Wire floats now use PostgreSQL's shortest-roundtrip layout, including exponent
thresholds, rounding endpoints/ties, `-0` and special values. `extra_float_digits`
accepts 1..3; values below 1 are refused because legacy rounding is unsupported.
CLI CSV formatting is unchanged. Wire DECIMAL retains its leading zero (0.500).
Known client display and CLI text differences remain; see the
[second fix record](../bench/results/20261107-pgwire-fixes2.md) for exact coverage.

The streaming performance budget is **not met**: final Rust raw/CLI is
3.661–4.407x against ≤1.20x. The [fix record](../bench/results/20261107-pgwire-fixes2.md)
includes matched PostgreSQL client references, on-VM profiles and the exact
measured source tree. Psql/reference ratios are context, not a changed target.
Use the recorded timings when
choosing this interface for large exports; first rows stream and slow clients
exert bounded backpressure, but that does not imply CLI-equivalent throughput.
