# Hash aggregate — performance step 1 (2026-11-07)

**Local, indicative, NOT a target verdict; local elapsed times cannot attribute gains.** Card 0005's frozen targets are
unchanged. This step changes hashing, cancellation polling, borrowed group-key
lookup and reservation amortization only. R12/P11, P4, P5 and spill volume remain
step 2 work. No GCP, push or merge.

## Method and limits

Local x86-64 Core i3-1220P KVM guest, eight visible logical CPUs.
[Machine/toolchain and input identity](raw/20261107-hash-aggregate-perf1/machine.json).

SF1 (6,001,215 lineitem rows), warm local Parquet; the complete card Q1 body and
high-cardinality HAVING query, 1/8 requested threads. Existing memory admission
can reduce task concurrency at small budgets. Memory controls explicitly use 8 GiB
for both engines; spill controls use 64 MiB for both, with an explicit 4 GiB
Nata disk quota so the already-documented small default disk cap cannot obscure
CPU work. The quota is a measurement override, not a changed default. The historical baseline/hash
stages validate values before timing (exact numeric multisets; only Q1's three
AVG columns use absolute 1e-8 tolerance). Each cell has five alternating
Nata/DuckDB process pairs, switching which engine goes first between rounds.
Startup, planning, decode and CSV rendering are included; timed CSV goes to
`/dev/null`. DuckDB OOM/failure gives no denominator. Spill correctness uses
DuckDB's memory control as oracle, not an incomplete 64 MiB answer.

Raw JSON retains all samples, exits, per-sample 1/5/15-minute machine loads,
and pre-timing gate counters (real spill, peak reservations, requests/releases).
Native tests separately assert that cleanup returns held bytes/live grants to zero.
This is a shared development machine; other sessions compile concurrently.
Neither isolation nor paired old/new Nata binaries in each round is claimed:
adjacent stages are the before/after controls, DuckDB runs in every stage.
The parent rejected local before/after elapsed-time attribution because shared
load and DuckDB drift dominate 10–30% changes. The historical tables below are
descriptive sanity evidence only. Subsequent fixes use local CPU-clock symbol
profiles and one sanity timing; an attributed timing table awaits the parent’s
settled c3d-highcpu-8 heavy job. ARM64 is unmeasured here.

Historical baseline/hash timing reproduction only (superseded; do not spend
more local rounds on it):

```sh
mkdir -p "$HOME/tmp-perf1/spill"
export TMPDIR="$HOME/tmp-perf1"
cargo build --release -p nata -p nata-check
python3 scripts/bench-aggregate-perf1.py STAGE --candidate target/release/nata --data DATA_SF1 --out "$TMPDIR"
```

## Fix 1 — canonical column hashing

Use the already-locked **twox-hash 2.1.5**, newly direct with defaults disabled
and only `xxhash3_64` enabled (no dependency additions). Parent decision:
prefer seeded XXH3-64; aHash 0.8.12 was rejected because AES target features
change its output, while foldhash is absent from the lockfile. This choice is
recorded here for the parent's ADR 0010 amendment; no accepted ADR is edited.

Version-1 hashing uses explicit type domains, little-endian scalar bytes,
UTF-8 values rather than dictionary codes, NULL distinct from empty text,
canonical NaN/zero, ordered column combination and fixed seed
`0x6e61746100000001`. Recursion uses that seed XOR depth; equal keys stay
together and each depth gets a fresh partition distribution. The table and
spill use the same canonical function. Column hashes are computed once per
selected batch and reused for probes. The extra hash vector is explicitly
included in the decode reservation. Full key equality still resolves collisions.

Fixed vectors at depths 0/1/8 were independently calculated using system
libxxhash 0.8.3 and checked against the locked Rust implementation. Unit tests
also compare batch hashes against owned keys for every supported key family,
selected rows, NULL junk, dictionary changes and canonical floats. These tests
will run unchanged on ARM64; this local run is x86-64 only.

[Unsafe inventory](raw/20261107-hash-aggregate-perf1/unsafe-exec.txt): twox-hash
has 75 textual unsafe keywords across 13 source files, including disabled
features/tests; this count is an upper bound, not a soundness proof. It was
already transitive. XXH3 includes portable scalar and SSE2/AVX2/NEON paths.
No own unsafe code is added.

**Harness correction, 2026-11-07:** the first hash-stage spill gate exposed an
invalid new assertion equating reservation requests with releases. Requests
include pending retries; only grants release. No spill timing had started.
The harness now checks peak ≤ B and releases ≤ requests; native cleanup tests
inspect held bytes/live grants directly. The four completed memory cells and
all their samples were retained unchanged, then the two untouched spill cells
were resumed. This is not a failed engine result or an extra timing round.

Seconds **min/median/max**, five rounds per cell. Load is the 1-minute load
range across Nata and DuckDB samples; raw JSON also retains 5/15-minute loads.

| Query | Threads | Nata before | Nata after | DuckDB before | DuckDB after | Load before → after |
|---|---:|---|---|---|---|---|
| q1 memory | 1 | 4.929/5.158/8.526 | 5.185/5.851/8.718 | 0.251/0.271/0.327 | 0.313/0.415/0.686 | 5.4–6.5 → 5.7–7.2 |
| q1 memory | 8 | 3.440/4.670/4.876 | 3.330/3.496/3.933 | 0.150/0.162/0.205 | 0.163/0.210/0.226 | 5.5–6.4 → 5.6–5.8 |
| having memory | 1 | 7.108/18.299/24.173 | 5.419/6.749/7.053 | 1.871/3.492/6.656 | 1.445/1.632/1.874 | 4.6–16.5 → 3.5–4.5 |
| having memory | 8 | 6.719/8.114/12.708 | 5.512/6.091/6.669 | 1.566/3.759/4.737 | 1.353/1.563/2.488 | 8.8–11.1 → 2.9–4.6 |
| having 64 MiB | 1 | 23.329/23.926/36.366 | 13.915/15.203/18.523 | 1.899/1.951/4.521 | 1.479/1.512/1.641 | 3.0–5.8 → 1.3–1.7 |
| having 64 MiB | 8 | 21.896/27.284/28.208 | 18.055/18.314/22.899 | 1.617/1.809/2.089 | 1.516/1.685/2.514 | 2.7–5.0 → 1.9–3.4 |

Raw: [baseline](raw/20261107-hash-aggregate-perf1/baseline.json), [hash](raw/20261107-hash-aggregate-perf1/hash.json).

Historical Q1 medians at eight threads were 4.670 and 3.496 s. The one-thread
Q1 cell is unresolved (both Nata and DuckDB slowed); baseline HAVING has very
large load/noise drift. These historical samples cannot attribute gains, even though only one source
fix changed between stages. No both-CPU or frozen-target
performance claim follows.

### Local profile comparison (CPU-clock user self samples)

Eight requested threads, same SF1 input; CPU shares, **not elapsed gains**.

| Q1 hash-related user self symbols | Before | After |
|---|---:|---:|
| SipHash write + owned-key hashing + Atom::hash_slice | 17.18% | no samples |
| Column hashes + value helper | absent | 2.41% |

Baseline Q1 has SipHash write **10.86%**, owned-key hashing **3.20%**, and
`Atom::hash_slice` **3.12%**. Hash-only Q1 has no sampled SipHash/Atom hash-slice
symbol; column hashing is **1.55%** plus **0.86%** in its value helper. Cancellation (**14.91%**) and contended
mutex locking (**11.72%**) now lead the profile, followed by scalar arithmetic.
All symbols (including the long tail) and the HAVING memory/spill controls are
retained in [baseline Q1](raw/20261107-hash-aggregate-perf1/baseline-q1.json),
[hash Q1](raw/20261107-hash-aggregate-perf1/hash-q1.json), and the adjacent
`baseline-*` / `hash-*` raw profiles. Profile metadata records load, binary
identity, and aggregate counters; elapsed time under profiling is not a timing
sample. Sampling uses `perf record -e cpu-clock:u -F 199 -g --call-graph dwarf,8192`;
GNU `c++filt -s rust` demangles Rust v0 symbols before sorting by self share.

The new recursion-seed mutant is **CAUGHT**, with the unmutated private-copy
control passing: fixed vectors fail and the recursive-pressure replay test also
fails. [Mutant](raw/20261107-hash-aggregate-perf1/mutants-hash.md).
[Required checks](raw/20261107-hash-aggregate-perf1/checks-hash.txt) all pass.

## Fix 2 — bounded cancellation polling

Partial accumulation polls at each batch/resume entry and at most 1,024 selected
rows apart in a larger batch. Ordinary production batches are capped at 256.
Resident merge/finalization and spill writes use the same bounded interval;
repartition polls once per already bounded IPC frame. ScanCancel's mutex,
registration/cancel handshake and memory-manager wakes are unchanged. Tests
count polls, trigger cancellation after an arbitrary resume cursor and during a
large batch, and require cleanup; they do not depend on wall-clock scheduling.

**Validation repair:** the first workspace run failed the existing wire
aggregate-backpressure test's assumption that socket buffers finish filling in
200 ms: produced-row snapshots were 1,958/2,825; an isolated run also failed at
2,771/2,797. The test now waits for an observed stall within its existing
10-second helper deadline, then keeps the unchanged-count, incomplete-output,
cancel/disconnect and released-grant assertions. No wire production code or
resource bound changes. The focused repaired test passes. The existing
unbounded-bridge and bridge-byte-bound mutants are rechecked below.

The parent superseded local elapsed-time attribution before this fix: only one
Q1 sanity timing and CPU-clock profiles are taken. Attributed timing is pending
the parent's settled-machine heavy job; no additional local timing rounds run.

| Q1 user self symbol | Before (hash only) | After (bounded polls) |
|---|---:|---:|
| ScanCancel::is_cancelled | 14.91% | 0.24% |
| Mutex::lock_contended | 11.72% | 0.48% |

Scalar `eval::arith` now leads at 12.25%; `Atom::get`, allocator/free work and
owned-key collection remain visible for fix 3. The one sanity run is 3.245 s at
1-minute load 5.40, **not an attributed elapsed gain**.
[Profiles](raw/20261107-hash-aggregate-perf1/cancel-profiles.json),
[all Q1 symbols](raw/20261107-hash-aggregate-perf1/cancel-q1.json),
[sanity](raw/20261107-hash-aggregate-perf1/cancel-sanity.json).
The new poll-removed and per-row-poll mutants are **2/2 CAUGHT**, zero invalid;
[results](raw/20261107-hash-aggregate-perf1/mutants-cancel.md).
Both existing wire-bound mutants are **CAUGHT**, with their private control
passing and the intended aggregate-bound assertion failing under the mutants:
[results](raw/20261107-hash-aggregate-perf1/mutants-cancel-wire.txt).
The complete [required check sequence](raw/20261107-hash-aggregate-perf1/checks-cancel.txt)
was rerun after the test repair and passes.

Current local profiling reproduction (no timing rounds):

```sh
python3 scripts/profile-aggregate-perf1.py STAGE --sanity --candidate target/release/nata --data DATA_SF1 --out "$HOME/tmp-perf1/profiles"
```

## Fix 3 — borrowed column lookup and a single probe

Existing groups are found with the batch's cached hash and value comparison
against borrowed columns. The returned group ID drives state updates directly,
replacing three probes and an owned `Vec<Atom>`/string key per row. Only a new
group allocates its owned key and states, after its entry/string capacity and
any table growth have been reserved. The per-entry reservation formula is
unchanged; merge/replay still own keys and use the same canonical hash.

A forced-collision test checks dictionary strings, NULL with an invalid junk
dictionary index, empty text, canonical zero/NaN, missing keys and key-arity
mismatches. Scalar arithmetic, aggregate state updates and argument evaluation
are unchanged. This removes group-key allocations for existing groups; it does
not implement step 2's shared/vectorized arithmetic kernels.

| Q1 user self symbol | Before (bounded polls) | After (borrowed lookup) |
|---|---:|---:|
| Owned-key Vec collection in Partial::resume | 2.26% | no samples |
| Atom slice equality | 3.38% | no samples |
| malloc | 4.27% | 2.67% |
| cfree | 4.59% | 1.73% |
| Atom::get (also used for aggregate arguments) | 6.53% | 4.08% |

Scalar `eval::arith` / `num::fits` lead at 15.38% / 8.48%; allocations for
batch evaluation and new groups remain. The one sanity run is 1.084 s at load
2.33, **not an attributed gain**. Q1 still makes 23,499 reservation requests;
fix 4 addresses those independently.
[Profiles](raw/20261107-hash-aggregate-perf1/borrow-profiles.json),
[all Q1 symbols](raw/20261107-hash-aggregate-perf1/borrow-q1.json),
[sanity](raw/20261107-hash-aggregate-perf1/borrow-sanity.json).
The borrowed-equality mutant is **CAUGHT**, with a passing private control and
zero invalid: [result](raw/20261107-hash-aggregate-perf1/mutants-borrow.md).
All [required checks](raw/20261107-hash-aggregate-perf1/checks-borrow.txt) pass.

## Fix 4 — task-owned scratch reservation chunks

The 23,499 Q1 requests were predominantly the string-expression scratch grant
created and released for every 256-row batch, rather than table growth. Each
scan task now retains a charged high-water capacity in the existing `Chunks`
service. Chunks are sized from the batch requirement (at least 64 KiB, clipped
to available capacity by the manager); only growth consumes new credit.
Existing table chunks stay charged and zero-byte table reservations return
immediately. No budget, progress reserve, ledger, gauge, debug assertion or
memory-manager synchronization changes.

`ScanState` drops evaluated/input allocations before scratch chunks and the
decode grant; chunks release on every task exit. The native test repeats 100
batches with one request, grows within spare credit, grows to another chunk,
and requires held bytes/live grants/pending requests to return to zero.

A second test exposed an initial-draft error: another task's differently sized
pending request was propagated as a minimum-set error. The negative control
fails at that exact path; the final cache treats denial/conflict as pressure,
preserving the previous caller's spill/retry and minimum-set error behavior.
After release/pressure cleanup, the test retries successfully and checks the
budget/cleanup invariants. [Negative control](raw/20261107-hash-aggregate-perf1/scratch-pressure-negative.txt),
[fixed control](raw/20261107-hash-aggregate-perf1/scratch-pressure-positive.txt).
All [required checks](raw/20261107-hash-aggregate-perf1/checks-chunks.txt)
pass on that final implementation. The new chunk-release mutant is CAUGHT
by both scratch tests, with passing unmutated controls; it is among the
[first 15 completed catches](raw/20261107-hash-aggregate-perf1/mutants-first-segment.txt).
The final profile is recorded below after the sequential sweep.

| Q1 counter / self symbol | Before (borrowed lookup) | After (task chunks) |
|---|---:|---:|
| Reservation requests | 23,499 | 67 |
| Reservation releases | 23,499 | 67 |
| MemoryManager::request self samples | 0.31% | no samples |
| Mutex::lock_contended | 1.26% | 0.93% |
| Peak reservation (bytes; B = 8 GiB) | 152,135,208 | 152,530,216 |

The request count is down by a deterministic workload count, not a local
elapsed-time claim. Numeric-key HAVING's memory control stays at 2,592 requests:
it did not have per-batch string scratch. The one Q1 sanity run is **1.344 s**
at load **1.64**, not an attributed gain. The small mutex share change is
sampling/noise-limited. [Profiles and counters](raw/20261107-hash-aggregate-perf1/chunks-profiles.json),
[Q1 sanity](raw/20261107-hash-aggregate-perf1/chunks-sanity.json).

### Cumulative mutation verification

The initial full-set sweep completed 15 real catches, including all five new
mutants and every existing aggregate/spill mutant, then the old
`case-evaluates-every-branch` mutant ran the newer deep aggregate CASE fixture
exponentially before reaching its original guard check. That incomplete run
was terminated and **is not counted as a catch or a timeout catch**. The CASE
mutant now runs its existing `case_evaluates_only_the_branch_a_row_takes` test;
full unmutated exec/CLI controls still run, and other mutants retain their full
suites. The remaining 48 mutants passed with that bounded test selection.
**All 63 executor/CLI mutants are CAUGHT (58 existing, five new), zero
SURVIVED, zero INVALID**, with passing private unmutated exec/CLI controls.
[Combined results](raw/20261107-hash-aggregate-perf1/mutants-all.md),
[remaining 48](raw/20261107-hash-aggregate-perf1/mutants-remaining.md).
The [outcome-classifier self-test](raw/20261107-hash-aggregate-perf1/mutant-classifier.txt)
passes; compile failures/global timeouts remain INVALID.
All [five memory mutants](raw/20261107-hash-aggregate-perf1/mutants-memory.txt)
are **CAUGHT by loom** at bound 2 (including explicit budget/credit/double-release
assertions and lost-wakeup/release deadlocks); unmutated native memory checks
and all 16 bound-2 memory models pass. The two existing wire-bound mutants
remain CAUGHT as recorded under fix 2.

## Final profile and step 2

**Local, indicative, NOT a target verdict.** CPU-clock user self shares, eight
requested threads, SF1. Q1 / HAVING memory / HAVING 64 MiB have
756 / 1,126 / 2,450 samples and zero reported lost samples. Starting
1-minute loads are 1.64 / 1.64 / 1.67. No elapsed attribution follows.
Full demangled names and the long tail are retained in the linked raw JSON.
[Per-fix evidence chart](raw/20261107-hash-aggregate-perf1/profile-summary.html).

### Q1 memory

[All symbols](raw/20261107-hash-aggregate-perf1/chunks-q1.json).

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 11.77% | `nata_exec::eval::arith` |
| 2 | 11.24% | `<nata_exec::aggregate::Partial>::resume` |
| 3 | 10.71% | `nata_exec::num::fits` |
| 4 | 5.82% | `<nata_exec::aggregate::State>::add` |
| 5 | 4.37% | `<nata_kernels::aggregate::ExactSum>::add` |
| 6 | 4.37% | `<nata_exec::aggregate::Atom>::get` |
| 7 | 3.44% | `<nata_exec::value::Col>::from_arrow` |
| 8 | 2.25% | `nata_exec::aggregate::hashing::columns` |
| 9 | 2.25% | `_int_malloc` |
| 10 | 1.98% | `nata_exec::aggregate::hashing::value` |
| 11 | 1.98% | `cfree@GLIBC_2.2.5` |
| 12 | 1.85% | `<nata_exec::eval::Evaluator>::eval` |
| 13 | 1.85% | `malloc` |
| 14 | 1.72% | `nata_exec::num::pow10` |
| 15 | 1.59% | `_int_free_chunk` |

### HAVING memory

[All symbols](raw/20261107-hash-aggregate-perf1/chunks-high_cardinality_tpch.json).

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 12.08% | `nata_exec::aggregate::hashing::key` |
| 2 | 6.93% | `<nata_exec::aggregate::Partial>::resume` |
| 3 | 4.53% | `<nata_exec::aggregate::State>::finalize` |
| 4 | 3.55% | `malloc` |
| 5 | 3.55% | `unlink_chunk.isra.0` |
| 6 | 3.46% | `hashbrown::RawTable<usize>::reserve_rehash` |
| 7 | 3.02% | `nata_exec::eval::arith` |
| 8 | 2.75% | `_int_free_chunk` |
| 9 | 2.31% | `__memmove_avx_unaligned_erms` |
| 10 | 2.22% | `_int_malloc` |
| 11 | 2.13% | `nata_exec::num::fits` |
| 12 | 1.87% | `<nata_exec::aggregate::State>::add` |
| 13 | 1.78% | `<arrow_buffer::buffer::scalar::ScalarBuffer<f64>>::slice` |
| 14 | 1.69% | `<nata_exec::aggregate::Groups>::insert` |
| 15 | 1.69% | `<alloc::sync::Arc<dyn arrow_array::array::Array>>::drop_slow` |

