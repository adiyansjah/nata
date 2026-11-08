# Card 0008 package 0 — continuation, 2026-11-08

The parent superseded the diagnostic-ordering stop: stats go to a caller-owned
file. The historical stop and its evidence remain below. No optimization,
result change, dependency, slice-5 target change, GCP execution, push or merge.

## 1. Separate same-run record and compatibility

Rule: stats must not mix with query output or diagnostics, including overflow
errors. Same code/data must pass aggregate 65 normal + 65 forced spill, SQL 252,
scan 5 with stats disabled and enabled. Overflow stdout/stderr are a byte oracle.

Design: `NATA_STATS_JSON=PATH` opts into query-owned stats. The CLI retains the
owner through execution, sink finish and signal-thread join, then atomically
publishes one JSON document using create-new temporary file + rename. Error
paths publish after their diagnostics; syntax/bind failures are included. A
writer error is returned as exit 1 without printing instrumentation diagnostics.
Use distinct paths for concurrent invocations. `--report` keeps its original
output. `EXPLAIN ANALYZE` displays the same execution owner and stage snapshot;
the final file additionally includes sink finish and teardown observations.

Counters include scan/decode, pushed/residual filter, expression evaluation,
group ID/hash resolution, state update, merge, finalize, output/sink, teardown,
bind, admission waits/grants, per-level spill bytes/frames/opens/tickets/codec
service, I/O job queue/busy service and scheduler waves/steps/yield/block/done.
Optional bounded traces retain timestamped intervals and disclose drops.
Durations are **summed service wall**, never critical-path estimates. Linux
process CPU ticks and scheduler thread CPU ns have separate labels.

Coverage honesty: `complete:false` remains intentional. This delivers the
listed stage observations, not full T6/R33 certification. `missing` retains
complete allocation inventory, all worker CPU scopes, ready/wake edges and
native sink-finish coverage across every adapter. The CLI sink finish is now
covered; native adapter completeness is not inferred. The verdict engine must
continue to return NOT RUN for an incomplete attribution record. No reference
cell may be promoted to MET on the strength of this implementation alone.

Measured gate numbers: **off and on each pass 65/65 aggregate, 65/65 forced
spill, 252/252 SQL, 5/5 scan** (774 checked query cases total). Four
parallel overflow byte controls pass; serial Q1 success-byte controls pass in
both spill modes; syntax/bind error publication and EXPLAIN owner identity pass.
No stats JSON is printed to stderr. The native records and byte hashes are
retained in [continuation raw evidence](raw/20261108-engine-perf-package0-continuation/).

One forced-spill Q1 record (SF0.01, 60,175 scanned rows, 1 thread, 16 MiB) has
238 scan/decode, 472 filter, 239 expression, 472 group-ID, 236 state-update,
722 merge, 35 finalize and 33 output/sink calls. It admits one task with
7,697,780 task bytes; records 712 grants and 712 releases, 236 write and 668
read I/O jobs, 636 frames each way, 874,336 spill bytes each way at level 0,
1,147 waves/steps, 906 yields and 208 blocked steps, zero dropped events. The
native spill totals reconcile exactly. Admission-wait service is zero for this
fixture, not a missing field. These are diagnostic observations, not performance
or allocator-completeness claims.

An extra parallel-Q1 byte control initially failed: six stats-off repeats
produced two byte sequences too, while all twelve off/on runs had the same row
multiset and empty stderr. Q1 has no ORDER BY and resident partition delivery
is already unordered. The success-byte control now uses one thread; the
required overflow byte controls remain parallel. No engine ordering or result
policy changed. The diagnostic hashes are retained, not hidden.

Engineering: final `cargo test` on all four touched crates passes **322 tests,
zero failures, two existing ignored**. `cargo fmt --all --check`, four-crate
Clippy with all targets and warnings denied, locked release build and the
unchanged lockfile check pass. Instrumentation commit **`520e4ed`**.

