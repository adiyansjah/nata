# Card 0008 package 0 — implementation checks, 2026-11-08

**Incomplete; not a performance certification.** Slice 5 remains pinned to
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

Measured checks: nine verdict unittests pass; eight deliberate Python verdict
mutants CAUGHT (slowdown, noise, vote count, missing stats, drops, floor direction,
round pairing and failed denominator). Both full inert harness self-tests pass
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
cannot mutate Python; the eight-source-mutation screen above is its applicable
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
records their exact attempted commands/errors when run there. No working ARM
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
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-x86 --approved-usd APPROVED_X86_BOUND
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --data-gb 150 --max-vm-hours 6 --run-id pkg0-baseline-arm --approved-usd APPROVED_ARM_BOUND
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
