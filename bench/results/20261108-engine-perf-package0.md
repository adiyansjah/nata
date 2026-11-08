# Card 0008 package 0 — implementation checks, 2026-11-08

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