Reproduction (one Cargo command at a time):

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_INCREMENTAL=0 RUST_TEST_THREADS=4 \
  cargo test --locked -j 4 -p nata-sched -p nata-parquet -p nata-exec -p nata
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_INCREMENTAL=0 \
  cargo clippy --locked -j 4 -p nata-sched -p nata-parquet -p nata-exec -p nata --all-targets -- -D warnings
cargo fmt --all --check
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_INCREMENTAL=0 \
  cargo build --locked --release -j 4 -p nata -p nata-check
python3 scripts/check-query-stats.py --binary /home/dev/perf2b/target/release/nata --data /home/dev/perf2d/data
# Bash: all four gates per mode. Use a fresh destination for each run.
for mode in off on; do
  stats=()
  if [ "$mode" = on ]; then stats=("NATA_STATS_JSON=/tmp/pkg0-$mode.stats.json"); fi
  for suite in aggregate sql scan; do
    env -u AI_AGENT -u CODEX_CI -u NATA_STATS_JSON -u NATA_QUERY_STATS "${stats[@]}" \
      /home/dev/perf2b/target/release/nata-check --candidate /home/dev/perf2b/target/release/nata \
      --data /home/dev/perf2d/data --queries "check/queries/$suite"
  done
  env -u AI_AGENT -u CODEX_CI -u NATA_STATS_JSON -u NATA_QUERY_STATS "${stats[@]}" \
    /home/dev/perf2b/target/release/nata-check --candidate /home/dev/perf2b/target/release/nata \
    --data /home/dev/perf2d/data --queries check/queries/aggregate --force-aggregate-spill
done
```

## 2. Disabled cost

Rule declared before measurement: compiled-in/default-disabled versus compiled
out must be within `1±N`, where N is the largest median `abs(primary/twin-1)`
across arms, under the existing 5% ceiling; median AND four of five must satisfy
both bounds. No allocations, clocks or locks in disabled hot-path hooks.
An unset relaxed flag avoids TLS access for materialization/grant callbacks;
coarse task timers carry None. Enabled traces may allocate; disabled hooks do
not allocate trace buffers. The existing engine's counters remain unchanged.

Design: `scripts/bench-stats-off-cost.py` extends the 2b method: read-only binary
copies, identical source/locked dependencies, `--cfg nata_stats_disabled` as the
compiled-out control, alternating five rounds, **primary + twin on both arms**,
DuckDB value gates before timings, full shared pipe-EOF timer, binary/input
hashes and topology. All stats-enabling/destination variables are removed.
Local evidence is always labelled unsettled; a clean reference run explicitly
uses `--settled`, checks topology, settles each cell and exits nonzero unless
every cell meets the bound. Inert controls prove alternation, four arms,
disabled environment, unsettled result and a planted 20% cost failing the gate.

Local numbers (4 GiB, five alternating rounds with primary/twin on each arm,
**80 timed observations** total):

| Query / local scale | Threads | Median paired disabled / compiled-out | Largest twin noise N | Predeclared admissible band | Verdict |
|---|---:|---:|---:|---|---|
| Q1 / SF1, 6,001,215 rows | 1 | 0.949736 | 1.1803% | 0.988197–1.011803 | UNRESOLVED (unsettled) |
| Q1 / SF1 | 8 | 0.972969 | 2.5708% | 0.974292–1.025708 | UNRESOLVED (unsettled) |
| HC / SF0.01, 60,175 rows | 1 | 0.970011 | 16.0113% | inadmissible: noise >5% | UNRESOLVED |
| HC / SF0.01 | 8 | 0.991528 | 20.0561% | inadmissible: noise >5% | UNRESOLVED |

All DuckDB value gates passed. Ratios are medians of paired round ratios,
not ratios of marginal medians. Q1's apparent reductions lie below the
predeclared two-sided band; they do not certify either direction in this
unsettled session. The tiny HC control is dominated by whole-CLI noise. The
rule gives **4/4 UNRESOLVED**, not a zero-cost or performance-improvement claim.
The clean GCP measurement remains required. No rerun was used to select a pass.

An independent build-identity probe requests a destination from both read-only
binaries: default build writes a valid record; compiled-out control writes no
record; both query byte streams match. Source identity is `520e4ed`, with the
complete Rust/source, input and binary hashes retained in the raw evidence.
Both builds use the same locked dependencies and release profile; only the
compiled-out control has `RUSTFLAGS='--cfg nata_stats_disabled'`.

Commands (after the serial default release build above):

```sh
mkdir -p /tmp/pkg0-cost-binaries
cp /home/dev/perf2b/target/release/nata /tmp/pkg0-cost-binaries/disabled
chmod a-w /tmp/pkg0-cost-binaries/disabled
RUSTFLAGS='--cfg nata_stats_disabled' CARGO_TARGET_DIR=/home/dev/perf2b/target-pkg0-without CARGO_INCREMENTAL=0 \
  cargo build --locked --release -j 4 -p nata