### HAVING 64 MiB

[All symbols](raw/20261107-hash-aggregate-perf1/chunks-high_cardinality_tpch-64MiB.json).

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 6.49% | `_int_malloc` |
| 2 | 4.90% | `malloc` |
| 3 | 4.29% | `malloc_consolidate` |
| 4 | 3.92% | `_int_free_chunk` |
| 5 | 3.31% | `__memmove_avx_unaligned_erms` |
| 6 | 3.31% | `cfree@GLIBC_2.2.5` |
| 7 | 2.08% | `replay::decode state collection (Iterator::try_fold)` |
| 8 | 2.08% | `nata_exec::aggregate::replay::encode` |
| 9 | 1.76% | `nata_exec::eval::arith` |
| 10 | 1.71% | `realloc` |
| 11 | 1.67% | `unlink_chunk.isra.0` |
| 12 | 1.63% | `flatbuffers::TableVerifier::visit_field (IPC schema fields)` |
| 13 | 1.59% | `<nata_exec::aggregate::Partial>::resume` |
| 14 | 1.55% | `flatbuffers::verifier::append_trace::<()>` |
| 15 | 1.55% | `<flatbuffers::verifier::TableVerifier>::deref` |

Step 2 must address the following measured remaining work; none is implemented here:

- **R12/P11 kernels:** scalar `eval::arith` (11.77%) and decimal `num::fits`
  (10.71%) lead Q1. Move evaluation onto shared selected-valid strict kernels
  and vectorized loops while preserving errors, NULL masking and exact types.
- **P4 fusion:** Q1 still spends 11.24% in `Partial::resume`, 5.82% in
  `State::add`, 4.37% each in `ExactSum::add` and `Atom::get`. Fuse evaluation
  and accumulation, including repeated Q1 expressions, against a measured control.
- **P5 overflow elision:** only remove repeated checks where a proof establishes
  the same accepted bounds and failure behavior. No elision occurs in this step.
- **Spill volume:** allocation/free, replay encode/decode and checked IPC
  metadata dominate the spill profile. HAVING at 64 MiB writes 960,268,264
  bytes, reads 1,219,572,960 bytes and opens 300,682 spill files (4,368 spilled
  partitions, recursion depth 2). Reduce partial/frame/open volume; preserve
  envelope verification, reservations and quotas. Resident HAVING also exposes
  canonical owned-key hashing (12.08%), state finalization (4.53%) and rehash
  (3.46%): track those costs when assessing fusion/volume changes.

Frozen targets, SF10 scaling and both-architecture certification remain open.
Local elapsed times and these self-share profiles cannot close them.

## Parent timing job — attribution pending

The four cumulative fixes are `94a7834` (hash), `3748b56` (polls), `8b8873b`
(borrowed lookup), and `135fd7d` (chunks). The parent will add the attributed
before/after table after the settled-machine run; no local timing ratio is
used to claim a gain.

