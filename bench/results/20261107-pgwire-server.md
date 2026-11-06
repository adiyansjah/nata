# Slice 4b step 2: local SIMPLE-query server

Scope: LOCAL x86-64 only, no cloud, no push. Implements card 0003 step 2;
steps 3–6 and slice-wide certification remain pending. Rust 1.99.1; sanitizer
nightly-2026-08-21. Shared Intel Core i3-1220P development machine, filesystem cache warm, disk TMPDIR.
No latency, 20% streaming-overhead or 64-session throughput claim is made.

## Built

- `nata-pgwire`: unsafe forbidden, hand codec derived from the retained bake-off;
  no pgwire dependency. Decodes all card frontend message shapes, preserving
  extended fields for step 3. Checks fixed headers, minimum and maximum length,
  counts before collection allocation, exact consumption, terminators and UTF-8.
  Startup cap 10,000; regular cap 1,114,112 including length; SQL 1 MiB.
- `nata serve --listen 127.0.0.1:5432`: trust startup 3.0, SSL/GSS refusal, required
  ParameterStatus, random-secret BackendKeyData, SIMPLE Query, RowDescription,
  NULL-aware text DataRow, SELECT count, empty response, errors/positions, ready,
  Terminate, EXPLAIN and EXPLAIN ANALYZE. Loopback restriction with explicit unsafe
  bypass. Session cap defaults to 64, eight reserved startup/cancel slots, five-second
  startup deadline, immediate excess-socket close. See `docs/serve.md` and CLI help.
- One globally admitted query, at most the configured CPU workers plus a
  coordinator and the existing bounded planning/evaluation helpers. No per-session
  full CPU pools, unbounded spawn_blocking pool or second I/O runtime. Two tokio I/O
  workers; file/planning/decode/evaluation stay off them. This conservative admission
  is reasoned adequate for step 2, not a step-5 scalability result.
- A 32-row bounded bridge holds the executor sink/window when the socket stalls.
  Cancel and disconnect close/wake it, cancel the actual query ScanCancel, and join
  all query workers before ready/admission release. Registry token replacement and
  cancel lookup serialize under locks; idle/stale/wrong keys cannot poison the
  next query. Log lines contain session id, duration, row count and state, no SQL,
  cells or keys. Errors use generic messages rather than source-library text.

## Protocol and differential checks (measured)

`TMPDIR=$HOME/tmp-4b2 cargo test -p nata-pgwire -- --nocapture`:
three codec tests and seven protocol tests pass. Tests use a minimal raw client and
real Parquet execution. Recorded tags: startup `RSSSSSSKZ`, three-row SELECT
`TDDDCZ`, positioned SQL error `EZ`, empty `IZ`, zero-row SELECT `TCZ`.
The tests verify ten OIDs and fixed sizes/numeric typmod, NULL versus empty,
booleans, numeric extremes, decimal scale, signed zero, float specials, Unicode
positions across newlines, no cell in errors, EXPLAIN, cancel 57014 plus exact
message and reuse, wrong key, queued/concurrent session isolation, disconnect,
oversized header with NO body, truncated frame, startup cap/deadline, max sessions,
reserved startup/cancel capacity, unsafe-listen refusal and Terminate.
`nata/tests/serve_cli.rs` checks help and CLI validation.

A slow reader of the 1,048,576-row / 1 KiB-text fixture stopped production at
2,585 rows in two samples 200 ms apart (a live socket and 32-row bridge still
pending). The producer never collects the complete result. The unbounded-bridge
mutant fails this plateau check. This is a production-path backpressure check,
not the SF10 RSS formula or an arbitrary-width-cell bound.

A failing-first transport observation: dropping an in-progress `write_all` on
cancel left a partial DataRow, then ErrorResponse corrupted framing; the raw
client timed out reading a body. Keeping the SAME pinned write future for up to
one second after cancel fixes it. The bridge is closed first to release blocked
CPU workers; a permanently blocked writer is closed, not followed by another frame.

`tests/corpus.py --nata target/debug/nata --data <generated-dir>` passes all
**251** `check/queries/sql` files: **221** successful results equal `nata sql --csv`
(names, every row, NULLs, text, SELECT count; EXPLAIN text compared, ANALYZE timing
not compared); **30** failures have ErrorResponse + ready and no completion.
Fixtures: existing `gen-sql-check-data.sh`, plus `gen-tpch.sh 0.01`; every DuckDB
invocation has AI_AGENT unset. Wire bool/special-float spellings are normalized to
CLI spellings only; no value tolerance or sorting is used. Existing workspace
DuckDB differential suites also pass.

