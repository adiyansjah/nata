# W-C step 1 redo: typed Arrow slices and checked Decimal64

Branch base: `2b7a415a183c3e06caa379c8c627abc4a43b89fe` (local main,
including package 0). This supersedes the implementation approach in rejected
experiment `b1d5adf` / `7d54475`, whose source was reverted. That experiment's
Col conversion share fell from 3.6% to 1.4%, but its per-row Input accessors
cost more than the copy removed. This redo does not reuse that adapter.

The implementation is a candidate for the parent's settled c3d/n4a verdict.
Local measurements are **unsettled screens**, with an identical-baseline twin;
there is no local performance acceptance or target/scaling certification.

## Design and reachability

- `aggregate/fusion.rs::Program::evaluate` keeps the existing column-first DAG
  order and first-occurrence source positions. Eligible Decimal64 nodes retain
  Arrow buffers, including sliced buffers. Arithmetic dispatch occurs once per
  node; `nata_kernels::native::Decimal64::column` runs checked i64 add/sub/mul
  loops over slices and selection/validity words. Both aligned operands and the
  result are checked against `10^P`; cancellation cannot hide operand overflow.
  Machine overflow also fails the decimal precision bound. Unsupported result
  widths/operations/layouts retain the checked Col/i128 path. Fallback cast and
  conditional outputs may narrow after their original evaluation succeeds.
- `aggregate.rs::Partial::resume` dispatches each argument once per column.
  Native `numbers::<i64>` streams the existing sorted selection directly when
  `null_count()==0`, without constructing a bitmap or matching row ordinals.
  Nullable inputs use word masks and preserve the selection ordinal for group
  IDs. Both paths feed the unchanged `ExactSum::add(i128)` state. There is no
  per-row input enum, virtual method, Arrow validity call, or value accessor.
  Fully active words have contiguous loops; no-NULL inputs bypass bitmap
  traversal entirely. Existing group/state lookup remains; W-A state storage and W-C step 2 are outside this change.
- `aggregate/native.rs` combines the sorted-unique aggregate selection with
  offset-aware Arrow bitmap chunks. This is not a duplicate/unsorted gather API.
  Keys and non-SUM/AVG consumers keep Col semantics. No dictionary-code identity,
  SUM/AVG state sharing, narrow accumulator/overflow flag, or unchecked product
  admission was added. Scalar constants still use batch buffers.
- Arrow parents live through the admitted batch and any spill continuation, then
  `release_text` releases native handles before the next decode. Native outputs,
  optional Col fallback storage and bitmap scratch fit the unchanged
  128-byte/expression/row allowance. Decode grants, defaults and caps are unchanged.
  Native materialization sites contribute to package 0's existing counters.

This implements ADR 0003's existing narrow-decimal/selected-valid rules; no ADR,
threshold, target or dependency changed. General text/date/float adapters and
R10/R27/R29's broader proof/ownership gaps remain open.

## Checks and proof inventory

- `CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --locked --release -j 4 -p nata-kernels -p nata-exec`: **158/158** (72 unit, 86 integration; child-process reruns excluded).
  [Final full test log](raw/20261108-engine-perf-wc-redo/checks/fast-tests.log).
- `cargo fmt --all --check`; release `cargo clippy --locked --release -j 4 -p nata-kernels -p nata-exec --all-targets -- -D warnings`: pass.
- `cargo build --locked --release -j 4 -p nata -p nata-check`: pass. Baseline is built from a git archive of branch base, with identical release settings. Workspace source timestamps were refreshed after switching the shared target from that archive to avoid stale Cargo artifacts.
- Fixtures: `scripts/gen-tpch.sh 0.01 /tmp/nata-wc-redo/gates` and `scripts/gen-sql-check-data.sh /tmp/nata-wc-redo/gates`.
- `env -u AI_AGENT .../nata-check --candidate .../nata --data /tmp/nata-wc-redo/gates --queries check/queries/aggregate`: **65/65**; forced spill **65/65**; SQL **252/252**; scan **5/5**.
- Legacy decimal and decimal-differences directories exit zero with **30 and 12 PENDING**, zero pending-wrong. They require no additional data, but unsupported literal/VALUES syntax means these are not arithmetic passes. Active decimal cases in SQL/aggregate suites do execute.
- Planted overflow: a product of `10^18` fits i64 but fails DECIMAL(18). Earlier node row 1 keeps position **2:10**, ahead of later node row 0 at **3:20**. Selecting only row 0 preserves **3:20**. Exact `EvalError` equality with the original evaluator is asserted; NULL-only selection does not fire.
- Timing-runner stats/value-gate regression test, inert heavy-runner self-test (including both machine plans), and `bash scripts/gcp/test/test-job-chains.sh`: pass. Prepared-step shell syntax and docs-only/missing/wrong-baseline controls pass, including under `PYTHONOPTIMIZE=1`.

