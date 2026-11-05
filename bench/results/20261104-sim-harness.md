# Deterministic simulation harness before any distributed code (verification-plan item 23)

Date: 2026-11-04. Code: `spikes/sim-harness/` (throwaway, own workspace). Edited nothing else.

**Status: written and compile-checked only. NOTHING has been run.** The owner's rule is that every run happens on GCP and this
box was limited to `cargo check`. Every statement below is therefore labelled **measured** (a command was run and its output read: only
crates.io/GitHub queries, `wc -l`, and `cargo check` here), **read** (taken from the source of the exact crate versions) or
**reasoned** (an argument, not yet checked by a run). **GCP results: to be filled** (section 11).

## 1. Question and provisional verdict

Should Nata be written against a small runtime abstraction with a REAL and a SIMULATED implementation, so that distributed bugs are
reproducible from a seed?

**Provisional verdict (reasoned; confirmed or overturned by the GCP run):** yes, and build the small part ourselves:

- **Node-level logic** (coordinator, workers, exchange over the network, object store access, retries, cancellation) is written
  generic over one small trait, `Env` (`sim/src/env.rs`, 62 lines: time, randomness, network, object store, task spawning, fault
  points, observation). Two implementations exist in the spike: a deterministic simulator and a real one (threads, wall clock,
  files).
- **The in-process scheduler and exchange** (`spikes/operator-trait`, `spikes/exchange-loom`) stay synchronous. In a simulated node
  a CPU fragment runs to completion inside one poll (the spike runs the real scheduler and the real exchange that way). Thread
  interleavings inside a fragment are a different problem, already covered by loom; shuttle is the candidate to scale that to bigger
  fragments (trial below).
- **Do not make madsim or turmoil the foundation.** madsim cannot run our scheduler inside a node (it forbids OS threads, and
  our design needs them), and it requires the whole dependency tree on its tokio fork; turmoil does not simulate files, the
  object store or threads, and only the `turmoil::net` types. Both stay candidates for tests of code that is already tokio-shaped.
- shuttle is the right tool for the scheduler/exchange thread level and not an alternative to the node-level harness.

What would change this: the GCP run showing that the hand-written harness is slower or leakier than claimed, a mutant it cannot
find that madsim or turmoil would, or a decision to put the engine on tokio's multi-thread runtime (then madsim's tokio fork or turmoil
become more attractive, and the `Env` trait would need `Send` futures).

## 2. The build-or-buy comparison

"Same toy": the toy protocol of section 4 is written once, generic over `Env` (`sim/src/protocol.rs`). The madsim and turmoil trials
implement `Env` for their framework and run the SAME protocol code on the SAME seeds (the scenario generator, the invariant
oracle and the object-store contents are shared). The shuttle trial runs the real scheduler and exchange.

### Size (measured, `wc -l`, 2026-11-04)

| piece | lines | what it is |
|---|---|---|
| hand-written executor, virtual time, timers, mailbox (`exec.rs`) | 608 | the runtime both implementations share |
| `Env` trait (`env.rs`) | 62 | the abstraction |
| simulated runtime: network faults, store, crash/restart, fault points (`sim.rs`) | 539 | the simulated implementation |
| real runtime: threads, wall clock, files (`real.rs`) | 259 | the real implementation |
| seeded RNG + trace hash (`rng.rs`, `trace.rs`) | 47 + 72 | |
| **madsim trial** (`Env` over madsim + driver, `trials/madsim-trial/src/lib.rs`) | 259 | plus 94 lines of shared helpers (`trialsupport.rs`) |
| **turmoil trial** (`Env` over turmoil + driver, `trials/turmoil-trial/src/lib.rs`) | 241 | same shared helpers |
| **shuttle trial**: the `sync` shim (the scheduler already had one for loom) + two small helpers | 27 + 82 | |

The adapters are small because the protocol is generic over `Env`; most of the madsim and turmoil lines are the scenario driver and
the object store they do not have. The hand-written simulated side is 1,328 lines (executor 608, simulated runtime 539, trait 62, RNG 47, trace 72) plus 259 for the real
runtime; it is the only candidate that is also the REAL runtime's executor and that has the object store and the named fault points.

### What each can and cannot express

| | hand-written | madsim 0.2.34 | turmoil 0.7.2 | shuttle 0.9.9 |
|---|---|---|---|---|
| drives `Operator`/scheduler/exchange from operator-trait and exchange-loom | yes: the real scheduler runs inside a simulated worker (`engine.rs`, 1 OS thread, run to completion) | **no by default**: `pthread_attr_init` is interposed and refuses OS threads (read: `sim/task/mod.rs`), unless `MADSIM_ALLOW_SYSTEM_THREAD=1`, which warns "may cause non-determinism". The trial's `probe-thread` runs it both ways | threads are not controlled (read: nothing in the crate intercepts them); tokio tasks only | **yes**, through the existing `sync` shim: only `sched.rs` and `exchange.rs` follow the shim; `ops.rs` mixes `std::sync::Mutex` with `sched::lock` and could not be included (15 type errors, measured with `cargo check`) |
| thread interleavings inside a fragment | no (outside the simulator; loom/shuttle cover them) | no | no | yes, seeded, replayable (`check_random_with_seed`, `replay`) |
| virtual time | yes | yes (interposes `clock_gettime`) | yes (tokio paused time, fixed tick; `Builder::epoch` defaults to `SystemTime::now()`, read: `config.rs`) | no (`thread::sleep` is a no-op yield) |
| network faults | latency, jitter (reorder), drop, duplicate, corrupt, partition, crash/restart | latency range and loss rate only (read: `net::Config`); clog node/link; kill/restart | latency range, loss (`fail_rate`), partition, hold/release, crash/bounce; no duplicates | none |
| object store with failures | yes (latency, failure, "applied but the reply was lost") | no (a `fs` sim exists; the trial uses a shared store with madsim-timed latency) | no (a `fs` shim exists behind a feature) | no |
| named fault points ("crash this node when it sends its last frame") | yes (`fault_point`, `CrashPoint`) | not mapped (nearest: `buggify`) | no | no |
| real runtime with the same protocol code | yes (`real.rs`, same `protocol.rs`) | std mode re-exports tokio (`#[cfg(not(madsim))]`): real code runs on tokio | tokio | normal std |
| needs the dependency tree to use its types | no | **yes**: `madsim-tokio` replaces tokio everywhere, `--cfg madsim` for the whole build | `turmoil::net` replaces `tokio::net`; HTTP clients need a turmoil connector | the `sync` shim only |
| Arrow, file I/O | not involved (`std::fs` only in the real runtime) | `madsim::fs` for files; Arrow is plain Rust and runs | `turmoil::fs` behind a feature | plain Rust runs |