[Harness](../../scripts/bench-aggregate-perf1-heavy.py) and
[heavy recipe](../aggregate-perf1.steps). Prepare the source bundle locally
before shipping the tree, since the existing uploader excludes `.git`:

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline e198d08 --fix hash=94a7834 --fix polls=3748b56 --fix borrow=8b8873b --fix chunks=135fd7d --bundle .aggregate-perf1-inputs
cp bench/aggregate-perf1.steps heavy.steps
```

Only the parent launches the existing `heavy` job, on **c3d-highcpu-8**.
The bundle and `heavy.steps` are ignored locally; the existing tree uploader
includes them. Five SHA-256-checked committed source archives build sequentially
with `--locked --release`, each in a distinct target directory. The run refuses
a CPU count/architecture other than x86-64/eight logical CPUs. It records input
and binary identities, toolchain, CPU model, all 1/5/15-minute loads, exits and
aggregate counters, without host/network identifiers.

The matrix is Q1 and HAVING × 1/8 threads × 8 GiB/64 MiB, five alternating
rounds of baseline, each cumulative fix and DuckDB, with an identical-binary
primary/twin arm for each: **480 timed processes**. Reverse the entire arm
order on alternate rounds. Match engine budgets/threads; include startup,
planning, metadata, decode and CSV-to-`/dev/null`. Correctness-gate all 40 Nata
cells against DuckDB before **any** timing; require peak ≤ B, no memory-control
spill and real HAVING spill at 64 MiB. Q1 AVG alone has the card's 1e-8 tolerance;
all other numbers/multiplicities match exactly. Settle below load 0.5 before
each cell and refuse an unsettled cell. No correctness/build job overlaps a
timed cell. Each restarted heavy timing step creates a new session directory;
incomplete sessions never mix.

`summary.json` reports min/quartiles/median/max for five primary and five twin
samples per variant/cell, paired candidate/baseline, candidate/previous-fix
and candidate/DuckDB latency ratios, and twin/primary ratios. A failed DuckDB
cell has no denominator. The parent compares each change with its twins/noise
and adds the attributed table here; the harness makes no target verdict.
SF10 scaling and ARM64 remain outside this SF1 job.

[Local harness checks](raw/20261107-hash-aggregate-perf1/heavy-self-test.txt)
launch no engines: self-test covers exact numeric
multisets/multiplicity, Q1 AVG tolerance versus exact SUM, truncated CSV,
archive tampering, alternating twins, distinct targets, matched budgets,
paired ratios and missing DuckDB denominators. An inert end-to-end run also checks five builds, all 40 gates before any
timed arm, and the complete 480-arm schedule. Preparing all five actual
archives and checking both queries in each archive, then `run --plan`, passed; the [saved plan](raw/20261107-hash-aggregate-perf1/heavy-plan.json)
contains five separate targets and all 480 planned processes.

```sh
python3 scripts/bench-aggregate-perf1-heavy.py self-test --work "$HOME/tmp-perf1/self-test"
python3 scripts/bench-aggregate-perf1-heavy.py run --bundle .aggregate-perf1-inputs --work "$HOME/tmp-perf1/heavy" --data DATA_SF1 --out "$HOME/tmp-perf1/heavy-results" --plan
```

## Validation and handoff

Before each production fix commit: `cargo fmt --check`, workspace/all-target
clippy with `-D warnings`, `cargo test --workspace`, `cargo deny check`, query
gates (scan 5, SQL 252, SQL differences 54, aggregate 65, forced-spill aggregate
65), all 16 memory loom models at bound 2, and staged leak scan **pass**.
Disk TMPDIR/spill storage and two build/test workers are used; checks run
sequentially. The final tooling commit repeats that entire sequence:
[check record](raw/20261107-hash-aggregate-perf1/checks-harness.txt).

Cumulative relevant mutation checks: **63 executor/CLI + five memory + two
wire-bound = 70 CAUGHT**, zero survivors/invalids. The five new mutants cover
recursion seed, removed/row-wise cancel polls, borrowed equality and chunk
release. The independent negative/positive pressure regression is retained.
The one interrupted exponential CASE attempt is excluded, as explained above.

The existing heavy-job chain test also passes using its local inert runner,
without a VM or cloud calls. The source bundle/plan and full simulated harness
trial pass. The parent runs the real settled-machine job and adds attribution;
no frozen target is judged here, and step 2 implementation is not started.

## Step 1 attributed timing on a settled machine (run `perf1-x86-r2`, added by the parent, 2026-11-07)

One settled Spot c3d-highcpu-8 (x86-64), five revisions built from committed archives (baseline `490d116`, then the four fixes
cumulatively), 5 alternating rounds with a twin per arm, DuckDB 2.0.0 alongside, SF1. VM compute $0.078. Raw: `raw/20261107-hash-aggregate-perf1-gcp/`.
A first attempt (`perf1-x86`) failed in two minutes on Python 3.11.2's missing `extractall(filter=)`; fixed in `eb17021`.

Medians in seconds (min/quartiles/max and twins are in `summary.json`):

| Query, threads, budget | baseline | +XXH3 hash | +batch cancel | +borrowed keys | +chunked reservations | DuckDB | final / DuckDB |
|---|---:|---:|---:|---:|---:|---:|---:|
| Q1, 1, in memory | 4.071 | 3.236 | 3.230 | 2.822 | 2.854 | 0.254 | 11.2x |
| Q1, 1, 64 MiB | 4.298 | 3.433 | 3.345 | 2.907 | 2.846 | 0.263 | 10.8x |
| Q1, 8, in memory | 1.625 | 1.431 | 1.394 | 1.184 | 1.190 | 0.116 | 10.3x |
| Q1, 8, 64 MiB | 1.927 | 1.862 | 1.610 | 1.498 | 1.440 | 0.116 | 12.4x |
| high cardinality, 1, in memory | 5.923 | 5.020 | 4.830 | 4.756 | 4.659 | 1.171 | 4.0x |
| high cardinality, 1, 64 MiB | 15.398 | 14.378 | 14.178 | 13.582 | 13.501 | 1.318 | 10.2x |
| high cardinality, 8, in memory | 3.859 | 3.586 | 3.376 | 3.327 | 3.353 | 0.931 | 3.6x |
| high cardinality, 8, 64 MiB | 18.296 | 17.564 | 17.378 | 17.328 | 17.633 | 1.251 | 14.1x |

**Verdict against the frozen targets: NOT MET in every cell** (Q1 ≤1.5x, hash queries ≤2x, spill ≤2x). Step 1 removed its overheads
(about 25-30% on Q1, 13-21% on the high-cardinality query) but leaves the row-at-a-time computation (step 2a) and the spill volume
(step 2d, almost untouched by step 1: -4% at 8 threads) as the gap.

## Performance step 2a — shared column kernels and group vectors

**Verdicts:** all required local checks pass; **7/7 focused runtime mutants are
CAUGHT**, zero survivors/invalids; **4/4 Kani harnesses pass, 8/8 covers**, and a
private Kani precision-check mutant is CAUGHT after its control passes. Full
existing mutant sweeps remain for the parent heavy job. **No performance target
is assessed or claimed met.** The final local sanity is slower, as recorded
below; there are no local twins or settled-machine denominators.

The frozen rule remains: full Q1 ≤1.5× DuckDB, agg_orderkey/agg_partkey ≤2×,
spill ≤2× in matched-budget cells where DuckDB completes, SF10 8/1 scaling ≥5×
ARM64 / ≥3.5× x86-64. No GCP run was made. Parent measurement is required.
[Machine, toolchain and SF1 input identity](raw/20261107-hash-aggregate-perf2a/machine.json);
[production source hashes](raw/20261107-hash-aggregate-perf2a/source-sha256.txt).

### Fresh local profiles and sanity times

SF1, eight requested threads, CPU-clock user self sampling at 199 Hz with DWARF
call graphs. Shares are relative, functions changed, and work moved into the
kernel; neither a disappearing symbol nor a lower share proves a lower latency.
The historical step 1 profile is not reused as this comparison's denominator.

| Self symbol | af2a5a2 before | Final |
|---|---:|---:|
| `nata_exec::eval::arith` | 12.87% | 0.21% |
| `nata_exec::num::fits` | 7.54% | 0.00% |
| `aggregate::Partial>::resume` | 9.62% | 15.51% |
| `decimal::Decimal>::column` | 0.00% | 11.11% |
| `aggregate::ExactSum>::add` | 5.18% | 4.30% |

![Local Q1 self sample shares](raw/20261107-hash-aggregate-perf2a/profile-summary.svg)

| Stage | One Q1 CLI sanity, s | Starting load (1 minute) |
|---|---:|---:|
| af2a5a2 | 1.136 | 3.23 |
| Column kernels and group vectors | 1.064 | 1.53 |
| Final: checked i64 multiply and in-place narrow SUM | 2.199 | 3.31 |

**Indicative only:** one completion sanity per change, without alternating
controls. Concurrent development work was present. The slower final observation
is retained; it is not filtered out or attributed to a particular change.
No further local timing rounds were run. The intermediate profile exposed SUM
state reconstruction; assembly inspection motivated updating the narrow sum in
place, with the same checks. There is no isolated timing claim for that fix.

| Profile | Before samples / lost | Final samples / lost | Before / final load (1 min) |
|---|---:|---:|---:|
| Q1 memory | 676 / 0 | 954 / 0 | 3.23 / 3.31 |
| HAVING memory | 1007 / 0 | 1522 / 0 | 3.61 / 3.13 |
| HAVING 64 MiB | 2418 / 0 | 3663 / 0 | 3.32 / 3.12 |

#### Final top 15: Q1 memory

[All symbols and counters](raw/20261107-hash-aggregate-perf2a/inplace-q1.json); [text](raw/20261107-hash-aggregate-perf2a/inplace-q1.txt). Long names below are shortened after 180 characters.

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 15.51% | `<nata_exec::aggregate::Partial>::resume` |
| 2 | 11.11% | `<nata_kernels::decimal::Decimal>::column` |
| 3 | 4.30% | `<nata_kernels::aggregate::ExactSum>::add` |
| 4 | 3.98% | `<nata_exec::value::Col>::from_arrow` |
| 5 | 3.88% | `_int_malloc` |
| 6 | 3.67% | `__memmove_avx_unaligned_erms` |
| 7 | 3.56% | `malloc` |
| 8 | 3.04% | `nata_exec::aggregate::hashing::columns` |
| 9 | 2.31% | `std::sys::backtrace::__rust_begin_short_backtrace::<nata_sched::cooperative::run<nata_exec::aggregate::pipeline::ScanState, alloc::vec::Vec<nata_exec::aggregate::Table>, nata_pa...` |
| 10 | 2.10% | `__memset_avx2_unaligned_erms` |
| 11 | 2.10% | `cfree@GLIBC_2.2.5` |
| 12 | 1.99% | `_int_free_chunk` |
| 13 | 1.68% | `nata_exec::aggregate::hashing::value` |
| 14 | 1.57% | `<std::sys::sync::mutex::futex::Mutex>::lock_contended` |
| 15 | 1.57% | `<arrow_array::array::primitive_array::PrimitiveArray<arrow_array::types::IntervalMonthDayNanoType> as arrow_array::array::Array>::is_valid` |

#### Final top 15: HAVING memory

[All symbols and counters](raw/20261107-hash-aggregate-perf2a/inplace-high_cardinality_tpch.json); [text](raw/20261107-hash-aggregate-perf2a/inplace-high_cardinality_tpch.txt). Long names below are shortened after 180 characters.

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 11.83% | `nata_exec::aggregate::hashing::key` |
| 2 | 10.38% | `<nata_exec::aggregate::Partial>::resume` |
| 3 | 5.19% | `malloc` |
| 4 | 4.34% | `<nata_exec::aggregate::State>::finalize` |
| 5 | 3.88% | `unlink_chunk.isra.0` |
| 6 | 3.42% | `_int_malloc` |
| 7 | 2.69% | `cfree@GLIBC_2.2.5` |
| 8 | 2.56% | `_int_free_chunk` |
| 9 | 2.50% | `<nata_kernels::decimal::Decimal>::column` |
| 10 | 2.37% | `<hashbrown::raw::RawTable<usize>>::reserve_rehash::<<nata_exec::aggregate::Groups>::try_reserve::{closure#0}>` |
| 11 | 2.30% | `__memmove_avx_unaligned_erms` |
| 12 | 1.64% | `<nata_exec::aggregate::Groups>::insert` |
| 13 | 1.38% | `<snap::decompress::Decoder>::decompress` |
| 14 | 1.38% | `syscall` |
| 15 | 1.18% | `<nata_kernels::aggregate::ExactSum>::decimal_sum` |

#### Final top 15: HAVING 64 MiB

[All symbols and counters](raw/20261107-hash-aggregate-perf2a/inplace-high_cardinality_tpch-64MiB.json); [text](raw/20261107-hash-aggregate-perf2a/inplace-high_cardinality_tpch-64MiB.txt). Long names below are shortened after 180 characters.

| Rank | Self | Symbol |
|---:|---:|---|
| 1 | 5.95% | `_int_malloc` |
| 2 | 5.02% | `malloc` |
| 3 | 3.99% | `malloc_consolidate` |
| 4 | 3.63% | `_int_free_chunk` |
| 5 | 3.19% | `cfree@GLIBC_2.2.5` |
| 6 | 2.87% | `__memmove_avx_unaligned_erms` |
| 7 | 2.07% | `<nata_exec::aggregate::Partial>::resume` |
| 8 | 1.91% | `<core::iter::adapters::map::Map<core::slice::iter::Iter<nata_sql::bind::AggregateExpr>, nata_exec::aggregate::replay::decode::{closure#2}> as core::iter::traits::iterator::Itera...` |
| 9 | 1.77% | `flatbuffers::verifier::append_trace::<()>` |
| 10 | 1.77% | `realloc` |
| 11 | 1.77% | `unlink_chunk.isra.0` |
| 12 | 1.58% | `__syscall_cancel_arch` |
| 13 | 1.56% | `nata_exec::aggregate::replay::encode` |
| 14 | 1.31% | `<flatbuffers::verifier::TableVerifier>::visit_field::<flatbuffers::primitives::ForwardsUOffset<flatbuffers::vector::Vector<flatbuffers::primitives::ForwardsUOffset<arrow_ipc::ge...` |
| 15 | 1.28% | `__memset_avx2_unaligned_erms` |

Reproduce profiles with a separately built baseline/final binary, on the same
SF1 data (this runs each query once under perf and Q1 once as a completion sanity):

```sh
python3 scripts/profile-aggregate-perf1.py STAGE --sanity --candidate BINARY --data DATA_SF1 --out DISK_DIRECTORY
```

Implementation:

- Arithmetic on integer and decimal argument columns now calls `nata-kernels`.
  The decimal kernel prepares scale factors and the result limit once per
  expression, dispatches ADD/SUB/MUL outside the selected-row loop, and retains
  checked machine arithmetic, both ADD/SUB operand-fit checks, and the result
  precision check. Decimal64 results use checked i64 multiplication after
  checked operand conversions, with checked i128 fallback for wide operands.
  A `2^59 * 32` sentinel catches a 64-bit wrap to zero. Errors retain the expression position and error class.
  Numeric cast/negation and comparison traversal use shared selection/validity
  adapters; exact rescaling/rounding helpers have one shared implementation.
  Existing text/date/float conversion and comparator semantics remain in their
  original adapters: this is not a claim that every scalar operation is proved.
- Hashes still use step 1's canonical XXH3 columns and collision-resolving
  `hashbrown` table. A bounded group-index vector resolves probe/insert first;
  aggregate columns then update from those IDs, with separate validity per
  argument. COUNT avoids constructing an Atom; SUM/AVG dispatch the input type
  once per column. MIN/MAX make column-wise passes with their existing typed
  comparator. Owned keys/state arenas and spill records remain compatible.
- SUM/AVG use checked i128 partials, promoting to the existing checked i256
  representation when i128 overflows. Promotion is exact, not an SQL overflow;
  final precision is checked only after merge. All value/count checks remain.
  Spill still writes the same 32 little-endian sum bytes plus non-NULL count;
  the validated IPC reader is unchanged. Existing oracle/boundary tests cover
  wide cancellation, AVG without narrowing SUM, and malformed replay.
- Each admitted prefix is updated before pressure spills its table; the denied
  row is retried with fresh IDs. A 1024-row vector is stack-bounded, and its
  16 KiB is charged in each task's decode/expression grant before work starts.
  Entry and 1024-row cancellation polls, string growth grants, partitioned
  recursive merge and cleanup are retained. No custom dense-key table was added.

### Checks and mutation evidence

With disk-backed `TMPDIR`, fmt, workspace clippy with warnings denied, workspace
tests, cargo deny, release build, scan **5/5**, SQL **252/252**, SQL differences
**55/55**, aggregate **65/65**, forced-spill aggregate **65/65**, and all **16**
memory loom models at bound 2 pass. No dependency or accepted ADR changed.
[Check record](raw/20261107-hash-aggregate-perf2a/checks.txt). Staged leak scanning is recorded there.

An unchanged scanner test initially reported `not_read=1, abandoned=0`: the slow
task had not started when LIMIT completed. Its isolated rerun and the complete
workspace rerun with `RUST_TEST_THREADS=1` pass, including all 15 parallel-scan
tests. No scanner test or implementation was changed. [Failure](raw/20261107-hash-aggregate-perf2a/parallel-first-failure.txt),
[isolated retry](raw/20261107-hash-aggregate-perf2a/parallel-retry.txt).

The executor private control passes, then **5/5** targeted mutations are CAUGHT:
selection ignored, SUM NULL bitmap ignored, group index off by one, decimal
precision check removed, and the existing per-row cancel-poll mutant retargeted
inside group resolution. [Results](raw/20261107-hash-aggregate-perf2a/mutants-exec.md).
Both new kernel mutations are CAUGHT after private controls pass: [decimal
precision](raw/20261107-hash-aggregate-perf2a/mutants-kernels.txt), [checked i64 multiply replaced by wrapping](raw/20261107-hash-aggregate-perf2a/mutants-kernel-machine.txt).
The Kani precision-check negative control is [CAUGHT](raw/20261107-hash-aggregate-perf2a/kani-negative.txt), with
[passing private control](raw/20261107-hash-aggregate-perf2a/kani-negative-control.txt) and [failing proof](raw/20261107-hash-aggregate-perf2a/kani-negative-mutant.txt).
Timeouts/compile failures are not counted as catches in these results.

All **66** executor mutation definitions have unique live anchors, verified by
`test_mutate.py`; moved arithmetic/cast mutations target the called kernel code,
and the kernel crate is copied privately before mutation. This is not a claim
that the full 66-mutant sweep was rerun locally. Existing full sweeps remain
pending for the parent, using a disk-backed working directory and one job at a
time (the long sweeps belong on its approved heavy VM):

```sh
python3 crates/nata-exec/mutate.py --work WORK/exec --out RESULTS/exec.md
python3 crates/nata-kernels/mutate.py --jobs 1
python3 crates/nata-kernels/scripts/mutate_guards.py
```

### Proof coverage and limits

All four new harnesses pass, with **8/8 covers satisfied** (two covers per
harness). [MUL](raw/20261107-hash-aggregate-perf2a/kani-mul.txt), [ADD](raw/20261107-hash-aggregate-perf2a/kani-add.txt), [SUB](raw/20261107-hash-aggregate-perf2a/kani-sub.txt), [i128 boundary](raw/20261107-hash-aggregate-perf2a/kani-machine_overflow.txt). The harnesses include the actual production `decimal.rs` by path, following the
existing Kani crate. The two-row ADD/SUB/MUL proof uses arbitrary signed 8-bit
values, arbitrary validity vectors and all subset selections, a 2-digit result
bound, and independent exact scalar formulas, with an unwind bound of 3. The
machine-boundary harness (unwind 8) exercises both
signed i128 machine-overflow boundaries and NULL masking. Covers require both
error and successful selected/NULL paths.

Trust points: full-domain i128 multiplication, arbitrary precision/scale
combinations, integer type-range/MIN÷−1 logic, division/remainder and exact
cast rescaling are not proved by
these bounded harnesses. They retain checked std primitives and are exercised
by the independent bounded scalar tests, existing expected-value and SQL
boundary tests, and normal/forced-spill differential gates. The new i128-to-i256
promotion and the shared COUNT helper are test-backed trust points: the proof
spike has only a NullBuffer stub, not Arrow's real i256, so it cannot prove that
path. The unchanged Arrow i256/count state bounds remain those of ADR 0010.
No overflow check was removed under a reasoned bound (P5).

Parent proof commands (also suitable for rechecking the local results):

```sh
cd spikes/kani-nata-kernels
cargo kani --harness decimal_column_selected_null_and_bounds
cargo kani --harness decimal_column_add_selected_null_and_bounds
cargo kani --harness decimal_column_sub_selected_null_and_bounds
cargo kani --harness decimal_column_machine_overflow
```

### Settled parent timing job

The existing heavy harness accepts one to four cumulative revisions and an
explicit `--machine` for either requested 8-vCPU architecture. With one fix it
runs **16 Nata correctness cells before 240 timed processes**: Q1 and the
high-cardinality HAVING query, 1/8 threads, 8 GiB/64 MiB, five alternating rounds,
primary and twin for both Nata revisions and DuckDB. All engines include CLI
startup, planning, decode and CSV to `/dev/null`. Budgets are matched; failed
DuckDB cells have no denominator. Source archives, binary/input hashes, loads,
spill counters, paired ratios and twin noise remain recorded. The extended
inert self-test exercises both the old five-revision x86 job and the new
two-revision ARM job, including gates preceding all timings.

Prepare locally after committing, then let the parent copy the bundle with the
source and run the command separately on a settled **c3d-highcpu-8** and
**n4a-highcpu-8** (substitute the matching MACHINE and disk-backed WORK/RESULTS):

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline af2a5a2 --fix vector=HEAD --bundle .aggregate-perf2a-inputs
python3 scripts/bench-aggregate-perf1-heavy.py run --machine MACHINE --bundle .aggregate-perf2a-inputs --work WORK --data DATA_SF1 --out RESULTS
```

[Parent heavy recipe](../aggregate-perf2a.steps). No target verdict follows from
SF1 local profiles; the frozen SF10 scaling cells still need the parent's runs.

### Following steps

- **2b / P4:** independently measure expression-to-aggregate fusion, including
  repeated argument materialization and scans of the group vector. Retain
  selection, NULL and positioned-error phase semantics; keep a generic control.
- **2c / P5:** prove bounds for the actual called kernels and promote/merge
  paths, mutation-check those proofs, then independently measure any check
  elision. Step 2a removes no checks.
- **2d:** measure and reduce spill volume and allocator/IPC work at matched
  budgets, while retaining reader validation, quota/FD bounds and recursive
  cleanup. Step 2a intentionally retains the spill state format.

R12/P11 have this implementation and its evidence. Their register entries stay
open pending the parent's settled both-CPU verdict and the remaining
text/date/float conversion adapters; the bounded proofs above do not certify
those adapters or the full i128 input domain.


## Step 2a timing on settled machines (runs `perf2a-x86`, `perf2a-arm64`, added by the parent, 2026-11-08)

Baseline `af2a5a2` against the vectorized commit, both built from committed archives on the same VM, 5 alternating rounds with a twin,
DuckDB 2.0.0 alongside, SF1. VM compute $0.037 (x86-64) and $0.042 (ARM64). Raw: `raw/20261108-hash-aggregate-perf2a-gcp/`. The
child's one local sanity run (2.2 s against 1.1 s) was machine noise: the settled runs show no regression in any cell.

| Machine | Query | Threads | Budget | baseline s | vectorized s | change | DuckDB s | vectorized / DuckDB |
|---|---|---:|---|---:|---:|---:|---:|---:|
| c3d-highcpu-8 | q1 | 1 | in memory | 2.628 | 2.145 | 0.82x | 0.235 | 9.1x |
| c3d-highcpu-8 | q1 | 1 | 64 MiB | 2.571 | 2.173 | 0.85x | 0.239 | 9.1x |
| c3d-highcpu-8 | q1 | 8 | in memory | 1.101 | 0.917 | 0.83x | 0.106 | 8.6x |
| c3d-highcpu-8 | q1 | 8 | 64 MiB | 1.199 | 1.072 | 0.89x | 0.104 | 10.3x |
| c3d-highcpu-8 | high_cardinality_tpch | 1 | in memory | 4.715 | 4.463 | 0.95x | 1.235 | 3.6x |
| c3d-highcpu-8 | high_cardinality_tpch | 1 | 64 MiB | 14.830 | 14.688 | 0.99x | 1.385 | 10.6x |
| c3d-highcpu-8 | high_cardinality_tpch | 8 | in memory | 3.100 | 3.097 | 1.00x | 0.890 | 3.5x |
| c3d-highcpu-8 | high_cardinality_tpch | 8 | 64 MiB | 17.254 | 17.332 | 1.00x | 1.236 | 14.0x |
| n4a-highcpu-8 | q1 | 1 | in memory | 2.241 | 1.829 | 0.82x | 0.230 | 7.9x |
| n4a-highcpu-8 | q1 | 1 | 64 MiB | 2.237 | 1.827 | 0.82x | 0.238 | 7.7x |
| n4a-highcpu-8 | q1 | 8 | in memory | 0.728 | 0.652 | 0.90x | 0.079 | 8.3x |
| n4a-highcpu-8 | q1 | 8 | 64 MiB | 1.079 | 0.932 | 0.86x | 0.078 | 11.9x |
| n4a-highcpu-8 | high_cardinality_tpch | 1 | in memory | 3.964 | 3.816 | 0.96x | 1.026 | 3.7x |
| n4a-highcpu-8 | high_cardinality_tpch | 1 | 64 MiB | 12.006 | 11.834 | 0.99x | 1.186 | 10.0x |
| n4a-highcpu-8 | high_cardinality_tpch | 8 | in memory | 2.725 | 2.707 | 0.99x | 0.723 | 3.7x |
| n4a-highcpu-8 | high_cardinality_tpch | 8 | 64 MiB | 13.463 | 13.513 | 1.00x | 0.980 | 13.8x |

**Verdict:** Q1 11-18% faster on both CPU types, the high-cardinality query and spill unchanged. Against the frozen targets: **NOT MET**
(Q1 7.7-11.9x DuckDB against 1.5x; high cardinality 3.5-3.7x in memory against 2x; spill 10-14x against 2x). With scalar arithmetic at
0.2% of the profile, most of Q1's remaining gap is outside the aggregate's arithmetic, while the slice 3 spike ran reduced Q1 at
0.79-0.94x DuckDB: step 2b profiles the whole path stage by stage against the spike on the same machine before more optimisation.

## Performance step 2b

**2026-11-08 continuation:** the owner resumed this work after the initial stop
recorded below. Latest sections 2b/3, 2b/4 (including its addendum), and 2b/5
retain the small lookup and shared expressions/buffers provisionally, reject
larger decoding, and leave settled both-CPU timing to the parent. The original
stop and measurements remain historical evidence.

**STOPPED at the required regression guard.** Retain the morsel scheduling
change; reject the i64 lane (Q1 +18.5% / +19.5% paired latency, no compensating
gain). Low-cardinality groups and expression fusion/buffer reuse are unstarted.
This is a partial handoff, not completion of step 2b.

Local x86-64 shared development VM, **unsettled**; no frozen target verdict.
The Q1 ≤1.5x, hash ≤2x and spill ≤2x targets and accepted ADRs are unchanged.
No GCP, push or merge is performed by this implementation task.
Raw evidence: [machine and input identity](raw/20261108-hash-aggregate-perf2b/machine.json).

### Whole-path baseline, before implementation

Built `838e394` with `cargo build --locked --release -j 4 -p nata -p nata-check`.
SF1, warm local Parquet, one requested thread, 8 GiB; complete Q1 including
CLI startup/planning/metadata, scan, expression evaluation, aggregation and CSV.
`perf record -e cpu-clock -F 999 -g --call-graph dwarf,8192` follows all query
threads and includes kernel samples. Zero lost samples. Self shares and inclusive
call graphs are retained, rather than treating user-only samples as whole-path time.
[Self profile](raw/20261108-hash-aggregate-perf2b/baseline-self.txt),
[inclusive profile](raw/20261108-hash-aggregate-perf2b/baseline-inclusive.txt),
[exact invocation and counters](raw/20261108-hash-aggregate-perf2b/baseline-command.json).

| Stage / symbol | Whole-process share | Interpretation |
|---|---:|---|
| Kernel `_raw_spin_unlock_irqrestore` + `finish_task_switch` | 14.16% self | Per-batch cooperative dispatcher round trips; all kernel self is 19.48% |
| `Partial::evaluate` | 31.57% inclusive | Filter, keys, all argument columns and their copies |
| `Decimal::column` | 12.08% self | Checked i128-backed expression lanes |
| `Col::from_arrow` | 2.50% self, 10.32% inclusive for principal instance | Copies/widening and allocation, also called by string scratch sizing |
| `Partial::resume` | 15.05% self, 19.63% inclusive | Group IDs, reservation preparation and aggregate updates |
| `ExactSum::add` | 3.52% self | Checked exact accumulation |
| `RowGroupCursor::next_batch` | 16.44% inclusive | Decode plus pushed filter and projection; Parquet reader `next` is 9.54% inclusive |
| Pushed WHERE predicate / Arrow filter materialization | 0.69% / 2.92% inclusive | Inside cursor, rather than an independent filter task |
| `plan_sql_with` | 0.05% inclusive | Startup/planning is included, sampling-limited here |

Inclusive shares overlap and must not be added. These are sample shares, not
attributed latency gains. In particular, the cursor includes filtering and
projection, so its inclusive share is not a Parquet decode-only percentage.

Design comparison (source inspection): `spikes/arrow-pipeline/src/queries.rs`
keeps scan → selected filter → projection → task-local sink within one morsel;
its batch shares Arrow data. `spikes/pipeline-model/src/main.rs::stages`
separates decode/filter/project/aggregate and its fused variant uses dense IDs.
`spikes/hash-ops/src/agg.rs` supplies general keys and exact states;
`spikes/push-cost/src/arms.rs` separates materialization, dispatch and scheduler
cost with reused buffers. The engine currently yields after each decode,
evaluation and update of only 256 rows, widens decimal input and clones each
materialized aggregate argument. The historical reduced-Q1 spike timings are
not this full-Q1 baseline and are not reused as a denominator.

The baseline's normal and forced-spill differential gates both pass **65/65**:
[raw checks](raw/20261108-hash-aggregate-perf2b/baseline-checks.txt).

### Experiment method and decision rule

Each implementation change gets its own immutable before/after binaries from
this worktree. `scripts/bench-aggregate-perf2b-local.py` gates values against
DuckDB first, then runs five alternating rounds of before, after and an
identical-before generic control (reverse order on alternate rounds). Full Q1
and high-cardinality HAVING, one/eight threads, 8 GiB, warm SF1, whole CLI with
CSV sent to `/dev/null`. Raw samples retain exits, loads and reservation/spill
counters; summaries retain min/quartiles/median/max, paired after/before and
control/before ratios and binary hashes. None of this task's builds or checks overlaps timed cells; other development
processes are not isolated.
A >3% regression without compensating gain stops the task; changes within
local control noise are unresolved pending the parent's settled timing.

### 2b/1 — keep a morsel on the CPU worker

**Rule:** remove per-batch dispatch without increasing admitted memory or changing
sticky wakes, chunked grants, spill pressure/retry, cancellation or error ordering.
A continuation now decodes/evaluates/accumulates consecutive batches until its
row-group morsel ends; pressure, disk tickets and real blocked states keep the
existing cooperative adapter. Cancellation is checked on every loop and inside
bounded accumulation. Batch size remains 256 so the minimum grant is unchanged.

| Query | Threads | Before median s | After median s | Generic control s | Paired after/before median |
|---|---:|---:|---:|---:|---:|
| Q1 | 1 | 2.417 | 1.591 | 2.329 | 0.684 |
| Q1 | 8 | 1.234 | 0.557 | 1.279 | 0.447 |
| High cardinality | 1 | 3.802 | 2.924 | 3.796 | 0.776 |
| High cardinality | 8 | 3.000 | 2.215 | 3.036 | 0.738 |

**Verdict: retain, locally unsettled.** All four cells improve; no >3% regression.
The complete one-thread Q1 profile changes kernel self **19.48→0.48%**, with
the two principal hand-off symbols **14.16→0.06%**. The work did not disappear:
`Partial::resume` becomes 22.40% self, `Decimal::column` 17.28%, `ExactSum::add`
4.74%, `Col::from_arrow` 4.37% of a smaller CPU denominator.
[Samples](raw/20261108-hash-aggregate-perf2b/morsel-samples.json),
[distributions and paired controls](raw/20261108-hash-aggregate-perf2b/morsel-summary.json),
[profile](raw/20261108-hash-aggregate-perf2b/morsel-self.txt).
Normal and forced-spill checks pass **65/65 each** after this change:
[checks](raw/20261108-hash-aggregate-perf2b/morsel-checks.txt).

```sh
python3 scripts/bench-aggregate-perf2b-local.py morsel --before BEFORE_BINARY --after AFTER_BINARY --data DATA_SF1 --out bench/results/raw/20261108-hash-aggregate-perf2b
```

### 2b/2 — rejected i64 exact accumulation lanes; stop rule triggered

**Rule:** use i64 storage and an overflow flag while it fits, falling back to
exact i128, then the existing checked i256 representation; retain all input,
count and final SQL precision checks, exact spill bytes and merge semantics.
The attempted lane kept the exact fallback after an i64 overflow (no wrapped
value was reused). `ExactSum::add` was also marked inline. Thus this experiment
measures that combined lane implementation, not an isolated inline effect.

| Query | Threads | Before median s | Lane median s | Generic control s | Paired lane/before median |
|---|---:|---:|---:|---:|---:|
| Q1 | 1 | 1.510 | 1.806 | 1.545 | 1.185 |
| Q1 | 8 | 0.447 | 0.543 | 0.463 | 1.195 |
| High cardinality | 1 | 2.966 | 2.950 | 2.911 | 0.997 |
| High cardinality | 8 | 2.166 | 2.116 | 2.129 | 0.984 |

**Verdict: REJECT and STOP under the brief's rule.** Q1 regresses 18.5% / 19.5%
on paired ratios. The generic-control ratios are 1.022 / 1.004. High-cardinality
changes are within control noise (controls 0.982 / 0.968), with no compensating
gain. These are **unsettled local** results, sufficient to reject this attempt,
not a both-architecture verdict about every possible i64 lane design.

The fresh one-thread whole-process self profile has `ExactSum::add` **4.74→5.81%**,
`Partial::resume` **22.40→21.85%**, `Decimal::column` **17.28→14.86%** and kernel
**0.48→0.96%**. Shares alone do not establish why the lane is slower; the extra
conversion/flag/fallback branching is a reasoned hypothesis, not an attributed
instruction-cost claim. No additional tuning experiment is run past the stop.
[Samples](raw/20261108-hash-aggregate-perf2b/lane-samples.json),
[distributions, hashes and controls](raw/20261108-hash-aggregate-perf2b/lane-summary.json),
[profile](raw/20261108-hash-aggregate-perf2b/lane-self.txt),
[SF1 value gates](raw/20261108-hash-aggregate-perf2b/lane-gates.json).
Normal and forced-spill aggregate gates pass **65/65 each**; no query result changed.
[SQL checks](raw/20261108-hash-aggregate-perf2b/lane-checks.txt),
[kernel tests](raw/20261108-hash-aggregate-perf2b/lane-kernel-tests.txt).

```sh
python3 scripts/bench-aggregate-perf2b-local.py lane --before MORSEL_BINARY --after LANE_BINARY --data DATA_SF1 --out bench/results/raw/20261108-hash-aggregate-perf2b
```

The lane is removed from the final production tree. Its exact source/proof patch
is retained as [rejected-lane.patch](raw/20261108-hash-aggregate-perf2b/rejected-lane.patch) for reproduction. Apply it with `git apply --unidiff-zero` in a private
copy. Steps **2b/3 low-cardinality
groups** and **2b/4 expression-to-aggregate fusion/buffer reuse** are **NOT STARTED**,
as required by the stop rule. This is a partial implementation handoff, not a
claim that performance step 2b or the frozen Q1 target is complete.

### Kani evidence for the rejected lane

Both harnesses import the actual attempted `crates/nata-kernels/src/lane.rs` by
path in the existing Kani spike. No Arrow/i256 stub is used for this new helper.
`i64_lane_fallback_exact` proves arbitrary full-domain signed i64 addends equal
independent i128 addition, including i64 overflow. `i128_lane_fallback_preserves_overflow`
proves arbitrary full-domain signed i128 initial states/addends match independent
checked addition, returning failure without changing state on i128 overflow.
**2/2 harnesses PASS; 4/4 covers satisfied.** This does not prove the unchanged
caller count bounds, decimal precision checks, or i256 implementation.
[64-bit proof](raw/20261108-hash-aggregate-perf2b/kani-i64-control.txt),
[128-bit proof](raw/20261108-hash-aggregate-perf2b/kani-i128.txt).

The negative control replaces `self.exact = next` with the old i64 value. Its
unmutated control passes and its full-domain i64 proof has a real assertion
failure: **CAUGHT**, not a compile failure or timeout.
[Negative control verdict](raw/20261108-hash-aggregate-perf2b/kani-negative.txt),
[failing proof](raw/20261108-hash-aggregate-perf2b/kani-i64-mutant.txt).
Reproduce in a private copy after applying the retained rejected-lane patch:

```sh
cd spikes/kani-nata-kernels
CARGO_BUILD_JOBS=4 cargo kani --harness i64_lane_fallback_exact
CARGO_BUILD_JOBS=4 cargo kani --harness i128_lane_fallback_preserves_overflow
# Negative control: change self.exact = next to self.exact = i128::from(self.small)
# in crates/nata-kernels/src/lane.rs, then rerun i64_lane_fallback_exact.
```

The four existing decimal-column Kani harnesses were also rerun unchanged:
**4/4 PASS; 8/8 covers satisfied**. Their individual logs are the adjacent
`kani-existing-*.txt` files. Together with the rejected lane's two harnesses,
this trial has **6 passing harnesses, 12/12 covers**, plus the caught lane
negative control. These results do not certify a new i256 implementation.

### Parent's settled timing handoff (not launched here)

[Heavy steps](../aggregate-perf2b.steps) use the existing heavy harness with
SF1 on **c3d-highcpu-8** and **n4a-highcpu-8**, independently. Baseline is
`838e394`; the candidate is this branch's HEAD containing only the retained
morsel change. The rejected lane is evidence, not active candidate code.
The job builds committed, SHA-256-checked source archives separately, gates all
16 Nata cells before 240 timed processes, alternates five rounds with twins
and DuckDB, and refuses unsettled cells. This covers Q1/high-cardinality ×
1/8 threads × 8 GiB/64 MiB. SF10 scaling and the frozen targets remain open.

Prepare the git-free inputs **after the final commit**. The parent supplies its
approved spend from each offline plan; the run commands below are instructions,
not actions taken by this task. No cloud identifiers belong in tracked files.

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline 838e394 --fix morsel=HEAD --bundle .aggregate-perf2b-inputs
cp bench/aggregate-perf2b.steps heavy.steps

scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --run-id perf2b-x86 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --run-id perf2b-x86 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_X86"

scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --run-id perf2b-arm64 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --run-id perf2b-arm64 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_ARM64"
```

Use fresh run IDs if those labels have already been used. Only the parent runs
these commands. No GCP plan, VM, API, push or merge was invoked locally.

### Mutation checks and validation notes

`cargo-mutants 27.1.0` was downloaded as a prebuilt verification tool outside
the repository; no dependency or lockfile changed. The rejected lane's three
focused runtime mutations are **3 CAUGHT, 0 MISSED, 0 INVALID**, after a passing
unmutated control: returning `None`, returning success without updating, and
reversing the overflow-flag test.
[Verdict](raw/20261108-hash-aggregate-perf2b/mutants-lane.txt),
[mutations](raw/20261108-hash-aggregate-perf2b/mutants-lane-caught.txt),
[outcomes](raw/20261108-hash-aggregate-perf2b/mutants-lane-outcomes.json).

The first valid pipeline sweep found **1 CAUGHT, 1 MISSED**. Replacing the
per-batch `force_spill || revoked` with `&&` survived: existing tests asserted
that some spill occurred, also satisfied by the final flush. The new native
test writes one 1,025-row morsel, verifies five 256-row decoded-batch partials,
the exact count and cleanup. This checks the explicit force-every-batch test
mode across the new inner loop. It adds no production optimization past the
stop. [Original sweep](raw/20261108-hash-aggregate-perf2b/mutants-morsel-initial.txt),
[new passing control](raw/20261108-hash-aggregate-perf2b/forced-batch-test.txt).

Tooling correction: one initial cargo-mutants invocation misplaced libtest's
`--test-threads`; it failed before testing any mutant. After correcting that,
the tool's default inherited jobserver FDs made the existing `ulimit -n 4`
owner-lock child fail while loading `libgcc_s`. Disabling the tool jobserver
restored the full native control and lets the explicitly supplied `-j4` govern
build concurrency. This is a harness/environment failure, not an engine query
mismatch; no such run is counted as a mutant catch.
[FD failure](raw/20261108-hash-aggregate-perf2b/mutants-jobserver-failure.txt).

Reproduction (lane command needs the rejected patch in a private tree; the
pipeline command runs against final HEAD):

```sh
CARGO_TARGET_DIR=WORK/target TMPDIR=WORK/tmp RUST_TEST_THREADS=1 cargo mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --profile release --output WORK/mutants-lane -p nata-kernels -f 'crates/nata-kernels/src/lane.rs' -F 'replace Lane::add.*with (None|Some)|lane.rs:44:16' --timeout 120 -- --lib
CARGO_TARGET_DIR=WORK/target TMPDIR=WORK/tmp RUST_TEST_THREADS=1 cargo mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --profile release --output WORK/mutants-morsel -p nata-exec -f 'crates/nata-exec/src/aggregate/pipeline.rs' -F 'pipeline.rs:(381:54|519:9):' --timeout 180
```

The heavy harness's inert self-test passes (both the five-revision x86 schedule
and the two-revision ARM schedule); it launches no engines or cloud calls.
[Self-test](raw/20261108-hash-aggregate-perf2b/heavy-self-test.txt).
This task's local Cargo build/test work is serialized. Direct builds/tests use `-j 4`;
final mutation runs disable the tool's overriding jobserver. Disk stayed above
20 GB free, so no stale-target cleanup was needed.

