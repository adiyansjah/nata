# Hash aggregate baseline — 2026-11-07

Verdicts: **keep bounded Arrow IPC; grant handshake feasible within the small model;
retain the deque seam with affinity unresolved; reuse hashbrown.** Throwaway [spike](../../spikes/agg-baseline/README.md),
card 0005 part 4 only; no production/card/ADR edits, cloud use, push or merge.

## 1. Spill format

- **Rule as written:** “Keep IPC if bounded rejection and exact round trips hold;
  otherwise stop for an ADR revision, not an unreviewed codec.”
- **Numbers:** 136 exact round trips, 352 damaged frames rejected; largest damaged
  decode allocation 18,448 bytes; five native tests pass under `ulimit -v 262144`
  (peak RSS 40,900 KiB for the test command). Payload ≤1 MiB, metadata ≤64 KiB,
  rows ≤1,024, read working grant ≥8 MiB. IPC **121,320 bytes**, median write/read
  **1,387/805 MiB/s**; record control **127,707 bytes**, **1,485/2,084 MiB/s**.
  IPC min/median/max write **1,002/1,387/1,619**, read **650/805/1,047 MiB/s**;
  control write **1,309/1,485/2,465**, read **1,749/2,084/3,130 MiB/s**.
  IPC twin last/first write **0.67/0.74/1.10**, read **0.82/0.94/1.50**: noisy,
  order-sensitive microtimings, **local, indicative only**, not a speed gate.
- **Verdict:** **keep Arrow IPC for this bounded schema**, as the fixed rule
  requires. The control's speed cannot override it. Wider schemas/states must
  extend the wrapper and pass the same checks before use.
- **Reproduce:** commands T and B below. The record format is an experimental
  control only. The tested schema is fixed, uncompressed, one partition/frame,
  dictionary UTF-8 + nullable DECIMAL(38,4) key, hash, count(*), non-NULL count,
  nullable i128 SUM, AVG sum/count, nullable MIN/MAX. Dictionaries are decoded by
  value; all fixture hashes are zero. Independent SUM/AVG values and NULL patterns; seed `0x000520261107`, 64 generated cases,
  plus 0/1/1,024 rows and signed 38-digit extrema, through both formats.

## 2. Shared grant/revoke/park/wake

- **Rule as written:** “grant sum never above B, no double release, revoke/cancel/
  drop races, no lost wake, eventual progress when a victim releases (1–2 workers,
  two operators, preemption bound 3; state bounds reported).”
- **Numbers:** four loom models, **99 executions** (30 release/park, 53 cancel/
  drop, 15 competing grants, 1 single-worker), bound 2; peak grant sum including
  progress reserve **10/10**, final spillable live grants/bytes **0/0** (the
  2-unit reserve is retained until manager drop). Three mutants caught.
- **Verdict:** **feasible in this model**, including wake-before-park and victim
  release progress. The full ADR acceptance check is **unresolved**: bound 3 and
  real-service/scheduler integration remain parent runs.
- **Reproduce:** command L. The actual spike manager uses a std/loom sync facade;
  registration, accounting and sticky notification share a lock. `park()`
  consumes a notification that arrived while Running. Wake grants no bytes;
  the waiter retries. Grants release once via RAII and a live-id ledger.
  B=10 units, progress reserve=2, fair reclaim target=4 per sink. Sink 0 borrows
  all 8 available units; sink 1 coalesces two requests for 4, revokes sink 0,
  parks and proceeds after release. One-worker requeue, two-worker release/park
  races, cancel/drop races and competing grants are separate models. Dispatcher
  waiting is a harness condvar, not a blocked CPU operator.

## 3. Unordered claim()/deque versus FIFO, affinity off/on

- **Rule as written:** “Check the ADR 0002 unordered `claim()`/deque seam against
  FIFO with identical work, including affinity on/off.” “keep affinity only for
  a gain beyond twin noise without regression on ARM. No silent affinity default.”