### Maintenance state (measured 2026-11-04 from the crates.io and GitHub APIs)

| crate | last release | repository activity | open issues | licence |
|---|---|---|---|---|
| madsim (https://crates.io/crates/madsim, https://github.com/madsim-rs/madsim) | 0.2.34, 2025-10-11 (13 months old); `madsim-tokio` 0.2.30, 2024-10-04 | last push 2026-02-16 ("bump tonic to 0.14 and etcd-client 0.18"); before that 2025-12-03, 2025-11-25 | 27 issues (33 with pull requests) | Apache-2.0 |
| turmoil (https://crates.io/crates/turmoil, https://github.com/tokio-rs/turmoil) | 0.7.2, 2026-04-24 (0.7.1 2026-01-29, 0.7.0 2025-11-03) | last push 2026-10-27; commits 2026-10-14, -16, -20 | 6 (16 with pull requests) | MIT |
| shuttle (https://crates.io/crates/shuttle, https://github.com/awslabs/shuttle) | 0.9.9, 2026-10-29 (six releases between 2026-09-22 and 2026-10-29); now split into `shuttle-engine`, `shuttle-std`, `shuttle-schedulers` | last push 2026-11-04 (today) | 45 (70 with pull requests) | Apache-2.0 |
| loom (reference, used by the existing models) | 0.7.2, 2024-04-23 | | | MIT |

Reading: shuttle is the most active, turmoil is maintained, madsim is slowing (last release 13 months ago, last commit 9 months ago).
Not an argument against madsim by itself, a risk to carry if the engine were built on it.

### Pick

The hand-written executor behind the `Env` trait, plus shuttle (and the existing loom models) for the scheduler/exchange thread
level. **Reasons:** (1) it is the only candidate that is also the real runtime's executor and so runs the same protocol code in both
modes; (2) it has the object store and named fault points the failure model needs and neither framework has; (3) it needs no change
to dependencies, while madsim forces its tokio fork on everything and turmoil forces its socket types; (4) madsim cannot run our
scheduler (read from the source; to be confirmed by the trial's `probe-thread` on GCP); (5) the cost is about 1,300 lines of
std-only code that we own (plus 260 for the real side). **Costs (honest):** we maintain it; it does not simulate arbitrary `tokio`/`hyper`/`object_store` code
(an `object_store` client must sit behind our `Env::store_*` calls, i.e. behind our own small trait; real S3 code is exercised
only by the real runtime); `async fn` in a trait is not dyn-compatible, so node code is generic (compile time, monomorphisation).

## 3. The abstraction (what must be injected)

`Env` (`sim/src/env.rs`) is everything a node may touch that is not pure computation:

| capability | `Env` method | simulated | real |
|---|---|---|---|
| time | `now()`, `sleep()`, `recv_timeout()` | virtual clock, jumps to the next timer | `Instant` since a shared start |
| randomness | `rand()` | per-node seeded stream | per-node seeded stream (not OS) |
| network | `send()` (fire and forget), `recv_timeout()` | latency+jitter, drop, duplicate, corrupt, partition, crash | in-process mailboxes between node threads |
| object store | `store_get()`, `store_put()` | latency, failure, "applied but reply lost" | files in a directory, atomic rename |
| task spawning | `spawn()` returns an abortable `TaskCtl` | tasks belong to a node; crash drops them | executor per node thread |
| crash points | `fault_point(name)` | `CrashPoint` kills a node at the nth hit | nothing |
| observation | `event()`, `track()` (leak counter), `gauge_set()`, `violation()` | hashed into the trace; oracles read gauges | ignored / collected |
| thread-local and global state | none allowed | | |

## 4. The toy protocol and its invariants

Coordinator (node 0) and three workers. The coordinator splits a scan into one task per input object; workers read the object
(retrying store errors), run the REAL operator protocol on it (`scan -> filter -> real exchange -> map -> collect`, the real
scheduler with one thread), and stream the result back in **frames** (`magic | kind | len | payload | crc32`) over the unreliable
network with sequence numbers, a **sliding window** (backpressure), cumulative acks and retransmission. Workers heartbeat; a silent or
restarted worker's tasks are **retried as a new attempt** on another worker (attempts emit their rows in a different order, so mixing
two attempts is detectable). The client may **cancel**; the coordinator waits until every reachable worker has dropped its tasks, then
shuts the workers down. A worker exits by itself after a lease period without news from the coordinator. The result is written to the
object store (rows, then the manifest as commit marker).

| invariant | checked | by |
|---|---|---|
| no result row lost or duplicated | end of run: committed rows equal the expected multiset; manifest consistent | `scenario::verify_result` (shown to fail on a lost row, a duplicate, a bad manifest: unit test `the_result_oracle_can_fail`) |
| every node terminates | the run ends with no task alive; hang, deadlock and livelock are reported | `RunEnd`, `scenario::check` |
| cancellation reaches every node | at the instant the coordinator declares the cancel complete, every worker that is up has seen it | `SimEnv::gauge_set("cancel_done")` |
| no task output counted twice after a retry | the rows oracle, plus the attempt check | rows oracle |
| no leaked tasks or buffers | counted resources (`Tracked`) are zero when every node has ended; no task alive | `World::buffers`, `Executor::live_tasks` |
| backpressure window never exceeded | by the sender after every send round, and by the receiver on every frame | `violation()` in `protocol.rs` |
| no frame accepted from a superseded attempt | by construction + the rows oracle | |

Eight scenario families, chosen by `seed % 8` (so any 8 consecutive seeds cover all): calm; lossy network (drop, duplicate, corrupt);
worker crash (restart never, soon or after the query is over); crash after the first frame; crash at the final frame; partition
(healed); cancel (with a crash at the instant the cancel is sent); store faults plus heavy loss.

## 5. Determinism, how it is shown

`simctl hashes <first> <count>` prints the FNV-1a 64 hash of the whole event trace of each seed (every message sent and delivered,
every task poll with its position in the seeded run queue, every protocol decision, with virtual time). `gcp-check.sh` runs it three
times in separate processes for the release build and three times for the debug build and requires all six outputs byte-identical,
runs each seed twice in-process with a different seed in between (catches global state), requires distinct hashes for distinct
seeds, and writes `trace-hashes-<arch>.txt`. **Compare that file between the ARM64 and the x86-64 run: it must be byte-identical.**
The check is shown to fail on three breakages (mutants `det_*`): HashMap iteration order, the wall clock in the trace, a process-global counter (caught only by
the in-process repeat). Integers only (probabilities in parts per million), no floats.

## 6. Replay and bug finding

`mutants/run_mutants.py` applies each mutant to a COPY (fresh mtimes via `shutil.copy`, never `copy2`), builds it, checks that its
binary differs from the control's and from every other mutant's (a stale build is INVALID), runs `simctl sweep 0 <seeds>`, prints the
first failing seed and REPLAYS it (`simctl replay <seed>`): the failure line must be identical. The real tree is hashed before and
after. The unmutated control must pass every seed and exercise every fault path (`--require-coverage`). `mutants/test_mutants.sh` tests the
driver itself: two invocations in a row in one work directory both pass their baseline; a baseline after mutants passes; the stale
build defect is reproduced (or not) and the fix works.

| mutant | the bug | expected to be found by (reasoned) |
|---|---|---|
| retry_counts_output_twice | a retried task keeps the rows of its abandoned attempt | rows oracle, families crash-after-first-frame, worker-crash |
| cancel_skips_restarted_worker | a worker restarted during the cancel is taken for already cancelled | cancel-reach oracle, family cancel (crash at the instant of the cancel) |
| window_off_by_one | one frame too many in flight | sender and receiver window checks, every family with jitter |
| lost_wake_when_window_full | no retransmit timer while the window is full: a dropped message is never repaired | hang, every lossy family |
| no_retry_when_last_frame_seen | a crash during the final batch is not retried when the last frame had already arrived | hang, family crash-at-final-frame (needs a lost earlier frame) |
| leak_at_shutdown | a worker forgets its buffers when it cancels or shuts down | leak oracle, family cancel |
| stale_attempt_accepted | frames of a superseded attempt are accepted | rows oracle, family partition |
| duplicate_frame_applied | a duplicated frame is applied twice | rows oracle, family lossy |
| store_error_means_empty_input | a transient store error is read as an empty file | rows oracle, family store-faults |
| cancel_done_without_acks | the cancel is declared complete before any worker acknowledged | cancel-reach oracle |
| det_hashmap_iteration_order, det_wall_clock_in_trace, det_global_counter | determinism rules broken | the determinism check |

**None has been run.** The detection column is an argument from the scenario design, not a result. If the GCP run shows a survivor,
the likely cause is a scenario that does not make the trigger common enough (the sweep reports, per family, how many seeds fail).

## 7. Cost of the abstraction (design; to be measured on GCP)

`sim/src/bin/simbench.rs`. The existing in-process path (producer pipeline -> real exchange -> consumer pipeline, one tiny batch per
morsel, so every batch crosses the exchange and many park and wake) is timed: `direct` (as today), `generic` (the exchange wrapped by
`HookedSink`/`HookedSource<RealHooks>`: one clock read and one event call per batch per side, static dispatch), `dyn` (the same through
`dyn Hooks`), `nohooks` (the wrapper alone: indirection without the clock read), and `control` (`direct` again: the noise floor). 1 and 4 worker threads
(1 and 4 tasks per pipeline), 15 rounds, arms interleaved in rotating order, each run's rows checked (count and sum) before its time counts.
Reported per arm: min, quartiles, median in ns per batch; the paired difference against `direct` per round with median and quartiles; the control arm's
difference as the noise floor. **Self-test:** an arm with a known cost (2 x 100 ns per batch, one thread) must be detected, or the benchmark
is INVALID and the script exits non-zero. Where the abstraction sits on the hot path is itself a result: the scheduler does not call the clock
or the network per batch; the exchange boundary is where a distributed engine must (reasoned). The number is for the wrapper at that boundary, not for
"the abstraction" in general; see "Not covered".

## 8. Design note for an ADR: interface, what violates it, rules

Interface: section 3. What in the current spikes violates it (measured: `grep` of `spikes/` on 2026-11-04):

| violation | where | consequence |
|---|---|---|
| wall clock in the scheduler: `Instant::now()` for the time slice | `spikes/fairness/src/sched.rs:575` (also `load.rs`) | the slice must come from an injected clock |
| `std::thread::spawn` and a Condvar pool inside `Query::start` | `spikes/operator-trait/src/sched.rs` (`worker`, `start`), same in `exchange-loom`, `fairness` | cannot run inside a single-threaded simulator; run-to-completion fragments or an injected pool |
| `thread::sleep` and a global `static TEST_WORKER_DELAY_US` | `operator-trait/src/sched.rs:24,364` | global mutable test state: harmless in single tests, forbidden in simulated nodes |
| `HashMap` iteration order (join table, aggregate) decides output order | `operator-trait/src/ops.rs` (11 uses), `fairness/src/ops.rs`, `arrow-pipeline/src/{sinks.rs,genops/engine.rs}` | batch order differs per process; fine only where the consumer sorts or the order is not observable |
| file I/O for spill: `std::fs` | `operator-trait/src/ops.rs`, `fairness/src/ops.rs`, `arrow-pipeline` | storage must go through the injected store/file API |
| tokio used directly: runtime, `tokio::time::sleep`, `spawn` | `spikes/async-bridge/src/harness.rs` | node code must not call tokio; the real `Env` may |
| `thread_local!` and `static` in the channel | `spikes/async-bridge/src/chan.rs`, `fairness/src/chan.rs` | per-thread hidden state |
| `available_parallelism` | `fairness` examples and tests | core count is an input, not a fact of the machine |

Rules for developers (draft):

1. Node-level code (anything that talks to another node, the store, or a timer) is generic over `Env` and `await`s only `Env` futures.
2. No `std::time::{Instant,SystemTime}`, `thread::spawn`, `thread::sleep`, `std::net`, `std::fs`, `tokio::{spawn,time,net}` in node-level code.
3. No `HashMap`/`HashSet` iteration where order can reach a message, an event or a result; use `BTreeMap`, or sort. (Lookup is fine.)
4. Randomness only from `Env::rand`; never `rand::thread_rng`, `RandomState`, or an OS source.
5. No `static mut`, `thread_local!`, `lazy_static`, or process-global counters in node-level code; a seed must be the complete description of a run.
6. Choices that depend on probabilities use integers (parts per million); no floats in decisions that reach the trace.
7. CPU fragments (operators, scheduler) are synchronous, take no `Env`, and are deterministic as a function of their input (output order-insensitive, or sorted); their thread interleavings are checked by loom/shuttle.
8. Every external effect has a named fault point or a store/network failure mode a test can inject; every retry path has a scenario that reaches it.
9. A protocol invariant is asserted inside the code that owns it (`violation()`), and has an oracle outside; both are shown to fail on a mutant.
10. A sweep failure is only done when its seed replays to the identical failure line and becomes a regression test with that seed.

## 9. What was checked on this box (measured) and what was not

Run here, through `scripts/local-heavy.sh`, one at a time: `cargo check -p nata-sim --tests --examples` (lib, 2 binaries, 4 test files), `cargo check -p shuttle-trial` (default and
with `--features bug-lost-shutdown`), `RUSTFLAGS="--cfg madsim" cargo check -p madsim-trial`, `cargo check -p turmoil-trial`, and a `cargo check` of the control and of
each of the 13 mutated copies (all compile; `mutants/run_mutants.py --check-anchors`: every anchor found exactly once). Also run: `wc -l`, the crates.io and GitHub API queries of section 2, `cargo fetch`.
**Not run: any test, simulation, sweep, benchmark, mutant, build to completion.** `bash -n` on the shell scripts, `python3 -I run_mutants.py --list`.

Could not be checked without running (everything else of the deliverables):
- that the control passes all seeds (the protocol was reasoned through, not run: a bug in it would show as a control failure with a seed to replay);
- that each mutant is caught, and how many seeds catch it; that every fault path is exercised (the `--require-coverage` list);
- the determinism claims, in-process, across processes, debug versus release, ARM64 versus x86-64;
- the unit tests (executor, framing, simulated network and store, protocol, real runtime), the real-runtime runs (timing-dependent, 1-2 s each);
- the sweep speed (the real scheduler spawns an OS thread per fragment: seeds per second unknown) and the whole run's duration;
- the benchmark numbers, the noise, and whether the self-test detects the injected cost;
- the trials: whether the shuttle control passes and finds the known scheduler bug with an identical replay; what madsim and turmoil do with the toy;
- that the mutation driver's stale-build test reproduces the defect (it may not on this cargo version; the fix is checked either way).

## 10. How to run on GCP (needs the owner's yes)

Machine and cost (estimate; prices from `scripts/gcp/prices.env`, Spot, us-central1, fetched 2026-11-03/04; recompute with `plan` first):
n4a-highcpu-8 (ARM64, 8 vCPU, 16 GB) $0.0984/h; c3d-highcpu-8 (x86-64, 8 vCPU, 16 GB) $0.0687/h. **Expected duration 35-50 minutes**
(reasoned from builds of about 8 minutes including the three trial crates, the sweeps, 13 mutant builds and sweeps, the mutation driver's own test, and about 2 minutes of benchmark), so about **$0.05-0.10 of VM time per machine**
plus disks and the external IP that `plan` adds; `--max-vm-hours 1.5` caps it. Not confirmed by a run.

`scripts/gcp/job.sh` has no job for this yet and this task was not allowed to edit it. Add this stanza next to the others:

```bash
  simharness)
    # spikes/sim-harness: build, unit tests, determinism, control sweep, mutants, trials, benchmark (verification-plan item 23).
    run sim-harness bash -c 'cd "$SRC/spikes/sim-harness" && ./gcp-check.sh "$RESULTS/sim-harness" "${SIM_SEEDS:-20000}"' ;;
```

Then (look first; `plan` calls nothing):

```bash
scripts/gcp/orchestrate.sh plan --job simharness --machines "n4a-highcpu-8" --max-vm-hours 1.5
scripts/gcp/orchestrate.sh plan --job simharness --machines "c3d-highcpu-8" --max-vm-hours 1.5
# after the owner's yes, with --approved-usd at least the bound that plan printed:
scripts/gcp/orchestrate.sh run --job simharness --machines "n4a-highcpu-8" --max-vm-hours 1.5 --approved-usd <bound> --out bench/gcp-runs/simharness-arm64
scripts/gcp/orchestrate.sh run --job simharness --machines "c3d-highcpu-8" --max-vm-hours 1.5 --approved-usd <bound> --out bench/gcp-runs/simharness-x86
```

On any clean Debian VM without the orchestrator: install rustup, put the repo there, then
`spikes/sim-harness/gcp-check.sh /tmp/sim-results 20000` (the script `cd`s into the repo so `rust-toolchain.toml` applies, and exports `RUSTUP_TOOLCHAIN` for the builds it makes outside it).
Results: plain text in `<results-dir>` (`summary.txt` first); the exit status is 0 only if everything passed. The load average is printed at every step and the
benchmark says TIMING SANITY ONLY when it is 2 or more. Seeds: second argument (default 20,000); `TRIAL_SEEDS` (default 400) and `SHUTTLE_SEEDS` (default: the seeds) tune the trials.
After both runs: `diff` the two `trace-hashes-*.txt` files (they must be identical).

## 11. GCP results: to be filled

Fill in per machine (ARM64 n4a-highcpu-8, x86-64 c3d-highcpu-8): run date and machine record; unit tests; real runtime; determinism (six runs identical?
`HASHDIGEST` equal across architectures?); control sweep (pass count, coverage, replay); each mutant (caught, first seed, replay identical, failing seeds per
family); mutation driver test; shuttle (control, known bug, replay); madsim (`probe-thread` both ways, sweep, determinism); turmoil (sweep, determinism);
benchmark (medians, quartiles, noise floor, self-test, load average); whole-run duration and cost.

## 12. Decisions needed from the owner

1. **Run it** (about $0.10 per machine, 35-50 minutes, two machines), and add the `simharness` stanza to `scripts/gcp/job.sh`.
2. Accept, once the numbers are in, the rule that **all node-level distributed code is generic over a small `Env` from the first line** (cost: generic code, no dyn; the
   alternative is to retrofit the interface later, which every spike so far suggests is the expensive path).
3. Whether the **real implementation** should stay one single-threaded executor per node (this spike) or become tokio's multi-thread runtime (then futures must be `Send` and the
   simulator's `Rc` state changes). This decides how close the real and the simulated runtime can be.
4. Whether madsim or turmoil are worth a second look for tests of **tokio-shaped** code (the real `object_store` client against a simulated object store): neither covers it without
   its types in the dependency tree. Not part of this verdict.

## Not covered

- The real engine's `tokio` code, `object_store`, and HTTP are not simulated; a store client must sit behind `Env::store_*`.
- Thread interleavings inside a fragment are outside the simulator (loom; shuttle trial on a small query only).
- The coordinator never crashes in the toy (coordinator failure is item 29); only workers crash, restart, are partitioned, and the client cancels.
- Network model: datagram faults, no connection semantics, no bandwidth, no clock skew between nodes, no disk faults (torn writes) beyond the store's failures.
- The benchmark measures the wrapper at the exchange boundary on one machine type per run, not the cost of `Env` in production node code.


## GCP results (2026-11-05)

Runs: `simharness-arm64-r2` and `simharness-x86-r2` (everything but a quiet benchmark), then `simharness-arm64-r3` and `simharness-x86-r3` (the final code, benchmark on a settled machine), one `n4a-highcpu-8` (Neoverse-N3) and one
`c3d-highcpu-8` (AMD EPYC 9B14, 4 cores x 2 threads) Spot VM each, 5-6 minutes and about $0.01 per run. The first attempt (two runs) was aborted after 58 minutes: a unit test, `a_deadlock_and_a_livelock_are_reported_not_hung`, looped for
ever because its "livelock" task never returned from a single poll (a zero-length sleep and `ready` are Ready at once), which no cooperative executor can interrupt. The test now models a real livelock (a task that wakes itself and returns Pending) and
the unit-test step has a 15-minute timeout. That attempt cost about $0.16.

**Result of the final runs: GCP-CHECK PASS on both architectures.**

| Check | ARM64 | x86-64 |
|---|---|---|
| Control sweep, unmutated | 20,000 of 20,000 seeds pass; all 13 required fault kinds covered | same |
| Replay of a failing seed | identical (seed 12345) | identical |
| Determinism in one machine | identical across release, debug, repeated runs; 100 seeds, 100 distinct traces, 0 in-process mismatches | same |
| **Trace hashes across the two architectures** | sha256 of the 100 per-seed hashes `8cf6e8c651b1cebc02c6fffd5bfde17451094988e43f73b996a8dc5e4471c87f` | **identical digest; 0 differing lines between the two per-seed files** |
| Mutants | **13 of 13 caught**, every failing seed replays identically (10 protocol bugs, found in 0 to 45 seeds, plus 3 determinism leaks: HashMap order, wall clock in the trace, a global counter) | same |
| Mutation driver | two invocations in a row both pass their baseline; a baseline after mutants passes | same |
| shuttle trial | control 20,000 seeds clean; the lost-wake bug is found at seed 18 and replays identically | same |
| turmoil trial | deterministic across processes (20 seeds); 400 of 400 seeds clean | same |
| madsim trial | deterministic across processes; **the real scheduler could not run inside the simulation** (it refuses OS threads); 399 of 400 clean, 1 hang on faults the framework cannot map | same |

**The decision recorded by the spike holds on the evidence:** a small environment trait plus our own deterministic executor is the foundation, shuttle is used for thread interleavings next to loom, madsim and turmoil are not adopted. A seed found
on one architecture reproduces on the other.

**Cost of the abstraction** (real runtime, paired and interleaved, 15 rounds, settled machine: load 0.42 on ARM64 and 0.49 on x86-64 after waiting 140 s). One "batch" of the toy does only 380 ns (ARM64) to 800 ns (x86-64) of work, so these are
costs per very small batch.

| Arm (against the direct call) | ARM64, 1 thread | x86-64, 1 thread | ARM64, 4 threads | x86-64, 4 threads |
|---|---|---|---|---|
| generic (trait + hooks) | +102 ns per batch (+26.8%) | +121 ns (+15.0%) | +42 ns (+5.3%, inside the noise) | -62 ns (-14.6%) |
| dyn (trait object + hooks) | +114 ns (+30.0%) | +130 ns (+16.1%) | +41 ns (+5.1%, inside the noise) | -38 ns (-9.0%, inside the noise) |
| no hooks | +2.3 ns (+0.6%) | +10 ns (+1.3%) | -22 ns (-2.8%) | -10 ns (-2.3%) |
| control twin (noise) | -0.2 ns | -2.1 ns | +12 ns | -4 ns |
| noise floor | 1.3 ns | 17.6 ns | 168 ns | 31 ns |

Readings. (1) The environment trait and executor ABSTRACTION alone cost about nothing (+0.6% and +1.3%); the cost sits in the fault-point HOOKS, about 100-130 ns per batch on one thread. (2) With four threads the differences are inside the noise floor on ARM64, and
on x86-64 the generic arm is FASTER than the direct call by more than the noise floor (-62 ns, -15%); there is no explanation for that here and it is not claimed as a gain. (3) Real batches carry microseconds to milliseconds of work, so about 100 ns per batch
is a small fraction of a percent there; that is reasoning, not a measurement of Nata. (4) The harness self-test DETECTS an injected cost, but it measured +487 ns (ARM64) and +417 ns (x86-64) for an injected 2 x 100 ns: it is trusted for "detected", not for the size
of an effect.

Not shown: the fault-point hooks inside a real engine; the cost with real network and storage; a scheduler of many nodes.

### Raw output, ARM64 (final run `simharness-arm64-r3`)

```
=== sim-harness gcp-check summary  arch=aarch64  2026-11-05T01:23:25Z  elapsed 369 s
--- control sweep:
SWEEP first=0 count=20000 jobs=8 pass=20000 fail=0 wall_s=5.0
COVERAGE_REQUIRED ok (13 kinds)
SWEEP first=0 count=2000 jobs=8 pass=2000 fail=0 wall_s=1.9
COVERAGE_REQUIRED ok (13 kinds)
== replay is identical
replay of seed 12345 identical: PASS seed=12345 family=lossy-network status=completed hash=59d8ea702703e193 events=969 end_ms=208
--- determinism:
IDENTICAL release-1 vs release-2 (100 seeds)
IDENTICAL release-1 vs release-3 (100 seeds)
IDENTICAL release-1 vs debug-1 (100 seeds)
IDENTICAL release-1 vs debug-2 (100 seeds)
IDENTICAL release-1 vs debug-3 (100 seeds)
HASHES first=0 count=100 distinct=100 inprocess_mismatches=0
HASHDIGEST arch=aarch64 seeds=100 sha256=8cf6e8c651b1cebc02c6fffd5bfde17451094988e43f73b996a8dc5e4471c87f
--- mutants:
MUTANT retry_counts_output_twice: CAUGHT  first failing seed 3  [first=0 count=20000 jobs=8 pass=13653 fail=6347 wall_s=4.9]  kinds: rows seeds=6347
    replay        : IDENTICAL
MUTANT cancel_skips_restarted_worker: CAUGHT  first failing seed 14  [first=0 count=20000 jobs=8 pass=19162 fail=838 wall_s=5.1]  kinds: violation seeds=838
    replay        : IDENTICAL
MUTANT window_off_by_one: CAUGHT  first failing seed 0  [first=0 count=20000 jobs=8 pass=1 fail=19999 wall_s=5.4]  kinds: violation seeds=19999
    replay        : IDENTICAL
MUTANT lost_wake_when_window_full: CAUGHT  first failing seed 1  [first=0 count=20000 jobs=8 pass=9841 fail=10159 wall_s=24.7]  kinds: hang seeds=10159
    replay        : IDENTICAL
MUTANT no_retry_when_last_frame_seen: CAUGHT  first failing seed 28  [first=0 count=20000 jobs=8 pass=19580 fail=420 wall_s=7.0]  kinds: hang seeds=420
    replay        : IDENTICAL
MUTANT leak_at_shutdown: CAUGHT  first failing seed 6  [first=0 count=20000 jobs=8 pass=15215 fail=4785 wall_s=5.4]  kinds: leak seeds=4785
    replay        : IDENTICAL
MUTANT stale_attempt_accepted: CAUGHT  first failing seed 45  [first=0 count=20000 jobs=8 pass=19633 fail=367 wall_s=5.1]  kinds: rows seeds=233,violation seeds=134
    replay        : IDENTICAL
MUTANT duplicate_frame_applied: CAUGHT  first failing seed 1  [first=0 count=20000 jobs=8 pass=11136 fail=8864 wall_s=9.6]  kinds: hang seeds=1287,rows seeds=7577
    replay        : IDENTICAL
MUTANT store_error_means_empty_input: CAUGHT  first failing seed 15  [first=0 count=20000 jobs=8 pass=18493 fail=1507 wall_s=5.0]  kinds: rows seeds=1507
    replay        : IDENTICAL
MUTANT cancel_done_without_acks: CAUGHT  first failing seed 6  [first=0 count=20000 jobs=8 pass=17839 fail=2161 wall_s=4.9]  kinds: violation seeds=2161
    replay        : IDENTICAL
MUTANT det_hashmap_iteration_order: CAUGHT by the determinism check (same seed, different trace: inside one process)  [the coordinator iterates its workers in HashMap order (random per process)]
MUTANT det_wall_clock_in_trace: CAUGHT by the determinism check (same seed, different trace: inside one process)  [the wall clock leaks into the trace]
MUTANT det_global_counter: CAUGHT by the determinism check (same seed, different trace: inside one process)  [a process-global counter leaks into the trace (same in separate processes, different in the second run of one process)]
MUTANTS real sources UNCHANGED
MUTANTS SUMMARY caught=13 of 13  not-caught-or-invalid=0
--- mutation driver test:
TEST 1 PASS: two invocations in a row both passed their baseline
TEST 2 PASS: a baseline after mutants in the same work directory passed
TEST 3 PASS: 3 lines; fix works
TEST 4 PASS: real sources unchanged in every run
TEST_MUTANTS PASS
--- shuttle trial:
/mnt/work/results/sim-harness/shuttle-control.txt:SHUTTLE SWEEP bug_feature=false seeds=20000 fail=0 wall_s=1.1
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE SWEEP bug_feature=true seeds=20000 fail=1042 wall_s=1.1
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE FIRST_FAIL seed=18 msg=deadlock! blocked tasks: [main-thread (task main-thread(0)), <unknown> (task main-thread(2))]
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE MUTANT bug-lost-shutdown: CAUGHT at seed 18, replay IDENTICAL
--- madsim trial:
MADSIM determinism: 3 runs in separate processes, 20 seeds: IDENTICAL
MADSIM HASH seed=0 hash=bef8052291b8d221 events=155
MADSIM HASH seed=1 hash=09dc20e670960ea1 events=132
MADSIM HASH seed=2 hash=020445d14e9e0fed events=123
MADSIM PROBE-THREAD the real scheduler could NOT run inside the simulation: assertion `left == right` failed
MADSIM PROBE-THREAD the real scheduler could NOT run inside the simulation: assertion `left == right` failed
MADSIM SWEEP seeds=400 pass=399 fail=1 wall_s=0.7
MADSIM FIRST_FAIL seed=361 hang: the coordinator did not finish within 5 virtual seconds (unmapped faults: crash-points,duplicates,corruption)
--- turmoil trial:
TURMOIL determinism: 3 runs in separate processes, 20 seeds: IDENTICAL
TURMOIL HASH seed=0 hash=d9514929c9611a19 events=46
TURMOIL HASH seed=1 hash=d0d91db63395dca4 events=75
TURMOIL HASH seed=2 hash=818dc66030bf5557 events=68
TURMOIL SWEEP seeds=400 pass=400 fail=0 wall_s=0.7
--- bench:
load average before: 0.42 1.52 0.85 1/202 2114367  [loadavg=0.42 (quiet enough for timing)]
BENCH arch=aarch64 profile=release rounds=15 batches=200000 loadavg_start=0.42
  SELFTEST injected cost 200 ns/batch (2 x 100 ns): measured +486.5, noise floor 3.1 -> DETECTED
  NOISE_FLOOR threads=1 ns_per_batch=1.3 (control arm vs direct: max(|median|, IQR))
  NOISE_FLOOR threads=4 ns_per_batch=168.5 (control arm vs direct: max(|median|, IQR))
BENCH loadavg_end=1.22
BENCH OK
load average after:  1.22 1.60 0.90 1/202 2114799  [loadavg=1.22 (quiet enough for timing)]
NOTE: every timing above is labelled by the load average at the moment of the label; a label that says SANITY ONLY means the numbers show the order of magnitude, not a result.
--- soft (recorded) results: madsim-probe-thread rc=1 madsim-probe-thread-allowed rc=1 madsim-sweep rc=1 madsim-determinism rc=0 turmoil-sweep rc=0 turmoil-determinism rc=0
GCP-CHECK PASS
```

### Raw benchmark, ARM64

```
settled: load 0.42 after 140 s
load average before: 0.42 1.52 0.85 1/202 2114367  [loadavg=0.42 (quiet enough for timing)]
BENCH arch=aarch64 profile=release rounds=15 batches=200000 loadavg_start=0.42
THREADS 1: ns per batch (min q1 median q3 max), 15 interleaved rounds
  direct       377.7    378.5    379.4    381.5    407.2
  generic      479.0    481.0    481.7    482.6    494.8
  dyn          491.5    492.5    493.5    494.4    497.3
  nohooks      379.4    380.6    382.3    384.6    390.0
  control      377.5    378.4    379.1    380.9    384.6
  injected     864.0    866.0    866.7    867.4    869.6
  paired difference against `direct` per round (ns per batch): median [q1, q3]
  generic     +101.8 [+100.7, +102.8]  (+26.8% of direct's median)
  dyn         +113.7 [+112.5, +115.1]  (+30.0% of direct's median)
  nohooks       +2.3 [+1.0, +3.6]  (+0.6% of direct's median)
  control       -0.2 [-0.6, +0.7]  (-0.1% of direct's median)
  injected    +486.5 [+485.9, +489.0]  (+128.2% of direct's median)
  SELFTEST injected cost 200 ns/batch (2 x 100 ns): measured +486.5, noise floor 3.1 -> DETECTED
  NOISE_FLOOR threads=1 ns_per_batch=1.3 (control arm vs direct: max(|median|, IQR))
THREADS 4: ns per batch (min q1 median q3 max), 15 interleaved rounds
  direct       646.7    754.8    801.3    816.5    914.0
  generic      724.7    771.0    811.6    858.2    952.6
  dyn          729.4    794.2    830.6    859.1    961.6
  nohooks      683.6    739.0    781.0    814.8    913.7
  control      710.0    756.8    791.2    866.1    914.6
  paired difference against `direct` per round (ns per batch): median [q1, q3]
  generic      +42.3 [-42.0, +94.9]  (+5.3% of direct's median)
  dyn          +40.6 [-14.7, +89.3]  (+5.1% of direct's median)
  nohooks      -22.2 [-42.3, +23.9]  (-2.8% of direct's median)
  control      +11.6 [-56.7, +111.8]  (+1.4% of direct's median)
  NOISE_FLOOR threads=4 ns_per_batch=168.5 (control arm vs direct: max(|median|, IQR))
BENCH loadavg_end=1.22
BENCH OK
load average after:  1.22 1.60 0.90 1/202 2114799  [loadavg=1.22 (quiet enough for timing)]
NOTE: every timing above is labelled by the load average at the moment of the label; a label that says SANITY ONLY means the numbers show the order of magnitude, not a result.
```

### Raw output, x86-64 (final run `simharness-x86-r3`)

```
=== sim-harness gcp-check summary  arch=x86_64  2026-11-05T01:24:55Z  elapsed 443 s
--- control sweep:
SWEEP first=0 count=20000 jobs=8 pass=20000 fail=0 wall_s=7.1
COVERAGE_REQUIRED ok (13 kinds)
SWEEP first=0 count=2000 jobs=8 pass=2000 fail=0 wall_s=2.4
COVERAGE_REQUIRED ok (13 kinds)
== replay is identical
replay of seed 12345 identical: PASS seed=12345 family=lossy-network status=completed hash=59d8ea702703e193 events=969 end_ms=208
--- determinism:
IDENTICAL release-1 vs release-2 (100 seeds)
IDENTICAL release-1 vs release-3 (100 seeds)
IDENTICAL release-1 vs debug-1 (100 seeds)
IDENTICAL release-1 vs debug-2 (100 seeds)
IDENTICAL release-1 vs debug-3 (100 seeds)
HASHES first=0 count=100 distinct=100 inprocess_mismatches=0
HASHDIGEST arch=x86_64 seeds=100 sha256=8cf6e8c651b1cebc02c6fffd5bfde17451094988e43f73b996a8dc5e4471c87f
--- mutants:
MUTANT retry_counts_output_twice: CAUGHT  first failing seed 3  [first=0 count=20000 jobs=8 pass=13653 fail=6347 wall_s=7.2]  kinds: rows seeds=6347
    replay        : IDENTICAL
MUTANT cancel_skips_restarted_worker: CAUGHT  first failing seed 14  [first=0 count=20000 jobs=8 pass=19162 fail=838 wall_s=7.2]  kinds: violation seeds=838
    replay        : IDENTICAL
MUTANT window_off_by_one: CAUGHT  first failing seed 0  [first=0 count=20000 jobs=8 pass=1 fail=19999 wall_s=7.5]  kinds: violation seeds=19999
    replay        : IDENTICAL
MUTANT lost_wake_when_window_full: CAUGHT  first failing seed 1  [first=0 count=20000 jobs=8 pass=9841 fail=10159 wall_s=44.1]  kinds: hang seeds=10159
    replay        : IDENTICAL
MUTANT no_retry_when_last_frame_seen: CAUGHT  first failing seed 28  [first=0 count=20000 jobs=8 pass=19580 fail=420 wall_s=10.7]  kinds: hang seeds=420
    replay        : IDENTICAL
MUTANT leak_at_shutdown: CAUGHT  first failing seed 6  [first=0 count=20000 jobs=8 pass=15215 fail=4785 wall_s=7.2]  kinds: leak seeds=4785
    replay        : IDENTICAL
MUTANT stale_attempt_accepted: CAUGHT  first failing seed 45  [first=0 count=20000 jobs=8 pass=19633 fail=367 wall_s=7.3]  kinds: rows seeds=233,violation seeds=134
    replay        : IDENTICAL
MUTANT duplicate_frame_applied: CAUGHT  first failing seed 1  [first=0 count=20000 jobs=8 pass=11136 fail=8864 wall_s=16.2]  kinds: hang seeds=1287,rows seeds=7577
    replay        : IDENTICAL
MUTANT store_error_means_empty_input: CAUGHT  first failing seed 15  [first=0 count=20000 jobs=8 pass=18493 fail=1507 wall_s=7.4]  kinds: rows seeds=1507
    replay        : IDENTICAL
MUTANT cancel_done_without_acks: CAUGHT  first failing seed 6  [first=0 count=20000 jobs=8 pass=17839 fail=2161 wall_s=7.4]  kinds: violation seeds=2161
    replay        : IDENTICAL
MUTANT det_hashmap_iteration_order: CAUGHT by the determinism check (same seed, different trace: inside one process)  [the coordinator iterates its workers in HashMap order (random per process)]
MUTANT det_wall_clock_in_trace: CAUGHT by the determinism check (same seed, different trace: inside one process)  [the wall clock leaks into the trace]
MUTANT det_global_counter: CAUGHT by the determinism check (same seed, different trace: inside one process)  [a process-global counter leaks into the trace (same in separate processes, different in the second run of one process)]
MUTANTS real sources UNCHANGED
MUTANTS SUMMARY caught=13 of 13  not-caught-or-invalid=0
--- mutation driver test:
TEST 1 PASS: two invocations in a row both passed their baseline
TEST 2 PASS: a baseline after mutants in the same work directory passed
TEST 3 PASS: 3 lines; fix works
TEST 4 PASS: real sources unchanged in every run
TEST_MUTANTS PASS
--- shuttle trial:
/mnt/work/results/sim-harness/shuttle-control.txt:SHUTTLE SWEEP bug_feature=false seeds=20000 fail=0 wall_s=1.6
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE SWEEP bug_feature=true seeds=20000 fail=1042 wall_s=1.6
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE FIRST_FAIL seed=18 msg=deadlock! blocked tasks: [main-thread (task main-thread(0)), <unknown> (task main-thread(2))]
/mnt/work/results/sim-harness/shuttle-mutant.txt:SHUTTLE MUTANT bug-lost-shutdown: CAUGHT at seed 18, replay IDENTICAL
--- madsim trial:
MADSIM determinism: 3 runs in separate processes, 20 seeds: IDENTICAL
MADSIM HASH seed=0 hash=bef8052291b8d221 events=155
MADSIM HASH seed=1 hash=09dc20e670960ea1 events=132
MADSIM HASH seed=2 hash=020445d14e9e0fed events=123
MADSIM PROBE-THREAD the real scheduler could NOT run inside the simulation: assertion `left == right` failed
MADSIM PROBE-THREAD the real scheduler could NOT run inside the simulation: assertion `left == right` failed
MADSIM SWEEP seeds=400 pass=399 fail=1 wall_s=0.7
MADSIM FIRST_FAIL seed=361 hang: the coordinator did not finish within 5 virtual seconds (unmapped faults: crash-points,duplicates,corruption)
--- turmoil trial:
TURMOIL determinism: 3 runs in separate processes, 20 seeds: IDENTICAL
TURMOIL HASH seed=0 hash=d9514929c9611a19 events=46
TURMOIL HASH seed=1 hash=d0d91db63395dca4 events=75
TURMOIL HASH seed=2 hash=818dc66030bf5557 events=68
TURMOIL SWEEP seeds=400 pass=400 fail=0 wall_s=0.7
--- bench:
load average before: 0.49 2.10 1.26 1/208 2115204  [loadavg=0.49 (quiet enough for timing)]
BENCH arch=x86_64 profile=release rounds=15 batches=200000 loadavg_start=0.49
  SELFTEST injected cost 200 ns/batch (2 x 100 ns): measured +416.8, noise floor 17.6 -> DETECTED
  NOISE_FLOOR threads=1 ns_per_batch=17.6 (control arm vs direct: max(|median|, IQR))
  NOISE_FLOOR threads=4 ns_per_batch=31.2 (control arm vs direct: max(|median|, IQR))
BENCH loadavg_end=0.90
BENCH OK
load average after:  0.90 2.06 1.27 1/205 2115652  [loadavg=0.90 (quiet enough for timing)]
NOTE: every timing above is labelled by the load average at the moment of the label; a label that says SANITY ONLY means the numbers show the order of magnitude, not a result.
--- soft (recorded) results: madsim-probe-thread rc=1 madsim-probe-thread-allowed rc=1 madsim-sweep rc=1 madsim-determinism rc=0 turmoil-sweep rc=0 turmoil-determinism rc=0
GCP-CHECK PASS
```

### Raw benchmark, x86-64

```
settled: load 0.49 after 140 s
load average before: 0.49 2.10 1.26 1/208 2115204  [loadavg=0.49 (quiet enough for timing)]
BENCH arch=x86_64 profile=release rounds=15 batches=200000 loadavg_start=0.49
THREADS 1: ns per batch (min q1 median q3 max), 15 interleaved rounds
  direct       797.6    801.3    805.8    818.5    881.8
  generic      842.2    913.5    923.0    937.6    952.2
  dyn          832.6    929.7    932.9    944.9    960.4
  nohooks      808.6    812.9    815.4    817.9    838.9
  control      721.8    797.8    803.6    817.3    833.3
  injected    1149.6   1219.1   1221.2   1226.1   1249.1
  paired difference against `direct` per round (ns per batch): median [q1, q3]
  generic     +120.8 [+101.5, +123.4]  (+15.0% of direct's median)
  dyn         +129.9 [+119.5, +133.7]  (+16.1% of direct's median)
  nohooks      +10.1 [+3.5, +14.8]  (+1.3% of direct's median)
  control       -2.1 [-13.5, +4.2]  (-0.3% of direct's median)
  injected    +416.8 [+409.8, +421.0]  (+51.7% of direct's median)
  SELFTEST injected cost 200 ns/batch (2 x 100 ns): measured +416.8, noise floor 17.6 -> DETECTED
  NOISE_FLOOR threads=1 ns_per_batch=17.6 (control arm vs direct: max(|median|, IQR))
THREADS 4: ns per batch (min q1 median q3 max), 15 interleaved rounds
  direct       392.8    402.9    425.9    438.6    461.6
  generic      345.6    359.2    364.9    405.9    466.1
  dyn          338.5    358.3    368.1    419.1    467.2
  nohooks      389.8    406.3    415.3    423.7    435.6
  control      395.7    411.9    418.2    429.9    454.8
  paired difference against `direct` per round (ns per batch): median [q1, q3]
  generic      -62.0 [-82.9, -5.1]  (-14.6% of direct's median)
  dyn          -38.4 [-68.5, +9.3]  (-9.0% of direct's median)
  nohooks       -9.7 [-22.5, +0.4]  (-2.3% of direct's median)
  control       -3.8 [-16.3, +14.9]  (-0.9% of direct's median)
  NOISE_FLOOR threads=4 ns_per_batch=31.2 (control arm vs direct: max(|median|, IQR))
BENCH loadavg_end=0.90
BENCH OK
load average after:  0.90 2.06 1.27 1/205 2115652  [loadavg=0.90 (quiet enough for timing)]
NOTE: every timing above is labelled by the load average at the moment of the label; a label that says SANITY ONLY means the numbers show the order of magnitude, not a result.
```