- **Kani 0.68 / CBMC 6.11:** fifteen final-source harnesses verified, zero failed properties; **40 reachable covers satisfied**, plus one intentionally unreachable overflow cover in the signed-8-bit traversal harness. The independent full-i64 production product proof takes 19.020 s and covers success, overflow and the exact `10^18` boundary. [Full results and logs](raw/20261108-engine-perf-wc-redo/proofs/summary.json).
- **Negative controls:** four actual source mutations, **4 caught** by completed failing Kani proofs (precision equality, ignored mask, wrong input row, product→addition). A timeout/compile error is not counted as a catch. [Outcomes](raw/20261108-engine-perf-wc-redo/kani-controls/controls.json).
- **cargo-mutants 27.1.0:** final focused run **6 caught, 0 missed, 0 timeout, 0 unviable** (four kernel, two bitmap-adapter mutants). Both unmutated baselines pass. The initial four-kernel run had **3 caught and 1 timeout**: the wrong-direction bit clear repeated a row indefinitely. The bitmap test now asserts each active row is visited once before running arithmetic, so that same mutant fails immediately. [Initial run](raw/20261108-engine-perf-wc-redo/mutants-kernels-initial.json), [final kernels](raw/20261108-engine-perf-wc-redo/mutants-kernels.json), [adapters](raw/20261108-engine-perf-wc-redo/mutants-adapter.json).
- Full touched-crate tests and all SQL gates were rerun after that test strengthening; restored-source unit tests pass **72/72** after the final mutations. A release rebuild is **byte-identical** to the immutable candidate binary. The final normal proof matrix and all four caught controls use the final no-NULL/nullable implementation. Native source hashes match the restored final file. [Source/binary/tool identities](raw/20261108-engine-perf-wc-redo/identity.json).
- Before timing, exact Nata/before versus Nata/after CSV multisets are identical for Q1 and high-cardinality at 1/8 threads, with no AVG tolerance. [Exact gates](raw/20261108-engine-perf-wc-redo/exact-gates.json).

Reproduction (serial; four Cargo build jobs, no concurrent build/test):

```sh
# From spikes/kani-nata-kernels; use exact fully qualified harness names.
CARGO_BUILD_JOBS=4 cargo kani -Z stubbing --exact --harness wc1_full_product::wc1_mul_full_i64
# Repeat for the fourteen other names in proofs/status.txt.
# From repository root:
CARGO_BUILD_JOBS=4 python3 bench/results/raw/20261108-engine-perf-wc-redo/kani-controls.py /tmp/wc1-controls
CARGO_PROFILE_RELEASE_DEBUG=1 /home/dev/perf2b/cargo-mutants mutants \
  --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib \
  --profile release --output /tmp/wc1-kernel-mutants -p nata-kernels \
  -f crates/nata-kernels/src/native.rs \
  -F '(replace < with <= in Decimal64::check|replace == with != in selected|replace << with >> in selected|replace numbers .* with Ok)' --timeout 180
CARGO_PROFILE_RELEASE_DEBUG=1 /home/dev/perf2b/cargo-mutants mutants \
  --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--lib \
  --profile release --output /tmp/wc1-adapter-mutants -p nata-exec \
  -f crates/nata-exec/src/aggregate/native.rs \
  -F '(replace selection .* with vec!\[\]|replace intersect with \(\))' --timeout 180
```

