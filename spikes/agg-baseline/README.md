# Aggregate baseline (throwaway)

Local-only card 0005 part 4 experiment. This is an isolated Cargo workspace, not
production code. Arrow is pinned to the root lockfile's 60.1.0; hashbrown to hash-ops' 0.17.1. No SQL engine,
GCP job or production scheduler is implemented here. Delete the spike after the
parent accepts the evidence and ports the relevant checks.

Run from the repository root, **one command at a time**:

```sh
mkdir -p "$HOME/tmp-agg-spike"
export TMPDIR="$HOME/tmp-agg-spike" CARGO_BUILD_JOBS=2
cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml --no-run
cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml -- --test-threads=1
(ulimit -v 262144; timeout 120 cargo test --locked --manifest-path spikes/agg-baseline/Cargo.toml --lib --bins --tests -- --test-threads=1 --nocapture)
spikes/agg-baseline/run-loom.sh
cargo fmt --manifest-path spikes/agg-baseline/Cargo.toml -- --check
cargo build --locked --release --manifest-path spikes/agg-baseline/Cargo.toml
# On a quiet local machine only; warm page cache, indicative, not a product gate.
(ulimit -v 524288; timeout 120 spikes/agg-baseline/target/release/agg-baseline)
```

`run-loom.sh` models the same grant code compiled through `cfg(nata_loom)`;
its default preemption bound is 2. It runs three tiny mutants without rebuilding
per mutant. A timeout, build failure, missing test, or unrecognized failure is
INVALID, fails the script, and never counts as CAUGHT. The dispatcher condvar is
a test harness stand-in for waiting on runnable tasks; CPU operator calls are
`request` and `park` and do not block. Four models cover two sink ids, one live grant per sink in
these models, budget 10, nonrevocable progress reserve 2, minimum request 4/8.
No claim of unbounded fairness or a model of the production scheduler.

The partition frame has a fixed schema/version and 1 MiB payload / 1,024-row
caps. Before Arrow: check file remainder, 8 MiB working grant, CRC of header and
payload, exact trusted schema message, message/metadata/body counts and lengths,
node counts, null counts, minimum buffer sizes, UTF-8 offsets, dictionary indices
and nonoverlapping backed buffer ranges. Compression,
nested/dynamic schemas, dictionary deltas and extra batches are rejected. Arrow
then performs its normal array validation. The record codec is only a
control arm, never a replacement decision. This tests i128 partial storage at
scale 4, **not** exact i256 accumulation/AVG rounding or partition recursion.

The scheduling experiment uses a common `claim()` interface with either a
locked FIFO or per-worker crossbeam deques with stealing. Every arm computes the
same per-morsel aggregate and compares exact results by task id with std's map.
Affinity is opt-in and checked; nothing changes a product default. Counters
include claims, local pops, steals, retries, steal probes and successful pins.
The table arm uses the hash-ops `HashTable<group-id>`/owned-arena pattern with
wide keys and richer states; it does not reuse hash-ops' narrower `HashAgg` kernel.
The table timing separates fresh and retained hashbrown instances and has a
second std-map control; IPC has a second sample per round; scheduling has a second unpinned FIFO control.
Std and hashbrown share the same precomputed hash and full-key equality.

Summarize a captured benchmark with `python3 spikes/agg-baseline/summarize.py <output-file>`.