cp /home/dev/perf2b/target-pkg0-without/release/nata /tmp/pkg0-cost-binaries/without
chmod a-w /tmp/pkg0-cost-binaries/without
python3 scripts/bench-stats-off-cost.py --commit 520e4ed \
  --without /tmp/pkg0-cost-binaries/without --disabled /tmp/pkg0-cost-binaries/disabled \
  --data /home/dev/nata-s5-fixes/sf1 --queries q1 --threads 1 8 --budgets 4294967296 --out /tmp/pkg0-cost-q1
python3 scripts/bench-stats-off-cost.py --commit 520e4ed \
  --without /tmp/pkg0-cost-binaries/without --disabled /tmp/pkg0-cost-binaries/disabled \
  --data /home/dev/perf2d/data --queries high_cardinality_tpch --threads 1 8 --budgets 4294967296 --out /tmp/pkg0-cost-hc
```

Use fresh output directories to repeat; originals remain evidence. Raw samples,
summary, topology, flags, source/input/binary identities and the build probe
are archived under `raw/20261108-engine-perf-package0-continuation/`.
The prepared GCP step screens Q1 at 1/8 threads and all three frozen budgets on
SF10. Both binaries are built serially from the same verified source archive.
A failed off-cost gate blocks subsequent Nata reference timings.

## 3. Verdict file consumption

Rule: every timed Nata arm consumes its own execution's file, never stderr or a
prior cell's record. The shared timer/verdict policy and slice-5 default stay
unchanged.

Design: the shared `invoke` assigns `<invocation>.stats.json`, removes a stale
file before launch, overrides inherited destinations and reads after process
completion. JSON must contain exactly one versioned document. Missing files
stay missing; malformed files fail. Records and paths are attached to samples;
closeout sidecars persist with the cell. The heavy wrapper also attaches its
same-run record before its temporary directory is removed.

Checks: thirteen verdict tests (including real subprocess file/diagnostic and
stale-file controls), nine planted Python mutants, both inert harnesses.
The native script reconciles real forced-spill records and requires nonzero
stage, grant, scheduler, frame and I/O counters. Both completion-timer tests
pass at the original 50 ms whole-child / 5 ms exit-to-EOF bounds. An earlier
attempt during concurrent compilation measured 85.96 ms and failed the 50 ms
bound; the lower-load rerun passed without changing a limit.

Focused Rust cargo-mutants: baseline PASS, **3/3 CAUGHT, zero missed**:
`QueryStats::add`, `QueryStats::level`, and `write_json -> Ok(())`. Command:

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_INCREMENTAL=0 RUST_TEST_THREADS=1 \
  /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false \
  --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --cargo-test-arg=query_stats \
  --output /tmp/pkg0-mutants-final -p nata-sched \
  -f crates/nata-sched/src/query_stats.rs -F 'add|level|write_json' --timeout 60
```