No production result/error mismatch, new lockfile dependency or ADR change was
needed. All final Cargo validation was serial with `-j 4` (Kani compilation via
`CARGO_BUILD_JOBS=4`). See the interrupted-trial exception below. Free disk stayed
above 20 GB; no stale-output deletion was triggered.

Production reachability and proof boundaries:

| Production function | Reference / checks | Kani domain | Negative control |
|---|---|---|---|
| `native::Decimal64::column` add/sub | checked wide `Decimal::scalar`, SQL suites, positioned-error test | `wc1_add`, `wc1_sub`: two rows, arbitrary i64 values, all subset masks; precision 18, scale 0 | accept equality at the decimal precision limit |
| same, alignment | checked wide scalar, boundary unit matrix | `wc1_alignment`: one row, arbitrary i64, add/sub, factor 100, precision 18; `wc1_q1_alignment`: precision 16, scale 2, the actual Q1 add/sub rule | same precision mutant |
| same, multiply traversal | checked wide scalar, precision/machine boundary unit matrix, SQL Q1 | `wc1_mul`: two rows, signed 8-bit operands, all subset masks | selection ignored (through shared bitmap traversal) |
| `native::Decimal64::product`, the inline arithmetic leaf called by that loop | independent exact i128 multiplication and decimal bound | `wc1_mul_full_i64`: all i64 operand pairs, precision 18; no stub or assumed arithmetic fact | replace checked product by checked addition |
| `native::selected` | independent expected active rows, offset/word/padding unit tests | the eight `wc1_numbers_*` selection families: three rows; `wc1_word_boundary`: 65 rows, full/sparse first word and last bit | ignore selection bits |
| `native::numbers::<i64>` | exact SUM/AVG integration and SQL gates; i128 instantiation also unit-tested | eight concrete selection-family harnesses: every subset of three rows, both no-NULL/nullable modes, all validity subsets, arbitrary i64 values; callback ordinals and skipped NULLs checked | read row 0 instead of selected row |
| Arrow offset/null composition and parent lifetime | sliced offset matrix, pointer sharing, read after original handles drop, existing null/spill gates | **not covered by Kani**; Arrow remains outside the isolated proof crate | focused adapter mutants |
| exact accumulation/merge/finalize | unchanged `ExactSum` production functions and existing aggregate tests | no new proof claim for wide accumulator internals | existing corpus retained |

The full-width proof targets the arithmetic function actually called by the SQL
column loop; its `#[inline(always)]` attribute adds no accessor/dispatch layer.
The bitmap/column proofs retain their separate row/value bounds. Initial combined
column×full-i64 trials were interrupted or timed out (the direct trial at 240 s);
they are not counted as proofs. One cancellation briefly left a CBMC solver
running as a replacement started; it was killed and final validation was rerun
serially, before any timing. The successful factored arithmetic proof does not
use the trial's asserted/assumed cut lemmas.

The harness includes the actual production source by path in the existing
isolated Kani crate. No proved copy or unused arithmetic helper is counted.
These are bounded proofs at the listed precisions/scales/row shapes, not a
universal proof of all SQL decimal types, Arrow, allocation or aggregation.

## Unsettled local screen and same-run stats

Nine rounds per cell, alternating before/after/identical-before, reversed arm order on odd rounds. Warm SF1, whole CLI including planning/decode/CSV, 8 GiB, unchanged release settings (`CARGO_PROFILE_RELEASE_DEBUG=1`; overflow checks on). **All numbers below are unsettled.** Ratios are medians of paired round ratios, not ratios of independent arm medians.

