# Slice 4b step 3: local extended protocol and session compatibility

Local x86-64 only, 2026-11-07, same shared Intel Core i3-1220P machine as step 2.
Rust 1.99.1, disk TMPDIR `$HOME/tmp-4b3`. No GCP, push, new runtime/pool,
performance claim or full item-35 certification. ADR 0008's dated amendment is
the decision record; docs/serve.md lists the exact supported surface.

## Built

Unnamed Parse, Bind, Describe statement and portal, Execute all, Close, Flush,
Sync. Parse validates SQL on the bounded CPU admission; Describe emits correct
parameter/result metadata or NoData; Execute avoids duplicate RowDescription.
Errors in Parse/Bind/Execute discard subsequent messages until Sync; Sync returns
ready and permits reuse. Named objects and nonzero fetch limits return 0A000,
with client settings in the error/docs. No collected or indefinitely parked portal.

Parameters enter Nata's AST as typed literals, never SQL text substitution.
Bounds, explicit OIDs, NULL, unspecified-as-text, injection-shaped strings,
integer range/grammar and metadata stability are checked. Numeric parameters
whose scale/precision would change result metadata require explicit CAST or fail
before rows. Standalone SQL still rejects placeholders. The session layer owns
harmless driver SET/SHOW/SELECT-1/version/current_setting/current_schema and BEGIN/COMMIT/
ROLLBACK markers without widening nata-sql. Foreign errors stay generic;
Nata messages/positions and Hint suggestions are forwarded without cell values.

Admission now has multiple bounded slots and a fixed share of the existing CPU
worker budget per query. Two-second unwritable-frame timeout closes/cancels and
joins workers; five-second admission timeout reports 53300. See ADR 0008 for
alternatives, reasoned defaults, fixed-share utilization cost and remaining bounds.

## Measured tests and corpus

Permanent codec suite (3 tests) and protocol suite (14 tests) pass. New recorded
sequences include JDBC-like `Parse Bind Describe(portal) Execute Sync` →
`12TDCZ`; psycopg-like `Parse Describe(statement) Flush` → `1tT`, then Bind Execute
Sync → `2DCZ`; statement and portal NoData, Close `33Z`, errors at all three stages,
ignored following work, and successful reuse after Sync. Named/binary/fetch-limit
refusals are explicit. Binary non-UTF8 payloads receive 0A000, not a decoder panic.
All ten fixture types, NULLs/empty strings, wire metadata, values, injection-shaped
text, expanded-size cap, stable parameter types, LIMIT parameters, each SQLSTATE,
Nata message text, suggestions and no-cell errors are asserted.

One session stops reading the million-row / 1 KiB-text fixture. Eight concurrent
other sessions each complete a real Parquet query within **1.5 seconds** while it
is stalled; the nonreader releases its workers after the two-second send timeout
without external cancel/disconnect. The complete 14-test protocol suite also
passes **three consecutive runs under four deterministic CPU burners**, all
burners joined afterwards. This is a deadline assertion on a shared local
machine, not a throughput benchmark. The single-admission mutant restores step 2's
policy and must fail the same test. Existing cancel/wrong-key/disconnect/bridge
plateau/isolation and session/socket bounds remain tested.

`TMPDIR=$HOME/tmp-4b3 python3 crates/nata-pgwire/tests/corpus.py --nata
target/debug/nata --data $HOME/tmp-4b2/data`: **251 queries**, **221 successful
row sets**, **30 matching error outcomes**, PASS through Parse/Bind/Describe/Execute/
Sync versus `nata sql --csv`. Same pre-existing generated fixtures as step 2.
Every successful name, row, NULL and SELECT count matches; only boolean/float wire
spellings normalize, no sorting/tolerance. EXPLAIN text matches, ANALYZE timing is
not compared. Existing DuckDB differential tests run as part of the workspace
suite, with AI_AGENT unset for CLI DuckDB invocations. The new server adapter does
not alter the executor's arithmetic.

## Client probes

Reused already downloaded user-local tools, outside the repo: Temurin OpenJDK
17.0.21+9, pgjdbc 42.7.7, Python 3.12.16 / psycopg 3.3.8 with binary libpq 18.0.6 (180006), extracted
psql 17.11. No system installation needed. Harness sources are
`crates/nata-pgwire/tests/Clients3.java` and `clients.py`; arguments are port,
three-row protocol fixture and million-row stalled-reader fixture (created by the
protocol tests under TMPDIR). Java uses the pgjdbc jar on its classpath.

- Both drivers connect with normal protocol defaults (including SSL probe/refusal)
  and read ten fixture types with correct Java/Python values/types, including
  signed TINYINT as string, int widths, signed zero, NaN, exact decimal scale,
  date, empty text and NULLs.