After adding the multi-batch force-spill test, the complete unmutated executor
control passes and both pipeline mutations are **2 CAUGHT, 0 MISSED, 0 INVALID**:
the per-batch force/revoke disjunction and the global-empty guard conjunction.
[Final sweep](raw/20261108-hash-aggregate-perf2b/mutants-morsel-final.txt),
[outcomes](raw/20261108-hash-aggregate-perf2b/mutants-morsel-final-outcomes.json).
Thus the final targeted runtime results are **5/5 distinct mutants caught**
(three on the rejected lane, two on the retained path). The initial missed
force-spill mutation and the invalid tooling attempts remain recorded above;
they are not silently reclassified as initial catches.

### Final retained-tree checks

All commands below completed successfully with disk-backed `TMPDIR`. The
combined raw log is [final-checks.txt](raw/20261108-hash-aggregate-perf2b/final-checks.txt).
[Final source hashes](raw/20261108-hash-aggregate-perf2b/final-source-sha256.txt).

| Check | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --locked --release -j 4 -p nata-exec -p nata-kernels -p nata-scan -p nata-sched -- --test-threads=1` | PASS, all four crates |
| `cargo clippy --locked --release -j 4 -p nata-exec -p nata-kernels -p nata-scan -p nata-sched --all-targets -- -D warnings` | PASS |
| `cargo build --locked --release -j 4 -p nata -p nata-check` | PASS |
| `nata-check --data DATA_SF001 --queries check/queries/aggregate --candidate FINAL_BINARY` | PASS, 65/65 |
| Same gate with `--force-aggregate-spill` | PASS, 65/65 |
| `CARGO_BUILD_JOBS=4 LOOM_MAX_PREEMPTIONS=2 bash crates/nata-sched/run_memory_loom.sh` | PASS, 16/16 models, bound 2 |
| Kani, including the preserved rejected lane | PASS, 6/6 harnesses, 12/12 covers; negative control CAUGHT |
| Final targeted cargo-mutants | PASS, 5/5 caught, zero remaining missed/invalid; initial missed mutant documented and closed |
| Heavy harness inert self-test / local harness Python compile | PASS |
| `git diff --exit-code -- Cargo.lock` | PASS, unchanged; no dependency added |

Final status: production contains only the tested morsel continuation change
and its multi-batch forced-spill regression test. The local evidence supports
retaining it; settled both-CPU timing is pending. The required regression guard
stopped the work at the rejected i64 lane. Low-cardinality grouping and P4
fusion/buffer reuse remain for a new brief, together with the frozen targets.

Staged leak scanning passes (`scripts/check-leaks.sh --staged`). Text logs have
only trailing whitespace/extra final blank lines normalized for repository checks.

### 2b/3 — bounded small-group lookup (continuation, 2026-11-08)

**Rule:** Use the existing owned arena for at most eight groups per partition, then fall back to the still-populated hashbrown index. Keep full canonical key equality, collision handling, partition hashes, grant-before-allocation and spill replay unchanged. No extra table or uncharged capacity.

Local shared x86-64 VM; **unsettled**, warm SF1, whole CLI, 8 GiB,
five alternating rounds and identical-before generic control. Frozen targets and
ADRs remain unchanged; no both-architecture or settled target claim.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 3.579 | 2.846 | 3.475 | 0.849 | 1.012 |
| q1 | 8 | 1.198 | 1.220 | 1.064 | 1.099 | 0.836 |
| high_cardinality_tpch | 1 | 4.661 | 4.521 | 5.540 | 0.964 | 1.190 |
| high_cardinality_tpch | 8 | 2.727 | 2.702 | 2.683 | 1.008 | 1.000 |

**Verdict:** Retain provisionally for parent measurement, not a settled win. One-thread Q1 improves 15.1% paired, compensating the eight-thread Q1 +9.9% cell; the identical-before eight-thread control drifts -16.4%, so that cell is unresolved rather than certified regression-free. High-cardinality ratios are 0.964/1.008. No ADR/table replacement claim: hashbrown remains populated and is the general path.

One-thread whole-process Q1 self shares (inclusive shares are not additive):

| Symbol | Before % | After % |
|---|---:|---:|
| Partial::resume | 20.22 | 15.86 |
| Decimal::column | 18.78 | 15.64 |
| ExactSum::add | 5.50 | 5.71 |
| Col::from_arrow | 4.00 | 3.79 |

[Samples](raw/20261108-hash-aggregate-perf2b/small-samples.json),
[distributions, paired controls and immutable binary hashes](raw/20261108-hash-aggregate-perf2b/small-summary.json),
[DuckDB value gates](raw/20261108-hash-aggregate-perf2b/small-gates.json).
Full per-cell profiles are adjacent `small-QUERY-THREADS-ARM.txt` files.
Commands use external immutable copies, never an in-place rebuilt binary:

```sh
python3 scripts/bench-aggregate-perf2b-local.py small --before /home/dev/perf2b-next/before --after /home/dev/perf2b-next/small --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2b --profile
```

The owner explicitly resumed after the earlier 2b/2 stop; the earlier handoff is
historical. This continuation does not retry the rejected i64 lane.

Validation: `cargo test --locked --release -j 4 -p nata-exec -- --test-threads=1`
passes ([log](raw/20261108-hash-aggregate-perf2b/small-tests.txt)). Fresh data:
`scripts/gen-tpch.sh 0.01 /home/dev/perf2b-next/data && scripts/gen-sql-check-data.sh /home/dev/perf2b-next/data`.
The locked release build and aggregate gates pass **65/65 normal, 65/65 forced
spill** ([log](raw/20261108-hash-aggregate-perf2b/small-checks.txt)).
No proved arithmetic kernel changed; Kani is not applicable to this lookup.

Focused cargo-mutants: **5 caught, 1 missed, 0 invalid**, after a passing control.
The missed `len <= 8` → `len > 8` swaps two semantically equivalent lookup
paths; full equality and the populated hash table make it a performance-only
mutant, not a lost-group gap. Returning None/Some(0)/Some(1), weakening key
arity conjunction, and reversing arity equality are all caught.
[Outcomes](raw/20261108-hash-aggregate-perf2b/small-mutants-outcomes.json).

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2b/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --profile release --output /home/dev/perf2b-next/mutants-small -p nata-exec -f crates/nata-exec/src/aggregate.rs -F 'Groups::find_row' --timeout 180 -- --lib aggregate::tests::
```

### 2b/4 — shared eager argument DAG and task buffer reuse

**Rule:** Compile eager column/literal/cast/arithmetic trees once per task into a typed DAG. Evaluate each node once for the same retained-row selection, then pass shared columns directly to aggregate updates. Keep conditional roots in the existing evaluator, all arguments before state updates/HAVING/LIMIT, exact checked kernels and grant accounting.

Local shared x86-64 VM; **unsettled**, warm SF1, whole CLI, 8 GiB,
five alternating rounds and identical-before generic control. Frozen targets and
ADRs remain unchanged; no both-architecture or settled target claim.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 1.454 | 1.286 | 1.426 | 0.884 | 0.981 |
| q1 | 8 | 0.430 | 0.391 | 0.459 | 0.908 | 1.072 |
| high_cardinality_tpch | 1 | 2.847 | 2.830 | 2.893 | 0.990 | 0.994 |
| high_cardinality_tpch | 8 | 2.066 | 2.030 | 2.045 | 0.975 | 1.001 |

**Verdict:** Retain, locally unsettled. Paired Q1 improves 11.6%/9.2%; many-group ratios 0.990/0.975 with twin controls 0.994/1.001. No cell regresses by 3%. This is expression/materialization fusion, not check elision or an arithmetic-width change.

One-thread whole-process Q1 self shares (inclusive shares are not additive):

| Symbol | Before % | After % |
|---|---:|---:|
| Partial::resume | 14.59 | 17.97 |
| Decimal::column | 15.71 | 11.93 |
| ExactSum::add | 5.05 | 3.76 |
| Col::from_arrow | 5.05 | 2.61 |

[Samples](raw/20261108-hash-aggregate-perf2b/fusion-samples.json),
[distributions, paired controls and immutable binary hashes](raw/20261108-hash-aggregate-perf2b/fusion-summary.json),
[DuckDB value gates](raw/20261108-hash-aggregate-perf2b/fusion-gates.json).
Full per-cell profiles are adjacent `fusion-QUERY-THREADS-ARM.txt` files.
Commands use external immutable copies, never an in-place rebuilt binary:

```sh
python3 scripts/bench-aggregate-perf2b-local.py fusion --before /home/dev/perf2b-next/small --after /home/dev/perf2b-next/fusion --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2b --profile
```

The DAG shares only eager nodes evaluated on the same selection, with the first
occurrence's type and positioned checks. It shares Q1's discounted-price subtree
and column arguments across SUM/AVG, without cloning their vectors. Numeric
input widening and result buffers, selected rows and hashes are retained per
task; old text is released before another decode. Generic/conditional expressions
retain the existing evaluator and its allocations; this is not zero-allocation
execution. Scalar SQL and the DAG call the same extracted `arith_into`,
`literal_into`, and `cast_into` implementations. No strict kernel changed.
Existing conservative expression/decode and string grants cover retained
capacities; no minimum grant, default, or spill-pressure rule changes.

Validation: full `cargo test --locked --release -j 4 -p nata-exec -- --test-threads=1`
passes, including shared-argument identity, two-batch buffer reuse, validity reset,
Decimal64/Decimal128, masked integer overflow, conditional branches and first
error positions. Locked release build and both aggregate gates pass **65/65**.
[Tests](raw/20261108-hash-aggregate-perf2b/fusion-tests.txt),
[gates](raw/20261108-hash-aggregate-perf2b/fusion-checks.txt),
[clippy](raw/20261108-hash-aggregate-perf2b/fusion-clippy.txt).

Focused mutations initially found **5 caught, 1 missed**: deleting decimal
arithmetic survived the library-only control. The integration gates do not
strengthen that library test. A concrete decimal-buffer test now checks both
Arrow widths, exact results, selection/NULL clearing and retained capacity.
Final sweep: **6 caught, 0 missed, 0 invalid**, including that deletion, the
DAG slot mapping, omitted buffer reset/Arrow input, altered cast equality and
omitted hash-vector filling. Initial evidence is retained, not reclassified:
[initial outcomes](raw/20261108-hash-aggregate-perf2b/fusion-mutants-initial-outcomes.json),
[final outcomes](raw/20261108-hash-aggregate-perf2b/fusion-mutants-final-outcomes.json).
One tool invocation rejected `--in-place -j 1` before testing; in-place mode is
already serial. The corrected command uses `--cargo-arg=--lib` for both build
and test, and `--cargo-arg=-j4`. That invocation is not a mutant catch.

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2b/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --profile release --output /home/dev/perf2b-next/mutants-fusion-final -p nata-exec -f crates/nata-exec/src/aggregate/fusion.rs -f crates/nata-exec/src/aggregate/hashing.rs -f crates/nata-exec/src/value.rs -f crates/nata-exec/src/cast.rs -f crates/nata-exec/src/eval.rs -F 'replace Program::push -> usize with 0|replace Col::reset with \(\)|replace Col::read_arrow.*with Ok\(\(\)\)|delete match arm \(Data::Dec.*in arith_into|replace == with != in cast_into|replace columns_into with \(\)' --timeout 180
cd spikes/kani-nata-kernels
for proof in decimal_column_selected_null_and_bounds decimal_column_add_selected_null_and_bounds decimal_column_sub_selected_null_and_bounds decimal_column_machine_overflow; do
  CARGO_BUILD_JOBS=4 cargo kani --harness "$proof"
