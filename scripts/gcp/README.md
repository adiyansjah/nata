# Running jobs on a Spot VM (GCP)

Runs `job.sh` on a cheap, reclaimable Spot VM and survives the VM being taken away. Nothing here runs by
itself: `orchestrate.sh run` needs an explicit dollar approval (see "Cost and safety"), and the rule in
`docs/way-of-working.md` (section 8, "Cloud spend") applies: ask first, show the cost, wait for a yes.

## How it survives a preemption

A Spot VM can be reclaimed with about 30 seconds' notice. So the VM is treated as disposable and all state
lives somewhere that outlives it:

| piece | where | survives preemption? |
|---|---|---|
| VM and boot disk | Spot, termination action DELETE | no (recreated) |
| toolchain, build cache, step markers, results | a separate 30 GB **data disk**, kept until the end of the run | yes |
| a copy of state, results and logs | your machine, `bench/gcp-runs/<run>/`, refreshed every 2 minutes | yes |

- **Steps** (`lib.sh`): every unit of work is a step with an atomic done-marker on the data disk. A step killed
  half way has no marker and simply runs again; a finished one is skipped. Long sweeps are split into shards so a
  preemption loses at most one shard. A step that *fails* is recorded and never retried, so a bug cannot loop and
  burn money. A benchmark is one step: if it is interrupted it restarts from the beginning, on one boot.
- **Orchestrator** (`orchestrate.sh`, on your machine): creates the data disk, then a VM, watches it, copies the
  state back, and when the VM vanishes it creates a new one on the same data disk; the startup script finds the
  source and `START` already on the disk and resumes the job. The source is uploaded once.
- **Machine chain:** `--machines "n4a-highcpu-8 c4a-highcpu-8"` is tried in order when a type is out of stock or
  over quota. A chain must use one disk class (the newer ARM families need Hyperdisk, T2A needs persistent disk).
  If no VM can be created in a zone and nothing has run there, the empty data disk moves to the next zone.
- **Preemption notice:** the VM polls the metadata server and sends the job SIGTERM, so it leaves clean markers.

## Cost and safety

1. `orchestrate.sh plan` prints everything a run would create and its **worst-case cost**, and calls nothing.
2. `orchestrate.sh run` refuses unless `--approved-usd` is at least that worst case. The worst case uses the
   dearest machine of the chain for all allowed VM-hours, plus disks, the external IP and a Hyperdisk allowance.
   Prices come from `prices.env` (with their source and date); recompute them before a run.
3. Total VM time is capped (`--max-vm-hours`), preemptions are capped (`--max-preemptions`), and each VM carries
   its own `--max-run-duration`, so it deletes itself even if the orchestrator dies.
4. Everything created is labelled `nata-run=<id>`. On every way out (success, failure, Ctrl-C, `kill`) the script
   deletes by that label, then lists to prove nothing is left, and writes `report.txt`. Resources of other runs
   or other work in the project are never touched. If the orchestrator dies, at most the data disk remains
   (`./orchestrate.sh cleanup --run-id <id>` removes it).
5. The VM has no service account and no scopes, project SSH keys are blocked, and login uses a throwaway key
   placed in the instance's own metadata, so the project's SSH settings are not changed.

## Use

```bash
scripts/gcp/orchestrate.sh plan --job ci                                  # look first; calls nothing
scripts/gcp/orchestrate.sh run  --job ci --approved-usd 0.25              # only after the human said yes
scripts/gcp/orchestrate.sh status --run-id <id>                           # what is still labelled
scripts/gcp/orchestrate.sh cleanup --run-id <id>                          # delete it all
```

