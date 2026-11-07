# Hash aggregate skeleton — step B, 2026-11-07


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