File-consumption commit **`c45e9f1`**. Python verdict tests 13/13, profiler tests
2/2, prepared-job/off-cost controls 3/3, timer tests 2/2, all nine Python
mutants, both complete inert harnesses and the job-chain checks pass.

## 4. R26 n4a trial (prepared, not run)

Rule: try ARM events and retain useful parent+leaf user stacks, unknown/lost
frames and wall/user/system CPU. Counter-only success is insufficient.

Design: `--all-events` tries `cpu-clock:u`, `task-clock:u`, `cycles:u` even after
a useful event succeeds. `precheck.json` contains all commands/exits/stderr,
working events and the first usable selection; raw data/stacks/usage stay in
results. If no event works, it exits 1 and `# gate-exits` stops the job. Scoped
heavy startup installs linux-perf and uses the existing perf-job kernel-access
setup only when its steps explicitly request this pre-check. No new dependency
was added to Cargo.lock. This prepares the n4a experiment; **R26 stays OPEN**.

Checks: two profiler tests, including software-event rejection, two successful
fallbacks and all-events-rejected negative controls; shell syntax and complete
job-chain checks pass. Commit `65ff547`.

## 5. Reference shipment and exact launch recipe (not executed)

Rule: early SF10 Q1 1/8 threads at frozen 4 GiB / 64 MiB / 256 MiB, with the
four-thread physical-core control and independent unpinned/pinned dimensions.
No Nata reference timing without owner freeze. No paid execution here.

Design: both steps files start `# gate-exits`, verify the source tar's sha256 and
extract it before running. The baseline validates owner acceptance, policy,
slice-5 pin, source SHA, reference manifest hash, successor/fixture hashes,
budgets, thread counts and the predeclared off-cost rule. The owner-supplied
freeze must contain `package0_off_cost_rule: "within-twin-noise-v1"`,
`budgets: [4294967296,67108864,268435456]`, `threads: [1,4,8]`,
`source_sha`, `manifest_path`, `manifest_sha256`, `fixture_hashes`,
`successor_fixture_hashes`, `slice5_sha: "bb946ab"`, `policy: "0008-v1"`,
and `owner_accepted: true`. No owner acceptance is fabricated here.

Sizing: pre-check **8 vCPU / 30 GB / 0.25 VM-hours** per CPU; reference
**8 vCPU / 150 GB / 6 VM-hours** per CPU. c3d has four physical cores with SMT;
n4a has eight physical cores. Pinned masks select one thread per physical core
before siblings, with lscpu topology retained. 150 GB covers two serial release
builds, stepped SF10 generation (`GEN_CHILDREN=10`), output/spill and evidence.
Reference work is six Q1 off-cost cells (120 timed processes), then nine Q1 cells
per affinity dimension (360 timed processes), plus gates/builds/settling. Six
hours is a conservative safety cap, not a latency forecast; no automatic retry
into a pass. The full owner reference manifest/freeze is still an external
prerequisite, not silently accepted by these steps.

After final commits and checks, prepare exactly as the slice-5 closeout record:

```sh
git archive HEAD --format=tar --output=.engine-perf-package0-source.tar
git rev-parse HEAD > .engine-perf-package0-commit
sha256sum .engine-perf-package0-source.tar > .engine-perf-package0-source.sha256
sha256sum -c .engine-perf-package0-source.sha256
cp bench/engine-perf-package0-precheck.steps heavy.steps
cmp bench/engine-perf-package0-precheck.steps heavy.steps
: "${PKG0_RUN_SUFFIX:?use a fresh suffix}"
bash scripts/gcp/orchestrate.sh plan --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-precheck-arm" --machines n4a-highcpu-8 --max-vm-hours 0.25 --data-gb 30
bash scripts/gcp/orchestrate.sh run --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-precheck-arm" --machines n4a-highcpu-8 --max-vm-hours 0.25 --data-gb 30 --approved-usd "${PKG0_APPROVED_USD:?owner-approved bound for this plan}"
bash scripts/gcp/orchestrate.sh plan --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-precheck-x86" --machines c3d-highcpu-8 --max-vm-hours 0.25 --data-gb 30
bash scripts/gcp/orchestrate.sh run --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-precheck-x86" --machines c3d-highcpu-8 --max-vm-hours 0.25 --data-gb 30 --approved-usd "${PKG0_APPROVED_USD:?owner-approved bound for this plan}"
# Only after useful stacks on both CPUs and the owner-supplied reference freeze:
python3 scripts/check-engine-perf-freeze.py .engine-perf-package0-freeze.json
cp bench/engine-perf-package0-baseline.steps heavy.steps
cmp bench/engine-perf-package0-baseline.steps heavy.steps
bash scripts/gcp/orchestrate.sh plan --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-baseline-x86" --machines c3d-highcpu-8 --max-vm-hours 6 --data-gb 150
bash scripts/gcp/orchestrate.sh run --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-baseline-x86" --machines c3d-highcpu-8 --max-vm-hours 6 --data-gb 150 --approved-usd "${PKG0_APPROVED_USD:?owner-approved bound for this plan}"
bash scripts/gcp/orchestrate.sh plan --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-baseline-arm" --machines n4a-highcpu-8 --max-vm-hours 6 --data-gb 150
bash scripts/gcp/orchestrate.sh run --job heavy --run-id "pkg0-${PKG0_RUN_SUFFIX}-baseline-arm" --machines n4a-highcpu-8 --max-vm-hours 6 --data-gb 150 --approved-usd "${PKG0_APPROVED_USD:?owner-approved bound for this plan}"
```

Current price verification, a passing local trial of the shipment and separate
cost approval precede any `run`. No infrastructure identifiers belong in these
files. Frozen-preparation tests include failing owner/source/hash/budget/thread/
successor/rule controls. Preparation commit `ce5af76`; subsequent evidence and
source identities are recorded below. No cloud call has been made.

Implementation commits: `65ff547` R26 event/tooling preparation; `ce5af76`
source shipment/freeze/off-cost steps; `c45e9f1` per-invocation file consumption;
`520e4ed` Rust instrumentation/writer/byte controls; `96df7bd` compatibility,
native-record and mutation evidence. The final report/evidence commits are
listed in the implementation handback. Every commit passed the staged leak
check. All Cargo builds/tests were serialized at `-j 4`; stale incremental
caches were deleted when the initial free space was below 20 GiB. No lockfile
change, dependency addition, push, merge or GCP execution occurred.

Verdicts: listed stage/compatibility/writer/verdict checks **MET**; local
cost **UNRESOLVED / unsettled**; R26 ARM capture and settled cost **NOT RUN**;
full T6 attribution remains **NOT RUN** (`complete:false`). These distinctions
are preserved in both records and the card's progress line.

---

# Historical implementation stop (superseded by the continuation above)

**STOPPED: stats-on forced-spill compatibility gate failed (63/65). Package 0 is incomplete.** Slice 5 remains pinned to
`bb946ab`; its targets, matrix, resource gates and manual noise judgment are
unchanged. No paid job or reference timing was run here. No dependency was added.

## Verdict engine and shared timer

Rule: explicit `0008-v1` policy, five paired alternating rounds, primary/twin
noise `median(abs(primary/twin-1))`, largest arm noise ≤5%, median AND four of
five within the cell's ceiling (or above its scaling floor). Missing cells are
NOT RUN, failed reference UNASSESSED, noisy/unsettled cells UNRESOLVED. These
proposal values still require the card's reference freeze. No slice-5 rebaseline.

Design: both existing runners use `scripts/bench_verdict.py`'s nanosecond
pipe-EOF timer (b383422 method), process-group deadline, full output and process
CPU/RSS. The closeout runner's default calls the extracted, unchanged slice-5
summary. `--policy 0008-v1` attaches a separate verdict requiring complete
same-run stats. An offline manifest evaluator supports cell-specific ceilings,
floors and dimensions; retains every sample and attached stats. No geometric mean.
Pinned affinity chooses one sibling per physical core before using SMT siblings;
the unpinned arm remains a separate session/dimension.