done
```

Kani regression rerun: **4/4 PASS, 8/8 covers**, importing the unchanged production
`Decimal::column`. Individual `fusion-kani-*.txt` logs are adjacent to the raw
profiles. These bounded kernels do not prove DAG compilation, buffer lifetime,
or general i256 arithmetic; runtime/mutation checks cover the changed wrappers.

### 2b/5 — reject grant-adaptive 1,024-row decoding

**Rule:** Keep the original 256-row admission/minimum, concurrency and expression phase spans. Admit a 1,024-row decode buffer only when every task fits its larger cursor grant plus the unchanged table-progress floor. Slice into the original 256-row spans before pushed filtering, so compaction cannot move argument errors across spans.

Local shared x86-64 VM; **unsettled**, warm SF1, whole CLI, 8 GiB,
five alternating rounds and identical-before generic control. Frozen targets and
ADRs remain unchanged; no both-architecture or settled target claim.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 1.301 | 1.304 | 1.303 | 1.008 | 0.988 |
| q1 | 8 | 0.396 | 0.384 | 0.402 | 0.967 | 1.011 |
| high_cardinality_tpch | 1 | 2.843 | 2.825 | 2.817 | 0.998 | 0.979 |
| high_cardinality_tpch | 8 | 2.043 | 2.006 | 2.029 | 1.015 | 1.023 |

**Verdict:** REJECT: no established gain beyond twin noise. Q1 ratios 1.008/0.967, high-cardinality 0.998/1.015; no >3% regression, but only the eight-thread Q1 median improves, by 3.3%, within the twin paired range 0.951–1.084 (candidate range 0.960–1.036). The keep-only-if-it-wins rule is not satisfied; decoding and expression batch sizes remain 256 in final code.

One-thread whole-process Q1 self shares (inclusive shares are not additive):

| Symbol | Before % | After % |
|---|---:|---:|
| Partial::resume | 19.72 | 17.02 |
| Decimal::column | 12.15 | 12.77 |
| ExactSum::add | 5.84 | 5.56 |
| Col::from_arrow | 3.16 | 3.27 |

[Samples](raw/20261108-hash-aggregate-perf2b/batch-samples.json),
[distributions, paired controls and immutable binary hashes](raw/20261108-hash-aggregate-perf2b/batch-summary.json),
[DuckDB value gates](raw/20261108-hash-aggregate-perf2b/batch-gates.json).
Full per-cell profiles are adjacent `batch-QUERY-THREADS-ARM.txt` files.
Commands use external immutable copies, never an in-place rebuilt binary:

```sh
python3 scripts/bench-aggregate-perf2b-local.py batch --before /home/dev/perf2b-next/fusion --after /home/dev/perf2b-next/batch --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2b --profile
```

The candidate's SF1 DuckDB gates all pass, and its 13 focused aggregate-spill
tests pass, including a positioned-error test across decode spans and
force-every-output-batch spill. The exact source is retained as
[rejected-batch.patch](raw/20261108-hash-aggregate-perf2b/rejected-batch.patch),
against `005d610`; [native log](raw/20261108-hash-aggregate-perf2b/batch-focused.txt).
The patch is removed from production. No changed proved kernel and no retained
batch change: full touched-crate gates and batch-specific mutations were not
run on this rejected candidate.

### 2b/4 addendum — complete conditional arguments and plan-owned text

**Rule:** Also share structurally identical complete conditional roots on the same retained-row selection, ignoring source positions only after preserving first evaluation order. Keep internal conditional selections in the existing evaluator. Reference text constants through that evaluator so instructions retain no extra owned text copy.

Local shared x86-64 VM; **unsettled**, warm SF1, whole CLI, 8 GiB,
five alternating rounds and identical-before generic control. Frozen targets and
ADRs remain unchanged; no both-architecture or settled target claim.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 1.663 | 1.691 | 1.866 | 1.018 | 1.026 |
| q1 | 8 | 0.566 | 0.583 | 0.582 | 1.047 | 1.038 |
| high_cardinality_tpch | 1 | 3.867 | 3.780 | 3.893 | 0.968 | 1.007 |
| high_cardinality_tpch | 8 | 2.709 | 2.662 | 2.607 | 0.978 | 0.960 |

**Verdict:** Retain provisionally for completeness, locally unsettled. Common eager-path ratios are 1.018/1.047 for Q1 and 0.968/0.978 for high cardinality. The eight-thread Q1 +4.7% cell has compensating many-group improvements of 3.2%/2.2%; its twin also drifts +3.8%. It is unresolved, not certified regression-free. No conditional-query latency gain is claimed from these eager-path timings.

One-thread whole-process Q1 self shares (inclusive shares are not additive):

| Symbol | Before % | After % |
|---|---:|---:|
| Partial::resume | 18.93 | 20.66 |
| Decimal::column | 12.86 | 11.53 |
| ExactSum::add | 5.48 | 5.71 |
| Col::from_arrow | 2.50 | 2.74 |

[Samples](raw/20261108-hash-aggregate-perf2b/roots-safe-samples.json),
[distributions, paired controls and immutable binary hashes](raw/20261108-hash-aggregate-perf2b/roots-safe-summary.json),
[DuckDB value gates](raw/20261108-hash-aggregate-perf2b/roots-safe-gates.json).
Full per-cell profiles are adjacent `roots-safe-QUERY-THREADS-ARM.txt` files.
Commands use external immutable copies, never an in-place rebuilt binary:

```sh
python3 scripts/bench-aggregate-perf2b-local.py roots-safe --before /home/dev/perf2b-next/fusion --after /home/dev/perf2b-next/roots-safe --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2b --profile
```

This completes shared argument materialization on the same selection: eager
subexpressions share DAG nodes; identical complete conditional roots share one
evaluator result. Internal CASE/AND/OR/BETWEEN selections remain independent.
Text literals are kept by plan reference through the generic evaluator, avoiding
an extra retained owned string in the instruction arena. The existing
conditional evaluator still allocates its internal temporaries; no conditional
SQL latency claim is made without a corresponding timed fixture.

The first complete-root candidate, before the text-constant guard, is preserved
in `roots-*` raw files: paired ratios Q1 **0.986/1.000**, many-group
**0.983/0.966**. Those numbers do not describe final HEAD. The final
`roots-safe-*` run above rebuilds and times the actual guarded code against the
same immutable fusion binary. No result mismatch occurred in either trial.

Final full executor tests and clippy pass; normal and forced-spill gates each
pass **65/65**. The new tests cover root sharing at different source positions,
roots with different values, masked branches, and absence of owned text-literal
instructions. Four focused mutations are **4 caught, 0 missed, 0 invalid**:
returning true/false from either the typed root comparator or eager eligibility.
[Tests](raw/20261108-hash-aggregate-perf2b/roots-safe-tests.txt),
[checks](raw/20261108-hash-aggregate-perf2b/roots-safe-checks.txt),
[mutation outcomes](raw/20261108-hash-aggregate-perf2b/roots-safe-mutants-outcomes.json).
Source SHA-256 is unchanged after in-place mutation restoration. No proved
kernel changed in this addendum; the four decimal Kani regression proofs above
remain applicable to the same unchanged kernel, not to this tree comparator.

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2b/tmp CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --profile release --output /home/dev/perf2b-next/mutants-roots -p nata-exec -f crates/nata-exec/src/aggregate/fusion.rs -F 'replace (same_expression|eager) -> bool with (true|false)' --timeout 180
```

### Continuation handoff and final reproduction

Retained implementation commits: `cbe0854` (2b/3) and `005d610` (initial 2b/4);
the following continuation commit contains complete-root sharing, the text guard,
rejected 2b/5 evidence and this handoff. The final report identifies that commit.
All local results are **unsettled**. The >3% Q1 cells on the small lookup and
complete-root extension have compensating gains and large/noisy twin controls;
they remain unresolved for the parent's settled test, not regression-free claims.
No target, ADR, minimum grant, default, spill format or dependency changed.
`Cargo.lock` is unchanged. Builds/tests from this task were serial at `-j 4`;
mutants ran serially in-place with their jobserver disabled. Observed free disk
stayed at least 25 GB, so stale-output deletion was unnecessary.

Final fresh-data verification and binary identity are recorded in
[continuation-final-checks.txt](raw/20261108-hash-aggregate-perf2b/continuation-final-checks.txt):
locked release build, both fixture generators, **65/65 normal and 65/65 forced
spill**, then byte comparison of the rebuilt CLI with the immutable final timed
candidate and `cargo fmt --all --check`. The final source hashes are adjacent.

```sh
export CARGO_TARGET_DIR=/home/dev/perf2b/target
cargo build --locked --release -j 4 -p nata -p nata-check
scripts/gen-tpch.sh 0.01 /home/dev/perf2b-next/final-data
scripts/gen-sql-check-data.sh /home/dev/perf2b-next/final-data
env -u AI_AGENT "$CARGO_TARGET_DIR/release/nata-check" --candidate "$CARGO_TARGET_DIR/release/nata" --data /home/dev/perf2b-next/final-data --queries check/queries/aggregate
env -u AI_AGENT "$CARGO_TARGET_DIR/release/nata-check" --candidate "$CARGO_TARGET_DIR/release/nata" --data /home/dev/perf2b-next/final-data --queries check/queries/aggregate --force-aggregate-spill
```

Parent-only commands, **not launched here**. Prepare after the final commit;
`perf2b=HEAD` includes all retained changes against baseline `838e394`.
Use the spend returned by each plan and fresh run IDs if these are occupied.
The steps cover SF1 Q1/high-cardinality × 1/8 threads × 8 GiB/64 MiB with
DuckDB and identical twins. SF10 scaling and frozen-target certification remain
open; a successful SF1 comparison alone cannot close slice 5.

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline 838e394 --fix perf2b=HEAD --bundle .aggregate-perf2b-inputs
cp bench/aggregate-perf2b.steps heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --run-id perf2b-20261108-x86 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --run-id perf2b-20261108-x86 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_X86"
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --run-id perf2b-20261108-arm64 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --run-id perf2b-20261108-arm64 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_ARM64"
```

No GCP plan, VM or API was invoked; no push or merge. Staged leak scanning is
required and run before each continuation commit.

## Step 2b timing on settled machines (runs `perf2b-c3d`, `perf2b-n4a`, added by the parent, 2026-11-08)

Baseline `838e394` and three cumulative candidates built from committed archives on the same VM: `morsel` = `bc13ccd` (2b/1),
`smallgroup` = `c1eafb2` (2b/3; this commit's parent also carries track B's `c1c2966` scan change), `fusion` = `4905e37` (2b/4 and
its addendum). SF1, every Nata cell value-gated against DuckDB 2.0.0 first, five alternating rounds with a twin of each arm, settled
load before each cell. c3d-highcpu-8 and n4a-highcpu-8, Spot; the ARM64 VM was preempted once and the job resumed on a new VM
(only the resumed directory's complete timings are used). Approved $0.18 + $0.26. Raw: `raw/20261108-hash-aggregate-perf2b-gcp/`
(timings, gates, `medians.py`, per-CPU summaries). Medians of the primary arm, seconds.

| CPU | Query | Threads | Budget | baseline | morsel | smallgroup | fusion | DuckDB | fusion / baseline | **fusion / DuckDB** |
|---|---|---:|---|---:|---:|---:|---:|---:|---:|---:|
| x86-64 | q1 | 1 | in memory | 2.118 | 1.410 | 1.434 | 1.234 | 0.226 | 0.58 | **5.5x** |
| x86-64 | q1 | 1 | 64 MiB | 2.158 | 1.436 | 1.470 | 1.272 | 0.236 | 0.59 | 5.4x |
| x86-64 | q1 | 8 | in memory | 0.910 | 0.340 | 0.354 | 0.300 | 0.107 | 0.33 | **2.8x** |
| x86-64 | q1 | 8 | 64 MiB | 1.200 | 0.516 | 0.515 | 0.462 | 0.108 | 0.38 | 4.3x |
| x86-64 | high_cardinality_tpch | 1 | in memory | 4.142 | 3.371 | 3.415 | 3.359 | 1.107 | 0.81 | **3.0x** |
| x86-64 | high_cardinality_tpch | 1 | 64 MiB | 13.924 | 13.023 | 13.094 | 12.926 | 1.369 | 0.93 | 9.4x |
| x86-64 | high_cardinality_tpch | 8 | in memory | 3.180 | 2.563 | 2.546 | 2.475 | 0.872 | 0.78 | 2.8x |
| x86-64 | high_cardinality_tpch | 8 | 64 MiB | 16.684 | 16.031 | 15.927 | 16.095 | 1.174 | 0.96 | **13.7x** |
| ARM64 | q1 | 1 | in memory | 1.800 | 1.102 | 1.121 | 0.993 | 0.217 | 0.55 | **4.6x** |
| ARM64 | q1 | 1 | 64 MiB | 1.805 | 1.103 | 1.119 | 0.994 | 0.224 | 0.55 | 4.4x |
| ARM64 | q1 | 8 | in memory | 0.644 | 0.172 | 0.173 | 0.160 | 0.072 | 0.25 | **2.2x** |
| ARM64 | q1 | 8 | 64 MiB | 0.914 | 0.398 | 0.403 | 0.366 | 0.069 | 0.40 | 5.3x |
| ARM64 | high_cardinality_tpch | 1 | in memory | 3.449 | 2.710 | 2.715 | 2.668 | 0.925 | 0.77 | **2.9x** |
| ARM64 | high_cardinality_tpch | 1 | 64 MiB | 11.009 | 10.293 | 9.993 | 10.221 | 1.069 | 0.93 | 9.6x |
| ARM64 | high_cardinality_tpch | 8 | in memory | 2.452 | 1.931 | 1.965 | 1.964 | 0.708 | 0.80 | 2.8x |
| ARM64 | high_cardinality_tpch | 8 | 64 MiB | 13.006 | 12.338 | 12.372 | 12.319 | 0.908 | 0.95 | **13.6x** |

**Verdicts.**
- 2b/1 morsel: **kept, settled win** on both CPUs: Q1 0.61-0.67x at one thread, 0.27-0.43x at eight; high cardinality 0.79-0.81x
  in memory.
- 2b/3 smallgroup: **no settled gain.** Q1 is 1.5-4% slower than `morsel` on every cell, the other cells within twin noise.
  Decision: remove the small-group lookup in the next aggregate change (step 2e replaces it with grouping on Parquet dictionary codes).
  The step 2c retry was branched before this verdict and keeps it until then.
- 2b/4 fusion: **kept, settled win:** Q1 0.86-0.90x of `smallgroup` on both CPUs; other cells within noise.
- Frozen targets: **NOT MET.** Q1 4.6-5.5x DuckDB at one thread and 2.2-2.8x at eight (target 1.5x); high cardinality in memory
  2.8-3.0x (target 2x); spill 9.4-13.7x (target 2x), almost untouched by step 2b (0.93-0.96x).
- The same jobs reran the browse oracle with the PG15 fixes (`a956048`, `ba2df5a`): **59/59 PASS on both CPUs** against PostgreSQL
  15.19 (`browse-oracle-pg15-*.txt`).

## Spill path profile on clean VMs (runs `spillprof-c3d`, `spillprof-n4a`, added by the parent, 2026-11-08)

Purpose: replace guesses with measured shares before step 2d (the parent's estimate that spill "has the most room" rested on the size of
the gap only). Main at `6932e88` plus the profile script (step 2b merged), SF1, `scripts/profile-spill-whole.sh` (high-cardinality query, 1 and 8
threads, in memory and 64 MiB) and `scripts/profile-aggregate-perf1.py`, with `perf record -e cpu-clock -g` including kernel samples.
c3d-highcpu-8 and n4a-highcpu-8, Spot, approved $0.09 + $0.15. Raw: `raw/20261108-hash-aggregate-spill-profile/`.

**Tooling gaps.** `/usr/bin/time` is not on the Debian 12 image (wall/CPU split missing; the `spill-whole` step exits 1 for that
reason only). On n4a ARM64, `perf` sampling is refused, so there are **no ARM64 profiles** (register R26).

x86-64 shares by binary (self samples):

| Cell | Nata | kernel | libc | Aggregate counters |
|---|---:|---:|---:|---|
| 1 thread, in memory | 60.6% | 13.7% | 25.8% | partial 1.69 s, merge 0.24 s |
| 1 thread, 64 MiB | 42.6% | **36.8%** | 20.2% | 604 MB written, 692 MB read, **272 partitions**, recursion depth 1 |
| 8 threads, in memory | 58.4% | 25.0% | 16.7% | partial 5.70 s (all workers), merge 1.12 s |
| 8 threads, 64 MiB | 37.1% | **35.0%** | **27.4%** | 960 MB written, 1.22 GB read, **4,368 partitions**, recursion depth 2 |

Top symbols, 1 thread at 64 MiB: kernel `_raw_spin_unlock_irqrestore` 8.8% and `syscall_enter_from_user_mode` 5.3% (wake-ups and
syscalls), `replay::decode` collecting a `Vec<State>` per row 2.9% + 1.8%, `malloc` 1.8%, flatbuffers `TableVerifier`/`append_trace`
2.3%, `std::sync::mpmc` `SyncWaker::notify` and the `MergeTask` channel 2.2%, `replay::encode` 1.1%. In memory: `State::finalize`
6.5%, `Partial::resume` 6.5%, `hashing::key` 5.2%, `Groups::insert`/`find_row`/rehash 8.3%, libc allocation and page faults about 10%.

**Findings for step 2d (measured, ordered by share):**
1. The merge/replay path still hands every task through `mpmc` channels with a kernel wake-up, the same per-batch hand-off 2b/1
   removed from the partial path.
2. Partition fan-out: 272 to 4,368 partitions with recursive re-spilling write 6.7-10.7x the ~90 MB result; the fan-out is not sized
   from the budget and the row count.
3. Replay allocates per row (`Vec<State>` from an iterator) and decodes states row by row.
4. Arrow IPC flatbuffer verification runs per small message; validation per file or per larger message keeps the reader's checks.
5. In memory, allocation churn and `finalize` are about 20%.

**Spike beside Nata (sanity only: load 3.2-3.6 when it ran, right after the builds).** The slice 3 arrow-pipeline spike's reduced Q1,
end to end with Parquet decode: 151 ms (x86-64) and 129 ms (ARM64) at one thread; Nata's full Q1 on the same machine types was 1.234 s
and 0.993 s in the settled 2b runs, DuckDB 0.226 s and 0.217 s. The reduced query computes fewer aggregates, so this bounds the design
gap only loosely; a settled like-for-like run belongs to card 0008.

## Performance step 2c

**2026-11-08 — STOPPED at the required proof guard; no check elision retained.**
Base `3e413b8`. The first full-domain Kani proof for the candidate batch-product
predicate did not complete within its 180-second cap (exit **124**, no verification
summary). A timeout is neither a proof nor a counterexample. The checked
production implementation was restored before handoff; no result/error change
was observed, but query gates were not run in this stopped experiment. Step 2c
is **incomplete**. No performance, mutation-coverage or target-pass claim follows.

Frozen targets remain Q1 ≤1.5x DuckDB, hash queries ≤2x and spill ≤2x, with the
original matched-budget/reference limitations. ADRs and decimal rules are
unchanged. No GCP, new dependency, push or merge. Local performance evidence
would be **unsettled**; this attempt collected no timings or profiles.

### Actually called paths and bound inventory

Source inspection at the base, rather than extrapolation from the older i64
spike: eager aggregate `Program` → `eval::arith_into` → `Decimal::column`, then
`Partial::resume` → `ExactSum::add`; resident and spilled states converge on
`State::merge` → `ExactSum::merge`. [Detailed inventory and computed row limits](raw/20261108-hash-aggregate-perf2c/kernel-inventory.txt).

| Path on Q1 / high-cardinality HAVING | Checks at the base; disposition |
|---|---|
| `1-discount`, Q1 `1+tax` | Each aligned operand: checked i128 scale multiplication and result-precision comparison; checked add/sub and result-precision comparison. Retained. |
| Net-price multiply, Q1 charge multiply | Decimal64 result: i64 operand conversions and checked i64 product, otherwise checked i128 product; decimal result-precision comparison. Declared operand precision alone does not prove capped results fit. Retained. |
| SUM and AVG updates | Input magnitude `<10^38`; count addition strictly below `u64::MAX`; checked i128 add. On overflow, checked i256 promotion/add and possible demotion. No premature SQL-result precision check. Retained. |
| SQL COUNT updates and merge | Checked count addition plus BIGINT conversion. AVG's count is the separate exact-sum count. Retained. |
| Resident partial merge | Checked count addition, checked i256 sum, possible i128 demotion. Retained. |
| Spill state validation and merge | Count validity, checked i256 `(10^38-1)*count`, checked absolute sum within that envelope; then the same merge as resident states. Retained. |
| Final SUM / AVG | Final SUM narrowing/precision check before HAVING; AVG type arithmetic, checked i256 scale-up/div-rem/remainder doubling/tie rounding, i128 narrowing/output precision. Retained. |

**Reasoned accumulation bounds, not completed Kani proofs:** for unscaled
DECIMAL(p,s), `M=10^p-1` and `|S_n|<=n*M`, independently of sign, scale and
merge order. A sufficient 38-digit envelope is
`n<=floor((10^38-1)/M)`; the i128 envelope uses `i128::MAX` in the numerator.
For p=18 the former limit is **100000000000000000100**, greater than
`u64::MAX`; the conservative induction envelope `n*2^60` also stays below
`2^124` for every permitted count. For p=38 the 38-digit limit is **1**;
two same-sign extreme inputs already require promotion, while later opposite
inputs may cancel. Thus a check against 38 digits at the batch boundary must
select the checked/promoting path, never raise a premature SUM error.

For general valid p≤38 states and count below `u64::MAX`, magnitude is below
`2^191`; AVG's at-most-six-place increase keeps it below `2^211`, inside i256.
This arithmetic argument does **not** prove Arrow's i256 operations or the
spill validator. No promotion, merge, replay or AVG check was removed.

### 2c/1 — rejected batch-product envelope

**Rule:** Elide the per-value product machine/decimal checks only after proving
that a bound from the declared result precision and the actual jointly-valid,
selected batch magnitudes makes those checks impossible. Otherwise run the
unchanged checked loop, with the same selected-row prefix and error position.
Proof and a failing negative control must precede measurement.

Candidate rule: scan the two selected-valid magnitude maxima `A,B` once; let
`bits(v)=128-leading_zeros(v)`. Admit only when
`bits(A)+bits(B)<=floor(log2(10^P))`. This is a conservative power-of-two
envelope, not a per-value decision. Multiplication would use the existing i128
lane's `overflowing_mul(...).0`; there is no i64-lane or batch-size experiment.
The generic fallback/control was exposed as `column_checked`.

The harness includes production `decimal.rs` by path. It quantifies arbitrary
signed i128 `x,y`, arbitrary u128 maxima and limit (up to `10^38`), assumes the
actual values are within those maxima and the production predicate accepts,
then asserts no machine overflow, magnitude below the result limit and equality
with checked multiplication. No reduced i8/i64 range is substituted. Its two
cover goals are not reported as satisfied without a completed Kani summary.
The candidate also contained an unrun p≤18 accumulation induction harness and
unrun tests planting 18-/38-digit overflow, `i128::MIN * -1`, machine overflow,
NULL/selection masking and the unchanged prefix before the first failed row.

**Numbers:** before/after/control timings **not run**; profile shares **not
collected**. **Verdict: reject and restore checked code**, because the full-domain
proof did not finish. The >3% regression guard was never reached. Add/sub,
accumulation, promotion/merge/replay elision and cargo-mutants were not started
past this explicit stop. Negative controls: **not run**; mutants caught/missed:
**not measured**, not zero survivors. There are **no kept implementation SHAs**.
The rejected source, tests and harnesses are preserved together as an
[applyable patch](raw/20261108-hash-aggregate-perf2c/rejected-product.patch)
against the base; no unproved helper remains in the production tree.

[Raw Kani output](raw/20261108-hash-aggregate-perf2c/kani-product-timeout.txt),
[exit status and verification limits](raw/20261108-hash-aggregate-perf2c/proof-status.json),
[pre-measurement decision rules](raw/20261108-hash-aggregate-perf2c/experiment-plan.txt),
[machine/input identity](raw/20261108-hash-aggregate-perf2c/machine.json).

Reproduce the stopped proof in an isolated checkout of `3e413b8`:

```sh
git apply bench/results/raw/20261108-hash-aggregate-perf2c/rejected-product.patch
cd spikes/kani-nata-kernels
CARGO_BUILD_JOBS=4 TMPDIR=/home/dev/perf2c/tmp timeout 180 cargo kani --harness perf2c_product_envelope
```

One initial invocation used the spike directory as the working directory for
root-relative copy paths and failed to load the candidate, producing an
unresolved-import compilation error. This was corrected before the timed
proof above; the invalid invocation is not a proof result or mutant catch.

### Verification and handoff

| Check | Result |
|---|---|
| Base `cargo build --locked --release -j 4 -p nata -p nata-check` | PASS, 6m46s; [log](raw/20261108-hash-aggregate-perf2c/baseline-build.txt) |
| Fresh `gen-tpch.sh 0.01` and `gen-sql-check-data.sh` | PASS; external `/home/dev/perf2c/data` |
| Candidate full-domain product Kani | NO RESULT, timeout 180s / exit 124; required stop |
| Candidate proof negative controls / focused cargo-mutants | NOT RUN; no elision kept |
| Candidate nata-exec/nata-kernels tests and clippy | NOT RUN after the proof stop; no implementation retained |
| Aggregate 65/65, forced-spill 65/65, SQL 252/252, decimal gates | NOT RUN after the proof stop; no gate-pass claim |
| Final fmt, source/lockfile restoration, rejected patch applicability | PASS; [log](raw/20261108-hash-aggregate-perf2c/final-evidence-checks.txt) |

Build/proof work from this task was serialized and used four Cargo jobs. Disk
remained above 20 GB free, so stale-output deletion was unnecessary. An immutable
base binary was copied outside the worktree before editing; its SHA-256 is in
`proof-status.json`. Neither the candidate nor another binary was timed.

[Parent-only steps](../aggregate-perf2c.steps) are prepared, but **there is no
retained implementation to benchmark**. Do not package this documentation-only
HEAD as a performance fix. The exact future commands below require a proved,
kept implementation commit first; use multiple cumulative `--fix` entries if
independent changes are retained. None of these commands was run here. Both-CPU
local trial/job-chain checks and cost approval remain prerequisites to cloud runs.

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline 3e413b8 --fix "perf2c=${KEPT_PERF2C_COMMIT:?supply a proved implementation commit}" --bundle .aggregate-perf2c-inputs
cp bench/aggregate-perf2c.steps heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --run-id perf2c-20261108-x86 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --run-id perf2c-20261108-x86 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_X86"
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --run-id perf2c-20261108-arm64 --data-gb 80 --max-vm-hours 2
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --run-id perf2c-20261108-arm64 --data-gb 80 --max-vm-hours 2 --approved-usd "$APPROVED_USD_ARM64"
```