Optional psql integration is skipped with an explicit SKIP line when absent from
PATH. Locally it was also run with the already extracted **psql 17.11**, and
passes, printing -128 from the real server. Other clients and extended mode await
step 3/item 35; this is not the full compatibility matrix.

## Decoder corpus and mutation evidence (measured)

Seed **322661912839** (`0x4b20261107`), exact bake-off xorshift64 generator and
length/tag/payload schedule, reset for startup and regular corpora. Default test:
10,000 cases each, no panic. Also ran **1,000,000 each**, no panic (2.48 s on this
shared machine, informational). `.github/workflows/nightly.yml` runs that larger
size with its printed shared job seed. Exact caps and valid-message controls are
separate assertions; random rejection counts alone are not a correctness claim.

`TMPDIR=$HOME/tmp-4b2 CARGO_BUILD_JOBS=2 python3 crates/nata-pgwire/mutate.py
--work $HOME/tmp-4b2/mutants`: unmutated suites pass; final **11 CAUGHT, 0 SURVIVED,
0 INVALID**. Each mutation is tested in a private copy; real sources are hashed;
compilation/infrastructure failures and timeouts count INVALID, never CAUGHT.

| Mutant | Detector | Final result |
|---|---|---|
| Cursor take guard removed | malformed frames / seeded decoder corpus panic | CAUGHT |
| INTEGER sent with text OID | independent type map assertion | CAUGHT |
| NULL sent as empty text | independent raw row-length assertion | CAUGHT |
| lost CommandComplete (tag changed) | recorded response tags/count | CAUGHT |
| another session's SQL reused for rows | distinct simultaneous session values | CAUGHT |
| wrong cancel secret accepted | stalled query remains running after wrong request | CAUGHT |
| generic spike cancel message | exact 57014 message assertion | CAUGHT |
| bridge holds complete million-row result | producer plateau | CAUGHT |
| session cap ignored | second startup must return 53300 | CAUGHT |
| startup cap replaced with regular cap | exact startup length assertion | CAUGHT |
| error position counts bytes | Unicode character-offset assertion | CAUGHT |

The first wrong-key mutant **SURVIVED**: checking running immediately after the
cancel connection closed raced worker teardown. The strengthened check allows
teardown to occur before asserting that the legitimate blocked query is still
running. The mutant is then caught; the second session is also queued behind the
blocked query and must subsequently receive only its own rows. An early parallel
mutation build was interrupted to reduce swap pressure, and another exploratory
run's source-hash assertion detected our concurrent implementation edits. Neither
is counted as a successful mutation run. Final run uses stable sources.
Step 3 must add the extended Sync-does-not-clear-error-state mutant; it is NOT
implemented or counted here.

## R13: session cancel boundary (complete for this slice, measured/reasoned)

Server sessions use ScanCancel directly; signal-hook / SIGINT remain in
`nata/src/sql.rs` only. No dependency replacement was justified. Raw cancel,
wrong-key, idle-key, reuse, queued other-session and disconnect tests pass.
The combined cancel/disconnect/backpressure/isolation test also passes three
consecutive runs with four deterministic CPU burners; all burners are joined
afterwards. This is local contention evidence, not a 64-session load benchmark.
`python3 scripts/unsafe-inventory.py nata-pgwire`: 9,878 textual unsafe keywords
in 64/86 packages; `... nata`: 10,035 in 73/103; **zero workspace unsafe**. This
existing script strips strings/comments, includes inactive cfg/tests, does not
expand macros and is an upper bound, not a soundness audit. The normal graph adds
tokio 1.54.1, tokio-macros 2.7.2, mio 1.2.4 and socket2 0.6.5; getrandom 0.3.4 was
already locked. No pgwire/TLS/crypto or new signal hooks. cargo-deny passes.

## R15: parser stack diagnosis and bounded repair (local depth fix complete)

Before: real `nata-exec/tests/deep.rs`, nightly ASan with build-std, **exit 1**,
AddressSanitizer stack-overflow on the 16 MiB `nata-sql` parse thread. `addr2line`
symbolizes the repeating frames as sqlparser 0.63.0 `parse_prefix` (mod.rs:1736),
`parse_subexpr` (1418), `parse_expr` (1390),
`parse_expr_prefix_by_reserved_word` (1568). Recursive descent, particularly CASE,
exceeds the original uninstrumented budget; this is not the executor's 2 MiB
entry stack or the iterative 10,000-term handling.