- **Numbers:** 7 rounds; FIFO off/on median **1.795/1.986 ms**, deque off/on
  **1.756/1.932 ms**. Min–max: FIFO off **1.722–2.284**, on **1.574–4.159**;
  deque off **1.526–2.025**, on **1.539–2.015 ms**. FIFO twin min/median/max
  ratio **0.785/0.917/1.184**. Every arm claims exactly **64** unique tasks;
  deque median local/steals **54/10** off and **55/9** on, retries **0**;
  successful pins **4** on, **0** off. Exact result comparisons all pass.
- **Verdict:** **the unordered seam works for either queue in this experiment;
  keep the accepted deque direction, affinity remains unresolved**. Differences
  are below twin noise, ARM is unmeasured, and this is not the production pool.
  ADR 0001's pool/affinity check stays open; no new default is selected.
- **Reproduce:** T (exact results) and B (timing/counters). Four workers, 64
  independent aggregation morsels × 4,096 identical rows each, same hashbrown
  work/export, unordered claims restored to task ids for exact std-map comparison.
  Deque workers pop locally then steal; FIFO locks once per claim. Pinning uses
  distinct allowed logical CPU ids and checks every affinity call.

## 4. Reuse the hash table

- **Rule as written:** “reuse `hashbrown::HashTable` with group IDs and owned typed
  key/state arenas, as in `spikes/hash-ops/src/agg.rs`, with std map as the baseline
  control.” “A custom replacement requires a top self-time share ≥5% and ≥1.5x
  improvement in the component on both CPUs, correctness/mutants and the resource
  bounds; otherwise keep reuse.”
- **Numbers:** exact std-map agreement for normal and all-collision 2,048-row
  fixtures; clear/reaggregate retains capacity **224**. Timed 16 × 8,192 rows:
  std min/median/max **2.814/2.882/2.915 ms**; fresh hashbrown
  **1.714/1.806/1.847**; retained hashbrown **1.818/1.847/1.992**.
  Std twin **2.704/2.808/2.929 ms**, twin/control ratio **0.932/0.985/1.017**.
- **Verdict:** **reuse hashbrown fits**. The table supports the owned wide keys
  and state arena without new probing code, and capacity can be reused. These
  local numbers do not establish a retention speed gain or satisfy the both-CPU
  custom-replacement gate; no custom table was built or justified.
- **Reproduce:** T and B. Same precomputed stable value hash/full-key equality
  for std and hashbrown 0.17.1 (the hash-ops lockfile version). HashTable stores
  arena group ids; count/sum/non-NULL count/min/max update together. AVG state is
  sum/non-NULL count. Exact normal/all-collision checks include wide decimal,
  NULL and string keys. Clear/reaggregate checks retained capacity. This adapts
  hash-ops' table/arena pattern, **not** its narrower HashAgg kernel or key code.

## Mutants and limits

| Mutation | Outcome | Failing oracle |
|---|---|---|
| Remove budget cap | **CAUGHT** (exit 101) | `grant cap exceeded` |
| Drop release wake | **CAUGHT** (exit 101) | loom `deadlock` |
| Release a grant twice | **CAUGHT** (exit 101) | `double release` |

**0 SURVIVED, 0 INVALID** in the final mutant run. Build failures are not mutant
results. The earlier bounded full-test command could not map rustdoc's LLVM
library; rustdoc now runs outside the limit (full suite passes), while all five
behavioral tests run inside it. No decoder failure was excused.

Proposed amendments for the parent (not applied): specify the sticky notification
handshake in ADR 0009's scheduler adapter and carry these counterexamples into
real-service models. For ADR 0010, require inner IPC validation in addition to an
envelope checksum: fixed trusted schema, metadata/body/row/message caps, node and
buffer layout/range/size checks, dictionary index and string offset checks, and no
compression/deltas. Byte caps alone do not justify passing arbitrary IPC to Arrow.
These are implementation clarifications, not proposals to change a budget default.