Jobs: `ci` (workspace tests and the operator-protocol tests six times), `bench` (SF1 data, kernel benchmark three
times, pull-versus-push, v2), `cpack` (three jobs in one VM: `crates` = object_store, crossbeam-deque and the compression crates built and tested on this architecture; `fairness-bench` = the overhead of the time slice on a quiet machine; `bridge` = the tokio-to-pool stress and the I/O latency measurements), `perf` (perf profiles of the generic and fused Q1 and Q6; vm-startup.sh installs perf for it), `decimal` (the full exhaustive decimal verification with every mutant), `hashops` (the hash join and aggregate spike: tests, DuckDB diff, NULL check, mutation run, peak memory, benchmarks), `kani` (the Kani proofs, one harness at a time with a time cap, on a high-memory machine), `verify` (clean-machine reproduction of the verification: workspace checks, lint fixtures, guarded-expression diff and mutations, the loom model and its deliberate bugs, and the overflow-checks cost comparison), `genops` (the generic-operator experiment: unit tests, correctness checks, benchmarks, DuckDB timings), `all`. Edit `job.sh` to change them.

## `icebergmodel`: the targeted TLC re-run (item 24)

The first `iceberg` run (`iceberg-full-arm64`, `bench/results/20261104-iceberg-commit.md` section 8) used up its model budget:
`three-store-ser` timed out, and `three-rest-snap-i4`, `live-2`, `live-3` and the 11 canaries were never started. `icebergmodel`
runs only those 15 TLC runs, without repeating the 2 h of work that already has a result.

- **What it runs:** `spikes/iceberg-commit/gcp-check.sh` with `IC_MODE=model`: the phases `tools`, `model-lint`, `model-parse-and-edits`
  and `model-full` (restricted to the chosen runs), nothing else: no Rust build or tests, no catalog build or start, no spike, no
  interop, no `model-smoke`, no mutants. (The common `setup-rust` and `setup-duckdb` steps of every job still run: about 15 s.)
  Results go to `results/iceberg-model/` (`summary.txt`, `model-summary.txt` in the format of the full job, one `*.tlc.txt` per run),
  `iceberg-model-run.txt` and `iceberg-model-exit.txt`.
- **Verdict:** `summary.txt` ends with `VERDICT PASS` only if every requested run ended as expected. A `TIMEOUT`, an `ERROR`, a run that
  was `NOT-RUN` because the budget was spent, a result other than the expected one, or an unknown run name is a mismatch and gives
  `VERDICT FAIL` and `exit status: 1`. A PASS here covers only the runs asked for, not the phases of the full check.
- **Machine:** `n4a-highcpu-16` (16 vCPU, 32 GB; one TLC worker per vCPU, `nproc`). The TLC heap is 62% of the RAM but at least
  10000 MB: 10000 on the 16 GB `n4a-highcpu-8` (as before) and about 19800 MB on 32 GB. How much faster 16 workers are than 8 has
  NOT been measured; TLC does not scale linearly. The chain must be this one machine: there is no price in `prices.env` for another
  16-vCPU type, and the price of `n4a-highcpu-16` is DERIVED (cores x core price + GiB x RAM price), not fetched: recompute it first.
- **Environment variables** (defaults in brackets; the orchestrator forwards no environment, so put `export NAME=value` lines in
  `spikes/iceberg-commit/icebergmodel.env` before the run: it is uploaded with the source, ignored by git, and copied into the results):

  | variable | meaning |
  |---|---|
  | `IC_MODEL_ONLY` (the 15 runs: `three-store-ser`, `three-rest-snap-i4`, `live-2`, the 11 `canary-*`, `live-3`) | comma-separated run names, done in this order; `python3 spikes/iceberg-commit/model/run_model.py --list` prints all 24; an unknown name is an error |
  | `IC_TLC_TIMEOUT_BIG` (4500) | seconds for one three-writer run (`three-*` and `live-3`) |
  | `IC_TLC_TIMEOUT` (2700) | seconds for every other run (as in the full job) |
  | `IC_MODEL_BUDGET` (9000) | seconds for all runs together; each run is also cut to the time left, so the budget is not overrun by a run's limit; a run that cannot start (under 30 s left) is `NOT-RUN` |
  | `IC_MODEL_MUTANTS` (0) | 1: run the gating `model-mutants` phase after `model-full` (the full re-run after a model change, 2026-11-06) |
  | `IC_MODEL_HEAP_MB` (62% of RAM, at least 10000) | TLC heap |

