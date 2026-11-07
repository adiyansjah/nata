# Hash aggregate skeleton — incomplete checkpoint, 2026-11-07

**NOT DELIVERED:** this branch contains checked prerequisites, not card 0005's
walking skeleton. No aggregate query runs through `nata sql` or `nata serve`.
Do not use this checkpoint as skeleton acceptance, slice closeout, a 64 MiB
resource verdict, or an SF1 performance result. No GCP, push or merge occurred.

**Implemented:** a reservation service in `nata-sched::memory`, behind its real
std/loom sync facade, and exact wide aggregate state in
`nata-kernels::aggregate`. The scan's FIFO and all current SQL/protocol behavior
are unchanged. The service is not yet connected to query execution.

## Checks

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

## Mutants

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

## Dependencies and unsafe inventory

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

## Still required to deliver the skeleton

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

## Branch and commit status

Branch: `t3/hash-aggregate-skeleton-2`. **No commit created.** The brief requires
the new aggregate differential set before every commit; that gate is absent.
The prerequisite changes are staged for review, with a passing staged leak scan.
They must be completed and committed before a parent job can use them from the
branch head. No dialect entries or successful aggregate gates are fabricated to
bypass the commit condition.

## To be run by the parent on GCP

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
