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
CPU work. The quota is a measurement override, not a changed default. Each
stage validates values before timing (exact numeric multisets; only Q1's three
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

Observed Q1 at eight threads improved from 4.670 to 3.496 s. The one-thread
Q1 cell is unresolved (both Nata and DuckDB slowed); baseline HAVING has very
large load/noise drift. These historical samples cannot attribute gains, even though only one source
fix changed between stages. No both-CPU or frozen-target
performance claim follows.

### Local profile comparison (CPU-clock user self samples)

Eight requested threads, same SF1 input; CPU shares, **not elapsed gains**.
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