- **Order:** the near-complete `three-store-ser` first, then `three-rest-snap-i4` (expected to end early in a violation of I4), `live-2`,
  the 11 canaries (each stops at its first violation), and the three-writer liveness run `live-3` last because it is the most
  expensive unknown. Costs of everything except `three-store-ser` are unknown (see the record); if the budget runs out, the rows at
  the end say `NOT-RUN`.
- **Preemption:** `IC_RESUME=1` makes a restarted job skip the runs that already ended as expected (their rows are marked
  `(resumed)`); the run that was interrupted starts again from its beginning.
- **Cost** (prices from `prices.env`, `n4a-highcpu-16` Spot $0.1968/h, derived; the `n4a-highcpu-8` run cost $0.224 for 2 h 17 min,
  i.e. $0.0984/h): the model budget is 9000 s = 2 h 30 min and the VM needs about 2 min more for tools, lint, parse and setup, so
  a run that uses the whole budget is about 2 h 35 min of VM time = $0.51 for the VM. The proposed cap is **3.0 VM-hours** (about 25
  minutes of margin): `scripts/gcp/orchestrate.sh plan --job icebergmodel --machines n4a-highcpu-16 --max-vm-hours 3.0` prints a worst case of **$0.646**
  (VM, external IP, boot and data disk, Hyperdisk allowance). The real cost is the VM time actually used at $0.1968/h plus about
  $0.02; if every run ends early it is lower. Run it only after the owner's yes:

  ```bash
  scripts/gcp/orchestrate.sh plan --job icebergmodel --machines n4a-highcpu-16 --max-vm-hours 3.0
  setsid nohup scripts/gcp/orchestrate.sh run --job icebergmodel --machines n4a-highcpu-16 --max-vm-hours 3.0 --approved-usd 0.65 > run.log 2>&1 < /dev/null &
  ```

- **Tests** (all local, no GCP, no TLC): `scripts/gcp/test/test-icebergmodel.sh` (the model mode end to end with a fake `java`),
  `scripts/gcp/test/test-job-chains.sh` (the job selects only its own step) and `spikes/iceberg-commit/model/test_model_tools.py`
  (`run_model.py`: per-run limits, the budget clamp, TIMEOUT and NOT-RUN as mismatches, unknown names, resume).

## `heavy`: any heavy run, on a machine sized for it (2026-11-07)

Heavy local work (fuzz at nightly size, mutant sweeps, long suites) made the 8-vCPU development VM run at load 30, so the owner decided
that heavy runs go to Spot VMs sized to the workload. `heavy` is the generic job for that. Write `heavy.steps` at the root of the tree
you run from (a worktree is fine; it is gitignored and never committed), one step per line, `NAME COMMAND...`:

```
fuzz-files NATA_FUZZ_SEED=20261107 NATA_FUZZ_CASES=30000 cargo test --release -p nata-parquet --features page-crc --test fuzz_files
sql-fuzz   cd spikes/sqlfuzz && NATA_BIN=$WORK/target/release/nata python3 -I fuzz.py run-subset --seed 20261107 --cases 100000 --jobs $(nproc) --target-cmd "python3 -I adapters/nata_adapter.py"
```

Each line is its own resumable step, run from the shipped tree, with its output in `results/NAME.txt` and its exit status in
`results/NAME.exit`. A failing command is a result, not a failed job: the other steps still run. Names use a-z, 0-9 and `-`.
Pick the machine for the work: `--machines c3d-highcpu-16` or `-32` (x86-64) or `n4a-highcpu-16`/`-32` (ARM64) for parallel CPU
work, 8 vCPU when the numbers must be comparable with earlier 8-vCPU baselines. Run the commands locally on small sizes first, then:

```
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-32 --max-vm-hours 1   # worst case about $0.29
scripts/gcp/orchestrate.sh run  --job heavy --machines c3d-highcpu-32 --max-vm-hours 1 --approved-usd 0.29
```

## `settle.sh`: timed blocks start on a quiet machine (2026-11-05)

`settle.sh LABEL [LIMIT=0.5] [MAX_SECONDS=600]` waits until the 1-minute load average is below the limit and prints one line,
`SETTLE <label>: settled, load 0.31 after 40 s (limit 0.5)` or `SETTLE <label>: NOT SETTLED, load 1.20 after 600 s (limit 0.5)`
(exit 0 or 1). The job carries on either way, so its other results are kept; a NOT SETTLED line is the record that the block after it is
SANITY ONLY. It replaces the inline loop of `job.sh` and is also called INSIDE the three blocks that missed the 0.5 rule on 2026-11-05:
between the phases of `spikes/null-path/run-all.sh` (`SETTLE_MAX=600` from the job; the default 0 only prints the load), before the
`bench` and the `membench` (engine-only) processes of the Arrow pipeline step, and in `spikes/push-cost/gcp-check.sh` (was: below 1.0;
the push-cost harness now also calls a start load of 0.5 or more SANITY ONLY, it was 2). Tests: `test/test-settle.sh`.

## `icebergspikes`: the code side of the Iceberg check without the model

