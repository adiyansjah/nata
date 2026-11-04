# The tokio -> CPU-pool hand-off (ADR 0002, the untested half)

Code: `spikes/async-bridge/` (throwaway; own Cargo project and target dir; delete once ADR 0002 is accepted).
Everything below was run on the shared 8-vCPU x86-64 VM; nothing was run on ARM64. The commands that produced
each table are in "Reproduce". Load averages are the 1-minute figure from `/proc/loadavg`, read by the program
itself, next to each result (it lags: it is a 1-minute average, and the benchmark's own threads are in it).

## What was built

- **Snapshot of the scheduler.** `src/api.rs`, `src/batch.rs`, `src/sched.rs` are copies of the COMMITTED files
  (`git show HEAD:spikes/operator-trait/src/...`, HEAD = 7f2e3be), not the working tree another agent was editing.
  `api.rs` and `batch.rs` are byte-identical; `sched.rs` differs only in its imports (they come from `src/sync.rs`,
  which is `std` normally and `loom` under `--cfg nata_loom`) plus one debug field and assertion that a task is
  never run by two workers at once. Exact diff and hashes: `spikes/async-bridge/SNAPSHOT.md`.
- **The bridge** (`src/chan.rs`, new): a bounded channel. The producer side is an async `send()` future (a tokio
  task gets `Pending` when the channel is full and its `std::task::Waker` is stored). The consumer side is
  `recv(&Waker)`, which never waits: it returns an item, `Closed`, or stores the scheduler's waker and returns
  `Blocked`. Everything is behind one mutex, wakers are fired after the lock is released. The last sender's drop
  closes the channel (and fires the consumer's waker). `Receiver::close()` (called from `Source::finish()`, so also
  on cancel) fails pending and future sends, frees queued chunks and wakes blocked senders. `ChanSource` keeps the
  chunk it just handed out alive until the next `next()` or `finish()` so the memory accounting includes the chunk
  being processed.
- **The mock store and producers** (`src/harness.rs`): tokio runtime with 2 worker threads; producers read a chunk
  after a configurable latency (none, one yield, fixed sleep, random sleep) and `send` it.

## 6a. Model check (loom): no lost wakeup, no deadlock, no double run

**This is not a hand-written model of the code.** `tests/loom_bridge.rs` compiles the real `chan.rs` and the real
snapshotted `sched.rs` (task state machine `Queued/Running{notified}/Parked/Done`, run queue and condvar, worker
loop, shutdown, cancel) against loom's `Mutex`, `Condvar`, atomics and threads. What IS a stand-in is the tokio
side: a loom thread plays "a tokio worker running a producer task" by polling the real `SendFuture` with a waker
backed by a loom parker (a ten-line `block_on`). Tokio's own scheduler, timer and I/O driver are not modelled; the
only property used from tokio is its documented contract: after a stored `Waker` is woken, from any thread, the
task is polled again.

Each model: `workers` pool threads, one consumer task over a channel of capacity `cap`, `producers` sender threads
each sending `items` chunks then dropping their sender, main thread calling `wait()` (and optionally `cancel()`
racing with everything). Checked in every interleaving loom explores:

- the query completes: loom reports any state where all threads are blocked as a deadlock (this covers a lost
  wakeup anywhere on the path, a lost shutdown notification, and a producer stuck in `send()` after a cancel, which
  the test joins);
- every item arrives, exactly once, in order per producer (cancel: what arrived is a prefix);
- `in_run` assertion: a task is never run by two workers at once.

| model | workers | cap | items x producers | cancel | preemption bound | interleavings | time (s) | result |
|---|---:|---:|---|---|---:|---:|---:|---|
| `w1_cap1_two_items` | 1 | 1 | 2 x 1 | no | 5 | 207,749 | 40 | pass |
| `w1_cap2_one_item` | 1 | 2 | 1 x 1 | no | 5 | 37,936 | 9 | pass |
| `w1_close_without_items` | 1 | 1 | 0 x 1 (close races with block) | no | 5 | 20,119 | 6 | pass |
| `w1_cancel_races_with_producer` | 1 | 1 | 2 x 1 | yes | 4 | 254,808 | 63 | pass |
| `w1_two_producers_cap1` | 1 | 1 | 1 x 2 | no | 3 | 56,044 | 8 | pass |
| `w1_two_producers_cancel` | 1 | 1 | 2 x 2 | yes | 2 | 85,598 | 19 | pass |
| `w2_cap1_one_item` | 2 | 1 | 1 x 1 | no | 3 | 138,801 | 62 | pass |
| `w2_close_without_items` | 2 | 1 | 0 x 1 | no | 3 | 129,013 | 68 | pass |
| `w2_cancel_races_with_producer` | 2 | 1 | 1 x 1 | yes | 3 | 368,171 | 338 | pass |

**Bounds (what is and is not covered).** Loom explores all interleavings up to the stated *preemption bound*
(number of times the model checker may switch away from a runnable thread); beyond it, nothing is claimed. Tiny
configurations only: at most 2 pool workers, 1 consumer task, at most 2 senders, at most 2 items, capacity 1 or 2.
No model has 3 or more senders or 2 consumer tasks. An earlier try of `w1_two_producers_cancel` at bound 3
did not finish in 600 s, so it ran at bound 2. Loom checks the memory-model effects of the primitives it replaces
(everything here is a `Mutex`, `Condvar` or `SeqCst` atomic); it does not model a weaker ordering than the code
asks for, and the real hardware (x86-64 here; ARM64 not run) was not involved. The test hooks (`HOOK_DELAY_US`,
`TEST_WORKER_DELAY_US`) are compiled out/zero in the model.

### The model can fail (mutation table)

Each mutation is applied to a scratch copy of the crate by `mutants/mutate.py` (the diff is printed by that
script; the real sources are never mutated), then every loom test is run in its own process there. "caught" means
loom reported a deadlock or the invariant assertion fired.

| mutation | what it breaks | caught by (of the 9 models) | reason |
|---|---|---|---|
| `chan_no_recheck` | emptiness check and waker store are no longer one critical section (the classic lost wakeup: store the waker without re-checking) | 5 of 7 first tests, plus `w1_two_producers_cap1`: all the non-cancel models | deadlock |
| `chan_close_no_wake` | the last sender's drop takes the waker but does not fire it | same 5 + `w1_two_producers_cap1` | deadlock |
| `chan_send_no_wake` | a send never fires the consumer's waker | `w1_cap1_two_items`, `w1_cap2_one_item`, `w2_cap1_one_item`, `w1_two_producers_cap1` | deadlock |
| `chan_close_rx_no_wake` | closing the receiver does not wake blocked senders | `w1_cancel_races_with_producer`, `w1_two_producers_cancel` (NOT `w2_cancel...`: its one item never blocks a sender) | deadlock |
| `sched_ignore_notified` | a wake that arrives while the task is Running is forgotten | all 7 first tests, plus `w1_two_producers_*` | deadlock |
| `sched_wake_queued_enqueues` | waking an already queued task enqueues it twice | `w1_cancel...`, `w2_cancel...`, `w1_two_producers_cancel` | `in_run` assertion: "task run by two workers at once" |
| `sched_shutdown_without_lock` | the lost-shutdown bug of the first prototype (flag set without the queue lock) | all 7 first tests, plus `w1_two_producers_*` | deadlock |
| none (copy of the unmutated crate through the same script) | control for the harness | 0 of 7 | all pass |

The seven-test results are from `mutants/loom_mutants.sh`; the two `w1_two_producers_*` tests were added after
that run and checked against the same seven mutations by a second script run (same mutations, same commands).
Coverage gaps the table shows: the double-run mutation is only reachable in the cancel models (a wake of a Queued
task needs cancel's wake-all); `w2` with capacity 1 and one item never has a sender wait, so it cannot see a
missing wake of blocked senders.

## 6b. Stress on the real bridge (tokio + snapshotted scheduler)

`examples/stress.rs`. Each run builds a fresh tokio runtime (2 workers) and a fresh CPU pool, and runs one
scenario from a 2,880-cell matrix (consecutive run indexes are spread across the whole matrix, so any prefix covers
every dimension): pool threads {1, 2} x channels = consumer tasks {1, 3, 8} x producers per channel {1, 3} x
capacity {1, 2, 8} x chunks per producer {0, 1, 5, 40, 200} x store latency {none, one yield, 50 us, random 0-400 us}
x kind {Normal; EarlyClose (each producer sends a random number of chunks, possibly zero, and closes); Abort (a
third of the producer tasks are aborted from outside after 1 us to 2 ms); Cancel (`RunningQuery::cancel()` after
1 us to 2 ms)}. Every run must finish within a 20 s watchdog and then be correct: per producer, what arrived is
exactly the chunks it sent, in order (Normal, EarlyClose), or a prefix (Abort, Cancel); no chunk arrives at the
wrong consumer; the query returns Ok (or `cancelled` for Cancel); every producer task terminates within 5 s of the
query ending (so a producer is never left blocked in `send()` after a cancel). Hooks: `HOOK_DELAY_US=50` makes the
channel sleep or yield for a random time up to 50 us (a) after releasing the lock and before firing a waker and
(b) after storing the consumer's waker and before returning `Blocked`; `TEST_WORKER_DELAY_US=200` is the existing
hook (a worker sleeps while holding the queue lock before it waits). "Contention" = 8 busy-loop threads (= core
count) for the whole phase.

| phase | runs | ok | wrong | hung (timed out twice) | really cut off mid-flight (Abort / Cancel) | slowest run | load before / after |
|---|---:|---:|---:|---:|---|---:|---|
| A: no extra contention, no hooks | 1,500 | 1,500 | 0 | 0 | 197 / 249 | 0.4 s | 5.3 / 6.5 |
| B: 8 busy threads | 1,500 | 1,500 | 0 | 0 | 219 / 289 | 0.5 s | 6.5 / 14.4 |
| C: hooks 50 us + worker delay 200 us | 600 | 600 | 0 | 0 | 88 / 116 | 0.8 s | 14.4 / 11.8 |
| D: 8 busy threads + hooks | 600 | 600 | 0 | 0 | 96 / 121 | 11.7 s | 11.8 / 13.9 |
| **total (final run)** | **4,200** | **4,200** | **0** | **0** | | | |

An earlier phase A (1,500 runs, same seed) ran while the VM's load average was 37 (other agents compiling), also
1,500 of 1,500 ok, slowest 1.1 s; a phase B started then was stopped by me because of the load and is not counted.
Every run that "timed out" would have been retried once with a 5x timeout and counted separately as
`slow_under_load_recovered_on_retry`; that count was 0 in all four final phases. The retry is a fresh run, not the
same schedule, so it could hide a rare hang; the mutant runs below show it does recover from real hangs
sometimes (3 of 24 runs of `chan_close_no_wake`), which is why a non-zero count must be re-run alone before it is
believed either way.

### The stress can fail (same mutants)

Mutant binaries built from the scratch copies (separate target directories, hashes of the four binaries differ),
400 runs requested, `--hook-us 50 --worker-delay-us 200 --timeout-s 3`, run on a quiet machine (load 2-3); it stops after 3 confirmed hangs:

| mutant | result |
|---|---|
| `chan_no_recheck` | 3 confirmed hangs in the first 9 runs |
| `chan_close_no_wake` | 3 confirmed hangs in the first 24 runs (3 more timed out once and finished on retry) |
| `sched_ignore_notified` | 3 confirmed hangs in the first 5 runs |
| `sched_shutdown_without_lock` | 3 confirmed hangs in the first 3 runs |

(With these hooks the shutdown bug is hit by the first run; the first prototype needed a deterministic test for it.)
The same scenarios also run as ordinary tests: `tests/bridge.rs` (5 tests, 3.5 s) and `tests/delay_hook.rs` (own
binary because the delays are process-wide; 60 runs).

## 7. Measurements

Method (way-of-working section 6): results are checked before timing (each run asserts that every chunk arrived,
in order per producer, and an untimed preflight of every arm runs first); arms are interleaved round by round, with
the starting arm rotated each round; every arm has a control; min / median / max across rounds are shown as the
noise; tokio = 2 workers, CPU pool = 2 threads. `uptime` was read before starting: 1-minute load 2.1-6.5 for the
tables below (the orchestrator's limit was 8). That load includes my own threads and the other agents' work; the
shared machine is the main source of tail noise (see below).

### 7a + 7b: is I/O delayed by a busy CPU pool? (`measure latency`)

Four producers on tokio, 200 chunks each, one chunk per producer per millisecond, delivered by "store" OS threads
that timestamp the moment a chunk is ready and hand the timestamp to the producer task (a network completion
waking a tokio task). Reported latency = from that timestamp to `send().await` returning. (My first version used
tokio's own timer for the store; its 1 ms tick put a 1.1 ms floor under every arm, so that data is discarded.) The
channel is large enough that a send never waits, except in the last arm.

Arms: **Idle**: CPU pool has only the consumer task. **CpuSat**: the pool's 2 threads run 4 CPU-heavy tasks that
never block until told to stop (the consumer is queued behind them: it consumed 0 chunks in every round, which
shows the pool was saturated for the whole window). **OsSpin**: control for the cause of a tail: idle pool, but 2
plain OS spin threads take two cores. **TokioSat1ms / TokioSat10ms**: the KNOWN-BAD design (ADR 0002 alternative A):
the same heavy work runs on the tokio runtime (4 tasks on 2 workers), in 1 ms or 10 ms morsels with a
`yield_now()` between them. **CpuSatBackpressure**: CPU pool saturated with capacity 4: sends wait for room.

Run 1 (30 rounds x 6 arms, load 4.5-5.8) and run 2 (a repeat, 30 rounds, load 5.2-6.5). Microseconds.

| arm | run 1 p50 of rounds (min/med/max) | run 2 p50 | run 1 p99 of rounds (min/med/max) | run 2 p99 | run 1 pooled p99.9 | run 2 pooled p99.9 |
|---|---|---|---|---|---:|---:|
| Idle | 30 / 36 / 44 | 31 / 39 / 52 | 62 / 94 / 3,520 | 66 / 1,346 / 2,794 | 725 | 3,311 |
| CpuSat | 9 / 27 / 34 | 17 / 25 / 32 | 45 / 114 / 3,788 | 96 / 1,222 / 4,303 | 3,449 | 3,340 |
| OsSpin (control) | 18 / 32 / 40 | 24 / 34 / 46 | 72 / 128 / 11,244 | 69 / 1,499 / 3,437 | 6,830 | 3,467 |
| TokioSat1ms (known bad) | 315 / 532 / 595 | 351 / 477 / 585 | 1,155 / 1,699 / 2,162 | 1,163 / 1,597 / 1,885 | 2,014 | 2,002 |
| TokioSat10ms (known bad) | 4,890 / 5,480 / 7,086 | 3,007 / 5,250 / 7,124 | 9,905 / 18,653 / 19,732 | 9,831 / 17,924 / 19,676 | 19,897 | 19,856 |
| CpuSatBackpressure (expected wait) | 497,251 / 498,953 / 504,833 | 495,541 / 499,089 / 503,336 | 591,005 / 593,917 / 598,137 | 588,388 / 593,391 / 596,335 | 598,352 | 596,452 |

Reading it:

- **A saturated CPU pool does not delay I/O at the median.** Median-of-rounds p50: Idle 36 / 39 us, CpuSat 27 / 25 us
  (run 1 / run 2). CpuSat is not slower; it is a little faster, probably because busy cores do not drop into idle
  states (a guess, not tested). Differences below the round-to-round spread (about +-10 us here) are not results.
- **The control can detect starvation, by a wide margin.** With the heavy work on the tokio runtime the median
  rises from about 36 us to 480-530 us (1 ms morsels) and to 5,000-5,500 us (10 ms morsels), 13-150 times, in all
  30 rounds of both runs (the lowest p50 of any round of TokioSat1ms, 315 us, is 7 times the highest of Idle). The
  `lag` column of the split (task waited for a tokio thread) carries it: p50 478 / 5,462 us against 29 us, while
  `send` itself stays at 0-17 us.
- **Tails are not resolved.** p99 and p99.9 of Idle itself moved from 94 us (run 1) to 1,346 us (run 2) and had
  single rounds with a p99 of 2.8-3.5 ms in both runs, so the other agents' load on the VM sets the tail. CpuSat, OsSpin (no
  scheduler involved) and Idle have the same tail within that spread (p99.9 3.3-3.5 ms in run 2; run 1 725 / 3,449 /
  6,830 us). No difference between an idle and a saturated pool at p99 or beyond can be claimed or excluded from this
  data; the OsSpin control shows that whatever tail CpuSat has also appears without the scheduler. TokioSat1ms's p99
  (about 1.6-1.7 ms) is not distinguishable from the noisy Idle tail of run 2 either; only its p50 and the 10 ms
  arm's p99 (about 18 ms) are clearly above noise.
- **Backpressure works and is visible.** With capacity 4 and a starved consumer, all 4 producers were still
  blocked at the end of the window in at least one round (max over rounds = 4) and the median send waited about
  0.5 s (the whole window): that is the bound doing its job, not I/O starvation. It is shown as the contrast to
  CpuSat, where nothing waits because there is room.
- **Not an I/O result but worth knowing:** with CPU-heavy tasks occupying every pool thread the consumer is starved
  (0 chunks consumed). The scheduler has no preemption or fairness: a task that never returns `Blocked` holds its
  worker until it finishes (known limit of the operator-trait prototype: one FIFO queue, no time slice). The
  hand-off cannot fix that.

### 7c: memory bound (`measure memory`)

Fast producers (no latency), a slow consumer (sink sleeps 1 ms per chunk), 1 MiB chunks, 240 chunks per producer,
CPU pool 2 threads. Every chunk's payload is counted from creation to drop (a global counter, with peaks), wherever
it is: being built, waiting in `send`, queued, or held by the consumer task. Each row ran in its own process.
Documented bound per channel: **N (queued) + P (one chunk built by each producer, waiting inside `send`) + 1
(the chunk the consumer task still holds)**; with C consumer tasks, C x (N + P + 1). Load 4.3 for all rows.

| P | consumer tasks C | capacity N | bound (chunks) | peak chunks | peak MiB | within bound | process VmHWM growth, MiB |
|---:|---:|---:|---:|---:|---:|---|---:|
| 1 | 1 | 1 | 3 | 3 | 3 | yes | 3 |
| 1 | 1 | 4 | 6 | 6 | 6 | yes | 12 |
| 1 | 1 | 16 | 18 | 18 | 18 | yes | 36 |
| 4 | 1 | 1 | 6 | 6 | 6 | yes | 13 |
| 4 | 1 | 4 | 9 | 9 | 9 | yes | 18 |
| 4 | 1 | 16 | 21 | 21 | 21 | yes | 36 |
| 4 | 2 | 4 | 18 | 18 | 18 | yes | 31 |
| 8 | 2 | 16 | 50 | 50 | 50 | yes | 80 |
| 4 | 1 | unbounded (capacity 1,000,000): CONTROL | n/a | 660 | 660 | n/a | 663 |

The peak equals the bound in all eight bounded rows, so the bound is tight and the counter reaches it (the check
is not vacuous); the unbounded control reaches 660 of 960 chunks. Detection check: a mutant with the capacity test
`<` changed to `<=` (one more slot) exceeds the bound by one chunk in every bounded row and the program reports
"NO" and exits non-zero (`chan_cap_off_by_one` in `mutants/mutate.py`; run once, under load 38, before the final
table). The process's peak resident set grows to about 2x the counted bytes for most rows; I did not investigate it
(allocator behaviour or the runtime's own memory are guesses). The counted payload is what the bound is about.

### 7c reverse: slow producer, fast consumer does not spin the pool (`measure idle`)

Two producers at 5 ms per chunk (400 chunks/s in total), capacity 4, fast consumer, CPU pool of 2 threads, window
1.5 s, 6 rounds interleaved with a control; load 4.0-4.2. CPU time of the pool's own threads is read from
`/proc/self/task/<tid>/stat` (10 ms ticks) before and after the window. The control is a source that, when the
channel is empty, wakes itself immediately instead of waiting for the producer (the classic busy re-queue).

| design | pool workers' CPU seconds per wall second (min / median / max), both threads together | tokio threads |
|---|---|---|
| the real bridge | 0.000 / 0.000 / 0.000 | 0.000 / 0.007 / 0.013 |
| KNOWN-BAD control (self-waking source) | 1.420 / 1.467 / 1.487 | 0.000 / 0.007 / 0.007 |

Idle workers use less than 10 ms of CPU in 1.5 s (below one tick); the control shows the measurement sees a spin
(about 1.5 cores).

## Limits

- x86-64 only, one shared VM at load 2-6 for the numbers above; ARM64 not run (loom does not substitute for it).
- The model is small (see Bounds). Not modelled: tokio's scheduler, timer and I/O driver (only their wake contract),
  more than 2 workers, more than 1 consumer task per channel, several pipelines or dependencies, `Blocked` sinks
  (a full downstream exchange), `HaveMoreOutput`, and spill I/O. The stress covers up to 8 consumer tasks, 3
  producers per channel and a 2-thread pool, with random timing, not exhaustive interleaving.
- The mock store is a thread or a timer; no real object store, no network, no Parquet decode.
- The I/O-latency tail (p99 and beyond) is dominated by the shared machine and could not separate an idle from a
  saturated pool; only the median is resolved. A quiet machine or a pinned set of cores would be needed.
- The heavy pool tasks never block; a fairness or time-slice policy for CPU tasks (so that the consumer is not
  starved) was not tested because the snapshotted scheduler has none.
- No real bug was found in the snapshotted scheduler or the new channel. One property of the harness: a 'cancel'
  issued after the query has already finished still makes `wait()` return `Err(cancelled)`; the stress accepts both.

## Reproduce

```
cd spikes/async-bridge
# 6a model check (separate target dir; the cfg name is nata_loom because tokio reacts to --cfg loom)
export RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=target/loom
cargo test --release --test loom_bridge -- --test-threads=1 --nocapture     # about 15 min in total
NATA_LOOM_PREEMPTIONS=2 cargo test --release --test loom_bridge -- --exact w1_cap1_two_items   # lower bound, or "none" for exhaustive
unset RUSTFLAGS CARGO_TARGET_DIR
# mutation table for the model (about 25 min); each mutation is printed by: python3 -I mutants/mutate.py <name> . /tmp/x
mutants/loom_mutants.sh [mutation ...]
# 6b stress (about 6 min on a quiet machine); add your own mutant binary with STRESS_BIN=...
./run_stress.sh                      # or: cargo run --release --example stress -- --reps 600 --contention --hook-us 50 --worker-delay-us 200
cargo test --release                 # bridge.rs, delay_hook.rs
# 7: check `uptime` first; do not run while the 1-minute load is above ~8
cargo run --release --example measure -- latency --rounds 30
cargo run --release --example measure -- memory
cargo run --release --example measure -- idle --rounds 6
```