- pgjdbc defaults: repeated no-parameter PreparedStatement succeeds four times,
  then returns 0A000 for named Parse on execution five. Typed integer parameters
  may use binary before the threshold and are explicitly refused. With
  `prepareThreshold=0&binaryTransfer=false`: seven parameterized executions,
  typed NULL, Boolean parameter, unknown-column 42703 at position 8, and cancel
  57014 PASS. The added Boolean probe failed first with 22P02: pgjdbc dumps TRUE
  in text mode. Case-insensitive Boolean binding fixes it, and the retained
  case-sensitive mutant fails both the raw parameter regression and client probe.
- psycopg defaults: repeated query succeeds through execution five, then named
  preparation on execution six gets 0A000; default integer `%s` parameter gets
  0A000 for binary. With `prepare_threshold=None` and `%t`, seven executions,
  NULL, 42703/position 8 and cancel 57014 PASS, using default autocommit=false
  (BEGIN/rollback compatibility markers exercised).
- psql 17.11 `-X -c` reads the fixture's three TINYINT rows including NULL: PASS.
  Full psql interactive, pgcli and ADBC compatibility stay with item 35's job.

These are explicit default-setting failures followed by passing configured text/
unnamed probes, not a claim that all drivers work unchanged by default.

## Mutation evidence and final checks

`TMPDIR=$HOME/tmp-4b3 CARGO_BUILD_JOBS=2 python3
crates/nata-pgwire/mutate.py --work $HOME/tmp-4b2/mutants`: unmutated codec and
protocol suites PASS; **29 CAUGHT, 0 SURVIVED, 0 INVALID** in each initial run
and repeat. Adding the failing-first pgjdbc Boolean case brings the final suite
to **30 CAUGHT, 0 SURVIVED, 0 INVALID** in the final complete run (exit 0).
INVALID never counts as CAUGHT. Private workspaces preserve a hash of all real
Rust sources.

| Mutant group | Detectors | Count / final result |
|---|---|---|
| Retained step-2 codec/type/NULL/completion/session/cancel/backpressure/cap/position mutants | Existing codec and real-server tests | 11 CAUGHT |
| Sync keeps error state; ignore error state removed | Error at Parse/Bind/Execute, ignored work, post-Sync successful reuse | 2 CAUGHT |
| Lost BindComplete, CloseComplete, ParameterDescription; portal described as statement | Recorded response tags and independent parameter OIDs | 4 CAUGHT |
| Nonzero Execute ignored; binary formats accepted | Refusal sequences and no unexpected rows | 2 CAUGHT |
| Parameter text lost, expansion cap removed, bound result type changes | Injection-shaped value equality, size error, numeric CAST/type stability | 3 CAUGHT |
| Case-sensitive Boolean text | pgjdbc TRUE probe and raw Boolean parameter equality | 1 CAUGHT |
| Session ParameterStatus lost; transaction status always idle | SET status assertions and I/T/E markers | 2 CAUGHT |
| Generic SQL message, lost Hint, unknown-column wrong code | Exact Nata text, suggestion, SQLSTATE table | 3 CAUGHT |
| Single admission; stalled send never expires | Eight clients finish within 1.5s, nonreader releases workers without cancel | 2 CAUGHT |

Checks: cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings;
TMPDIR=$HOME/tmp-4b3 cargo test --workspace; cargo deny check (only pre-existing
unmatched license-allowance warnings); scripts/check-leaks.sh --staged. Final
Rust sources receive a fresh passing workspace run after the compatibility adjustment.
AI_AGENT is unset (also explicitly removed for the final workspace command). Both commits run all required checks; the second
includes a fresh workspace run after the Boolean compatibility fix.

## Remaining step 4 work and deviations

Named statements and suspended portals intentionally refused (card permits the
explicit decision); exact text-only driver settings are required. Parameter OID
zero resolves to text rather than general PostgreSQL type inference. Numeric
result parameters need explicit CAST to fix precision/scale; SMALLINT/FLOAT
parameters use the existing dialect's INTEGER/DOUBLE CASTs. Session commands are
one statement per message; nonzero statement_timeout, startup options, transaction
options and catalogs are explicit refusals. These are documented limits.

Step 4: expand stateful hostile extended-sequence fuzz, cancellation/disconnect
races at every phase, lifecycle/handle/leak counters, delayed bridges and CPU
contention, malformed and valid binary/text counts, clean VM and sanitizer runs,
and remaining permanent checks needed to delete the excluded bake-off spike.
Step 5: both-CPU streaming/scalability/fixed-share utilization benchmarks and
original-data R16 allocator/RSS profiling. R15 full sanitizer certification and
R16 SF10 RSS repair remain open as recorded in step 2; this step changes neither
SQL depth limits nor the memory formula. Step 6/item 35: full client matrix,
including pgcli catalogs/ADBC gaps and exceptional Date32 domains. No GCP work
was done, and no push is authorized in this step.