Control: 32 MiB reservation passes **all four** deep tests, including 1,000 levels
of parentheses, unary, CASE, CAST and NOT, 10,000 terms, WHERE/projection/LIMIT,
EXPLAIN/ANALYZE and one-past-limit positioned errors. Final: **64 MiB**, all four
pass again, **exit 0**, no ASan finding. The chosen budget is 2x the measured
passing envelope for instrumentation/compiler variation, not a claimed precise
per-recursion-frame measurement. Reservation is virtual, not pre-touched RSS;
server admission bounds concurrent planning. No documented SQL limit is reduced.

Reproduce with stack constant 16 MiB for the failing control, 64 MiB for final:

```
mkdir -p "$HOME/tmp-4b2"
TMPDIR="$HOME/tmp-4b2" CARGO_BUILD_JOBS=2 RUSTFLAGS=-Zsanitizer=address \
  CARGO_TARGET_DIR="$HOME/tmp-4b2/asan" \
  cargo +nightly-2026-08-21 test -Zbuild-std --target x86_64-unknown-linux-gnu \
  -p nata-exec --test deep
```

The existing executor `deep-expressions-on-the-small-stack` mutant also runs:
unmutated executor suite PASS; mutant CAUGHT by a crashed deep-test process
(1 CAUGHT, 0 SURVIVED, 0 INVALID). Both older executor/Parquet private-workspace
harnesses now include nata-pgwire; the Parquet staged workspace resolves with
`cargo metadata --offline`. Full ASan workspace certification
(the earlier unrelated 1 ms timing assertion included), TSan rerun, ARM64 and
Miri R17 are not claimed by this bounded local depth repair.

## R16: bounded diagnosis, original miss remains OPEN

The original x86 SF10 miss is 114,964 KiB SQL versus 71,652 scan, despite identical
inflight counters. Original SF10 input is absent locally; no GCP run was made.
`tests/rss_probe.py --nata target/release/nata --work <disk-dir>` isolates the
same narrow BIGINT shape with 59,986,052 monotonic values, 122,880-row target row
groups (489 groups here versus 488 original), warm-cache pre-step-2 native release binary (same executor/output code),
8 workers/window 16, CSV to a null sink, default allocator and arena control
alternated. Every run returns exactly 59,986,052 rows. It is not a TPC-H replica.

| Allocator control | SQL peaks KiB (two runs) | Scan peaks KiB (two runs) |
|---|---|---|
| default | 56,512; 54,648 | 48,860; 49,560 |
| MALLOC_ARENA_MAX=1 | 29,900; 29,820 | 26,940; 27,896 |

Both paths account 15,817,312 peak inflight bytes and 1,050,112 largest-task bytes
in every run, unaffected by the arena control. **Measured:** allocator arena
retention contributes substantially to RSS that the live row-group accounting
does not see; part of this synthetic SQL/scan gap changes with the allocator.
**Source inspection:** bare-column projection clones Arrow references, but also
constructs an Evaluator/cache and an unused all-rows selection before detecting
that every output is a column. Parser/schema helpers and CLI SIGINT also add
thread/allocator arenas. **Not established:** how much each explains of the
original 43,312 KiB gap, or a fix for the original SF10 workload. No production
allocator setting or projection optimization is landed: this synthetic probe
already stays below the original formula and cannot certify restoring the original
miss on both CPUs. K=3 and the fixed 32 MiB allowance are unchanged. R16 stays open
for original-data allocator profiling and two-CPU validation; the existing
unbounded-window resource counterexample remains gating.

## Checks and remaining work

Before each logical commit: `cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `TMPDIR=$HOME/tmp-4b2 cargo test --workspace`,
`cargo deny check`, `scripts/check-leaks.sh --staged`; final tree leak scan too.
No lowered fuzz/test limits; TMPDIR is on disk. Full ordinary workspace checks
pass; specific additional checks above pass. No push/cloud spend.

Step 3: unnamed Parse/Bind/Describe/Execute/Sync/Close/Flush state machine;
text literal parameters with expanded-SQL cap; binary rejection; Describe metadata;
bounded suspended portals for nonzero Execute limits; discard-until-Sync recovery
and its mutant; driver SET/SHOW/version/current_setting bootstrap surface. Preserve
simple-query error recovery, admission, cancellation and privacy. Steps 4–6 still
own expanded contention/leak/fuzz certification, two-CPU performance/RSS/scalability,
and the full real-client job. Keep the excluded spike until permanent step-4
coverage replaces its remaining probes. R6b/R12/R17 and broader executor/parser
rewrites were not taken in. R15/R16 work stayed with stack budgeting and a small
allocator-control probe; no expansion into allocator/evaluator redesign.