## Performance step 2c (retry)

**Closeout:** only aligned add/sub is retained, as a **provisional** candidate
(`0118a85`). The exact-binary run has material regressions and an eight-thread
Q1 gain; it does not establish a regression-free win. Product admission is
rejected for no established gain on the capped workload, and the independently
measured SUM implementation is rejected for cost. Supported correctness gates
pass; all local timing remains unsettled. The canonical decimal set stays pending.


The owner-authorized retry replaces the stopped arbitrary-i128 SAT obligation
with separately checked admission, reduced-width arithmetic lemmas, i128
boundary properties and a named full-width arithmetic trust point (R27).
The earlier stop remains historical evidence, cherry-picked as `7b87ea9` from
`1006777`; no cloud run, target or ADR amendment is part of this retry.

### Input invariant and scope

`nata-parquet/src/reader.rs::column_types_are_as_requested` checks Arrow
metadata, not the values against decimal precision. Both
`nata-exec/src/value.rs::Col::read_arrow` and `Col::from_arrow` copy Decimal64
and Decimal128 values without a magnitude validation. Consequently no
optimization trusts source metadata alone: selected, jointly valid operands
must pass a batch maximum-magnitude scan; failure replays the original checked
column loop, rather than raising a new input-validation error. Boundary tests
compare the full output/validity prefix before the failed row with that loop.

Q1's inputs are DECIMAL(15,2). Its `1-discount` / `1+tax` intermediates are
DECIMAL(16,2), but the products are capped at DECIMAL(18,4)/(18,6):
`15+16 > 18`, `18+16 > 18`. The type-only product rule cannot elide those
checks. Add/sub alignment with a spare result digit can admit the intermediates.
This follows the existing decimal rules; widening the result type to manufacture
admission would change the contract and was not attempted.

### Retry A — product admission (rejected)

**Rule:** Admit only `1<=p1,p2,P<=38 && p1+p2<=P`, then require both operand
maxima over selected jointly valid rows below their declared powers of ten.
Keep the original checked loop on any admission failure.

**Verdict:** reject: the timed queries have capped products, so the new product
path does not execute; apparent many-group gains are not established beyond the
identical-before controls. Local shared x86-64 VM, warm SF1, whole CLI including
planning and CSV output, 8 GiB; five alternating rounds, **unsettled**.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 1.365 | 1.397 | 1.354 | 1.032 | 0.992 |
| q1 | 8 | 0.412 | 0.413 | 0.395 | 0.982 | 0.966 |
| high_cardinality_tpch | 1 | 2.987 | 2.879 | 2.964 | 0.964 | 0.958 |
| high_cardinality_tpch | 8 | 2.169 | 2.146 | 2.191 | 0.972 | 0.925 |

Q1 one-thread self shares: Decimal::column **11.67%→14.58%**,
ExactSum::add **6.48%→5.96%**, Partial::resume **18.88%→17.71%**.
These are sampled shares, not causal attribution; the candidate does not admit
these products. All SF1 value gates passed. Per-cell distributions, min/median,
paired noise and immutable binary hashes: [summary](raw/20261108-hash-aggregate-perf2c/retry-product-summary.json),
[samples](raw/20261108-hash-aggregate-perf2c/retry-product-samples.json),
[gates](raw/20261108-hash-aggregate-perf2c/retry-product-gates.json).
The [rejected patch](raw/20261108-hash-aggregate-perf2c/rejected-product-retry.patch)
and [kernel snapshot](raw/20261108-hash-aggregate-perf2c/retry-product-decimal.rs)
preserve this experiment; its precision proof imports that snapshot, not a
fictional retained product path. Full touched-crate gates were deferred for this
rejected candidate; no product implementation is kept.

```sh
python3 scripts/bench-aggregate-perf2b-local.py retry-product --before /home/dev/perf2c-retry/before --after /home/dev/perf2c-retry/product --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2c --profile
```

### Retry B — aligned add/sub admission (provisionally retained)

**Rule:** require each declared operand precision plus its alignment digits to
be strictly below P, with P<=38, then validate selected jointly valid batch
magnitudes. Both aligned operand checks and the add/sub result check are then
redundant: each operand is below 10^(P-1), so their sum/difference is below
2*10^(P-1)<10^P. Admission failures use the unchanged checked loop. The scalar
reference stays checked. Q1's capped multiplication checks stay checked.

Local shared x86-64 VM, warm SF1, whole CLI, 8 GiB, five alternating rounds and
identical-before control; **unsettled**. No frozen-target or ARM64 claim.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 1.236 | 1.203 | 1.296 | 0.956 | 1.025 |
| q1 | 8 | 0.382 | 0.371 | 0.381 | 0.942 | 0.992 |
| high_cardinality_tpch | 1 | 2.874 | 2.885 | 2.866 | 0.981 | 0.996 |
| high_cardinality_tpch | 8 | 4.587 | 3.919 | 4.334 | 0.849 | 0.956 |

**Verdict:** retain provisionally: Q1 paired time improves 4.4%/5.8%, against
controls +2.5%/-0.8%; no measured cell regresses. The high-cardinality eight-thread
cell is particularly noisy under concurrent development-machine load and is
unresolved, not a settled 15.1% gain. Raw distributions include minima, quartiles
and paired control ranges. Q1 one-thread self shares: Decimal::column
**13.24%→9.63%**, ExactSum::add **5.10%→4.56%**, Partial::resume
**19.30%→19.93%**. Shares are whole-process samples, not additive phase times.

[Summary and binary hashes](raw/20261108-hash-aggregate-perf2c/retry-aligned-summary.json),
[samples](raw/20261108-hash-aggregate-perf2c/retry-aligned-samples.json),
[SF1 DuckDB gates](raw/20261108-hash-aggregate-perf2c/retry-aligned-gates.json).

```sh
python3 scripts/bench-aggregate-perf2b-local.py retry-aligned --before /home/dev/perf2c-retry/before --after /home/dev/perf2c-retry/aligned --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2c --profile
```

### Proofs and controls

Eight new harnesses pass: `perf2c_precision` (all u8 precisions), `perf2c_scan`
(arbitrary i128 values, validity maps and subset selections at two rows;
nonzero limits), `perf2c_aligned_precision` (all u8 precisions and valid alignment
exponents), `perf2c_sum_envelope` (all u128 current magnitudes/bounds and u64
counts), plus exhaustive-symbolic i16 product, binary-magnitude product and
aligned add/sub lemmas and a u16/u8 accumulation-envelope lemma.
The batch envelope rounds n upward to a power of two, checks that shifting the
bound loses no bits, and uses checked u128 addition; its strict `<2^127` result
is a sufficient condition for the requested `|current|+n*(10^p-1)<2^127` rule.
This can reject a safe batch; rejection is never an SQL error.

All four existing decimal column harnesses also pass. Each of the 12 completed
harnesses satisfies both cover goals: **12/12 proofs, 24/24 covers**. Raw logs
are `retry-kani-HARNESS.txt` under the [raw directory](raw/20261108-hash-aggregate-perf2c/).
The retained add/sub proofs import production source; the rejected product's
precision predicate and the rejected SUM envelope are explicitly imported from
their preserved candidate snapshots.

Four independently weakened predicates all produce real verification failures
(exit 1 and `VERIFICATION:- FAILED`, not compilation failure/timeout): allow
`p1+p2<=P+1`, drop the magnitude scan, remove the aligned spare digit, and allow
the envelope to equal `2^127`. [Negative outcomes](raw/20261108-hash-aggregate-perf2c/retry-negative-outcomes.json).
The portable runner restores each source in a `finally` block:

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/kani-target python3 spikes/kani-nata-kernels/negative-perf2c.py
```

The named full-width trust point is [R27](../../docs/improvements.md#r27--full-width-arithmetic-lemmas-for-admitted-decimal-batches-2026-11-08).
It covers lifting the arithmetic inequalities to i128; admission logic passing
Kani does not certify full-width multiplication or Arrow i256. A separate
20-second full-width Kissat trial timed out (exit 124), with no proof claim.
The parent-only steps allow **four hours each** for CaDiCaL and Kissat.

Aligned-add/sub checks: full `cargo test --locked --release -j 4 -p nata-exec
-p nata-kernels` (test threads 1), fmt, clippy on both crates/all targets with
warnings denied, and the locked release `nata`/`nata-check` build pass.
Fresh SF0.01 and SQL fixtures pass **65/65 aggregate, 65/65 forced spill,
252/252 SQL**. The canonical decimal suite reports **30 pending**, not 30 passes:
these unchanged fixtures use unsupported SELECT-without-FROM or VALUES/subquery
forms. The SQL gate and kernel properties exercise supported decimal arithmetic.
[Complete check log](raw/20261108-hash-aggregate-perf2c/retry-aligned-checks.txt).
The preserved product candidate's standalone boundary tests also pass **3/3**,
including all in-range precision pairs at signed `10^p-1` boundaries and
`i128::MIN` fallback/prefix controls:
[log](raw/20261108-hash-aggregate-perf2c/retry-product-properties.txt).

Focused cargo-mutants on `aligned_precision|batch_magnitudes`: **28 caught,
0 missed, 0 timeouts, 0 invalid**, after a passing unmutated control.
[Outcomes](raw/20261108-hash-aggregate-perf2c/retry-aligned-mutants-outcomes.json),
[summary](raw/20261108-hash-aggregate-perf2c/retry-aligned-mutants.txt).

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2c-retry/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --profile release --output /home/dev/perf2c-retry/mutants-aligned -p nata-kernels -f crates/nata-kernels/src/decimal.rs -F 'aligned_precision|batch_magnitudes' --timeout 180
rustc --edition 2024 --test bench/results/raw/20261108-hash-aggregate-perf2c/retry-product-decimal.rs -o /home/dev/perf2c-retry/product-properties
/home/dev/perf2c-retry/product-properties
```

### Exact-binary add/sub confirmation

The added boundary/mutation tests changed the rebuilt binary's hash (production
arithmetic logic unchanged). This second complete five-round run uses the exact
release binary that passed the checks above: SHA-256
`2d7b8e6f17042c80d3e6d46b3471a89fc7c97ffbf247160d348c32812d07151a`.
These numbers describe the kept commit `0118a85`; the earlier run remains
historical evidence rather than being silently replaced.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 2.453 | 2.542 | 2.734 | 1.095 | 1.072 |
| q1 | 8 | 1.062 | 1.041 | 1.029 | 0.929 | 0.968 |
| high_cardinality_tpch | 1 | 6.628 | 6.152 | 6.452 | 1.180 | 0.952 |
| high_cardinality_tpch | 8 | 4.549 | 4.430 | 4.054 | 1.053 | 0.982 |

**Verdict:** retain only as an unsettled candidate for the parent's settled
comparison, not an established win. The exact-binary run has +9.5% one-thread
Q1 and +18.0%/+5.3% many-group paired regressions, with a compensating -7.1%
eight-thread Q1 cell. The stated no-compensating-gain rejection condition is not
met, but regression-free performance is not established. Whole-process elapsed
times also moved substantially within this shared session; the control ratios
and individual rounds must be considered. Ratios are medians of paired rounds,
so they need not equal the ratios of the marginal time medians.