Measured checks: thirteen verdict unittests pass; nine deliberate Python verdict
mutants CAUGHT (slowdown, noise, vote count, missing stats, drops, floor direction,
round pairing, failed denominator and the frozen Q1 ceiling). Both full inert harness self-tests pass
(36 SF1 + 36 SF10 + 5 SF100 cells; five-source/480-arm heavy simulation).
The two completion-timer tests pass with the smoke child's terminal timestamp
flushed immediately before `_exit`; interpreter teardown had produced 5.0–5.4 ms
exit residuals against the existing 5 ms bound. No timer bound was loosened.

Commands:

```sh
python3 -m unittest discover -s scripts -p test_bench_verdict.py
python3 scripts/check-verdict-mutants.py
python3 -m unittest discover -s scripts -p test_bench_aggregate_closeout.py
python3 scripts/bench-aggregate-closeout.py --self-test
python3 scripts/bench-aggregate-perf1-heavy.py self-test --work /tmp/package0-harness
python3 scripts/bench_verdict.py --manifest MANIFEST.json --timings TIMINGS.json --out VERDICTS.json
```

Verdict: control checks MET; production T6/T7 **NOT RUN** until stats completeness,
both-CPU profiling, the target freeze and settled reference runs. `cargo-mutants`
cannot mutate Python; the nine-source-mutation screen above is its applicable
counterpart. Rust stats mutation results are recorded separately below.

## Profiler pre-check / R26

Rule: before an expensive matrix, require useful symbol-known parent+leaf stacks,
loss/unwind diagnostics and wall/user/system CPU capture. `perf stat` alone cannot
pass T6. Design: external one-second native smoke; try `cpu-clock:u`,
`task-clock:u`, then `cycles:u`, retaining command, exit and stderr for each
attempt. No in-process sampler, permissions/sysctl changes or cloud operations.

Local x86 measurement: **MET** for the smoke only: `cpu-clock:u`, 99 useful
parent+leaf callchains, zero unknown frames, zero reported lost records,
1.00 s wall / 1.00 s user / 0.00 s system. Pre-check unit control passes: separate
leaf/parent samples do not count as a useful unwind. Raw capture stays outside git.

```sh
python3 -m unittest discover -s scripts -p test_profile_precheck.py
python3 scripts/profile-precheck.py --out /tmp/package0-profile-precheck
```

ARM64: **NOT RUN / R26 OPEN**. This worktree has no authorized n4a execution.
The existing recorded n4a `cpu-clock:u` rejection is retained; neither
`task-clock:u` nor `cycles:u` was tried on n4a by this implementation. The script
records their exact attempted commands/errors when run there. Generic `heavy`
VM setup does not currently install `linux-perf`; a fresh VM without it fails
this pre-check immediately. Tool provisioning is also a reference-run prerequisite. No working ARM
sampling path is claimed; a failed pre-check must stop reference timing and be
returned to the owner for a userspace-tool or alternate-machine brief.

## Prepared jobs (not run)

`bench/engine-perf-package0-precheck.steps` is the inexpensive event/tooling job;
`bench/engine-perf-package0-baseline.steps` gates timing on an owner-accepted
`.engine-perf-package0-freeze.json`, then prepares SF10 Q1 at **1/4/8 threads ×
4 GiB / 64 MiB / 256 MiB**, first unpinned then pinned. It includes the requested
1/8 cells and a four-thread physical-core control. On x86 the pinned four-thread
mask selects four distinct physical cores; eight threads then uses their SMT
siblings. ARM uses four/eight distinct cores. Every cell saves topology and
keeps affinity as its own output dimension. Nothing edits slice 5's fixtures.

Prepare shipment on this branch only after all checks/coverage gaps are settled:

```sh
cp bench/engine-perf-package0-precheck.steps heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --data-gb 30 --max-vm-hours 0.25 --run-id pkg0-precheck-x86
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --data-gb 30 --max-vm-hours 0.25 --run-id pkg0-precheck-arm
# After pre-checks, reference evidence and the owner's explicit freeze:
cp bench/engine-perf-package0-baseline.steps heavy.steps
git rev-parse HEAD > .engine-perf-package0-commit
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-x86
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-arm
# NOT EXECUTED. Only after local trial + separate cost approval:
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-x86 --approved-usd 0.58
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-arm --approved-usd 0.76
```

Sizing is reasoned: eight logical CPUs preserve the frozen comparison topology;
16 GiB fits the frozen 4 GiB caps, and stepped SF10 generation (`GEN_CHILDREN=10`)
avoids generating the full scale in memory. 150 GB leaves room for toolchain,
builds, SF10 Parquet, generation scratch, complete CSV and spill; six VM-hours
bounds two nine-cell blocks with twenty timed processes per cell, gates, load
settling and build/generation. The 1800-second per-process deadline remains a
safety cap, not an expected latency. No automatic retries into a pass.

Offline `plan` only prints repository-price bounds of $0.577 x86 / $0.755 ARM.
These are **not current-price verification or spend approval**. A run still
requires the current pricing check, a passing local trial of the exact shipment,
and owner approval. `scripts/gcp/test/test-job-chains.sh`: all checks pass.

**Reference preparation gap:** the full card reference manifest (all kernel,
scan/expression, successor sort/join/window fixtures and their hashes) is not yet
prepared. The baseline steps do not claim to freeze it; their explicit freeze
gate refuses its absence. Successor budgets and package-0 cost certification
remain NOT RUN, not silently accepted.


## Stop: diagnostic compatibility changed

The required on/off gate is a failing control. **Stats off**: aggregate **65/65**,
forced spill **65/65**, SQL **252/252**, scan **5/5**. **Stats on**: normal
aggregate **65/65**, forced spill **63/65**, with `having_overflow` and
`limit_overflow` failing. Stats-on SQL and scan were **NOT RUN** after this failure.

Cause confirmed from source and saved gate output: forced-spill checks pass
`--report`; the new error branch in `crates/nata/src/sql.rs` prints `query_stats:`
before `finish_error`. `nata-check` deliberately checks the first nonempty stderr
line (`run_spilling_engine`/`first_line`), so the expected overflow diagnostic is
replaced by the stats record in the check's observation. This violates the
brief's unchanged-behaviour requirement. **No fix or rerun was attempted after
the stop condition.** The instrumentation is left **uncommitted** for inspection.

Raw gate logs, mutation summaries and hashes of the uncommitted Rust sources and
tested binary: [raw package-0 evidence](raw/20261108-engine-perf-package0/).
No result cells, machine identities or credentials are in these logs.

Reproduce the failing gate (the paths identify the local checked fixture/build):

```sh
env -u AI_AGENT -u CODEX_CI NATA_QUERY_STATS=1 NATA_QUERY_TRACE=0 \
  /home/dev/perf2b/target/release/nata-check \
  --candidate /home/dev/perf2b/target/release/nata \
  --data /home/dev/perf2d/data --queries check/queries/aggregate --force-aggregate-spill
```

## Same-run instrumentation — partial, uncommitted

