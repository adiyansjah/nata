# Hash aggregate skeleton — steps B, C, C2 and D, 2026-11-07



## Step D — serve, clients, fuzz, resources and restart cleanup (2026-11-07)

**PASS locally:** aggregates through `nata serve`; all **50 default-client
cells**; exact SF1 HAVING multisets at 1/8 threads in memory and at 64 MiB;
20,000-query SQL-fuzzer smoke (zero unexplained differences); independent CLI
and concurrent-server RSS probes; real SIGKILL/restart cleanup; **five focused
mutants CAUGHT, zero SURVIVED, zero INVALID**; required workspace/differential
checks. **Known MISS retained:** default D=4B=256 MiB cannot complete the SF1
HAVING query at B=64 MiB. Successful small-budget controls use explicit D=1 GiB.
No resource target, card or accepted ADR is edited or frozen here. Both-CPU
heavy/deep campaigns and all frozen-target timing verdicts remain for the parent.

### Serve and admission

The server now uses one node memory pool N, with each admitted query capped at
floor(N / max-queries), including EXPLAIN ANALYZE and binary COPY. The query's
admission permit lasts through task/I/O teardown. Disk caps also divide half
the available volume among admissions; explicit per-query D above that share
is rejected. Startup diagnostics state N, B, D and the memory source. Unknown
physical/cgroup limits require an explicit cap, rather than a guessed default.
The CLI keeps a single-query cap; `serve --aggregate-memory-bytes` sets **N**.

Native protocol checks cover BIGINT SUM/count OID 20, numeric OID 1700 with
DECIMAL(38,s)/exact AVG typmods, text and binary values, repeated named
preparation (including parameters in arguments and HAVING), binary COPY,
empty/global output, overflow **22003**, and positioned unsupported constructs
**0A000**. A real two-query spill test observes two nonempty private spill
owners concurrently under N=64 MiB / B=32 MiB, cancels both and requires
released grants/files and healthy session reuse; completed grouped queries
also agree. A lost execution cancel token is caught by this check. Aggregate
output separately exercises bounded backpressure, cancel and disconnect;
an injected nine-byte spill-write failure has no success trailer.

The seeded aggregate server contention/delay/kill smoke passes **28 scenarios**
with two busy contention threads and **1,801 healthy queries**; baseline/final
FDs are 16/8 (eight healthy socket ends closed), threads 5/5, RSS 26,120/27,720
KiB. This is finite native evidence, not a model of the new continuation adapter.
[Record](raw/20261107-hash-aggregate-step-d/aggregate-reliability.txt).

### Default client matrix

SF0.01 lineitem, plus a four-row integer/BIGINT/decimal/NULL fixture; each cell
compares values as a multiset. Its BIGINT SUM is 18,446,744,073,709,551,613,
exceeding BIGINT, and must retain the exact numeric result in every client. Q1's three DuckDB DOUBLE AVG columns alone use
an absolute 1e-8 tolerance; tiny AVG results use independent exact rational
answers. Other numeric values compare exactly. JDBC calls `getObject` and
asserts Long/BigDecimal before CSV transport; psycopg checks native int/Decimal
and OIDs, including empty output. PGCLI returns int/Decimal; ADBC 1.13's opaque
numeric extension reads as exact strings, with binary COPY enabled by default.
Psql's native interface is text. CSV transport types in the raw JDBC rows are
not a claim that JDBC returns String for numeric columns.

| Query | Rows | psql | pgjdbc | psycopg 3 | pgcli | ADBC PostgreSQL |
|---|---:|---|---|---|---|---|
| Full Q1 body | 4 | PASS | PASS | PASS | PASS | PASS + R22 |
| High-cardinality HAVING | 12,758 | PASS | PASS | PASS | PASS | PASS + R22 |
| Global aggregate | 1 | PASS | PASS | PASS | PASS | PASS |
| Empty grouped input | 0 | PASS | PASS | PASS | PASS | PASS |
| NULL keys | 3 | PASS | PASS | PASS | PASS | PASS |

Every cell passes both ordinary execution and **64 MiB forced spill**. JDBC
and psycopg repeat preparation ten times with default driver settings; no
prepareThreshold, binaryTransfer, use_copy or fetch-size option is changed.
The original ADBC outcome is retained: terminal whitespace after a semicolon
fails for Q1/HAVING under R22, then `rstrip()` normalization passes. This known
driver-wrapper spelling deviation is not reclassified as an unnormalized pass.
Versions and all 50 outcomes: [matrix JSON](raw/20261107-hash-aggregate-step-d/client-matrix.json).
Local versions include pgjdbc 42.7.7 / OpenJDK 21.0.13, psycopg 3.3.8 and ADBC
1.13.0; the exact Python package inventory is in that record. The heavy setup
pins JDBC and ADBC to those versions and records the others on each CPU.

### SQL fuzzer

The subset alternates unchanged scalar seed batches and new aggregate batches:
column/expression GROUP BY, boolean HAVING, COUNT/SUM/AVG/MIN/MAX, global and
empty input, NULLs and all supported key/extreme families. Grouped results
compare as multisets, preserving multiplicity; unordered LIMIT subsets are not
generated. New narrowly typed SUM metadata classifiers have positive/negative
tests; the CSV adapter itself cannot certify wire metadata. Direct-column AVG
uses an independent exact sum/count and half-away-from-zero Decimal oracle,
with guards against unsupported shrink rewrites. Decimal JSON digits and
DuckDB's HUGEINT-as-string transport are normalized without a float conversion.

**Seed 20261107: 20,000 queries (10,000 aggregate), 49,316 oracle rows,
zero unexplained differences**, 1,725 oracle errors and 270 optimizer-dependent
DuckDB answers. Seven inherited scalar CASE precision constructs are explicit
coverage skips; **no aggregate coverage skips**. Five differences classify by
message and seven after shrinking, all existing documented scalar differences.
The exact AVG oracle checks 2,953 query shapes in both optimizer modes.
[Smoke](raw/20261107-hash-aggregate-step-d/fuzz-smoke.txt).
An additional **400 queries**, seed 22014202119, at 16 MiB forced spill have
zero differences/skips. [Forced control](raw/20261107-hash-aggregate-step-d/fuzz-forced.txt).
The planted-fault control is clean and **9/9 faults are caught**, including
COUNT ignoring validity, AVG dividing by all rows and empty SUM returning zero.
The older string fault needs 3,238 queries and therefore survives a preliminary
2,000-query trial; the 8,000-query proof catches it.
[Proof](raw/20261107-hash-aggregate-step-d/fuzz-prove.txt).
Python tests: 26 pass, one optional longer proof test skipped (27 total); that proof was
run separately as above. [Tests](raw/20261107-hash-aggregate-step-d/python-tests.txt).

### Independent RSS and proposed defaults

Local x86-64 Core i3-1220P KVM guest, eight visible logical CPUs, 16,382,784 KiB
physical memory. Fresh release processes, warm data, stdout discarded for CLI;
OS `wait4` peak RSS, independent of grants. All **65 aggregate gate queries**
(including checked expected errors), plus SF1 HAVING, run at default/64/256 MiB
and requested 1/8 threads. The table reports maximum RSS over the whole set;
in every cell the maximum is SF1 HAVING. One valid final sample per cell is
indicative evidence, not a timing comparison or a noise/scaling claim.

CLI no-data baselines (1 / 8 requested threads): **13.03 MiB** / **12.92 MiB**. Rule checked:
baseline + B + 32 MiB. All **264 explicit-budget observations PASS**; default
controls are far below the shared default-pool getter's 7,999.4 MiB budget.
All successful nonempty SF1 small-budget runs actually spill and retain exact
results. Registered reservations stay within B.

| Requested threads | B | Peak RSS MiB | SF1 peak reservation MiB | SF1 written / read bytes | Depth |
|---:|---|---:|---:|---|---:|
| 1 | default (7,999.4 MiB) | 587.32 | 1478.47 | 0 / 0 | 0 |
| 1 | 64 MiB | 40.24 | 64.00 | 603,884,584 / 692,151,432 | 1 |
| 1 | 256 MiB | 129.22 | 256.00 | 250,657,912 / 250,657,912 | 0 |
| 8 | default (7,999.4 MiB) | 683.56 | 2515.85 | 0 / 0 | 0 |
| 8 | 64 MiB | 48.61 | 64.00 | 960,196,632 / 1,219,526,664 | 2 |
| 8 | 256 MiB | 74.74 | 256.00 | 666,587,736 / 723,016,432 | 1 |