Q1 one-thread self shares, before→after: Decimal::column **11.65%→9.20%**,
ExactSum::add **5.58%→5.12%**, Partial::resume **19.44%→18.70%**.
[Final summary/hashes](raw/20261108-hash-aggregate-perf2c/retry-aligned-final-summary.json),
[samples](raw/20261108-hash-aggregate-perf2c/retry-aligned-final-samples.json),
[value gates](raw/20261108-hash-aggregate-perf2c/retry-aligned-final-gates.json),
[source hashes](raw/20261108-hash-aggregate-perf2c/retry-aligned-source-sha256.json).

```sh
python3 scripts/bench-aggregate-perf2b-local.py retry-aligned-final --before /home/dev/perf2c-retry/before --after /home/dev/perf2c-retry/aligned-final --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2c --profile
```

### Retry C — SUM batch envelope

**Rule:** For each selected valid decimal batch, first validate values against
the declared precision. For a narrow state, round the row-count upper bound n
upward to a power of two and require
`|current|+rounded_n*(10^p-1)<2^127`, with checked u128 addition and a
leading-zero guard before shifting. Also guard the existing strict count bound.
This stronger envelope implies the requested n-based bound for every prefix.
It may decline an otherwise safe batch; that always invokes the existing
checked/promoting update, never a premature SUM error.

The executor candidate tries this only for at most eight touched groups, using
fixed stack storage. It preflights every touched group and all selected input
values before any update; on any failure it keeps original row order. Successful
admission permits group-at-a-time batch updates without any possible data/count
error. Larger group sets retain the original one-pass path. Integer input updates,
merge, spill replay, final SUM narrowing and AVG finalization stay checked.
No grants, batch sizes, spill formats or dependencies change.

The 26 kernel tests pass, including independent checked accumulation, all
precisions 1..38, wide promotion followed by cancellation/demotion, a planted
invalid value preserving the prior two updates, strict count exhaustion,
`i128::MIN` rejection, and checked envelope boundaries:
[properties](raw/20261108-hash-aggregate-perf2c/retry-sum-properties.txt).
The full-domain envelope and reduced arithmetic proofs/negative control are
listed above. Timing compares only this independent addition against the exact
accepted add/sub binary, with the same five-round/DuckDB/control method.

| Query | Threads | Before median s | After median s | Control median s | Paired after/before | Paired control/before |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 2.593 | 4.067 | 2.925 | 1.585 | 1.057 |
| q1 | 8 | 1.077 | 1.414 | 1.083 | 1.335 | 1.017 |
| high_cardinality_tpch | 1 | 4.348 | 4.291 | 4.672 | 0.955 | 1.081 |
| high_cardinality_tpch | 8 | 2.786 | 2.948 | 2.942 | 0.963 | 1.015 |

**Verdict: reject.** Q1 regresses 58.5%/33.6%, against twins +5.7%/+1.7%.
Many-group paired shifts -4.5%/-3.7% do not demonstrate compensating gain:
that path does not admit the new grouped SUM loop, and its controls shift
+8.1%/+1.5%. All local numbers remain **unsettled**. The extra group/value scans
are not a useful way to remove these checks. On one-thread Q1 the candidate's
`ExactSum::add_batch` alone takes **29.60%** self time; a correct envelope does
not make this batch interface efficient.

Q1 one-thread self shares, before→after: Decimal::column **8.65%→6.41%**,
ExactSum::add **7.47%→not listed** (the new inlined/generic batch cost is not an
`add` self sample), Partial::resume **17.88%→16.50%**. The add_batch name embeds
Partial::resume in a generic type: it is not counted again as Partial::resume.
[Summary/hashes](raw/20261108-hash-aggregate-perf2c/retry-sum-summary.json),
[samples](raw/20261108-hash-aggregate-perf2c/retry-sum-samples.json),
[all SF1 DuckDB gates](raw/20261108-hash-aggregate-perf2c/retry-sum-gates.json).

[Rejected patch against 0118a85](raw/20261108-hash-aggregate-perf2c/rejected-sum-retry.patch)
and [admission snapshot/properties](raw/20261108-hash-aggregate-perf2c/retry-sum-admission.rs)
preserve the candidate. It is removed from production, including the initially
unexported admission helper. The SUM proof module now explicitly imports this
rejected snapshot. Full executor/clippy/SF0.01 gates are not claimed for the
rejected SUM candidate: it passed 26 kernel tests and all SF1 value gates before
rejection, and final production returns to the previously checked add/sub source.

Focused cargo-mutants on the envelope: **14 caught, 1 missed, 0 timeouts,
0 invalid**, after a passing control. The survivor changes
`bound.leading_zeros() < shift` to `<= shift`, strengthening the early-rejection
guard. It is equivalent: if leading_zeros==shift, the shifted contribution has
bit 127 set and the original final envelope test already rejects it.
`perf2c_sum_equal_shift_rejected` proves this for all u128 current/bound values
and all positive u64 counts, with both covers satisfied:
[proof](raw/20261108-hash-aggregate-perf2c/retry-kani-sum-equal-shift.txt),
[outcomes](raw/20261108-hash-aggregate-perf2c/retry-sum-mutants-outcomes.json).
All unsafe precondition weakenings in the focused sweeps are caught. The final
proof count is **13/13 completed harnesses, 26/26 cover goals**; the full-width
heavy harness remains unproved.

```sh
python3 scripts/bench-aggregate-perf2b-local.py retry-sum --before /home/dev/perf2c-retry/aligned-final --after /home/dev/perf2c-retry/sum --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2c --profile
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2c-retry/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --profile release --output /home/dev/perf2c-retry/mutants-sum -p nata-kernels -f crates/nata-kernels/src/sum_admission.rs --timeout 180
```

The mutation command runs on the preserved candidate (`0118a85` plus its patch),
not the final production tree, where the rejected helper is intentionally absent.
The portable negative-control runner continues to target the preserved helper.

### Final verification and parent handoff

Kept runtime change: `0118a85` (aligned add/sub only, provisional). Preserved
stopped evidence: `7b87ea9` (cherry-pick of `1006777`, re-recorded after staged
leak scanning). The following evidence commit records the exact-binary run and
SUM rejection and moves the unused SUM helper out of production; its SHA is in
the handoff. No product/SUM optimization remains. Final source differs from
`3e413b8` only in the decimal kernel and evaluator's precision adapter; no
aggregate/merge/replay/finalization source or lockfile difference remains.

After restoring checked SUM, the final locked release build passes and the CLI
is **byte-identical** to the exact timed add/sub binary and to the binary that
passed **65/65 aggregate, 65/65 forced spill, 252/252 SQL**. Thus those gates
apply to final runtime behavior. Final fmt and local job-chain stubs pass; every
heavy command also passes `bash -n`. Decimal suite: 30 unchanged pending cases,
not a pass claim. Proofs: 13 completed / 26 covers, plus four real failing
negative controls; full-width heavy proof unproved. Focused mutation total:
**42 caught, 1 equivalent survivor, 0 timeout/invalid**, with the survivor's
equivalence separately proved. No query result or error change occurred.

Builds/tests were serial locally with `-j 4`; the other child's independent
compilation is part of why local timings are unsettled. As free space crossed
the conservative 20 GiB warning line, stale perf2b proof caches and already-run
integration test executables were deleted, with immutable benchmark binaries
preserved. Inactive old temporary build caches were also removed; active release
outputs were retained. No new dependency, target/threshold/ADR amendment, GCP
API/plan/run, push or merge occurred. Staged leak scanning precedes each commit.

The parent-only [steps](../aggregate-perf2c.steps) include baseline `3e413b8`,
fix `0118a85`, the thirteen short harnesses, and **separate four-hour CaDiCaL
and Kissat full-width attempts**. Timeout/failure exit files remain results,
never certified proofs. Package after this evidence commit so the root-level
proof runner and rejected snapshots are available. The fixed runtime revision
contains the same hot code as final HEAD; the final rebuilt binary identity is
recorded below. Existing SF1 targets are unchanged; SF10/scaling/frozen-target
certification remain open.

Exact parent commands, **not run here**. Take each approved amount from its plan;
use fresh run IDs if these are occupied. The ten-hour VM bound allows two
four-hour solver attempts plus setup/timing; it is not authorization to spend.

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline 3e413b8 --fix aligned=0118a85 --bundle .aggregate-perf2c-inputs
cp bench/aggregate-perf2c.steps heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --run-id perf2c-retry-20261108-x86 --data-gb 80 --max-vm-hours 10
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --run-id perf2c-retry-20261108-x86 --data-gb 80 --max-vm-hours 10 --approved-usd "$APPROVED_USD_X86"
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --run-id perf2c-retry-20261108-arm64 --data-gb 80 --max-vm-hours 10
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --run-id perf2c-retry-20261108-arm64 --data-gb 80 --max-vm-hours 10 --approved-usd "$APPROVED_USD_ARM64"
```

## Performance step 2d

**Closeout: partial, stopped at the accepted ADR boundary.** Step 0 is kept
(`b89356153b9bfb4b2e7c2558c9b6c7a750d8e8e3`); step 1's ready-read loop is
rejected for spill regressions; steps 2–5 are not implemented. The proposed
fan-out amendment below is for owner acceptance, not an edit to ADR 0010.
No result mismatch, weakened spill fault/damage test, new dependency, GCP run,
push or merge occurred. Frozen performance targets remain unfulfilled.

Work starts at `10acff0`; frozen targets and accepted ADRs remain unchanged.
The clean-VM spill profile above is the motivation, not a denominator for local
measurements. Local observations below are **unsettled**, warm SF1, shared
x86-64 host, whole CLI including startup/planning and CSV rendering to `/dev/null`.
The runner value-gates every before/after/control cell against DuckDB before
five alternating rounds. The control is the identical immutable before binary,
not a build from another day. Both Q1 and high-cardinality cells use 1/8 threads
and 8 GiB/64 MiB; every observation retains spill counters and load.
Profiles use whole-system `cpu-clock` samples, with DSO shares and the first 90
symbol report lines retained; full perf data is discarded. No local number
certifies either CPU's frozen target. No GCP job was run.

Evidence: [host and limits](raw/20261108-hash-aggregate-perf2d/environment.json),
runner `scripts/bench-aggregate-perf2d-local.py`. Source binaries are copied out
of the build tree before timing; summary files record their SHA256 hashes.
All local build/test commands use `-j 4`, serially, with the reused
`CARGO_TARGET_DIR=/home/dev/perf2b/target`; no dependencies are added.

### 0. Remove the small-group lookup

Rule: always probe the existing hashbrown index with full canonical key equality;
remove the `entries.len() <= 8` linear scan from `c1eafb2`. Keep collision,
NULL-junk, canonical-float and growing-table lookup tests; rename the former
fallback test to describe table growth.

**Verdict: kept as the directed cleanup supported by the settled two-CPU
no-gain verdict. Local results are mixed, not a regression-free speed claim.**
The paired Q1/8/64 MiB gain is 7.3%, while Q1/1/64 MiB and high-cardinality/1/
8 GiB regress 11.1% and 11.5%. Those regressions are retained explicitly;
the latter's identical control also moves by -12.3%, and another control moves
+14.3%. The compensating Q1/8 spill-budget gain permits retention under the
brief's no-compensating-gain guard, but settled validation remains required.
Spill itself shows no established improvement (0.996x/0.988x versus controls
1.050x/0.958x).

Seconds, medians; paired ratios are medians of per-round ratios (not ratios
of medians). Min/quartiles/max, all five observations and binary hashes are
in the linked summary/samples.

| Query | Threads | MiB | Before | After | Control | Paired A/B | Paired C/B |
|---|---:|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 8192 | 2.584 | 2.741 | 2.771 | 1.034 | 1.038 |
| q1 | 8 | 8192 | 0.935 | 0.912 | 0.957 | 0.986 | 1.043 |
| q1 | 1 | 64 | 2.191 | 2.965 | 2.574 | 1.111 | 0.968 |
| q1 | 8 | 64 | 1.222 | 1.141 | 1.255 | 0.927 | 1.000 |
| high_cardinality_tpch | 1 | 8192 | 5.555 | 6.067 | 5.109 | 1.115 | 0.877 |
| high_cardinality_tpch | 8 | 8192 | 4.266 | 4.574 | 4.861 | 0.995 | 1.143 |
| high_cardinality_tpch | 1 | 64 | 12.785 | 12.732 | 13.442 | 0.996 | 1.050 |
| high_cardinality_tpch | 8 | 64 | 34.582 | 33.636 | 33.135 | 0.988 | 0.958 |

At 64 MiB, high-cardinality counters (decimal MB):

| Threads | Arm | Written MB range | Read MB range | Partitions | Depth |
|---|---|---:|---:|---:|---:|
| 1 | all three | 603.860 | 692.132 | 272 | 1 |
| 8 | before | 959.047–959.190 | 1218.312–1218.485 | 4368 | 2 |
| 8 | after | 959.070–959.402 | 1218.324–1218.681 | 4368 | 2 |
| 8 | control | 959.006–959.365 | 1218.254–1218.684 | 4368 | 2 |

Local whole-system self-sample shares, Nata/kernel/libc, before → after:
1-thread spill **46.18/29.94/23.66% → 45.83/30.15/23.72%**;
8-thread spill **46.45/25.23/28.05% → 45.57/26.15/28.11%**.
This isolates removal; it does not establish any repair of the spill bottleneck.
DSO/symbol reports for every cell are in the raw directory.

[Summary/hashes](raw/20261108-hash-aggregate-perf2d/remove-small-summary.json),
[samples/counters/load](raw/20261108-hash-aggregate-perf2d/remove-small-samples.json),
[24 DuckDB gates](raw/20261108-hash-aggregate-perf2d/remove-small-gates.json),
[checks](raw/20261108-hash-aggregate-perf2d/remove-small-checks.txt).
Checks: `cargo test --locked --release -j 4 -p nata-exec` (including unchanged
spill fault/cleanup/corruption tests), fmt, clippy of nata-exec/all targets,
release CLI/check build, aggregate 65/65, forced-spill aggregate 65/65,
SQL 252/252: **PASS**. No loom model or scheduler/service primitive is changed.

```sh
python3 scripts/bench-aggregate-perf2d-local.py remove-small --before /home/dev/perf2d/before --after /home/dev/perf2d/remove-small --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2d --profile
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2d/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --profile release --output /home/dev/perf2d/mutants-remove-small -p nata-exec -f crates/nata-exec/src/aggregate.rs -F 'find_row' --timeout 180
```

Focused cargo-mutants on `Groups::find_row`: **5 caught, 0 missed, 0 timeout,
0 unviable**; unmutated baseline passes. [Outcomes](raw/20261108-hash-aggregate-perf2d/remove-small-mutants-outcomes.json),
[deciding log](raw/20261108-hash-aggregate-perf2d/remove-small-mutants.txt).

### 1. Replay ready reads on the current worker

Rule: retain a partition continuation on the worker while it can make progress;
process completed reads, replay, finalize or repartition there. After submitting
a read, poll it directly rather than unconditionally returning `Step::Yield`.
Return `Blocked` for a pending ticket: no worker waits for disk, and the
existing sticky wake, memory grants, disk tickets, I/O admission, error order
within the partition, cancellation and cleanup remain. Check cancellation on
every loop iteration; retain bounded finalization polls. This can remove eager
handoffs but necessarily retains handoffs for genuinely pending reads, as ADR
0009 requires. It does not claim worker affinity across a parked disk ticket.

Checks before timing: execution tests (including existing spill fault/damage,
recursive replay and cleanup tests), fmt, clippy, release build, aggregate
65/65, forced spill 65/65, SQL 252/252: **PASS**.
[Complete log](raw/20261108-hash-aggregate-perf2d/merge-ready-checks.txt).
No loom model or scheduler/memory/I/O primitive is touched.

Focused `merge_step` cargo-mutants: **11 caught, 0 missed, 1 unviable,
1 timeout**, unmutated baseline PASS. The `task.row += 1` → `*= 1` mutation
fails progress and is killed by the fixed 180-second test deadline; it is
reported separately, not as an assertion-caught mutant. Tool exit 3 denotes
that timeout, not a baseline failure. [Outcomes](raw/20261108-hash-aggregate-perf2d/merge-ready-mutants-outcomes.json),
[deciding log](raw/20261108-hash-aggregate-perf2d/merge-ready-mutants.txt).

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2d/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--test=aggregate_spill --cargo-arg=--locked --profile release --output /home/dev/perf2d/mutants-merge-ready -p nata-exec -f crates/nata-exec/src/aggregate/pipeline.rs -F 'merge_step' --timeout 180
python3 scripts/bench-aggregate-perf2d-local.py merge-ready --before /home/dev/perf2d/remove-small --after /home/dev/perf2d/merge-ready --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2d --profile
```

Decision before the replay timing verdict: retain only with a spill gain larger
than the identical-control movement, and reject any >3% regression without a
compensating gain. A reduction in handoffs without an established timing gain
is insufficient to keep the experiment.

**Verdict: rejected and reverted.** Both spill cells regress beyond 3%:
paired after/before 1.065x at one thread and 1.041x at eight, versus identical
controls 0.963x and 1.017x. No compensating gain is established in the other
cells: the small apparent in-memory improvements are within control movement.
The simple ready-read loop does not remove handoffs for the normally pending
small disk reads. Profiles also show no material kernel-share reduction.
That last causal explanation is **reasoned**, not a measured count of dispatches.
A worker may not wait for disk under ADR 0009, so blocking it to force partition
affinity is not a permissible follow-up. No rejected code remains in the tree.

Seconds, medians; paired ratios, with min/quartiles/max in the summary:

| Query | Threads | MiB | Before | After | Control | Paired A/B | Paired C/B |
|---|---:|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 8192 | 3.080 | 3.165 | 3.107 | 1.021 | 0.947 |
| q1 | 8 | 8192 | 1.036 | 0.985 | 0.998 | 0.976 | 0.947 |
| q1 | 1 | 64 | 2.582 | 2.902 | 3.108 | 1.017 | 1.142 |
| q1 | 8 | 64 | 1.248 | 1.225 | 1.233 | 1.004 | 0.977 |
| high_cardinality_tpch | 1 | 8192 | 6.226 | 5.382 | 4.794 | 0.985 | 0.758 |
| high_cardinality_tpch | 8 | 8192 | 2.355 | 2.310 | 2.384 | 0.986 | 1.012 |
| high_cardinality_tpch | 1 | 64 | 12.918 | 13.772 | 12.481 | 1.065 | 0.963 |
| high_cardinality_tpch | 8 | 64 | 15.095 | 15.732 | 15.288 | 1.041 | 1.017 |

High-cardinality counters at 64 MiB (decimal MB):

| Threads | Arm | Written MB range | Read MB range | Partitions | Depth |
|---|---|---:|---:|---:|---:|
| 1 | all three | 603.860 | 692.132 | 272 | 1 |
| 8 | before | 959.433–959.690 | 1218.698–1218.957 | 4368 | 2 |
| 8 | after | 959.533–959.691 | 1218.815–1218.947 | 4368 | 2 |
| 8 | control | 959.460–959.627 | 1218.736–1218.900 | 4368 | 2 |

