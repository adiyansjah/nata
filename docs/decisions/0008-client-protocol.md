# 0008: Client protocol

Status: accepted (by the assistant under the owner's delegation, 2026-11-07)
Date: 2026-11-07

## Context

[Card 0003](../slices/0003-pgwire-minimal.md) brings real clients forward from slice 10 to slice 4b. Slice 4 already has
positioned SQL errors, typed Arrow results and a cancellable, windowed executor. [ADR 0007](0007-sql-dialect-and-strictness.md)
remains the SQL contract: speaking Postgres wire does not mean implementing Postgres SQL.
The card fixed R1 (reuse versus build) and R2 (lossless type mapping) before the experiment.

## Decision

- Speak **Postgres wire 3.0**, simple query and extended query for **unnamed statements and portals**, no parameters or text parameters
  representing dialect literals. Use text results; reject binary requests explicitly. Support Parse/Bind/Describe/Execute/Sync/Close,
  Flush, Terminate, empty queries, structured errors and the card's bounded session chatter. A nonzero Execute row limit needs a bounded
  suspended portal, never a collected result. Discard extended messages after an error until Sync; Sync restores ReadyForQuery.
- **Write the supported message codec ourselves.** R1 selects it because pgwire 0.42.1 panics on malformed Parse messages.
  Do not add pgwire to production dependencies. Tokio remains the I/O runtime; do not introduce another general executor.
- Apply **R2** using the table below. NULL is a DataRow field length of `-1`, distinct from an empty string of length zero.
  RowDescription has table/attribute zero (no Postgres catalog), text format zero, correct fixed type sizes and numeric
  typmod `4 + (precision << 16) + scale`; otherwise typmod `-1`. Column names and values are encoded separately.
- Connections run on the small **ADR 0002 tokio I/O runtime**. Planning, local file work, Parquet decode and evaluation stay off it,
  on CPU execution workers. Bound query admission/total worker allocation; 64 connected sessions do not authorize 64 full CPU pools.
  Preserve the existing big-stack planning/evaluation boundaries while R15 repairs instrumentation safety. Do not use an unbounded
  `spawn_blocking` pool as a query scheduler. The synchronous executor hands batches to a bounded bridge; async writers await socket
  readiness. A full bridge holds the executor's sink/window slot, stopping further scan claims. Cancel and disconnect must close/wake
  both halves and join the query's tasks. The spike's per-query coordinator/scan threads are an integration probe, not a shared-pool implementation.
- **Loopback only**, default `127.0.0.1:5432`, trust authentication. Refuse other listen addresses unless `--unsafe-listen` is explicitly
  supplied. That flag is unsafe until authentication and TLS exist; no TLS in this slice. Answer SSL/GSS encryption probes with refusal,
  never advertise encryption. Do not reuse the CLI's process-wide SIGINT watcher for session cancellation (R13).
- **Limits (reasoned initial defaults):** 64 authenticated session slots; 8 additional bounded pre-startup/cancel slots, with a five-second
  startup deadline so a full session set can still cancel. At most 72 accepted sockets; close excess sockets without spawning a session.
  Startup packets at most 10,000 bytes. Other frontend messages at most **1,114,112 bytes**, including the four-byte length but excluding
  the tag; SQL text at most the dialect's 1 MiB. Read only the fixed header first, reject lengths below four, negative lengths and excessive
  lengths **before reserving a body buffer**. Validate startup's minimum eight bytes, exact CancelRequest size 16, each body field/count,
  UTF-8/terminators and exact frame consumption. Check counts against remaining bytes before allocating collections. Parameter expansion
  must obey the SQL bound too. These are production requirements; only the common frontend header bound was exercised live in the spike.
- Send **BackendKeyData** with a fresh per-session id and unpredictable 32-bit secret. A separate CancelRequest connection matches both
  against a live-session registry and cancels **only that session's active query token** (`ScanCancel`), with SQLSTATE `57014`.
  Unknown/stale keys and idle sessions have no effect; close the cancel connection without a reply. Remove the registry entry on session
  close, use a fresh token per query, and serialize token replacement with cancel so an old request cannot poison the next query.
  Cancellation is cooperative at executor boundaries; a blocked send must also be interruptible. The spike's deterministic keys are not production keys.

### R2 type map

Readback columns are **measured** on pgjdbc 42.7.7 `getObject` and psycopg 3.3.8, text protocol, for the fixture values in the
[baseline](../../bench/results/20261107-pgwire-baseline.md). Generalizing to each type's entire domain is **reasoned**, with edge gaps below.

| Nata slice-4 output type | Postgres OID / type | Wire text | pgjdbc reads | psycopg reads |
|---|---|---|---|---|
| BOOLEAN | 16 / bool | `t` or `f` | Boolean | bool |
| TINYINT (signed 8-bit) | 25 / text | signed decimal integer | String | str |
| SMALLINT | 21 / int2 | signed decimal integer | Integer | int |
| INTEGER | 23 / int4 | signed decimal integer | Integer | int |
| BIGINT | 20 / int8 | signed decimal integer | Long | int |
| FLOAT (32-bit column) | 700 / float4 | shortest round-trip decimal; preserve signed zero; `NaN`, `Infinity`, `-Infinity` | Float | float |
| DOUBLE | 701 / float8 | shortest round-trip decimal; same special spellings | Double | float |
| DECIMAL(p,s), p ≤ 38 | 1700 / numeric | exact fixed-scale decimal, no conversion through float | BigDecimal (scale retained) | Decimal |
| VARCHAR | 25 / text | UTF-8, no CSV quoting | String | str |
| DATE | 1082 / date | ISO `YYYY-MM-DD` for years 1–9999 | java.sql.Date | datetime.date |

**No exact Postgres type:** TINYINT has no signed eight-bit Postgres equivalent; R2 chooses text rather than silently changing its type
identity to int2. VARCHAR has a value-equivalent text representation without a declared length. `SqlType::Null` is binder-only and cannot
be a bound result; typed NULL uses its column's OID. Unsigned integers, timestamps, binary and nested types are rejected by slice 4's SQL
binder, so are not additional wire mappings.

**Representation edges still to certify:** Parquet Date32 can carry BC, infinity and years outside 1–9999, beyond the literal/cast date
range and psycopg's standard date object. Do not claim lossless default-driver readback for these values. Before full-domain compatibility,
item 35 must establish a column-wide text fallback for exceptional date domains (announced before rows), or an explicit conversion error;
never change an OID mid-result or silently clamp a date. Signed zero, float specials, all NULL/empty-string cases, extreme dates, numeric
precision/scale metadata and parameter boundaries need the permanent R2 tests. The table fixes the ordinary mapping, not a certification
of those edges. No extra SQL types are introduced here.

## Why

**Measured:** pgwire meets the thread and streaming criteria, and cargo-deny accepts its graph, but its frontend decoder panics on
2,015/10,000 seeded regular-frame cases; the five-byte empty Parse frame alone reproduces it. The checked hand decoder has zero panics
in 10,000 regular and 10,000 startup cases; removing its bounds check makes the same test fail. R1's disqualifier applies without a
speed comparison. **Reasoned:** owning the small supported codec is preferable to placing an additional complete validator in front of
pgwire or forking it. That would already mean owning the hostile-input boundary.

## Alternatives considered

- **Reuse pgwire 0.42.1**, defaults disabled, feature `server-api`: closest alternative, good streamed response API, no hidden runtime;
  loses R1(iii). A panic catch would contain unwinding but would not make “fuzzed without panics” true. Default TLS features also add
  aws-lc-rs, intentionally absent here.
- **Patch/fork pgwire or validate every field before calling it:** potentially viable later; not the unchanged reuse candidate tested.
  Neither removes our obligation to own and fuzz the complete supported decoder.
- **Keep CLI/CSV only:** preserves today's interface but cannot satisfy the card's real-client goal. HTTP/Arrow Flight would require
  different clients and does not answer item 35.

## Evidence

- **Measured, local x86-64 only:** [baseline](../../bench/results/20261107-pgwire-baseline.md), including source audit, thread counts,
  RSS/backpressure with a collect-all mutant, seeded decoding with a guard-removal mutant, added-workspace cargo-deny and its banned-crate
  mutant, dependency unsafe inventory, psql/pgjdbc simple + extended + cancel, and a pgjdbc/psycopg type fixture. Both codecs deliver all
  1,000,000 ordered integer values, also checked against Nata CSV and DuckDB. The invalid boolean wire form is caught by pgjdbc.
- **Reasoned:** protocol scope, security, session limits, production admission/bridge design, secret generation and registry lifecycle;
  full-domain type mapping beyond the fixture.
- **Not covered:** production `nata serve` (not built in step 1), permanent decoder fuzz/leak suites, TLS/auth, 64-session scaling,
  fairness, reliable cancel during blocked output, R15 sanitizer safety/R16 SF10 RSS repair, ARM64, speed budgets on either CPU,
  binary/transactions/COPY, all driver chatter and item 35's full real-client job. Local resource probes do not close R16.

## Confidence

**High** that unchanged pgwire 0.42.1 fails R1(iii), and that the bounded response path can stream on the existing runtime.
**Medium** for the hand decoder within the tested corpus and for ordinary type readback. **Low** for untested physical Date32 edges
and production session/cancel resource behaviour. A fixed upstream decoder plus the same passing bake-off would change the implementation choice.

## Consequences

We own framing, checked body reads, error recovery, protocol state and their permanent mutants. No protocol dependency lands in production
in this design step. Keep a small excluded spike until the parent can replay the failure; delete it when permanent step-4 checks replace it.
Session-scoped cancellation and backpressure become explicit integration obligations rather than assumptions about a library.

## Revisit when

Upstream pgwire fixes and fuzzes these decoders; item 35 finds a mapping/chatter incompatibility; a new Nata type needs an OID;
TLS/auth or a non-loopback deployment is required; the two-CPU benchmarks expose a bridge/codec bottleneck.

## Implementation note, 2026-11-07 (step 2, local)

The SIMPLE server uses one global query-admission slot and the existing executor's
configured CPU workers, plus one coordinator; connected sessions cannot multiply
full CPU pools. This is deliberately conservative pending the step-5 scalability
measurement. Extended messages are decoded but explicitly rejected until step 3.
Non-cancel SQLSTATE classes use 42 (syntax/bind), 0A (unsupported), 22 (evaluation),
58 (source I/O), XX (internal); positions count characters across the whole query.
The R15 planning stack is now 64 MiB: local ASan/build-std fails at 16 MiB and
passes at 32 MiB; the final reservation has 2x headroom without changing SQL limits.
Dates outside years 1–9999 and embedded zero bytes return class 22 explicitly.
Cancellation preserves an in-progress backend frame for up to one second before
sending ErrorResponse; if the reader remains blocked the socket closes. Evidence
and remaining certification: `bench/results/20261107-pgwire-server.md`.

## Amendment, 2026-11-07: step 3 extended protocol, compatibility and admission

**Decision (reasoned, locally checked):** implement unnamed Parse/Bind/Describe/
Execute/Sync/Close/Flush, retaining text-only results. Reject **named** objects
and **nonzero Execute max-rows** with 0A000. Alternative: named caches/suspended
portals. They add retained plans, admissions and idle lifecycle bounds beyond
this small slice; bounded unnamed state plus explicit client settings meets the
card without unbounded caches or parked workers. Sync clears the extended error
state and releases the portal; errors discard following work until Sync.

Parameters become literal expression nodes at the parser-to-Nata-AST seam.
The standalone parser supplies no nodes and continues to reject placeholders.
No bound data is pasted into the user's SQL, nor used as an identifier/path.
Boolean text is case-insensitive: the real pgjdbc probe sends TRUE, not just t.
Unspecified OIDs resolve to text; explicit SQL CAST supplies numeric context.
SMALLINT/FLOAT parameters widen to INTEGER/DOUBLE, the existing dialect CAST
surface. Numeric OID 1700 has no precision/scale: a result depending on its value
must be explicitly cast to stable metadata, or returns 0A000 before rows.
Compare the bound result's wire types to Parse metadata before sending data.
Bounds: 1,024 parameters and a conservative 1 MiB expansion budget (counts repeated
references). Binary payloads are opaque to the codec and refused by the session
with 0A000, including non-UTF8 bytes. NULL is distinct from empty string.

**Driver bootstrap source audit (2026-11-07):**

- [pgjdbc 42.7.7 ConnectionFactoryImpl](https://github.com/pgjdbc/pgjdbc/blob/REL42.7.7/pgjdbc/src/main/java/org/postgresql/core/v3/ConnectionFactoryImpl.java)
  sends startup client_encoding UTF8, DateStyle ISO and TimeZone; optional
  application_name/search_path/options/replication depend on properties. Its
  initial query sets application_name when not sent in startup. For reported
  version 16 it skips extra_float_digits; older-server paths set 2 or 3.
  [Driver settings](https://jdbc.postgresql.org/documentation/use/) default to
  binaryTransfer=true and prepareThreshold=5. Required here:
  **prepareThreshold=0&binaryTransfer=false**, default fetch size zero.
- [psycopg connection base](https://github.com/psycopg/psycopg/blob/master/psycopg/psycopg/_connection_base.py)
  defaults to autocommit=false and sends BEGIN before the first operation;
  nondefault isolation/read-only/deferrable options can extend BEGIN (refused).
  libpq supplies startup user/database/client_encoding/application_name as
  configured. [Preparation](https://www.psycopg.org/psycopg3/docs/advanced/prepare.html)
  automatically names after five uses. Required here: **prepare_threshold=None**
  and **%t placeholders** to force text parameter dumps; binary=False results.
  Default %s adapts integers to binary even before preparation.
- [psql startup](https://github.com/postgres/postgres/blob/master/src/bin/psql/startup.c)
  uses libpq startup/ParameterStatus for version and encoding, and then user SQL;
  no mandatory SQL bootstrap for `psql -X -c`. Startup application_name is allowed.
  User psqlrc SQL, catalog backslash commands and completion are outside this
  guarantee. The local extracted client was exercised, not installed system-wide.
- [pgcli PGExecute](https://github.com/dbcli/pgcli/blob/main/pgcli/pgexecute.py)
  uses psycopg, autocommit and a client encoding, registers type loaders, and
  fetches connection metadata. Its `SELECT 1` connection probe and TIME ZONE
  spellings are accepted. Without a supplied host it can query pg_settings for
  unix_socket_directories (unsupported here). Its search-path
  query uses unnest(current_schemas(true)), falling back to current_schemas(true);
  both are unsupported. Completion queries pg_catalog
  namespaces, relations, types and functions. Those remain explicit unsupported
  errors, not fabricated catalog data. Full pgcli compatibility awaits item 35.

The explicit SET/SHOW/version/current_setting/current_schema/transaction-marker
allowlist is the [serve contract](../serve.md). It runs before nata-sql and does
not widen the SQL dialect. Harmless formatting/search-path/timezone settings are
stored no-ops; nonzero statement_timeout and semantic changes are refused rather
than falsely promised. Report ParameterStatus for application_name, client_encoding,
DateStyle, TimeZone and standard_conforming_strings. BEGIN/COMMIT/ROLLBACK are
compatibility no-ops with I/T/E ReadyForQuery markers, not transactional isolation.

**Diagnostics:** forward Nata's own syntax/bind/evaluation messages, retain the
position in the original query and put column suggestions in Hint. Substitute
parameter errors use generic text where a literal diagnostic could quote bound
data. Foreign IO/library strings stay generic; no Detail with possible cells.
Nata diagnostic families map to 42601 syntax, 42703 column, 42P01 relation/file,
42883 function, 42804 type/rule mismatch, 22012 divide-by-zero, 22003 overflow/range,
22P02 bad cast text, 0A000 unsupported, 57014 cancel, 08P01 protocol, 53300 session/
admission limit; residual conversion errors use 22000 and internal errors XX000.
The mapping is tested by diagnostic family; no change to CLI error classes.

**Admission amendment under ADR 0002:** replace step 2's one global slot with a
bounded semaphore, default slots = configured total CPU workers, configurable via
--max-queries (1..=threads). Each admission receives floor(threads/slots) workers
in the **existing** executor/scheduler; no new pool, runtime or spawn_blocking.
Thus concurrently admitted scan workers never exceed the total worker budget;
coordinators/planning/evaluation helpers remain bounded by admissions. Admission
wait: five seconds, 53300. A frame unwritable for two seconds cancels its query
and closes that socket, logging 57014; close avoids appending an error to a partial
frame. Cancel/disconnect closes the bounded bridge and joins workers before
release. Alternative: keep one slot or let stalled writers wait forever; the
stalled-reader control demonstrates starvation. A shared multi-query scheduler
would permit borrowing idle shares, but belongs to later scheduler work. The
fixed-share design may underuse cores for a lone query; step 5 must measure that.

**Measured, local x86-64 only:** recorded JDBC-like/psycopg-like sequences,
error recovery and mutants, all SQL corpus rows through extended mode versus CLI,
one nonreader plus eight completing clients, and actual driver outcomes/versions
are in [step 3 results](../../bench/results/20261107-pgwire-extended.md).
Default settings connect/read types but do **not** pass repeated preparation and
binary parameters; this is a documented compatibility limit, not a default-driver
certification. Confidence medium for the tested surface, reasoned for limits and
worker-share sizing. No ARM64, two-CPU performance, SF10 RSS closure, full-domain
date certification, sanitizer/leak certification or pgcli/ADBC claim. Step 4
owns expanded hostile-sequence fuzz, contention and lifecycle checks. Revisit
named/suspended support when item 35 requires it, or text client settings prove
insufficient; revisit fixed shares after step-5 measurements.


## Amendment, 2026-11-07 (step 4, local reliability and default clients)

This amendment supersedes step 3's unnamed/text-only limitation, not the dialect
or transaction contract. **Decision:** retain per-session named and unnamed
statements and portals. Duplicate named Parse is `42P05`; duplicate named Bind
portal is `42P03`; unnamed objects are replaced silently. Bind/Describe/Execute
unknown lookups use `26000` (statement) / `34000` (portal). Protocol Close is
idempotent for absent names, exactly as the
[PostgreSQL 16 Close handler](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/backend/tcop/postgres.c)
does; the unknown-name errors apply to lookup operations. Closing a statement
does not invalidate already bound portal copies. Statements survive Sync and
simple Query until Close, DEALLOCATE or session end. Portals are destroyed at
Sync outside BEGIN, or at COMMIT/ROLLBACK/session end. Simple Query removes the
unnamed objects. Add `DEALLOCATE [PREPARE] name` and `DEALLOCATE ALL`: psycopg's
normal rollback recovery clears its prepared cache using that SQL command.

**Bounds:** 256 statements (including unnamed); 4 MiB aggregate stored SQL plus
names. Budget exhaustion uses SQLSTATE **54000**, class 54 program limit exceeded.
At most 256 portals and a separate 16 MiB charge over portal names, copied SQL,
descriptions, and a conservative 4 MiB per portal for parameter ASTs. The byte
budget is stricter than the portal count (at most three live small portals).
The SQL/parameter/frame caps remain unchanged. Invalid named Parse retains its
old statement; invalid unnamed replacement removes its old statement. Closing,
replacement, Sync/transaction end and disconnect release storage. These default
budgets are **reasoned**, not measured optimal capacities.

**Default driver formats:** accept binary scalar parameters for bool, int2/int4/
int8, float4/float8, UTF8 text/varchar and date; reject malformed widths/values
with `22P03`. Support binary results for all slice-4 mapped OIDs, including exact
base-10000 numeric and date days relative to 2000. Evaluation still uses typed
literal ASTs; no SQL interpolation. Text remains the default result format and
supports exact numeric parameters; binary numeric input is explicitly `0A000`.
Float-special parameter literals remain outside the dialect. No extra SQL types
or transactional semantics are added. Alternative: require users to disable
preparation/binary transfer; rejected because ordinary default applications fail
at their preparation threshold. Alternative: implement every PostgreSQL binary
type; unnecessary for the bounded slice-4 type surface.

**Execute max-rows:** retain `0A000` for nonzero values on row-producing queries.
Rowless commands accept a limit: pgjdbc uses max-rows=1 for ROLLBACK, and those
commands cannot require suspension. A regression test verifies rollback recovery. A paused running query
would retain an admission, worker share and scan window between arbitrary client
fetches. Defining idle-portal expiration and fair admission for those resources
is beyond this step; collecting rows or silently ignoring max-rows is forbidden.
Thus pgjdbc fetchSize > 0 with autocommit off and SQL DECLARE/FETCH/server-side
cursors remain unsupported. Unlimited Execute on named portals works. This is a
recorded compatibility deviation, not a claim of general PostgreSQL cursors.
Revisit when a bounded parked-query policy is specified, or item 35 requires it.

**Evidence, measured local x86-64:**
[step 4](../../bench/results/20261107-pgwire-reliability.md) records default pgjdbc
42.7.7 / psycopg 3.3.8 repeats, binary/text type values and NULLs, errors/positions,
cancel and fetch refusal; seeded live-server hostile sequences, contention with
slow file opens/reads, real child SIGKILL, healthy sessions, exact gauges and
/proc fd/thread/RSS bounds; private-copy mutation checks and workspace gates.
The corpus is seeded empirical evidence, not exhaustive concurrency coverage.
**Confidence:** high for tested default-client values and named lifecycle;
medium for sampled resource/fuzz behavior. **Not covered:** ARM64, sanitizers,
full-domain type certification, arbitrary-width RSS bounds, R16 SF10 closure,
TLS/auth, suspended/SQL cursors, two-CPU throughput or item 35's full client matrix.
Step 5 must measure the streaming overhead, first-row latency and fixed worker
shares on both architectures; no performance claim follows from functional load.

**Upstream evidence and revisit-when:** the decoder panic was reported as
[pgwire issue 481](https://github.com/sunng87/pgwire/issues/481), reproduced by the
parent on 0.42.1 and master `12b7569` for short Parse/Bind/Describe/Execute/Close
bodies. Revisit reuse when that fix is **released**, then rerun all R1 checks;
a merged fix alone is insufficient. Production still has no pgwire dependency.

## Amendment, 2026-11-07 (step 5 and item 35 closeout)

Retain the step-4 protocol and fixed admission shares. The paired clean-VM
measurements and exact client matrix are in
[closeout](../../bench/results/20261107-pgwire-close.md), including every failed
target and the superseded first-run job defects. The 20% streaming budget is
unchanged: the implementation misses it by a large margin. Raw/psql similarity
on ARM64 isolates a substantial common server cost; per-row packet allocation,
bounded-channel transfer, async write and timeout setup are **reasoned** contributors,
not a profile proving their individual shares. Batch bounded wire output before
claiming the performance target; preserve cancellation and bounded backpressure.
Queue instrumentation reports time around semaphore acquisition and the fixed
configured worker share; it is not a CPU-utilization measurement.

Default ADBC PostgreSQL 1.13.0 queries pg_catalog.pg_attribute with ORDER BY during
connection setup and receives 0A000. No default ADBC operation reaches execution,
COPY or binary results after that failed bootstrap. Record those as blocked,
not as tested COPY failures. Adding a fake catalog or weakening query refusals
would widen this slice. Binary results already supported by step 4 do not make
ADBC's catalog/COPY path compatible. Revisit ADBC after the catalog/metadata slice;
then test COPY explicitly against its own decision.

Psql's SQL PREPARE is 0A000; it has no client API for wire Parse/Bind, so repeated
simple queries do not certify SQL preparation. pgjdbc/psycopg/pgcli default wire
preparation is measured separately. psql exposes display strings, whereas pgcli
intentionally installs a text loader for DATE. These are known client differences,
not a change to the R2 OID map. Boolean and float-special spellings differ from
CLI CSV; exact text mismatches stay visible alongside typed value comparisons.

**Confidence:** high for the tested values/errors and large streaming miss;
medium for sampled lifecycle/resource bounds and concurrency timings (twins and
min/max expose noise). **Not covered:** exceptional Date32 domains, arbitrary
cell-width memory bounds, full sanitizer certification (R15), or the original
SF10 SQL allocator gap (R16). R13's session-versus-signal audit is complete; keep
the standard CLI signal dependency. R15/R16 remain explicit closeout deviations
within the carried-in improvement budget; do not enlarge stack/scan allowances
to disguise a miss. TLS/auth, SQL/suspended cursors, nondefault transaction
semantics, catalog emulation and COPY remain out of scope.


## Amendment, 2026-11-07 (reopened step 6: bounded output and ADBC bootstrap)

Supersede the per-row bridge and the blanket bootstrap-catalog refusal above.
**Decision:** encode result DataRows on the executor coordinator, including any
requested scalar binary conversion, into at most **64 KiB** buffers. A batch
boundary flushes its pending buffer even below that size; no whole-result
collection. A wide row can span buffers without changing its framing. The
channel holds two buffers (**128 KiB queued bytes**); the producer and async
writer can each own one more (**256 KiB total bridge payload**), plus a 64 KiB
socket response buffer. Current-row encoding scratch and the executor's existing
scan window are separate from that bridge bound. Metadata/EXPLAIN packets over
64 KiB return 54000. Backpressure blocks the coordinator and holds its scan sink;
cancel/disconnect close the receiver and join the producer. Preserve the same
in-progress write/flush future during cancellation. The two-second stalled-send
deadline covers the complete buffer write **and flush**, including socket
readiness. Close on a partial buffer rather than append an error into a frame.

Accepted sockets set TCP_NODELAY. A bounded response writer coalesces small
frames; flush at ReadyForQuery, frontend Flush, before waiting for input, and
after each streamed buffer. In particular a client may await Parse/Describe
without Sync: never wait for another frontend message with its response pending.
Alternative: only enable NODELAY. It fixes the timer floor but retains per-row
channel/syscall cost. Alternative: collect all rows or use an unbounded channel.
Both violate the existing resource/backpressure contract.

**Session compatibility, not SQL dialect/catalog expansion:** recognize exactly
the two ADBC PostgreSQL 1.13.0 RebuildTypeResolver queries from
[type_resolver_init.cc](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/type_resolver_init.cc).
Token equality permits whitespace, unquoted identifier case and one terminal
semicolon; string literals, predicates, ordering and projection must match.
The pg_attribute projection (attrelid, attname, atttypid, ordered by attrelid,
attnum) is empty because no composite/table row types are exposed. The pg_type
projection is oid, typname, typreceive, typbasetype, typrelid, typarray, with ten
fixed rows: bool/int8/int2/int4/text/float4/float8/varchar/date/numeric and the
actual built-in receive-function names. Base/class/array OIDs are zero: no
domains, composite or array capabilities are advertised. Bootstrap OID values
use integer result columns; names use text. Other catalog SQL returns clear
0A000. There is no mutable or general-purpose pg_catalog implementation.

**COPY decision:** retain its explicit refusal. In the exact 1.13.0
[statement source](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/statement.cc),
ExecuteQuery defaults to UseCopy for PostgreSQL and ExecuteCopy wraps the query
in COPY (...) TO STDOUT (FORMAT binary). However its documented
`adbc.postgresql.use_copy=false` option selects PqResultArrayReader.
[result_reader.cc](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/result_reader.cc)
sets **ordinary binary Bind results**, not text; existing scalar binary support
makes that path work without COPY. Numeric is the driver's opaque Arrow
extension backed by exact text, rather than a native Arrow decimal. Adding COPY
requires its own streaming framing, trailer, cancellation and mutation coverage;
it is unnecessary for the bounded bootstrap fix. Default Arrow fetch remains a
measured compatibility miss (R18), while the documented option is tested.

Wire DECIMAL now retains a leading zero for fractional values even when precision
equals scale (0.500/-0.500), matching PostgreSQL; the CLI's DuckDB spelling remains
unchanged. Bool and float specials already use PostgreSQL spellings.

**Measured evidence, confidence and limits:**
[fix results](../../bench/results/20261107-pgwire-fixes.md) records syscall profiles,
paired socket controls, both CPU timings, every threshold verdict, driver probes
and mutations. High confidence in the measured syscall reduction and timer cause;
medium in finite lifecycle/resource certification. A fixed byte bound is a
reasoned invariant exercised with wide rows and a bound-removal mutant. Remaining
streaming, compatibility, Date32, R15/R16 and text-format limitations are recorded
there; no performance threshold or executor memory allowance changes.

**Final framing correction in the reopened step:** a byte buffer can end inside
a wide DataRow. Track the five-byte backend header and remaining body length
across completed writes, using constant memory. On cancel, finish the same
in-progress buffer future for up to one second; if the resulting stream still
ends inside a row, close instead of inserting ErrorResponse into that row.
Likewise close if a producer failure leaves a partial frame. Normal boundary
cancellation retains 57014. The wide-row cancellation test and its boundary-check
removal mutant exercise this distinction. Final clean-CPU performance profiles
include this cursor; the earlier syscall comparison predates it.


**Final measurement amendment:** use the compiled Rust count/discard client for
card raw/CLI (unchanged ≤1.20x), retaining Python separately. Same-VM distribution
PostgreSQL, loaded with the same million BIGINTs, supplies matched Rust/psql
reference arms only. Final ratios and native perf (x86)/Callgrind instruction
profile (ARM sampling unavailable) are in the fix record. Per-row allocations
and encoding remain measured costs; P14 stays open, no denominator redefinition.
Measured code-only tree `ad89453261a3b894039ed4280f7d95a56a100b99` and its blob
manifest match the final committed source. The strict old-writer NODELAY control
was repeated from full 4373a63 copies; earlier mixed-session control numbers are
retained but excluded. Final ARM64 catches all 51 mutants and passes the long
replay. Confidence is high for the demonstrated timer/transport causes and
measured misses, medium for the finite seeded resource/lifecycle evidence.

## Amendment, 2026-11-07 (reopened second fix: default COPY and float text)

Supersede the COPY refusal and finite-float spelling gap above. **Decision:**
accept only `COPY (<slice-4 SELECT>) TO STDOUT (FORMAT binary)`, optionally
`WITH` before the parenthesized options and a single-quoted `binary` value.
Unquoted keywords are case insensitive. The 1.13.0 driver sends `(FORMAT binary)`
through `PQexecParams`, as shown in
[ExecuteCopy](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/result_helper.cc).
No COPY FROM, files, PROGRAM, text/CSV, table shorthand or additional options:
return clear 0A000. The inner SELECT uses the existing planner/parameter ASTs;
no SQL grammar or type-map expansion. Errors retain positions in the full command.

Emit CopyOutResponse with overall/per-column binary format 1, then CopyData
containing PostgreSQL's binary signature, zero flags/extension length, tuples
with int16 column count and int32 lengths (-1 for NULL), and int16 -1 trailer.
The header accompanies the first tuple (or the trailer for an empty result), as
PostgreSQL does and ADBC's reader requires. End with backend CopyDone and
CommandComplete `COPY n`. Binary scalars reuse the existing encoders: signed
big-endian integers, IEEE floats, bool byte, UTF8 text, date days since 2000,
and numeric base-10000 digits with ndigits/weight/sign/dscale. Canonical numeric
omits leading/trailing zero groups and normalizes zero weight/sign, retaining
scale. TINYINT still maps to text. No binary numeric input is added.

COPY uses the **same** two-slot 64 KiB byte bridge, executor admission, blocking
producer, writer, two-second write/flush deadline, framing cursor and cancel/
disconnect/join path as DataRows. A tuple may span transport buffers, with one
CopyData frame per tuple; scratch for the current row remains separate. No
second export queue, whole-result collection or executor allowance increase.
Parse validates the inner query, Describe returns NoData, Bind formats do not
change COPY's own binary formats, Execute runs COPY to completion even with a
nonzero max-rows, and ReadyForQuery waits for Sync in extended mode. An error
still discards extended messages through Sync. Simple mode ends with ReadyForQuery.
This follows PostgreSQL's utility-command behavior, not suspended row portals.

**Float text:** only the wire boundary changes; CLI CSV stays DuckDB-oriented.
Use shortest round-trip digits, PG's open rounding endpoints and ties-to-even,
then positional notation for decimal exponents -4 through 14 (float8) or -4
through 5 (float4); otherwise signed exponent with at least two digits.
No trailing `.0`; preserve `-0`, `NaN`, `Infinity`, `-Infinity`. Rust's shortest
format is the starting precision, not sufficient by itself: PG disables Ryu's
`acceptBounds`, and exact endpoint/tie cases need an explicit adjustment.
SET/startup `extra_float_digits` accepts 1..3 and rejects values below 1 with
0A000 rather than silently ignoring older rounding semantics.

Alternatives: retain use_copy=false and the old formatter (leaves ordinary
ADBC and PG text compatibility broken); implement general COPY and legacy float
rounding (unneeded dialect/options/resource expansion). The narrow wire adapter
keeps the established type map and bounded execution model.

**Evidence and confidence:** [second fix record](../../bench/results/20261107-pgwire-fixes2.md)
contains byte-for-byte PostgreSQL COPY comparisons, seeded float4/float8 text
comparisons, default ADBC corpus checks, known-bad mutants and clean-CPU runs.
High confidence for exact outputs in that corpus; this is sampled float-domain
coverage, not exhaustive enumeration. Resource/lifecycle claims remain bounded
empirical evidence. No literal CLI text equality, ≤1.20x throughput, exceptional
Date32, R15/R16, Flight SQL, TLS/auth or SQL/suspended-cursor closure follows.
Revisit when another client's COPY spelling/options are required, or an oracle
finds a remaining formatter difference.

## Amendment, 2026-11-07 (exceptional Date32 output)

Supersede the four-digit AD-only wire output restriction. Read raw Date32 days,
range-check against PostgreSQL's finite Julian domain, and retain date OID 1082
for text, binary and COPY. Text uses PostgreSQL's BC suffix without parentheses
and extended AD years; binary uses checked days since 2000. Outside the finite
range return 22008, including raw days the CLI historically spells as infinity.
No mid-result OID substitution, implicit VARCHAR cast or chrono year truncation.
DATE input literals/casts/parameters retain their existing 0001–9999 restriction.

Alternatives: retain the four-digit restriction (unnecessary data loss); change
to text OID for a whole/partial result (breaks stable schema); use chrono for all
dates (does not cover PostgreSQL's upper range). Civil-day arithmetic covers the
actual protocol domain without adding a calendar dependency.

Measured: local PostgreSQL 17.11 exact text/DataRow/COPY oracle, native ADBC
Date32, pgjdbc LocalDate and psycopg explicit Python-domain errors, with known-bad
mutation controls, in [Debt A](../../bench/results/20261107-debt-a.md). Default
JDBC legacy Date/string presentation limitations match PostgreSQL and are
recorded, not claimed as exact full-domain calendar displays. High confidence
in the finite edge corpus and checked day arithmetic; this is not exhaustive
client/version certification. Revisit for DATE input-domain expansion or
explicit infinity metadata.