| Query | Threads | Before min / median s | After min / median s | Paired after/before [Q1,Q3] | Control/before [min,max] |
|---|---:|---:|---:|---:|---:|
| Q1 | 1 | 1.037880 / 1.125207 | 0.936680 / 1.008435 | 0.911229 [0.870256,0.914598] | 0.994343 [0.952334,1.012897] |
| Q1 | 8 | 0.352770 / 0.383095 | 0.303677 / 0.341693 | 0.915586 [0.866057,0.930140] | 0.985483 [0.805184,1.044149] |
| HC | 1 | 2.455873 / 2.543945 | 2.412531 / 2.459596 | 0.966843 [0.957111,0.979738] | 0.994563 [0.950350,1.147727] |
| HC | 8 | 1.692362 / 1.795062 | 1.658442 / 1.732886 | 0.986760 [0.952088,1.026229] | 0.999360 [0.962886,1.127951] |

No median regression exceeds the observed identical-baseline control spread. No local speed/target/scaling acceptance is assigned. In particular the multithread and HC controls remain noisy. The parent must judge the settled both-CPU matrix.

[Final samples](raw/20261108-engine-perf-wc-redo/screen-fast/native-samples.json), [summary and binary/input hashes](raw/20261108-engine-perf-wc-redo/screen-fast/native-summary.json), [same-run stats sidecars](raw/20261108-engine-perf-wc-redo/stats-fast/), [comparison chart](raw/20261108-engine-perf-wc-redo/screen-fast/comparison.html). Twelve value gates precede 108 timed arms; all exits are zero. All 120 native invocations retain individual stats files. The eight saved perf reports use cpu-clock/499 Hz/DWARF with release debuginfo and report zero lost samples; they are local supporting profiles, not an ARM attribution. [Local CPU/topology](raw/20261108-engine-perf-wc-redo/local-machine.json) records an 8-logical-CPU Intel i3-1220P guest with 8 virtual cores/one thread per core, not c3d's physical-core/SMT topology.

The first screen of `e755653` is retained separately: [initial samples/summary](raw/20261108-engine-perf-wc-redo/screen/native-summary.json). Its paired medians were Q1 **0.991279 / 1.006825**, HC **1.004344 / 0.999060**, at 1/8 threads. It was **not rejected on local timing**. Its Q1/1t same-run expression medians fell 246.725→146.885 ms, while update rose 207.220→285.471 ms. Source inspection found unnecessary bitmap/ordinal work on the no-NULL path. `73e561e` replaces that with the typed direct-selection loop; nullable word traversal remains. The final screen above compares that cumulative source against the same branch base afresh. There is no cross-session speed ratio between the two candidates.

Command for the final screen:

```sh
NATA_QUERY_STATS=1 NATA_STATS_DIR="$PWD/bench/results/raw/20261108-engine-perf-wc-redo/stats-fast" \
python3 scripts/bench-aggregate-perf2b-local.py native \
  --before /tmp/nata-wc-redo/before --after /tmp/nata-wc-redo/after \
  --data /home/dev/nata-s5-fixes/sf1 \
  --out bench/results/raw/20261108-engine-perf-wc-redo/screen-fast --rounds 9 --profile
```

`NATA_QUERY_STATS=1` makes `bench_verdict.invoke` assign a fresh
`NATA_STATS_JSON` per native invocation. The timing helper now preserves those
sidecars under `NATA_STATS_DIR` before deleting temporary process files, and
stores each path plus its parsed stats in the sample. A fake-engine regression
test checks distinct files survive cleanup and a wrong candidate fails before
any timed arm. The prepared GCP job uses the same mechanism per cell.

Package 0 reports **service wall time**, not exclusive CPU time. At eight threads
these intervals can overlap and cannot be added to derive CLI wall time. Its
`complete=false` and explicit missing-allocation/worker-CPU/wake/native-sink
inventory are retained, not promoted into complete attribution. Stats-on applies
to both arms and the identical-baseline control in these same runs.

Per-stage medians from the **same nine timed invocations per arm**, service wall milliseconds (before → after). Stage medians are not summed into a synthetic execution. All four local cells are in memory; spill stages are zero. The prepared GCP job adds the 64-MiB cells.