Rule: counters and durations come from one owner per execution; service-wall
sums are not critical path or CPU time. Off-path hooks with no owner allocate no
buffers and take no clocks/locks; a safe TLS lookup can remain at coarse
materialization/grant boundaries, and an unset OnceLock is read per scan task.
The compiled-out control is `RUSTFLAGS='--cfg nata_stats_disabled'`.
Predeclared off-cost check: default-disabled versus compiled-out must lie within
`1±N` (largest arm's twin noise), under the existing noise ceiling and median /
four-of-five rule. **Measured cost/bound: NOT RUN**; the failing compatibility
gate stopped the run before building/timing that control. No zero-cost claim.

Implemented hooks: query identity; separate scan/decode, pushed/residual filter,
expression, hash/group resolution, state update, merge, finalize, output and
teardown service-wall timers; actual decode/task admission; tracked Col copy /
materialization counters; grant counts; spill bytes/frames/opens/tickets and
codec service by level; I/O job-kind queue/busy sums; continuation/wave counters
and scheduler-runtime deltas. Optional `NATA_QUERY_TRACE=1` records bounded
pipeline/task/wave and I/O events and reports drops. Ordinary counters do not
allocate the detailed trace buffer. The same frozen record is available to
`--report` and EXPLAIN ANALYZE when `NATA_QUERY_STATS=1`; benchmark observations
attach it. The record deliberately has **`complete:false`**.

Remaining required coverage: complete allocation-site inventory and ownership,
all worker CPU scopes and clock validation, stable row-group-to-step / ready-wake
edges and lock-site intervals, native sink finish / final owner teardown, and
full fake/native reconciliation. Native process CPU is labelled in Linux ticks;
the shared closeout timer additionally collects process user/system seconds.
The stage/counter record is useful diagnostic scaffolding, **not completed T6 or
R33**. Prepared jobs must not be treated as ready for certification.

The shared verdict module uses exact rational ratios from integer nanoseconds
(where present); 5% and 10% boundary controls avoid float-rounding changes to the
rules. Missing stage/counter fields, dropped events, broken interval pairs and
spill totals that disagree with native counters fail reconciliation. Explicit
`0008-v1` mode applies Q1's 1.5 ceiling at every frozen SF1/SF10 budget and the
2.0 orderkey/partkey ceiling; the default slice-5 report policy is unchanged.

## Engineering checks and mutants

- `cargo fmt --all --check`: PASS.
- `cargo clippy --locked -j 4 -p nata-sched -p nata-parquet -p nata-exec -p nata --all-targets -- -D warnings`: PASS.
- Initial four-crate `cargo test --locked -j 4 -p nata-sched -p nata-parquet -p nata-exec -p nata`: test-summary totals of 279 passed, one failed (the synchronization facade check), two ignored, including subprocess checks. The facade violation was fixed through `nata-sched::sync`.
- Follow-up `cargo test --locked -j 4 -p nata-sched -p nata-exec`: test-summary totals of 165 passed, zero failed, one existing ignored test, including subprocess checks. Includes result equivalence / fresh execution IDs with stats and forced spill, trace bounds, aggregation and facade tests. This is not a claim that a final full four-crate command passed.
- Locked release build of `nata` and `nata-check`, `-j 4`: PASS.
- Thirteen verdict unittests, two completion-timer tests, profiler control test, both existing full inert harness self-tests and Python compilation: PASS. Nine Python source mutants CAUGHT.
- Focused Rust `cargo-mutants`: baseline PASS, **2/2 CAUGHT**, zero missed: remove `QueryStats::add`, remove `QueryStats::level`. Python verdict code is tested with the source mutation screen; cargo-mutants does not mutate Python.
- Full stats-on/off differential requirement: **NOT MET**, as above. Off-cost timing, ARM profiling and paid reference runs: **NOT RUN**.
- No lockfile change or new dependency. No push or merge. Local cargo jobs were serialized at `-j 4`. Stale incremental caches were deleted when free space crossed below 20 GiB; source and result files were preserved.

Commands for the focused Rust check (completed before the compatibility failure):

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_INCREMENTAL=0 RUST_TEST_THREADS=1 \
  /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false \
  --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --cargo-test-arg=query_stats \
  --output /tmp/package0-mutants -p nata-sched \
  -f crates/nata-sched/src/query_stats.rs -F 'add|level' --timeout 60
```

Committed parts before the stop: `65febcc` (shared timer / initial verdict policy),
`3458c52` (external profiler pre-check), `78d7d91` (gated job preparation).
The final passing harness refinements and this stop report are committed
separately; Rust instrumentation has no implementation commit because its
required gate failed.