Not covered: production code/integration or task cancellation during real disk I/O;
three-operator fairness, real cgroup/RSS reservations, rehash peak accounting, FD/disk
quotas, recursion, no-progress partitions, crashes/cleanup, ENOSPC/short writes;
float/cross-scale canonical keys; i256 partials or order-independent wide SUM/AVG
arithmetic/rounding; SQL/HAVING/clients; nightly fuzz, sanitizers or either clean
reference architecture. i128 spill round trips do not certify the wider arithmetic
required by ADR 0010. CRC detects accidental corruption, not malicious forgery;
CRC-repaired structural attacks are tested separately. No custom codec is accepted.

Parent-only future runs, **not executed or authorized here**: bound-3 real-service
loom; spill reader fuzz at 30,000 cases per shape; ARM64/x86-64 reference/production
pool affinity and table measurements on quiet matched machines, then the card's
DuckDB/SF1+ reference freeze and resource gates. No local fuzz/model run exceeded
15 minutes. The bound-2 models and three runtime mutants are a short smoke trial,
not the production mutation sweep/deep-check closeout.

## Method, evidence and commands

Local, indicative only. x86-64 Intel Core i3-1220P, 8 reported logical CPUs
(1 reported thread/core); Rust 1.99.1. Base `5daf1e4`. Pre-run aggregate CPU
busy **0.58%** over 3 seconds, load1 **0.94** (recent builds); no build/test running
at timing. Benchmark peak RSS **8,116 KiB**, virtual limit **512 MiB**. Seven rounds per arm (14 IPC samples with its twin) with
controls interleaved in the same process, warm page cache; writes include array
construction, serialization, file create/write/close, **no fsync**. Reads include
framing, checksum, preflight, Arrow decode and owned output. This is not physical
disk bandwidth. Table timings include 16 × 8,192 rows, sorted owned export and
exact comparison; fresh std/fresh hashbrown/retained hashbrown plus a std twin.
Scheduling excludes input/oracle generation and thread creation, includes barrier,
aggregation/export and joins; exact comparison follows timing. FIFO has a twin.
No end-to-end engine speed, cross-machine ratio or hardware-default claim follows.

From repository root, one command at a time:

```sh
mkdir -p "$HOME/tmp-agg-spike"
export TMPDIR="$HOME/tmp-agg-spike" CARGO_BUILD_JOBS=2
# T: build first, then bounded, serial tests
cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml --no-run
cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml -- --test-threads=1
(ulimit -v 262144; /usr/bin/time -f 'max_rss_kib=%M' timeout 120 cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml --lib --bins --tests -- --test-threads=1 --nocapture)
# L: short loom models and in-process mutants, sequential
spikes/agg-baseline/run-loom.sh
# B: build first; run only once the machine has settled
cargo build --locked --release --manifest-path spikes/agg-baseline/Cargo.toml
(ulimit -v 524288; /usr/bin/time -f 'max_rss_kib=%M' timeout 120 spikes/agg-baseline/target/release/agg-baseline)
python3 spikes/agg-baseline/summarize.py bench/results/raw/20261107-hash-aggregate-baseline/bench.txt
cargo fmt --manifest-path spikes/agg-baseline/Cargo.toml -- --check
cargo clippy --locked --manifest-path spikes/agg-baseline/Cargo.toml --all-targets -- -D warnings
scripts/check-leaks.sh --staged
```

Full native tests, bounded native tests, four bound-2 models, all three mutants,
fmt and clippy (`-D warnings`) pass; checks ran sequentially.

Raw local outputs: [tests](raw/20261107-hash-aggregate-baseline/tests.txt),
[full tests](raw/20261107-hash-aggregate-baseline/full-tests.txt),
[models/mutants](raw/20261107-hash-aggregate-baseline/loom.txt),
[timings/counters](raw/20261107-hash-aggregate-baseline/bench.txt),
[summary](raw/20261107-hash-aggregate-baseline/summary.txt),
[host/quiet check](raw/20261107-hash-aggregate-baseline/host.txt).