| Stage | Q1 / 1t | Q1 / 8t | HC / 1t | HC / 8t |
|---|---:|---:|---:|---:|
| scan_decode | 209.160 → 203.806 | 449.792 → 433.357 | 264.410 → 212.302 | 626.503 → 468.702 |
| filter | 81.690 → 80.661 | 174.480 → 173.649 | 132.078 → 109.487 | 340.273 → 267.477 |
| expression | 253.152 → 147.533 | 467.730 → 285.509 | 318.786 → 287.197 | 472.285 → 418.257 |
| group_id | 226.432 → 226.155 | 489.472 → 479.951 | 750.819 → 816.444 | 1479.009 → 1635.411 |
| state_update | 212.403 → 194.128 | 457.392 → 424.320 | 105.964 → 98.396 | 212.468 → 205.318 |
| merge | 0.008 → 0.007 | 0.131 → 0.042 | 0.044 → 0.045 | 919.987 → 901.100 |
| finalize | 0.038 → 0.038 | 0.046 → 0.048 | 390.090 → 373.878 | 532.222 → 521.235 |
| output_sink | 0.078 → 0.075 | 0.105 → 0.087 | 714.453 → 701.727 | 812.516 → 811.020 |
| spill_write | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| spill_read | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| teardown | 0.207 → 0.192 | 0.155 → 0.288 | 16.459 → 17.454 | 30.286 → 28.354 |
| admission_wait | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| bind_metadata | 1.643 → 1.373 | 2.104 → 1.826 | 1.648 → 1.836 | 1.706 → 1.751 |

Q1/1t copied-byte site counters fall **520,660,560→118,332,372**. Materialized-byte site counters are **520,660,560→357,996,716**. These are package 0’s scoped counters, not a complete allocator/RSS inventory; the native output sites are counted, while inherited missing-site flags remain. No grant or charge reduction follows from these numbers.

[All stage and counter tables](raw/20261108-engine-perf-wc-redo/screen-fast/stage-tables.md), [machine-readable stages including control arms](raw/20261108-engine-perf-wc-redo/screen-fast/stats-summary.json).

## Parent's settled GCP handoff (prepared, not run)

[`aggregate-wc1.steps`](../aggregate-wc1.steps) uses `# gate-exits`, checks that the
source archives contain an actual Rust change from the branch base, records
CPU topology/affinity, runs the profile pre-check, the inert timing-harness test,
and the aggregate/spill/SQL/scan gates before timing. The settled 2b runner
compares baseline and final checked source, plus identical twins and DuckDB:
SF1, Q1/high-cardinality, 1/8 threads, 8 GiB/64 MiB, five alternating rounds,
independently on c3d-highcpu-8 and n4a-highcpu-8. Every native timed cell has
persisted stats sidecars; no cloud identifiers are committed.

Preparation from the final committed branch (no source changes after checks):

```sh
WC_BASE=2b7a415a183c3e06caa379c8c627abc4a43b89fe
WC_FIX=$(git rev-parse HEAD)
git merge-base --is-ancestor "$WC_BASE" "$WC_FIX"
WC_SHIP=$(mktemp -d /tmp/nata-wc1-ship.XXXXXX)
git archive --format=tar --output "$WC_SHIP/source.tar" "$WC_FIX"
sha256sum "$WC_SHIP/source.tar" > "$WC_SHIP/source.sha256"
mkdir "$WC_SHIP/tree"
tar -xf "$WC_SHIP/source.tar" -C "$WC_SHIP/tree"
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline "$WC_BASE" \
  --fix wc1="$WC_FIX" --bundle "$WC_SHIP/tree/.aggregate-wc1-inputs"
sha256sum "$WC_SHIP/tree/.aggregate-wc1-inputs/"*.tar.gz > "$WC_SHIP/bundle.sha256"
git rev-parse "$WC_FIX" > "$WC_SHIP/tree/.aggregate-wc1-commit"
cp bench/aggregate-wc1.steps "$WC_SHIP/tree/heavy.steps"
```

Exact sizing and commands for the parent (two independent 8-vCPU jobs; 80 GB data
disk each, maximum two VM-hours each; owner-approved currency bounds supplied at
run time). Plan/run are **not executed by this implementation turn**:

```sh
"$WC_SHIP/tree/scripts/gcp/orchestrate.sh" plan --job heavy --machines c3d-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc1-x86
"$WC_SHIP/tree/scripts/gcp/orchestrate.sh" plan --job heavy --machines n4a-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc1-arm
"$WC_SHIP/tree/scripts/gcp/orchestrate.sh" run --job heavy --machines c3d-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc1-x86 --approved-usd "${WC_APPROVED_X86_USD:?owner-approved bound required}"
"$WC_SHIP/tree/scripts/gcp/orchestrate.sh" run --job heavy --machines n4a-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc1-arm --approved-usd "${WC_APPROVED_ARM_USD:?owner-approved bound required}"
```

Initial implementation/checks commit: `e755653659f6b795c6fbf43bf7439d7d25a5228c`.
Kept cumulative implementation and no-NULL fast path:
`73e561ed4166b76816ea5be6e87368aec7a78ac7`. The subsequent evidence commit adds
this report, the card progress line, local samples and stats. `WC_FIX=HEAD` above
nominates that final evidence/source commit, whose Rust source is unchanged.
`scripts/check-leaks.sh --staged` is run before each commit. No push, merge or
GCP operation was made. Final shipment paths and SHA256 manifests are reported
in the implementation handoff; they contain the final commit, not an uncommitted
working tree.

## W-C step 1 timing on settled machines (runs `wc1-x86`, `wc1-arm`, added by the parent, 2026-11-08)

Baseline `2b7a415` against W-C (`9400b26`), SF1, value-gated against DuckDB 2.0.0, five alternating rounds with twins, c3d-highcpu-8 and
n4a-highcpu-8, Spot; every gate step exited 0 on both CPUs. Raw: `raw/20261108-engine-perf-wc1-gcp/` (timings, per-CPU summaries). Medians,
seconds. "Now / DuckDB" is W-C against DuckDB.

| CPU | Query | Threads | Budget | Baseline | W-C | W-C / baseline | Now / DuckDB |
|---|---|---:|---|---:|---:|---:|---:|
| x86-64 | q1 | 1 | in memory | 1.366 | 1.199 | **0.88** | 4.3x |
| x86-64 | q1 | 1 | 64 MiB | 1.396 | 1.261 | 0.90 | 4.6x |
| x86-64 | q1 | 8 | in memory | 0.331 | 0.285 | **0.86** | 2.5x |
| x86-64 | q1 | 8 | 64 MiB | 0.387 | 0.342 | 0.88 | 2.9x |
| x86-64 | high cardinality | 1 | in memory | 3.053 | 3.030 | 0.99 | 2.4x |
| x86-64 | high cardinality | 8 | in memory | 2.308 | 2.332 | 1.01 | 2.3x |
| ARM64 | q1 | 1 | in memory | 1.006 | 0.961 | 0.95 | 4.4x |
| ARM64 | q1 | 1 | 64 MiB | 1.005 | 0.962 | 0.96 | 4.3x |
| ARM64 | q1 | 8 | in memory | 0.161 | 0.158 | 0.98 | 2.3x |
| ARM64 | q1 | 8 | 64 MiB | 0.291 | 0.281 | 0.96 | 4.0x |
| ARM64 | high cardinality | 1 | in memory | 2.340 | 2.335 | 1.00 | 2.4x |
| ARM64 | high cardinality | 8 | in memory | 1.535 | 1.565 | 1.02 | 2.1x |

**Verdict: kept.** Q1 is 10-14% faster on x86-64 and 2-5% on ARM64; high cardinality is neutral. Frozen targets remain NOT MET.

**Regression found in the baseline, not caused by W-C:** high cardinality at 64 MiB and 8 threads is 39.804 s (x86-64) and 25.762 s (ARM64)
at `2b7a415`, against about 4.9-5.5 s in the step 2e settled run. The 1-thread 64 MiB cells did not regress. Being bisected. The `profile`
step reported the pre-check as unavailable on both CPUs; it does not affect these timings.