Whole-system self-sample shares, Nata/kernel/libc, before → after:
1-thread spill **45.34/30.08/24.42% → 45.31/30.61/23.87%**;
8-thread spill **46.31/23.02/30.52% → 45.33/23.83/30.63%**.
[Summary/hashes](raw/20261108-hash-aggregate-perf2d/merge-ready-summary.json),
[samples/counters/load](raw/20261108-hash-aggregate-perf2d/merge-ready-samples.json),
[24 DuckDB gates](raw/20261108-hash-aggregate-perf2d/merge-ready-gates.json),
[rejected patch against b893561](raw/20261108-hash-aggregate-perf2d/rejected-merge-ready.patch).
No code commit for this rejected experiment; its compiled binary is identified
by SHA256 in the summary. The candidate's complete checks and mutants above
remain evidence, not a certification of a retained replay fix.

### Step 2 boundary: accepted fan-out rule

ADR 0010's accepted spill decision says: “initial fanout 16, fanout 16 per
recursion and maximum depth 8.” Product code implements that choice in
`PARTITIONS`, `files`, the repartitioner and typed replay's `hash & 15`
membership check. A budget-sized logical fan-out that exceeds 16 changes that
accepted decision. Per this task's stop rule, **step 2 and the subsequent
ordered steps 3–5 are not implemented**. Validation guarantees and ADR text are
unchanged. This is a design stop, not a result mismatch or a dependency stop.

Proposed amendment for owner acceptance (not applied):

> Permit a power-of-two logical spill fan-out from 16 through 256. Estimate
> resident state bytes from expected partial rows and charged key/state/table
> capacity, using serialized spill bytes only as supporting evidence; serialized
> bytes alone underestimate hash-table memory. Choose the smallest fan-out whose
> estimated resident partition fits half the available merge-table budget.
> Clamp at 256 and reduce simultaneous merge partitions when the estimate
> exceeds their grants. FD and disk quotas, parent/child overlap accounting,
> the 64 KiB scratch floor, canonical XXH3 hashing and validated Arrow IPC remain
> mandatory. Reader partition-membership checks must use the trusted fan-out
> chosen by the query, not an unchecked value from disk. Recursive fan-out stays
> bounded at 16 with depth at most 8 for estimation error/skew; exhausted depth,
> no progress and oversize records still fail cleanly. No grant is bypassed.

The follow-on implementation must test estimator overflow, min/max clamps,
partition membership under hostile input, skew/collision/no-progress, FD/quota
bounds and reduction of merge concurrency before any performance verdict.

### Settled prefix handoff (not run)

The [steps](../aggregate-perf2d.steps) compare baseline `c9cc13c`, provisional
`addsub=10acff0`, then cumulative `no-small=b893561`. There are two fixes,
below the heavy runner's limit of four. Rejected replay is intentionally absent.
Both Q1 and high cardinality are included at 1/8 threads and 8 GiB/64 MiB;
per-arm twins, matched DuckDB budgets, five alternating rounds and all spill
counters are provided by the existing heavy runner. This only measures the
implemented prefix; it cannot certify a completed step 2d or its frozen targets.

Validation of the handoff: all four parsed step commands pass `bash -n -c`;
the existing heavy runner's full inert orchestration self-test passes (including
ARM, matched spill cells and incomplete DuckDB denominators).
[Syntax log](raw/20261108-hash-aggregate-perf2d/steps-syntax.txt),
[self-test](raw/20261108-hash-aggregate-perf2d/heavy-self-test.txt).

**Four-hour cap per architecture**, including all replacement VMs: allow up to
45 minutes for three clean builds, 80 minutes for eight bounded settle waits,
30 minutes of alternating measurements, 20 minutes of fixture/value gates/setup,
and 45 minutes for preemption/restart margin (220 minutes total, rounded up).
No long proofs or unbounded retry allowance are hidden in this cap. The parent
must obtain each plan's approved amount; these commands are not spend approval.
The Debian 12 image lacks `/usr/bin/time`: the `tools` step installs the `time`
package before any optional profile uses it. The timing runner uses Python's
clock and does not depend on GNU time.

Run from this branch after the evidence commit. Use fresh run IDs if occupied:

```sh
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline c9cc13c --fix addsub=10acff0 --fix no-small=b89356153b9bfb4b2e7c2558c9b6c7a750d8e8e3 --bundle .aggregate-perf2d-inputs
cp bench/aggregate-perf2d.steps heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --run-id perf2d-prefix-20261108-x86 --data-gb 80 --max-vm-hours 4
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --run-id perf2d-prefix-20261108-x86 --data-gb 80 --max-vm-hours 4 --approved-usd "$APPROVED_USD_X86"
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --run-id perf2d-prefix-20261108-arm64 --data-gb 80 --max-vm-hours 4
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --run-id perf2d-prefix-20261108-arm64 --data-gb 80 --max-vm-hours 4 --approved-usd "$APPROVED_USD_ARM64"
```

Reproduce the kept-change checks (the recorded runs used the external target
cache described above; these commands use the default target directory):

```sh
PERF2D_DATA=$(mktemp -d /tmp/nata-perf2d-check.XXXXXX)
cargo test --locked --release -j 4 -p nata-exec
cargo fmt --all --check
cargo clippy --locked --release -j 4 -p nata-exec --all-targets -- -D warnings
cargo build --locked --release -j 4 -p nata -p nata-check
scripts/gen-tpch.sh 0.01 "$PERF2D_DATA"
scripts/gen-sql-check-data.sh "$PERF2D_DATA"
env -u AI_AGENT target/release/nata-check --candidate target/release/nata --data "$PERF2D_DATA" --queries check/queries/aggregate
env -u AI_AGENT target/release/nata-check --candidate target/release/nata --data "$PERF2D_DATA" --queries check/queries/aggregate --force-aggregate-spill
env -u AI_AGENT target/release/nata-check --candidate target/release/nata --data "$PERF2D_DATA" --queries check/queries/sql
```

Local build/test commands were serial, at `-j 4`. Disk headroom stayed above
20 GB, so no stale-output deletion was needed here. All staged commits passed
`scripts/check-leaks.sh --staged`. Neither Cargo.lock nor an accepted ADR was edited.

After reverting replay, the final normal release build and clippy pass.
Its CLI SHA256 is `fcdb2e9973dd2219cf597d2bfb5f5aa76dac273a95114e11439f65307dc2ae1a`,
**identical to the immutable step-0 binary that passed all gates**.
[Final rebuild and digest comparison](raw/20261108-hash-aggregate-perf2d/final-build.txt).

## Performance step 2d (fan-out)

2026-11-08, continuation from `b065675` after the parent accepted ADR 0010's
last amendment. This supersedes the earlier design stop, not the rejected
ready-read experiment. The main equivalent of kept `b893561` is `a3f71fe`.
No ready-read handoff change, dependency, SQL kernel, input batch size,
performance target or ADR change is included. GCP is **not run**; the parent
makes the settled two-CPU decision. Timings below are **unsettled**.

Local evidence uses warm SF1, whole CLI/startup/planning/CSV to `/dev/null`,
the existing five-round alternating before/after/identical-before control,
DuckDB value gates for all 24 arms, and whole-system cpu-clock profiles.
The pre-change binary SHA256 matches the previous kept no-small binary
(`fcdb2e9973dd2219cf597d2bfb5f5aa76dac273a95114e11439f65307dc2ae1a`).
Use counters and profile shares as local evidence; local timing alone rejects
only a regression beyond the twin-control spread (the preceding experiment's
controls ranged 0.76–1.14). Neither local timing nor a reduced kernel share
certifies frozen targets. Raw evidence is under
`raw/20261108-hash-aggregate-perf2d/`, with `fanout-`, `decode-` and `ipc-` prefixes.

### 1. Budget-sized logical initial fan-out

**Rule:** estimate resident bytes from expected partial rows, charged keys,
states and table capacity; choose the smallest power of two in 16..256 whose
estimated partition fits half the available merge-table budget. Reduce merge
concurrency when that estimate exceeds per-task grants. Keep recursive fan-out
16, depth 8, the 64 KiB scratch floor, canonical XXH3 and every IPC/typed-reader
check. The reader takes fan-out from query-owned storage, never disk.

The estimate is checked `input_rows * (512 + 128*keys + 192*aggregates)`:
256 base entry bytes, 64/key and 128/state from `reserve_entry`, 256 for table
capacity/rehash overlap, plus an estimated 64 text bytes/key or state. Input
rows conservatively bound expected partial rows; the text allowance is a
heuristic, **not** a bound. Grants, skew fallback, record caps and depth still
bound execution independently of this estimate. Overflow returns a resource
error. Available bytes exclude admitted runtime, registry, I/O, workspace and
progress reservations; final merge recomputes them after scan teardown.

The 16 resident tables and their allocation thresholds are preserved. Spill
routing sorts each consumed table in place by logical bucket, then writes its
chosen partitions with one bounded encode buffer. This avoids multiplying
minimum table chunks by 256. The consumed probing index is never used after
routing. Files are opened sequentially through the existing registry; live
parent/child bytes remain charged together until successful replacement.
The existing collision/no-progress process-deadline test still reaches and
checks depth 8; no fault/damage assertion was weakened.

**Verdict: kept for the parent’s settled run.** Both high-cardinality spill
cells eliminate recursion, sharply reduce bytes and file opens, and reduce
kernel share. Local paired spill time improves, but remains unsettled.
No other cell regresses beyond the twin-control spread.

High cardinality, 64 MiB; exact ranges over five timed observations:

| Threads | Arm | Bytes written | Bytes read | Partitions | Depth |
|---|---|---:|---:|---:|---:|
| 1 | before | 603859856 | 692131960 | 272 | 1 |
| 1 | after | 253743048 | 253743048 | 256 | 0 |
| 1 | control | 603859856 | 692131960 | 272 | 1 |
| 8 | before | 959500184–959718912 | 1218843288–1218956000 | 4368 | 2 |
| 8 | after | 279644192–286307024 | 279644192–286307024 | 256 | 0 |
| 8 | control | 959539920–959716584 | 1218825240–1218943736 | 4368 | 2 |

Unsettled seconds, medians; paired after/before and identical-control/before ratios:

| Query | Threads | MiB | Before | After | Control | A/B | C/B |
|---|---:|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 8192 | 1.123 | 1.116 | 1.137 | 1.012 | 0.992 |
| q1 | 8 | 8192 | 0.331 | 0.323 | 0.332 | 0.989 | 1.034 |
| q1 | 1 | 64 | 1.125 | 1.116 | 1.097 | 0.972 | 0.978 |
| q1 | 8 | 64 | 0.615 | 0.608 | 0.584 | 0.986 | 0.970 |
| high_cardinality_tpch | 1 | 8192 | 2.688 | 2.665 | 2.615 | 0.996 | 0.972 |
| high_cardinality_tpch | 8 | 8192 | 1.973 | 1.945 | 1.943 | 1.003 | 0.989 |
| high_cardinality_tpch | 1 | 64 | 11.766 | 4.953 | 11.737 | 0.418 | 1.005 |
| high_cardinality_tpch | 8 | 64 | 15.178 | 7.299 | 15.003 | 0.475 | 0.984 |

Whole-system self-sample shares (Nata/kernel/libc), high cardinality at 64 MiB:

1 thread(s): 44.93/30.93/23.89% → 57.17/15.02/27.73%.
8 thread(s): 46.85/23.59/29.37% → 52.36/22.70/24.65%.

At the value gate, file opens were 143,628 → 12,800 (one thread) and
299,373 → 56,995 (eight); the full timed ranges are retained in samples.
[Samples/counters/load](raw/20261108-hash-aggregate-perf2d/fanout-samples.json),
[summary/binary hashes](raw/20261108-hash-aggregate-perf2d/fanout-summary.json),
[24 value gates](raw/20261108-hash-aggregate-perf2d/fanout-gates.json),
[checks](raw/20261108-hash-aggregate-perf2d/fanout-checks.txt).
Self-symbol and DSO reports accompany every profiled cell in the same directory.

Checks: `cargo test --locked --release -j 4 -p nata-exec` (including spill,
cleanup, damage, collision/no-progress and new estimator/membership/256-file
FD/quota/merge-concurrency tests), fmt, clippy/all targets, locked release build
of nata/nata-check, fresh SF0.01 plus SQL fixtures, aggregate **65/65**,
forced-spill aggregate **65/65**, SQL **252/252**: **PASS**.

```sh
python3 scripts/bench-aggregate-perf2d-local.py fanout --before /home/dev/perf2d-fanout/before --after /home/dev/perf2d-fanout/fanout --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2d --profile
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2d/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --profile release --output /home/dev/perf2d-fanout/mutants-fanout -p nata-exec -f crates/nata-exec/src/aggregate/pipeline.rs -f crates/nata-exec/src/aggregate/storage.rs -F 'estimate_resident|initial_fanout|merge_concurrency|member' --timeout 180
```

Focused mutants: **41 caught, 1 missed, 1 unviable, 0 timeout**, unmutated
baseline PASS (43 tested). The miss is `member`'s explicit `< fanout` →
`<= fanout`: masked hash equality still requires `partition < fanout`, so the
extra admitted comparison boundary cannot accept a record (equivalent within
validated 16..256). It is recorded as missed, not caught. The unviable mutant
changes `4 << 20` to `4 >> 20`, producing a compile-time divide by zero.
[Outcomes](raw/20261108-hash-aggregate-perf2d/fanout-mutants-outcomes.json),
[deciding log](raw/20261108-hash-aggregate-perf2d/fanout-mutants.txt).
Profile reports retain the first 90 symbol lines, with padding collapsed;
full perf recordings are discarded. All local cargo jobs are serial at `-j 4`.
Disk headroom remained 25 GiB; no stale-output removal was needed.

### 2. Decode state columns into retained group slots

Fan-out commit: `6a9dc6eb6008aef90e00f3ed00641e274edbd96b`.

**Rule:** bind the validated Arrow columns once per replay batch, map rows to
charged table group IDs, then decode aggregate state columns directly into
those groups. Remove the temporary `Vec<State>` collected per replay row.
Do not remove type/version/arity, length, UTF-8, canonical-value, count, exact
wide-state, trailing-byte, checksum, schema or partition-membership checks.

`Records` borrows the charged batch; fixed arrays for at most the existing
256 rows hold group IDs and borrowed state cursors. Each column consumes one
validated state from each cursor. New groups reserve their persistent vector's
capacity before allocation and push the first contribution directly; existing
groups merge it. Vectors retained per distinct group and owned canonical keys
remain necessary. No transient state vector is allocated per replay row. The
recursive repartitioner reuses one state scratch vector rather than collecting
a new one per row. No IPC schema or envelope changes.

Reasoned invariant: after column k, each newly inserted group has exactly k+1
initialized slots, and each prior group retains its full arity. No table is
published until all columns and cursor ends validate. String growth is charged
before retention; pressure discards the entire tentative table and replays the
parent, preventing partial-column double counting. Every grant and existing
cleanup path remains active. The 256-row replay bound is checked before stack
indexing; query scan/evaluation batches stay unchanged.

**Verdict: kept for settled validation.** Spill work stays effectively constant,
state replay no longer collects a temporary vector per partial row, and no
cell regresses beyond the identical-control spread (observed paired twins
0.886–1.124). The local spill gains are small relative to that spread; a net
speed/allocation benefit is **not established** here.

| Threads | Arm | Bytes written | Bytes read | Partitions | Depth |
|---|---|---:|---:|---:|---:|
| 1 | before | 253743048 | 253743048 | 256 | 0 |
| 1 | after | 253743048 | 253743048 | 256 | 0 |
| 1 | control | 253743048 | 253743048 | 256 | 0 |
| 8 | before | 281104728–286389576 | 281104728–286389576 | 256 | 0 |
| 8 | after | 280236640–288062248 | 280236640–288062248 | 256 | 0 |
| 8 | control | 281887224–285085520 | 281887224–285085520 | 256 | 0 |

Unsettled seconds, medians; paired after/before and identical-control/before ratios:

| Query | Threads | MiB | Before | After | Control | A/B | C/B |
|---|---:|---:|---:|---:|---:|---:|---:|
| q1 | 1 | 8192 | 1.176 | 1.198 | 1.166 | 1.011 | 0.991 |
| q1 | 8 | 8192 | 0.360 | 0.363 | 0.365 | 1.012 | 0.997 |
| q1 | 1 | 64 | 1.212 | 1.247 | 1.198 | 0.986 | 0.985 |
| q1 | 8 | 64 | 0.647 | 0.663 | 0.686 | 1.024 | 1.055 |
| high_cardinality_tpch | 1 | 8192 | 2.812 | 2.866 | 2.824 | 0.990 | 0.995 |
| high_cardinality_tpch | 8 | 8192 | 2.135 | 2.072 | 2.043 | 1.011 | 0.978 |
| high_cardinality_tpch | 1 | 64 | 5.307 | 5.193 | 5.287 | 0.977 | 0.998 |
| high_cardinality_tpch | 8 | 64 | 7.243 | 6.911 | 6.986 | 0.954 | 0.965 |

Whole-system self-sample shares (Nata/kernel/libc), high cardinality at 64 MiB:

1 thread(s): 59.99/12.69/27.29% → 59.40/13.23/27.28%.
8 thread(s): 52.63/23.31/23.90% → 52.33/21.12/26.29%.

In the retained first-90-symbol reports, state decoder iterator/collect symbols
(`replay::decode` with AggregateExpr) account for 2.91% → absent (one thread),
2.42% → absent (eight). The old production decoder is gone; absence is not an
allocation-count measurement. Global `malloc` self shares rise 5.56% → 5.79%
and 4.20% → 4.99%, respectively. No global malloc reduction is claimed.

[Samples/counters/load](raw/20261108-hash-aggregate-perf2d/decode-samples.json),
[summary/hashes](raw/20261108-hash-aggregate-perf2d/decode-summary.json),
[24 value gates](raw/20261108-hash-aggregate-perf2d/decode-gates.json),
[checks](raw/20261108-hash-aggregate-perf2d/decode-checks.txt).
Checks: serial `cargo test --locked --release -j 4 -p nata-exec`, including
unchanged typed/truncated/invalid-wide-state, spill/cleanup/fault/damage and
recursive-pressure tests; fmt, clippy/all targets, locked release CLI/check
build, fresh fixtures, aggregate **65/65**, forced aggregate **65/65**, SQL
**252/252**: **PASS**. No fault/damage test is edited or weakened.

```sh
python3 scripts/bench-aggregate-perf2d-local.py decode --before /home/dev/perf2d-fanout/fanout --after /home/dev/perf2d-fanout/decode --data /home/dev/src/nata/data/tpch-sf1 --out bench/results/raw/20261108-hash-aggregate-perf2d --profile
CARGO_TARGET_DIR=/home/dev/perf2b/target TMPDIR=/home/dev/perf2d/tmp RUST_TEST_THREADS=1 /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib --cargo-arg=--locked --profile release --output /home/dev/perf2d-fanout/mutants-decode -p nata-exec -f crates/nata-exec/src/aggregate/pipeline.rs -f crates/nata-exec/src/aggregate/storage.rs -F 'estimate_resident|initial_fanout|merge_concurrency|member' --timeout 180
```

Repeated focused estimator/fan-out/concurrency/membership mutants: **41 caught,
1 missed (the same equivalent membership boundary), 1 unviable (the same
constant divide by zero), 0 timeout**; unmutated baseline PASS.
[Outcomes](raw/20261108-hash-aggregate-perf2d/decode-mutants-outcomes.json),
[deciding log](raw/20261108-hash-aggregate-perf2d/decode-mutants.txt).
