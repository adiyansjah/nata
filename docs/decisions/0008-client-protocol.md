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