The server probes run **two simultaneous SF1 HAVING queries**, drain real text
DataRows without retaining results, and require 1,273,748 rows from each and
zero remaining spill files. N is the sum of the admitted caps; the tested
process allowance is its no-data baseline + N + 32 MiB.

| Per-query B MiB | Node N MiB | Baseline MiB | Peak RSS MiB | Allowance MiB | Verdict |
|---:|---:|---:|---:|---:|---|
| 3,999.7 | 7,999.4 | 7.00 | 1,153.29 | 8,038.40 | PASS |
| 64.0 | 128.0 | 6.93 | 54.83 | 166.93 | PASS |
| 256.0 | 512.0 | 6.97 | 187.46 | 550.97 | PASS |

All three server controls observed actual nonempty spill files, including the
default 3,999.7 MiB per-query share; the CLI default controls did not spill.
That observation is retained without a performance diagnosis. Native admission
checks independently inspect per-query peak/held/cancelled reservation snapshots.
[CLI 1](raw/20261107-hash-aggregate-step-d/rss-1.json),
[CLI 8](raw/20261107-hash-aggregate-step-d/rss-8.json),
[server](raw/20261107-hash-aggregate-step-d/server-rss.json).

Stack candidates are sampled every 20 ms from main/guarded no-huge-page native
stack mappings: maximum sampled resident totals **92 / 396 KiB** at
requested 1/8 threads; the server has at most 18 sampled threads. These samples
can miss short-lived stacks and are not an exhaustive stack proof. Code bounds
planner/coordinator stacks at 64 MiB virtual each and bounds worker counts by
admission; all stack RSS remains INCLUDED in the process gate. An early
adjacency-only selector counted allocator arenas and was corrected; those
stack numbers are excluded. Outcome validation also caught a CLI invocation
bug for comment-prefixed SQL; `--command=...` fixes it. Earlier full-suite RSS
runs with rejected SQL are excluded, and all final outcomes/grants are checked.

**Proposals for the parent, not frozen defaults:** retain baseline + B + **32
MiB** allocator allowance (server: sum of admitted B), subject to the three
repeats on both clean CPU types. Retain owner decision 4: N=half the smaller
physical/cgroup limit, B=floor(N/admissions). Here N=8,387,985,408 bytes; CLI B=N,
server Q=2 B=4,193,992,704 bytes. With Q=8 the rule would give 1,048,498,176 bytes
per query; eight-way admission RSS is not measured here.

The current D=min(4B, volume share) still **MISSES** SF1 at 64 MiB / 8 threads:
quota error, exit 1, cleanup complete. Explicit **D=1 GiB** succeeds at 64 and
256 MiB, including the concurrent server probes. Propose a small-budget floor:
**D=min(max(4B, 1 GiB), half available volume divided by admissions)**. This is
1 GiB at B=64/256 MiB and retains 4B at larger caps when space permits. It needs
parent acceptance/an ADR 0009 amendment and both-CPU confirmation; the running
default is unchanged. [Current default miss](raw/20261107-hash-aggregate-step-d/default-disk.txt).
Spill volume/read amplification and hundreds of thousands of file opens remain
visible in the counters above; no effort to reduce them was made in Step D.

### Restart cleanup and mutants

`nata cleanup-spill <trusted-spill-directory>` is the documented restart
command. Ownership uses a held file lock, not a PID-existence guess; creation
and cleanup serialize against the base directory. Live owners are skipped,
unknown files/links are skipped, and spill contents are never read as data.
The owner lock consumes one of the 32 spill descriptors (at most 31 data handles).
A real child at FD limit four also forces ownership-marker creation to fail
after mkdir and requires the private directory and grant to disappear. Its fifth mutant is caught. The final matrix and all RSS probes above were
rerun after this fix; a separate 8-thread / 64 MiB SF1 probe also passes at
**46.49 MiB**, below B + 32 MiB even without adding a baseline.
[Post-fix probe](raw/20261107-hash-aggregate-step-d/post-owner-fix-rss.json).
Native tests SIGKILL a child with actual IPC bytes, skip its live registry,
remove it after death, and check idempotence/unknown files/symlink refusal.
The real-server test SIGKILLs during observed spill, restarts, executes a fresh
zero-count query, then removes **16 abandoned files / one directory** with the
command. [Restart record](raw/20261107-hash-aggregate-step-d/restart.json).

| Focused mutant | Verdict | Failing check |
|---|---|---|
| aggregate-numeric-wrong-oid | CAUGHT | Aggregate metadata/binary/default scalar/COPY checks |
| aggregate-cancel-token-lost | CAUGHT | Two observed spilling queries fail the wire-read deadline |
| aggregate-admission-pool-multiplied | CAUGHT | Two-query budget snapshot invariant |
| restart-cleanup-skipped | CAUGHT | SIGKILL/restart cleanup test |
| owner-initialization-cleanup-skipped | CAUGHT | Real EMFILE child leaves no directory |

Passing private-copy controls precede the mutants; compile/edit failures and
whole-suite timeouts are INVALID. The executor runner now also refuses unknown
non-test failures as catches, with outcome-classifier tests.
[Wire results](raw/20261107-hash-aggregate-step-d/mut-wire.txt),
[executor result](raw/20261107-hash-aggregate-step-d/mut-exec.txt),
[failing tests](raw/20261107-hash-aggregate-step-d/mutant-failures.txt).
Full package sweeps are estimated to exceed the 15-minute local time box and
are prepared for the parent.
Every executor/wire edit anchor was checked to apply exactly once; that static
check is not a substitute for a sweep. No crate, lockfile change or own unsafe
block is added; C2's two statvfs unsafe blocks remain the only own-code ones.

### Checks and exact heavy commands

Required pre-commit commands pass sequentially with disk TMPDIR and two test
threads: fmt; workspace/all-target clippy with warnings denied; workspace tests;
cargo-deny; scan **5/5**, SQL **252/252**, differences **54/54**, aggregate
**65/65**, forced aggregate **65/65**; staged leak scan. The job-chain stub suite
also passes. The SQL-fuzzer smoke and first SF1 multiset controls used runtime `8775bcb`.
The final matrix and RSS probes also include the subsequent owner-lock
initialization error cleanup; aggregate arithmetic and spill data paths are
unchanged. Binary hashes are in [runtime identity](raw/20261107-hash-aggregate-step-d/runtime.txt). No GCP, push or merge was performed.

The complete, versioned, directly copyable **[heavy.steps content](../aggregate-step-d.steps)**
contains both bound-3 memory/spill models, two 100,000-query seeds (second forced
spill), the 30,000-case-per-shape spill and inherited Parquet campaigns, one
million wire decoder cases per corpus, 4,000 live scenarios / 20,000 wire cases,
full executor/SQL/wire/memory/
kernel mutant sweeps, native aggregate contention, restart, client setup/matrix,
and three-repeat CLI/concurrent-server RSS probes. Models cover the real memory
service and its spill interactions; they do not instrument the native disk
continuation adapter. The corruption campaign is the inherited physical IPC
boundary campaign, not an exhaustive arbitrary typed-state proof.

```sh
cp bench/aggregate-step-d.steps heavy.steps
# Parent ships this exact file separately to an ARM64 n4a and an x86-64 c3d
# heavy job, sizes the VM for --jobs 4, and supplies cloud approval/budget.
```

Local reproduction uses `TMPDIR=$HOME/tmp-skelD`, a release build, DATA containing
TPC-H SF0.01 plus `scripts/gen-sql-check-data.sh` fixtures, and DATA_SF1 with the
standard SF1 fixture. Each command is a separate sequential run:

```sh
env -u AI_AGENT python3 scripts/check-aggregate-clients.py --candidate target/release/nata --data "$DATA" --out "$TMPDIR/clients"
env -u AI_AGENT python3 scripts/check-aggregate-spill.py --candidate target/release/nata --data "$DATA_SF1" --rounds 0 --out "$TMPDIR/sf1"
env -u AI_AGENT python3 scripts/check-aggregate-rss.py --candidate target/release/nata --data "$DATA" --sf1 "$DATA_SF1" --threads 1 --rounds 1 --out "$TMPDIR/rss-1"
env -u AI_AGENT python3 scripts/check-aggregate-rss.py --candidate target/release/nata --data "$DATA" --sf1 "$DATA_SF1" --threads 8 --rounds 1 --out "$TMPDIR/rss-8"
env -u AI_AGENT python3 scripts/check-aggregate-rss.py --server --candidate target/release/nata --data "$DATA" --sf1 "$DATA_SF1" --threads 8 --rounds 1 --out "$TMPDIR/server-rss"
env -u AI_AGENT python3 scripts/check-aggregate-restart.py --candidate target/release/nata --data "$DATA" --out "$TMPDIR/restart"
```

Clients need the item35 Python dependencies, psql/Java on PATH and PGJDBC_JAR;
`aggregate-client-setup.sh` prepares those in a parent heavy job. Fuzzer smoke
and replay commands are in `spikes/sqlfuzz/README.md`; all large commands are in
the single heavy file above. Scratch is removed at handback; raw evidence is
retained here.

**SKELETON COMPLETE — check paths, not card closeout.** Closeout still runs the
prepared heavy/deep checks on both CPUs, freezes RSS/default proposals with the
parent, measures timing verdicts against the frozen targets on clean VMs, and
delivers the performance work of **R12/P11/P4/P5 and spill volume** (plus actual
pool affinity evidence). No clean-VM performance or full native-adapter formal
coverage is claimed by this step.

## Step C2 — spill integration (2026-11-07)

**C2 spill gates PASS; slice/card closeout remains open.** The implementation adds
resumable pressure-triggered partial spill and independent partition merge,
typed key/state replay, seeded recursive repartition with an eight-level bound,
query-local disk/descriptor admission, and spill diagnostics. Native checks are
local x86-64 only. Earlier sections remain historical checkpoint records.
No GCP, push, merge-to-main or frozen-target timing verdict is part of C2.

The native spill SQL tests cover all supported scalar key/extreme types,
NULLs and dictionary changes, SUM/AVG counts and wide cancellation, pressure and
recursive readback, ENOSPC, quota, cancellation, hot-string coalescing, source
error precedence, and forced depth under a 15-second child-process deadline.
The aggregate differential mode uses 16 MiB and flushes every nonempty partial;
CI's gating differential job runs it on both configured architectures.

Preallocation allowances cover runtime/metadata/output, footer parsing and
retained metadata, selected-column decode overlap, evaluator columns and
possible dictionary-text expansion, table/rehash/key/state capacities, partial
ownership transfers, merge state, and the IPC service/workspaces. Capacity
credit stays charged until its allocation owner is freed. Final extrema move
without copying. Native gauges assert held/live grants return to zero and peak
reservations never exceed B. These conservative allowances are not an allocator
quota or independent whole-process RSS certification. The inherited Parquet
trust-boundary limitations remain; C2 does not certify that parser fixed.

One I/O worker owns synchronous syscalls. CPU continuations register sticky
completion callbacks and return their CPU slots while blocked; grant pressure
can reclaim finished as well as active local partials. Scan task concurrency
shrinks to fit the query-wide decode allowance. Recursive children become
visible only after all replacement writes succeed; parent and child live bytes
both count against D. The coordinator publishes bounded output, never an
all-groups final table. Native synchronization uses the scheduler facade; this
new continuation adapter is outside loom instrumentation, so the inherited 16
memory models do not certify it.