`icebergspikes` runs `spikes/iceberg-commit/gcp-check.sh` in `IC_MODE=spikes`: tools, Rust build and tests, the Apache REST reference
fixture, the commit spike against it and against conditional create, and the pyiceberg interop. It skips every TLA+ phase (the model result
does not depend on the VM's architecture; it is in the `iceberg` and `icebergmodel` runs). `interop-pyiceberg` is GATING in this mode, so a
skip (the first run's setup defect) is a FAIL. Intended machines: `n4a-highcpu-8` and `c3d-highcpu-8` (x86-64). Tests: `test/test-icebergspikes.sh`.

## What has been verified, and what has not

- **Tested locally, with a fake `gcloud` and `ssh`** (`test/test-orchestrate.sh`, 27 checks, nine deliberate
  breakages of the script each caught): the approval gate; preemptions and resuming on one data disk; the machine
  chain; moving zones; a failing job; the VM-hour budget; `kill` and Ctrl-C; cleanup leaving other runs' resources
  alone; no call to the real `gcloud`. `test/test-steps.sh` tests the step runner against SIGTERM and SIGKILL.
- **Checked offline against the real `gcloud` help text:** every flag the script passes exists.
- **Verified on real Google Cloud** by two runs (`bench/results/20261104-arm64-run1-operator-protocol.md`, `20261104-arm64-run2-benchmarks.md`): Spot creation of N4A and C4A VMs despite `PREEMPTIBLE_CPUS = 0`, Hyperdisk boot and data disks, the startup script mounting the data disk, SSH with the throwaway key, upload, resume markers, state copy-back, and label-based cleanup. Jobs took 3 and 6 minutes.
- **Preemption recovery verified** by a deliberate loss of the VM mid-job (`bench/results/20261104-arm64-run3-preemption-test.md`): a new VM on the same data disk resumed the job, with the interrupted step run again and nothing else repeated.
- **Not yet exercised on real Google Cloud:** the 30-second notice path (the hard loss tested gave no warning), the machine fallback chain, moving zones, `kill` mid-run, and whether Hyperdisk's default IOPS and throughput stay within the free baseline (the cost bound includes an allowance for that).

## When the machine that runs the orchestrator goes down (found 2026-11-04)

`orchestrate.sh run` is an ordinary process on the machine that started it. If that machine reboots or its session is
restarted, the orchestrator dies with it, and nothing pulls results or deletes the VM and data disk. The job itself keeps
going on the VM (it is started by the VM's startup script and writes its own step markers), and the VM's own
`--max-run-duration` deletes the VM at the limit, but the labelled DATA DISK stays and keeps billing until someone deletes it.
What to do, in this order (all of it was done by hand once, for the run `nata-1104-172548-4307`):

1. Look: `gcloud compute instances list` and `gcloud compute disks list` (every resource carries the label `nata-run=<id>`).
2. Is the job done? ssh to the VM with the run's key (`bench/gcp-runs/<id>/key`, user `nata`) and read
   `/mnt/work/state/job.finished` or `job.failed` and the step markers.
3. PULL BEFORE YOU DELETE: `ssh ... 'cd /mnt/work && tar -czf - state results logs' | tar -xzf - -C bench/gcp-runs/<id>/final`.
   Never delete first: the data disk is the only copy.
4. Delete: `scripts/gcp/orchestrate.sh cleanup --run-id <id>`; then list again to prove nothing is left.
5. Write the cost down by hand: the report is only produced by the orchestrator (VM seconds x the rate in `prices.env`).

Two related defects found the same day and fixed: the orchestrator pulls `state`, `results` and `logs` every two minutes, so
a job must NEVER put large files under `$RESULTS` (a 3.7 GB results directory made the pull time out, the orchestrator called a
healthy VM `stuck` and recreated it); and the run id now carries a random suffix because runs started in the same second
shared one directory and one label. `scripts/gcp/orchestrate.sh recover --run-id <id>` (added 2026-11-06) does steps 1 to 5 itself: it lists the run's resources,
pulls state, results and logs from a running VM into `bench/gcp-runs/<id>/` BEFORE anything is deleted, reads the job's state, deletes
only when the job has ended (a running job, a VM it cannot pull from, or a data disk without a VM is kept, exit 5; `--force` deletes
anyway), and writes `report-recover.txt` with a VM cost estimate from each VM's creation time (a lower bound: VMs of the run that are
already gone are not counted). Tested with the fake `gcloud` in `test/test-orchestrate.sh`.

## Start `run` detached, never as a child of an interactive session (found 2026-11-04, run `failmodel-arm64`)

An orchestrator started as a background child of a chat or terminal session is killed when that session is interrupted or torn down.
It died mid-cleanup: the VM was deleted, the data disk was not, and the job (50 minutes in, with the longest model check still running)
was lost. Start long runs as `setsid nohup scripts/gcp/orchestrate.sh run ... > log 2>&1 < /dev/null &` so they survive, and then watch
the log or the process; the recovery steps in the section above apply if one dies anyway.

## The project's CPU quota (found 2026-11-06)

The project allows 96 vCPUs in total across all regions (`CPUS_ALL_REGIONS`). Runs started in parallel share it: with a 64-vCPU run and an
8-vCPU run going, a 32-vCPU VM is refused ("Quota 'CPUS_ALL_REGIONS' exceeded. Limit: 96.0 globally"), in every zone. Plan a wave so that
its machines (peers of a multi-VM run included) add up to at most 96 vCPUs, or start the large run after the others have ended.

## `slice4close`: SQL slice closeout

Run on `n4a-highcpu-8` and `c3d-highcpu-8`: SF1/SF10, all SQL/front-end/executor/CLI
release tests, 3,000 injected fault/delay scenarios and 2,000 process scenarios under
CPU contention, the scan/SQL/differences differential sets, then `sql-bench.py` and the
leak scan. Every timed block settles independently. Scratch is on the data disk;
resource probes preserve the fresh VM's descriptor limit. Large fixtures stay outside
results. See `crates/nata-bench/sql-bench.py` for the alternating arms and included costs.
`eng-checks` additionally covers SQL, execution and CLI sanitizers, SQL pushdown and
executor scalar tests under Miri, dependency checks, unsafe inventory and leak scan.
Miri does not run process/signal/file-decoding tests; ASan and TSan cover those.
The two RSS-budget tests run separately without instrumentation: sanitizer quarantine
and shadow metadata are outside the normal allocator budget. Sanitizer defaults
remain enabled; the remaining CLI and resource tests still run instrumented.

`crates/nata-bench/regress.sh BASELINE NEW [fraction=0.20]` compares machine-specific
`metric,median_secs` CSVs. Missing metrics and invalid samples fail. There is deliberately
no CI job: shared CI runner noise makes it unsuitable for this performance gate.
The stored baseline, observed twin spread and fail/pass demonstrations belong beside
results. `test-regress.sh` exercises passing, slowdown, missing and invalid inputs.

After a completed `slice4close` run, reduce its two raw benchmark directories:

```bash
python3 crates/nata-bench/regression-summary.py RESULTS_DIR new-medians.csv
crates/nata-bench/regress.sh crates/nata-bench/baselines/slice4-arm64.csv new-medians.csv
```

Use the baseline for the same CPU type; `slice4-x86.csv` is the other baseline.
The reducer gates the planning corpus median, SQL example/full-execution medians,
and the scan benchmark at one/eight threads on SF1/SF10. The full benchmark
summary still checks each query's planning target; this reducer does not replace
those per-query verdicts. Its optional final argument selects SQL rounds, used
for the closeout's disjoint baseline/held-out demonstration. Never regenerate a
baseline merely to accept a slowdown; explain it and record the decision.


## `pgwirecheck`: slice 4b local reliability replay

Runs the release pgwire suite, 2,000 lifecycle scenarios under `nproc` busy-loop
competitors plus 10,000 live fuzz cases (seed 322661912839), and all private-copy
mutants. Results are `pgwire-protocol.txt`, `pgwire-live.txt`, and
`pgwire-mutants.txt`. Generated Parquet and TMPDIR stay on the work disk.
No client dependency installation, performance benchmark, or cloud run is claimed
by adding this job. See `bench/results/20261107-pgwire-reliability.md` for the
exact approved-run commands and the remaining step-5 measurements.

## `pgwirebench` and `item35`: slice 4b measurements

`pgwirebench` builds the release server and CLI, checks one million ordered BIGINT
rows against DuckDB/raw/psql, then times five alternating warm-cache rounds with
identical twin arms at one/eight workers. Each timed block requires load below
0.5. It records streaming ratios against the fixed 1.20 ceiling, first DataRow
versus producer completion (including bounded-bridge backpressure), 64 concurrent
versus sequential connections (2.0 ceiling), fixed admission share, and named/text/
binary parameter roundtrip costs. Queue wait is measured only around semaphore
acquisition; `worker_share` is the configured share, not CPU utilization. The
permanent one-byte/ms test records peak RSS and produced rows separately. Results
are small JSON/CSV/text files; generated Parquet and build caches stay on WORK.

`item35` installs distribution psql/JDK and official Maven's latest stable 42.x
pgjdbc, plus PyPI psycopg binary, pgcli, ADBC PostgreSQL and pyarrow. It records
versions, runs nata-check's SQL oracle, all successful SQL corpus queries through
each default client, native type/preparation/error/cancel probes, and a transparent
frontend trace that omits cancel secrets and parameter payloads. Exact text
mismatches remain recorded; a second typed comparison distinguishes PostgreSQL
boolean/float spellings and native client formatting from incorrect values.
Default ADBC catalog bootstrap failures are recorded without emulating catalogs
or introducing COPY. Neither job launches cloud resources itself; plan each
architecture before an explicitly approved run.

The pgwire benchmark writes `bench/medians.csv`. Compare it with the same CPU's
`crates/nata-bench/baselines/pgwire-arm64.csv` or `pgwire-x86.csv` using `regress.sh`
and the unchanged 20% tolerance. Closeout retains the primary/twin comparison and
a planted 30%-lower baseline; a noisy twin failure is retained as a failure.
The compatibility corpus uses TPC-H SF0.01 plus the full SQL fixtures so that
multi-million-row CSV scratch never enters pulled results. This is separate from
the benchmark's exact one-million-row fixture.