C2 adds **two own-code unsafe blocks**, confined to `spill/admission.rs`, using
already-locked libc: statvfs with a checked CString/aligned output pointer, and
success-only initialization of the output. The crate lint changes from forbid
to deny with this single named function exception; operator/codec code remains
safe. No crate or version is added. Disk defaults implement min(4B, half the
volume's available bytes); descriptor admission subtracts existing handles,
bounded scan workers and headroom from the process soft limit before the query
cap of 32. Explicit disk overrides remain bounded by their supplied D.

### Gate status and native failures

(a) **PASS:** the card's SF1 HAVING query returns DuckDB's exact numeric row
multiset, **1,273,748 rows**, at requested **1 and 8 threads**, both in memory
and at **64 MiB**. Multiplicities are preserved; no numeric tolerance is used
for this query. The successful small-memory cells use **explicit D=1 GiB**.
The default **D=4B=256 MiB fails** cleanly during recursive overlap; the final
release default-quota probe confirms the quota error and no files remaining.
This resource miss stays visible; neither B nor the default D was enlarged.

(b) **PASS:** release `nata-check`: scan **5/5**, SQL **252/252**, SQL differences
**54/54**, aggregate **65/65**, forced-spill aggregate **65/65**, all with zero
mismatches, unsupported queries, unexpected errors and reference errors.
The forced mode adds `--report`, requires a nonzero write counter for nonempty
successful results, and uses 16 MiB; truly empty grouped input has no state to
write. As in step B, declared AVG tolerances against DuckDB's DOUBLE are not an
exact AVG oracle; inherited independent rational vectors, exact SQL metadata
and native wide-state replay checks supply that separate evidence.
(c) **PASS:** `cargo test -p nata-exec --test aggregate_spill injected_write_failure_returns_error_and_removes_every_file`.
A query-local ENOSPC after nine bytes returns a clean named error; its private
files and directory disappear and held memory returns to zero. A separate
nine-byte disk-quota test requires the explicit quota-error text and cleanup.
(d) **PASS:** `cargo test -p nata-exec --test aggregate_spill cancel_during_spill_joins_io_and_removes_every_file`.
A watcher cancels only after observing a nonempty spill file, with a 15-second
watcher deadline. Execution joins I/O; directory and grant gauges return to zero.

### Spill counters and indicative timings

Counters below are from the correctness runs, not timing medians. File opens
are cumulative successful descriptor admissions, including the 16 up-front
progress files even for an in-memory query. Nonempty spill files, including
recursive children, count as partitions. Read bytes exceed write bytes because
an oversized partition is partly read for merge and then reread to repartition.

| Requested threads | Memory | Written bytes | Read bytes | Partitions | Depth | Peak reservation bytes | File opens |
|---:|---|---:|---:|---:|---:|---:|---:|
| 1 | default, in memory | 0 | 0 | 0 | 0 | 1,550,285,901 | 16 |
| 8 | default, in memory | 0 | 0 | 0 | 0 | 2,654,986,484 | 16 |
| 1 | 64 MiB | 603,884,584 | 692,151,432 | 272 | 1 | 67,108,864 | 143,642 |
| 8 | 64 MiB | 960,182,104 | 1,219,485,376 | 4,368 | 2 | 67,108,864 | 300,617 |

Release, warm SF1, shared local KVM guest: x86-64 Core i3-1220P, eight visible
logical CPUs (one visible thread per core), 16 GB RAM, no affinity. Three
alternating in-memory/spill rounds per requested thread count, full CLI CSV to
`/dev/null`, including startup/planning/decoding/output. Results were checked
before timing. All four correctness cells and all twelve timed observations
are in [the raw observations](raw/20261107-hash-aggregate-step-c2/observations.json).
These are **indicative Nata controls**, not a paired DuckDB timing comparison,
not a cross-session before/after claim, and not a frozen-target verdict.

| Requested threads | Memory | Seconds min / median / max | Range / median |
|---:|---|---|---:|
| 1 | in memory | 5.059 / 5.145 / 5.177 | 2.3% |
| 1 | 64 MiB | 14.454 / 14.729 / 15.091 | 4.3% |
| 8 | in memory | 4.464 / 4.837 / 5.066 | 12.5% |
| 8 | 64 MiB | 17.280 / 17.986 / 18.184 | 5.0% |

The eight-thread spill cell does more recursive work: depth two versus one,
more bytes and roughly twice the file opens. A single bounded I/O worker also
executes the IPC codec and streaming repartition routing. These measured
counters and that code path explain additional work; they are not a CPU
self-time profile. `partial_ns`/`merge_ns` sum CPU-continuation wall time;
parked waits and I/O-worker time are excluded. Admission may reduce scan
concurrency to fit B; the table labels requested threads. The merge was not
redesigned for performance here, and these observations stay for that later
performance step.

### Mutants

**Five CAUGHT, zero SURVIVED, zero INVALID**, after a passing private-copy
executor control. Control 20 seconds; mutants 16/28/20/23/25 seconds, about
132 seconds total, below the brief's approximate 15-minute local limit.
[Results](raw/20261107-hash-aggregate-step-c2/mutants.txt).

| Mutant | Result | Failing check |
|---|---|---|
| spill-partition-not-read-back | CAUGHT | SQL typed/hot-string/readback and physical IPC checks |
| spill-reservation-not-released | CAUGHT | Real table grants retained after spill; cancel/pressure cleanup gauges |
| spill-cleanup-skipped-on-error | CAUGHT | Injected query ENOSPC and primitive cleanup |
| spill-recursion-bound-removed | CAUGHT | Forced collisions plus a one-byte partition threshold, with a 15-second child-process deadline; requires the explicit depth-eight error |
| spill-descriptor-cap-removed | CAUGHT | Real descriptor cap and released-slot test |

The recursion trial reaches the bound without relying on a random skew. A hot
single string group is tested separately and must coalesce at depth zero.
Removed recursion checking can also reach the physical IPC depth rejection;
that does not satisfy the test's required bounded-recursion error. Build/edit
failures would be INVALID and would fail the sweep.

### Checks, inventory and remaining work

**PASS:** final `cargo fmt --check`, workspace/all-target clippy with `-D
warnings`, `cargo test --workspace` (disk TMPDIR, two test threads), cargo-deny,
all five differential runs, and staged leak scan. The native ten-test SQL spill
suite and two typed-record tests pass. No C2 build dependency or lockfile edit.
An initial workspace run caught a direct-channel facade bypass; it was fixed.
An early deny invocation overlapped a test run; all final pre-commit checks
were rerun sequentially. CI itself and ARM64 execution remain unrun here. The current textual unsafe inventory
has **83 packages / 8,836 keywords in 64 packages**, including **two workspace
keywords**. It includes inactive/test dependency code and is not an audit or a
macro-expanded count. [Inventory](raw/20261107-hash-aggregate-step-c2/unsafe-exec.txt).

Step D must verify serve/admission and the five default clients, preparation
and binary COPY, cancellation/disconnect/slow consumers; extend the SQL fuzzer
grammar and documented difference classes before its aggregate campaigns;
run the full package mutant sweeps and inherited damaged-spill campaign;
exercise the new native adapter with contention/delays and the remaining deep
checks on ARM64 and x86-64; and measure independent RSS, stacks, default/admission
budgets and affinity. Those are not certified by C2's reservation gauges.
Restart cleanup after process death remains open. Performance redesign of the
merge and timing verdicts against the frozen targets stay outside this work.
No serve/client or SQL-fuzzer grammar implementation is changed here.


### Reproduce and parent heavy commands

Run local checks one at a time; DATA is the existing SF0.01/sqlcheck directory
and DATA_SF1 is the standard SF1 directory. Unset AI_AGENT in every DuckDB child.

```sh
mkdir -p "$HOME/tmp-skelC2"
export TMPDIR="$HOME/tmp-skelC2" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
env -u AI_AGENT RUST_TEST_THREADS=2 cargo test --workspace
cargo deny check
cargo build --release -p nata -p nata-check
for suite in scan sql sql-differences aggregate; do
  env -u AI_AGENT target/release/nata-check --data "$DATA" \
    --queries "check/queries/$suite" --candidate "$PWD/target/release/nata"
done
env -u AI_AGENT target/release/nata-check --data "$DATA" \
  --queries check/queries/aggregate --candidate "$PWD/target/release/nata" \
  --force-aggregate-spill
env -u AI_AGENT python3 scripts/check-aggregate-spill.py \
  --candidate target/release/nata --data "$DATA_SF1" --rounds 3 \
  --out "$TMPDIR/sf1"
env -u AI_AGENT python3 crates/nata-exec/mutate.py --work "$TMPDIR/mutants" \
  --out "$TMPDIR/mutants.txt" spill-partition-not-read-back \
  spill-reservation-not-released spill-cleanup-skipped-on-error \
  spill-recursion-bound-removed spill-descriptor-cap-removed
scripts/check-leaks.sh --staged
```

Exact sequential `heavy.steps` commands for the parent, **not run on GCP here**
and not cloud cost approval. Use an on-disk TMPDIR before all steps. Full sweeps,
bound-three loom, architecture/resource runs and larger campaigns remain heavy:

```sh
aggregate-build cargo build --release -p nata -p nata-check
aggregate-forced env -u AI_AGENT target/release/nata-check --data "$DATA" --queries check/queries/aggregate --candidate "$SRC/target/release/nata" --force-aggregate-spill
aggregate-sf1 env -u AI_AGENT python3 scripts/check-aggregate-spill.py --candidate "$SRC/target/release/nata" --data "$DATA_SF1" --out "$RESULTS/aggregate-sf1" --rounds 5
aggregate-exec-mutants env -u AI_AGENT CARGO_BUILD_JOBS=2 python3 crates/nata-exec/mutate.py --work "$RESULTS/mut-exec" --out "$RESULTS/mut-exec.txt"
aggregate-sql-mutants env -u AI_AGENT CARGO_BUILD_JOBS=2 python3 crates/nata-sql/mutate.py --work "$RESULTS/mut-sql" --out "$RESULTS/mut-sql.txt"
aggregate-kernel-mutants python3 crates/nata-kernels/mutate.py --jobs 1
aggregate-memory-mutants CARGO_BUILD_JOBS=2 LOOM_MAX_PREEMPTIONS=3 python3 crates/nata-sched/mutants/mutate.py --only '^memory-' --jobs 1 --out "$RESULTS/mut-memory.txt"
aggregate-memory-loom LOOM_MAX_PREEMPTIONS=3 MEMORY_MODEL_TIMEOUT=3000 crates/nata-sched/run_memory_loom.sh
spill-build cargo test --locked -p nata-exec --test spill --no-run --message-format=json | python3 -c 'import json,sys; r=[json.loads(s) for s in sys.stdin]; print(next(x["executable"] for x in r if x.get("reason")=="compiler-artifact" and x.get("target",{}).get("name")=="spill" and x.get("executable")))' > "$RESULTS/spill-test-bin"
spill-corruption TASK_SPILL_BINARY=$(cat "$RESULTS/spill-test-bin"); (ulimit -v 262144; NATA_SPILL_CASES=30000 NATA_SPILL_SEED=22014202119 "$TASK_SPILL_BINARY" seeded_corruption_rejected_before_arrow_allocation --exact --nocapture --test-threads=1)
```

The final corruption command exercises the physical IPC envelope/schema, not
an implemented aggregate SQL grammar or exhaustive typed-state campaign. Do
not treat the old scalar SQL fuzzer as an aggregate campaign before step D
extends its grammar and difference classes. No absent campaign is given a
fabricated passing command.


## Step C — incomplete checkpoint

**NOT DELIVERED:** SQL aggregation does not yet spill. The 64 MiB SF1 spill
acceptance, forced-spill differential gate and recursive partition merge are
**unimplemented**, not passes. No timing verdict against the frozen targets,
GCP, push or merge. This checkpoint adds the reservation-contention diagnosis
and chunking fix requested by the parent, plus independently tested spill-file
infrastructure. It does not replace the remaining step C integration.

### Reservation contention: local, indicative before/after

**Measured:** the 8-thread HAVING query contends on per-group reservation and
release. A step B release `perf record -F 199 -g` capture has 2,855 task-clock
samples, zero lost samples: request **8.27%**, grant drop **3.05%**, contended
mutex lock **6.30%**, kernel spin unlock **9.74%**, kernel spin slowpath **6.48%**,
task switch **7.50%**, hash-table rehash **1.19%**, merge self-time **1.40%**.
Self-time alone does not include merge descendants or waiting. The counter
trial below measures those phase totals. [Symbol summary](raw/20261107-hash-aggregate-step-c/perf-symbols.txt).

Same-session, alternating before/after release binaries, warm SF1, five rounds
per arm/thread count, shared local x86-64 development machine (Intel Core i3-1220P), full CLI CSV to
`/dev/null`. Both variants include the same native instrumentation; the before
binary retains per-entry grants. This differs from the parent's uninstrumented
run and is not a cross-session speed comparison. SF1 Q1 and HAVING pass the
DuckDB differential check before timing (**2/2**). No merge algorithm change.

| Threads | Reservation mode | CLI seconds min / median / max | Reservation calls (median) | Lock acquisition seconds, summed | Partial seconds, summed | Merge seconds, summed |
|---|---|---|---:|---:|---:|---:|
| 1 | per entry | 3.622 / 4.262 / 4.366 | 1,494,396 | 0.115510 | 2.291 | 0.230 |
| 1 | chunks | 3.098 / 3.440 / 3.641 | 2,054 | 0.000240 | 1.943 | 0.258 |
| 8 | per entry | 5.152 / 5.607 / 5.962 | 2,776,627 | 15.948889 | 13.950 | 12.855 |
| 8 | chunks | 1.783 / 2.063 / 2.280 | 3,244 | 0.000530 | 3.494 | 4.408 |

Phase times are worker wall time summed, including lock waits, not CPU or query
elapsed time. Lock acquisition measures from immediately before locking to
immediately after, including clock and uncontended lock overhead. Requests and
releases match in every timed run. Counters are exposed by `--report` and
EXPLAIN ANALYZE, with peak reservations. [All 20 observations](raw/20261107-hash-aggregate-step-c/reservations.json).

Retained partials contain **1,493,405 groups** at one thread and **1,493,439** at
eight. The largest partial set (16 partition tables per task) is **1,493,405**
at one thread; at eight its median is **212,664 before / 273,722 after**.
Partial rehashes are **256** at one thread and median **1,664 / 1,680** at eight.
Median peak reservation is **1,519,884,352 / 1,526,441,216 bytes** at one thread
and **2,684,048,704 / 2,447,923,584 bytes** at eight. These use the existing
system-derived budget, **not 64 MiB**, and do not certify RSS or total allocation
accounting. The partial pool's scheduling changes its table-size distribution.

**Reasoned from these measurements:** reservation contention explains the
negative scaling in this workload. Chunking removes millions of ledger/lock
operations, with no redesign of ADR 0010's merge. The remaining merge and output
costs are reported, not optimized. Chunk capacity stays charged, including
unused space and bookkeeping, until its owning tables are freed. The target
chunk is 1 MiB, reduced for small budgets to `clamp(B / 256, 4 KiB, 1 MiB)`;
this is allocation granularity, not a budget/default increase. Growth/rehash
charges still precede allocation. A debug assertion checks live reservations
against B. Full allocation accounting remains unverified.

Reproduce the indicative diagnostic, sequentially (the before binary was saved
locally before replacing the ledger; its path is a local artifact):

```sh
export TMPDIR="$HOME/tmp-skelC" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo build --release -p nata -p nata-check
python3 scripts/profile-aggregate-reservations.py \
  --before "$TMPDIR/nata-before-chunks" --after "$PWD/target/release/nata" \
  --data "$DATA_SF1" --rounds 5 --out "$TMPDIR/profile-chunks.json"
# perf capture uses the same HAVING SQL and --threads 8, stdout to /dev/null.
```

### Spill infrastructure, not yet wired to the aggregate

`nata_exec::spill` owns exclusive-created files in a private query directory
(0700/0600 on Unix); uses O_NOFOLLOW when reopening; charges a supplied D before
append; caps simultaneous descriptors at `min(32, supplied process allowance)`;
and removes files/directory by RAII on normal completion, error, cancel, drop
and task unwind. Query admission must still compute D/process allowance and
connect these owners to the executor. Restart cleanup is not implemented.

The IPC boundary adapts the spike's validation to a fixed schema: UInt64 hash,
Binary key bytes and Binary state bytes, non-null, no dictionaries/compression.
Frames have a version, partition/depth, row/payload lengths and CRC over header
and payload. Before Arrow decoding: schema byte equality, bounded FlatBuffer
verification (depth 16, tables 64), metadata <=16 KiB, payload <=64 KiB, rows
<=256, exactly schema + record messages + EOS, node/buffer counts, non-overlap,
backed sizes and monotone binary offsets. File size must match the registry's
reserved byte count, including rejection of complete-frame truncation. A
512 KiB reserved workspace includes codec copies; a decoded batch retains its
reservation and prevents another read from multiplying that workspace. This
physical schema is tested; typed key/state encoding and semantic replay into
`ExactSum::from_spill` remain to be connected and tested.

A query-owned bounded I/O service has one worker, nonblocking submission and
sticky completion registration; it catches task panics, drops cancelled queued
jobs, and joins on teardown. Capacity includes completed, unconsumed tickets.
Its native tests do not model-check its std channels or worker internals.
The aggregate's existing scan/sink still runs to completion, so these APIs are
not a substitute for its required blocked/requeue continuation adapter.

### Checks and remaining work

Local checks: workspace fmt, clippy with warnings denied, tests and cargo-deny
**PASS**. The final chunk retry regression was then exercised by focused
`nata-sched`/`nata-exec` tests and the memory-model/control runs; subsequent
spill accounting changes were checked by the private executor control and the
fresh bounded spill executable. Native regression gates: scan **5/5**, SQL
**252/252**, SQL differences **54/54**, aggregate **65/65**; SF1 Q1/HAVING **2/2**.
Forced-spill gate: **unimplemented**. There is no nonzero SQL spill counter.

All **9 spill tests pass under `ulimit -v 262144`**, including the seeded
corruption trial: **64 cases per damage kind**, four kinds (truncation, flipped
byte, wrong/huge length and length beyond the file), seed **22014202119** printed;
plus a repaired-checksum binary-offset attack. No panic or large allocation in
those corruption reads. The intentional task-panic tests are caught and clean
up. [Bounded run](raw/20261107-hash-aggregate-step-c/spill-bounded.txt).
The first all-spill bounded invocation hit a stale private-mutant executable
because the local mutation target cache had been shared. That run failed
cleanup and is not counted as a production result. Only this worktree's
`nata-exec` artifacts were cleaned and rebuilt from real source; the fresh run
passes all nine tests. The temporary target symlink was removed. Parent runs
must keep the private mutation target separate.

**16 real-service loom models / 1,009 executions PASS at bound 2**. New cases:
spill request racing release, cancellation while a spill retains its grant,
chunked allocation/release at spill completion, and a pending chunk retry after
free memory changes. The last case preserves the original queued request size;
a retry must not calculate a different chunk size and get `RequestChanged`.
[Models](raw/20261107-hash-aggregate-step-c/memory-loom.txt).
Bound 3 is unrun locally. Native I/O channels/workers and deque internals remain
outside this loom instrumentation.

**Five targeted mutants CAUGHT, zero SURVIVED, zero INVALID**, with passing
controls. The four spill mutants exercise the production primitives, not a
query-level spill path. No recursion-bound execution mutant exists yet.

| Mutant | Check |
|---|---|
| spill-partition-not-read-back | Missing decoded frame / corruption/readback tests |
| spill-reservation-not-released | Workspace lifetime and final grant gauges |
| spill-descriptor-cap-removed | Explicit cap and released-slot test |
| spill-cleanup-skipped-on-error | Injected ENOSPC cleanup test |
| memory-chunk-credit-inflated | Native and loom chunk accounting assertions |

Executor control + four trials took **87 seconds** (sequential); memory mutant
**99 seconds**, separate private control **213 seconds**. These are check
runtimes, not engine-performance results. [Executor mutants](raw/20261107-hash-aggregate-step-c/mut-exec.txt),
[memory mutant](raw/20261107-hash-aggregate-step-c/mut-memory.txt),
[memory control](raw/20261107-hash-aggregate-step-c/mut-memory-control.txt).

Promoted direct dependencies, all already locked: arrow-ipc **60.1.0**,
flatbuffers **25.12.19**, crc32fast **1.5.4**, libc **0.2.191** (O_NOFOLLOW constant;
no own unsafe). Textual unsafe counts respectively **434 / 138 / 30 / 1,599**;
normal/build tree **83 packages, 8,834 keywords**, workspace **zero**. These count
inactive/test source too, do not expand macros, and are not an audit. No new
locked crate, policy exception or version change. Cargo-deny passes all four
categories with existing policy warnings. [Inventory](raw/20261107-hash-aggregate-step-c/unsafe-exec.txt).

Parent-only `heavy.steps` commands, **not run here and not cloud approval**:

```sh
memory-loom LOOM_MAX_PREEMPTIONS=3 MEMORY_MODEL_TIMEOUT=3000 crates/nata-sched/run_memory_loom.sh
memory-control CARGO_BUILD_JOBS=2 LOOM_MAX_PREEMPTIONS=3 python3 crates/nata-sched/mutants/mutate.py --control --jobs 1 --out "$RESULTS/memory-control.txt"
memory-mutants CARGO_BUILD_JOBS=2 LOOM_MAX_PREEMPTIONS=3 python3 crates/nata-sched/mutants/mutate.py --only '^memory-' --jobs 1 --out "$RESULTS/memory-mutants.txt"
spill-mutants CARGO_BUILD_JOBS=2 python3 crates/nata-exec/mutate.py --work "$RESULTS/spill-mutants" --out "$RESULTS/spill-mutants.txt" spill-partition-not-read-back spill-reservation-not-released spill-descriptor-cap-removed spill-cleanup-skipped-on-error
spill-build mkdir -p "$RESULTS/tmp" && export TMPDIR="$RESULTS/tmp" && cargo test --locked -p nata-exec --test spill --no-run --message-format=json | python3 -c 'import json,sys; r=[json.loads(s) for s in sys.stdin]; print(next(x["executable"] for x in r if x.get("reason")=="compiler-artifact" and x.get("target",{}).get("name")=="spill" and x.get("executable")))' > "$RESULTS/spill-test-bin"
spill-corruption export TMPDIR="$RESULTS/tmp"; TASK_SPILL_BINARY=$(cat "$RESULTS/spill-test-bin"); (ulimit -v 262144; NATA_SPILL_CASES=30000 NATA_SPILL_SEED=22014202119 "$TASK_SPILL_BINARY" seeded_corruption_rejected_before_arrow_allocation --exact --nocapture --test-threads=1)
```

The larger corruption command covers 30,000 cases of each damage kind of this
physical IPC schema; it is not a typed SQL spill campaign. Stage C's recursive
merge/forced-spill/SF1-budget jobs cannot be supplied before their entry points
exist. Parent preflight remains `scripts/gcp/test/test-job-chains.sh`; ARM64,
quiet-machine resource/affinity checks, sanitizers, Miri and Kani are unrun.

Still step C: resumable scan/aggregate pressure and I/O integration, typed
state encoding/replay, recursive partition merge, pre-decode/merge/output
reservations, query disk/process-FD admission, spill counters and forced-spill
CLI/differential/fault acceptance. These are **not deferred to step D**. Step D
remains serve/default clients, aggregate fuzzer grammar and full mutant sweeps.
No commit is permissible until the brief's forced-spill pre-commit gate passes;
that gate is not yet implemented. The branch remains an uncommitted checkpoint: `t3/aggregate-budget-spill`,
HEAD `8775a11`; all changes are staged, with a clean staged leak scan. No new
commit, push, merge or cloud operation was performed.

## Step B

**PASS: SQL and in-memory aggregation, with the aggregate differential gate.**
**NOT the complete walking skeleton:** partitioned spill and whole-query budget
integration are step C; serve/default clients, aggregate fuzzer grammar and full
mutant sweeps are step D. No timing verdict, GCP, push or parent-branch merge.
The requested prerequisite merge brings in `2116d97`; the implementation commit
accompanying this record is on `t3/hash-aggregate-sql-execution`.

### What runs

`nata sql` executes GROUP BY source columns/expressions, boolean HAVING, COUNT(*),
COUNT(x), SUM, exact AVG, MIN and MAX, grouped or global, with WHERE and LIMIT.
Metadata is fixed during binding, including the owner's integer SUM/AVG types.
EXPLAIN shows keys, aggregate result types and partial/partition/final phases.
Unsupported aggregate modifiers, nested aggregates, aggregates in WHERE and
GROUP BY, and grouping aliases/ordinals have positioned unsupported errors;
ungrouped columns have positioned bind errors.

A scan task pushes into its own partial sink; completed local tables return to
an operator pool and are reused by later morsels. Thus retained partial tables
scale with concurrent tasks, not row-group count. The reused
`hashbrown::HashTable` stores group IDs into owned key/state arenas. NULL,
dictionary text, canonical NaNs and signed zeros have value-based key identity.
Sixteen disjoint partitions merge as separate deque tasks; the coordinator
publishes batches of at most 1,024 finalized groups, with no global final table.
WHERE precedes argument evaluation; exact kernel state merges before final
checks/HAVING, and LIMIT never skips accumulation. Output projection evaluates
only admitted groups (including those OFFSET subsequently drops). Deep argument,
HAVING and projection expressions use sized stacks.

State/table growth and evaluator scratch use the prerequisite memory service;
grants travel with allocations and release on success, cancel and error. Memory
pressure is a clear temporary resource error: no spill is implemented. These
conservative charges are not a total-RSS bound: pre-decode grants, accurate
allocator bookkeeping, output buffering, progress workspace and admission
sharing remain step C. The CLI's provisional single-query state budget uses
half the smaller physical/cgroup limit, including ancestor cgroup limits;
`ExecConfig::aggregate_memory_bytes` permits an explicit test budget. No memory
default or 64 MiB resource target is certified by this step.

### Differential gate

DuckDB 2.0.0; local x86-64 only. Required sets: scan **5/5**, SQL **252/252**,
SQL differences **54/54**, aggregate **65/65**. All have zero mismatches,
unsupported queries, unexpected errors and reference errors. The aggregate
set is now in the gating CI job on both architectures (CI itself is unrun here).
Q1's full body and the high-cardinality HAVING query also pass at SF1: **2/2**.
No timing comparison was taken.

The aggregate set has 60 successful row-multiset comparisons (multiplicity is
preserved), three shared overflow-error checks and two documented Nata-only
strictness errors. Eight AVG queries and Q1 use declared numeric tolerances for
DuckDB's DOUBLE AVG; these comparisons are not the exact AVG oracle. Exactness
is checked by the inherited 347 independent integer/rational vectors, SQL
metadata assertions, wide cancellation/overflow tests and exact Nata-only AVG
expectations. All slice-4 group-key types, NULLs, empty/global/all-NULL inputs,
HAVING keys/aggregates, multi-morsel and high-cardinality input are included.

New dialect entries and twelve `sql-differences/d10_*.sql` queries hold integer
SUM metadata/value rules, exact decimal AVG, unsupported DISTINCT/FILTER,
float SUM/AVG and grouping aliases/ordinals, and three observed strictness
differences: DuckDB can eliminate aggregates under HAVING false or LIMIT 0,
and rewrites SUM(i + 1) to avoid the argument's INTEGER overflow. Binder tests
assert result types independently of CSV value comparisons.

Reproduce, one heavy command at a time:

```sh
mkdir -p "$HOME/tmp-skelB"
export TMPDIR="$HOME/tmp-skelB" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
unset AI_AGENT
cargo build -p nata -p nata-check
# DATA contains TPC-H SF0.01 and scripts/gen-sql-check-data.sh fixtures.
for suite in scan sql sql-differences aggregate; do
  target/debug/nata-check --data "$DATA" --queries "check/queries/$suite" \
    --candidate "$PWD/target/debug/nata"
done
mkdir -p "$TMPDIR/sf1queries"
cp check/queries/aggregate/q1.sql check/queries/aggregate/high_cardinality_tpch.sql "$TMPDIR/sf1queries/"
target/debug/nata-check --data "$DATA_SF1" --queries "$TMPDIR/sf1queries" \
  --candidate "$PWD/target/debug/nata"
```

For example, with the generated sqlcheck fixture:

```sh
target/debug/nata sql --csv --threads 2 -c "SELECT count(*) AS n, avg(i) AS a FROM '$DATA/sqlcheck/types.parquet' WHERE id IN (2,7,12)"
# n,a
# 3,3.333333
target/debug/nata sql --csv --threads 2 -c "SELECT f, count(*) AS n FROM '$DATA/sqlcheck/types.parquet' GROUP BY f HAVING count(*) > 3"
# f,n
# true,6
# false,6
# Grouped row order is unspecified.
```

### Checks and mutants

Workspace fmt, clippy (`--workspace --all-targets -- -D warnings`), tests and
cargo-deny **PASS**. Tests used disk TMPDIR and two test threads; all workspace
tests ran, including the inherited Parquet differential/damaged-file campaigns
at their default sizes. An earlier run failed the unchanged pgwire
`reserved_socket_cap_and_startup_deadline` assertion; the final full run passes
it. No server fix or assertion relaxation was made, and its root cause is not
established here. The scheduler facade guard also passes. The staged leak scan
is clean. ARM64/clean-machine verification remains unrun.

Native aggregate tests cover exact multi-partition contributions, actual task
overlap, forced hash collisions, canonical float keys, NULL/global/empty input,
wide cancellation, final precision/range errors, deep expressions, unordered
LIMIT/OFFSET, budget failure, sink failure and cancellation. Reservation-ledger
and scan gauges return to zero on observed success/error/cancel paths.
The real memory-service loom control passes all **12 models / 733 executions**
at preemption bound 2; this is not a model of the new deque path.

**Five targeted mutants caught; zero survivors and zero INVALID outcomes.**
Controls pass before the SQL/executor/kernel trials; the reservation trial uses
the passing real memory tests/models as its control. The native reservation
mutant times out and loom detects a deadlock; these are execution failures of a
successfully built mutant, not build failures counted as catches.

| Mutant | Result | Check |
|---|---|---|
| SQL `aggregate-wrong-sum-type` | CAUGHT | Binder aggregate result metadata |
| Kernel `sum-overflow-not-checked` | CAUGHT | Independent oracle and wide boundary tests |
| Executor `aggregate-having-before-merge` | CAUGHT | Cross-morsel count threshold and strict finalization |
| Executor `aggregate-partition-double-counted` | CAUGHT | Exact partition contribution count, plus integration tests |
| Scheduler `memory-reservation-not-released` | CAUGHT | Native cleanup tests/timeouts and real-code loom deadlock |

The executor control/trials take about five minutes and the SQL control/trial
about four minutes locally, below the brief's approximate 15-minute limit;
these are test runtimes, not query-performance measurements. Full sweeps remain
step D. Reproduce the local trials and required checks sequentially:

```sh
# Use the TMPDIR/build environment above; never /tmp.
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
RUST_TEST_THREADS=2 cargo test --workspace
cargo deny check
python3 crates/nata-exec/mutate.py --work "$TMPDIR/mut-exec" aggregate-partition-double-counted aggregate-having-before-merge
python3 crates/nata-sql/mutate.py --work "$TMPDIR/mut-sql" aggregate-wrong-sum-type
python3 crates/nata-kernels/mutate.py --only sum-overflow-not-checked --jobs 1
LOOM_MAX_PREEMPTIONS=2 crates/nata-sched/run_memory_loom.sh
LOOM_MAX_PREEMPTIONS=2 python3 crates/nata-sched/mutants/mutate.py --only '^memory-reservation-not-released$' --jobs 1
scripts/check-leaks.sh --staged
```

Commands for the parent's eventual `heavy.steps`, not run here and not cloud
cost approval (run one sweep at a time):

```sh
aggregate-sql-mutants CARGO_BUILD_JOBS=2 python3 crates/nata-sql/mutate.py --work "$RESULTS/mut-sql" --out "$RESULTS/mut-sql.txt"
aggregate-exec-mutants CARGO_BUILD_JOBS=2 python3 crates/nata-exec/mutate.py --work "$RESULTS/mut-exec" --out "$RESULTS/mut-exec.txt"
aggregate-kernel-mutants python3 crates/nata-kernels/mutate.py --jobs 1
aggregate-memory-mutants CARGO_BUILD_JOBS=2 LOOM_MAX_PREEMPTIONS=3 python3 crates/nata-sched/mutants/mutate.py --only '^memory-' --jobs 1 --out "$RESULTS/mut-memory.txt"
memory-loom LOOM_MAX_PREEMPTIONS=3 MEMORY_MODEL_TIMEOUT=3000 crates/nata-sched/run_memory_loom.sh
```

### Dependencies and remaining trust boundaries

The permitted `crossbeam-deque` 0.8.6 brings crossbeam-epoch 0.9.21 and
crossbeam-utils 0.8.23. `hashbrown` 0.17.1 was already locked and is promoted
with default features disabled; no allocator/hash dependencies are added.
Cargo-deny passes advisories/bans/licenses/sources, with existing policy
warnings. The textual unsafe inventory counts 41/267/153 keywords in the three
crossbeam crates (461 total); hashbrown's existing source count is 454. The
nata-exec normal/build tree has 82 packages and 8,804 keywords in 62 packages;
workspace packages contain zero. These counts include inactive/test code and
are not an audit or macro-expanded active-code count.

All scheduler synchronization names still go through `sync.rs`. Crossbeam,
scoped threads and bounded publication channels on the new unordered path are
native-tested and explicitly excluded from loom instrumentation. The inherited
memory-service models do not certify deque internals. ARM64 execution,
ASan/TSan, Miri/Kani on the new integration, full nightly fuzz and clean-machine
resource/affinity measurements remain unrun. No accepted ADR is amended.

Step C must implement partitioned state spill and the validating IPC envelope
and inside checks of ADR 0010; parked/requeued I/O and grant pressure; recursive
repartitioning with depth/no-progress bounds; memory/disk/FD accounting,
private registry cleanup and injected failures; damaged-spill fuzz; and measured
whole-query/default/admission budgets including scan and output. Step D must
verify serve and the five default clients (preparation/COPY included), add the
aggregate fuzzer grammar/difference classes and nightly campaigns, and run the
full mutant sweeps and remaining cross-architecture/deep checks. No service or
client implementation is changed or certified here. P4/P5/R12/P11 and full
slice closeout remain open.


## Prerequisite checkpoint (historical)

The text below predates checkpoint commit `2116d97`. Its missing-work and
branch-status paragraphs describe that earlier checkpoint, not step B above.

**NOT DELIVERED:** this branch contains checked prerequisites, not card 0005's
walking skeleton. No aggregate query runs through `nata sql` or `nata serve`.
Do not use this checkpoint as skeleton acceptance, slice closeout, a 64 MiB
resource verdict, or an SF1 performance result. No GCP, push or merge occurred.

**Implemented:** a reservation service in `nata-sched::memory`, behind its real
std/loom sync facade, and exact wide aggregate state in
`nata-kernels::aggregate`. The scan's FIFO and all current SQL/protocol behavior
are unchanged. The service is not yet connected to query execution.

### Checks

Local x86-64 only; ARM64 and clean-machine checks remain unrun.

- Twelve memory-service models pass locally at preemption bound 2: **733
  executions** in total. They cover grant caps, unique RAII release, FIFO
  coalescing/successor progress, abandoned requests, sticky release and cancel
  notifications, concurrent release/cancel/park, parked-victim wake, fair-share
  protection, and callbacks outside the manager lock.
  The same source is included in `loom_sched`, so existing nightly model
  discovery includes these models. The waiting condvar belongs only to the
  test dispatcher; service requests and park registration never wait.
- Five exact-state tests pass, including **347** independent Python integer/
  rational oracle vectors over precisions 1–38, several scales, both signs,
  exact ties, varying partial sizes and forward/reverse merge order. Wide
  intermediates may cancel; AVG never narrows SUM before division. Replayed
  sum/count bounds and count/result overflows are checked. This is sampled
  arithmetic evidence and a reasoned width bound, **not Kani certification**
  of Arrow i256 division or a validating IPC reader.
- Workspace fmt, clippy (`-D warnings`), and tests pass. Cargo-deny passes
  advisories/bans/licenses/sources, with existing policy warnings unchanged.
  Existing `nata-check` sets pass: scan **5/5**, SQL **252/252**, deliberate SQL
  differences **42/42**, using existing SF0.01/sqlcheck fixtures. **No aggregate
  set exists or passed.** These are regression checks, not aggregate acceptance.
  The initial symbol-rich workspace test build exhausted the available disk at
  link time. Only this worktree's generated target directory was cleaned
  (`cargo clean`, 12.4 GiB). Subsequent checks disable debug symbols and
  incremental compilation through environment variables; no product/build
  profile defaults were changed.

Reproduce locally, from the repository root, one command at a time:

```sh
mkdir -p "$HOME/tmp-skel"
export TMPDIR="$HOME/tmp-skel" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo test -p nata-sched --test memory
LOOM_MAX_PREEMPTIONS=2 crates/nata-sched/run_memory_loom.sh
cargo test -p nata-kernels --test aggregate
python3 crates/nata-kernels/tests/aggregate_oracle.py > "$TMPDIR/aggregate.txt"
cmp "$TMPDIR/aggregate.txt" crates/nata-kernels/tests/fixtures/aggregate.txt
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
# DATA is the existing TPC-H directory containing sqlcheck fixtures.
for suite in scan sql sql-differences; do
  env -u AI_AGENT target/debug/nata-check --data "$DATA" \
    --queries "check/queries/$suite" --candidate "$PWD/target/debug/nata"
done
scripts/check-leaks.sh --staged
```

The final models also pass as `memory::` tests inside `loom_sched` at bound 2
(12 tests; 22 existing models filtered out). Existing nightly discovery therefore
runs them; artifact names now use the matrix index to accommodate Rust's colons.
`scripts/gcp/test/test-job-chains.sh` passes locally with mocked cloud commands.

### Mutants

The scheduler's existing private-copy `mutants/mutate.py` gains reservation
leak, cap removal, sticky-wake loss and double-release mutants. Its memory
mutants run the real service tests and models. `nata-kernels/mutate.py` gains
SUM overflow omission, incorrect AVG tie rounding, missing merge-count guard,
premature partial narrowing, double-counted merge and ignored wide replay.
An initial harness control was INVALID because manifest substitution also
rewrote `rust-version`; this was corrected to replace whole declaration lines
and is not counted as a catch. The final runner requires a passing private-copy
control and a successful build before classifying a behavioral failure as CAUGHT. INVALID never counts.

Local short trials (sequential):

| Mutant | Result | Oracle |
|---|---|---|
| `memory-budget-cap-removed` | CAUGHT | Native tests and bound-2 loom (`budget cap removed`) |
| `sum-overflow-not-checked` | CAUGHT | 38-digit boundary and independent exact oracle |

The final kernel private-copy control passes. **Two mutants caught, no survivors,
no INVALID results in these final trials**; the earlier invalid control above
never counted. Other defined mutants are unrun locally, not presumed caught.

```sh
LOOM_MAX_PREEMPTIONS=2 python3 crates/nata-sched/mutants/mutate.py --only '^memory-budget-cap-removed$' --jobs 1
python3 crates/nata-kernels/mutate.py --only sum-overflow-not-checked --jobs 1
```

SQL SUM-type and HAVING-phase mutants, and partition-read/partition-double-count
mutants, remain **unimplemented**, because those execution paths do not exist
on this checkpoint. An arithmetic merge mutant is not a substitute for an
execution partition mutant.

### Dependencies and unsafe inventory

No manifest, dependency version, lockfile or dependency-policy changes. No
newly direct dependencies; no new dependency unsafe-inventory entries are
needed. Existing `arrow-buffer` i256 checked helpers are reused. Both changed
Rust crates retain `unsafe_code = "forbid"`. The earlier textual-inventory
method and its limitations remain those in
[engineering checks](20261106-eng-checks.md) and
[dependency inventory](20261107-pgwire-baseline.md#iv-dependency-policy-and-unsafe-inventory).
Cargo-deny passes all four categories. `scripts/unsafe-inventory.py` reports
0 own-code unsafe keywords in each changed crate; scheduler normal/build tree:
1 package, 0 keywords; kernel tree: 47 packages, 5,973 keywords in 36 packages
(all pre-existing dependencies). These are textual upper bounds, not an audit or
macro-expanded active-code count. No new trust exception is added.

### Still required to deliver the skeleton

All of SQL GROUP BY/HAVING/aggregate binding and result metadata; EXPLAIN;
per-task partials, disjoint final partition tasks and the unordered deque path;
bounded spill I/O and the extended validating IPC reader; registry cleanup and
fault injection; CLI/server budgets/default admission and preallocation grants
for scan/expressions/output; real cancel/error/drop integration; Q1 and the
high-cardinality query through the five default clients and COPY; SF1 spill
counters and the indicative timing; aggregate differential queries, forced-spill
mode, fuzzer grammar/difference classes and damaged-IPC fuzzing.

In particular, the existing `BatchMap` returns only a batch/drop/error, and the
existing row-group `Work::run` runs to completion. Connecting spill requires
an actual blocked/requeue adapter and task-local sink lifecycle. Counting
memory after decode or doing spill synchronously in `BatchMap` would not meet
ADRs 0009/0010. No such workaround is introduced here. Service bookkeeping,
active-operator lifecycle, RSS, decoded-buffer reservation and the progress
workspace itself still need accounting/integration; toy model units are not
measured allocation bytes.

**Dialect differences added: none.** The SQL dialect remains unchanged because
these APIs do not enable SQL aggregates. Adding documentation that advertises
the owner's new result types before execution exists would overstate support.
No ADR/card amendments are proposed or applied. No memory/disk/affinity defaults
or performance thresholds are changed. P4/P5/R12/P11 and remaining slice work
are not closed by these prerequisites.

### Branch and commit status

Branch: `t3/hash-aggregate-skeleton-2`. **No commit created.** The brief requires
the new aggregate differential set before every commit; that gate is absent.
The prerequisite changes are staged for review, with a passing staged leak scan.
They must be completed and committed before a parent job can use them from the
branch head. No dialect entries or successful aggregate gates are fabricated to
bypass the commit condition.

### To be run by the parent on GCP

These are exact `heavy.steps` lines for the code in this worktree, once committed;
**not run here**, and not cost approval:

```sh
memory-loom LOOM_MAX_PREEMPTIONS=3 MEMORY_MODEL_TIMEOUT=3000 crates/nata-sched/run_memory_loom.sh
memory-mutants CARGO_BUILD_JOBS=2 LOOM_MAX_PREEMPTIONS=3 python3 crates/nata-sched/mutants/mutate.py --only '^memory-' --jobs 4 --out "$RESULTS/memory-mutants.txt"
aggregate-mutants python3 crates/nata-kernels/mutate.py --jobs 4
```

The requested two 100,000-query SQL campaigns can use these existing commands
**after the aggregate grammar/difference classes are implemented**. On this
checkpoint they test the old scalar subset only, not aggregates:

```sh
sql-fuzz-a cd spikes/sqlfuzz && env -u AI_AGENT NATA_BIN="$SRC/target/release/nata" python3 -I fuzz.py run-subset --seed 20261107 --cases 100000 --jobs 4 --target-cmd 'python3 -I adapters/nata_adapter.py'
sql-fuzz-b cd spikes/sqlfuzz && env -u AI_AGENT NATA_BIN="$SRC/target/release/nata" python3 -I fuzz.py run-subset --seed 20261108 --cases 100000 --jobs 4 --target-cmd 'python3 -I adapters/nata_adapter.py'
```

**Missing handoff:** there is no production damaged-spill fuzz target to invoke
at 30,000 cases per record shape, no aggregate SQL/executor mutant sweeps, and
no aggregate client/SF1 job. No command is invented for an absent target.
The parent must complete these before launching skeleton acceptance jobs.
`scripts/gcp/test/test-job-chains.sh` remains a required local preflight for
those eventual jobs. This checkpoint does not authorize a launch.
