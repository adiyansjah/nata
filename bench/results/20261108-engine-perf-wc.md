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

## W-C step 2, part 1: dictionary-value group IDs (2026-11-08)

**Kept for the parent's settled measurement; local evidence is UNSETTLED.**
Baseline is slice 5 closing commit `709674d47891c340df63273d4e8e8e415743f830`.
No query result/error, hash seed/version, spill format, grant/default, target,
ADR or dependency changes. The general hash path remains the fallback.

Rule: remove per-row string hashing/comparison only when every key is a small
string dictionary. Hash each dictionary value with the existing canonical XXH3
function; combine hashes once per dictionary tuple, including NULL. The product
of `(dictionary length + 1)` must fit **256 and selected-row count**, otherwise
fall back. A typed checked code loop builds batch-local tuple addresses; the
first observed tuple probes by full value equality and later rows reuse its
canonical table group ID. Codes never establish cross-batch/file identity.
Duplicate/reordered dictionaries still resolve by values. The cache resets on
resume after spill; stable entry indices survive hash-table growth. String
MIN/MAX keeps per-row string-capacity checks. Existing per-expression/row task
workspace covers the bounded hash arrays and row addresses (no charge cut).

Checks (serial, Cargo `-j 4`, release profile with `CARGO_PROFILE_RELEASE_DEBUG=1`):

- Touched-crate `cargo test --locked --release -j 4 -p nata-kernels -p nata-exec`:
  **164 passed**, excluding the two child-process reruns. fmt and clippy
  (`--all-targets -- -D warnings`): pass.
- `cargo build --locked --release -j 4 -p nata -p nata-check`: pass.
  Fresh data: `env -u AI_AGENT scripts/gen-tpch.sh 0.01 /tmp/nata-wc2/gates`
  and `scripts/gen-sql-check-data.sh /tmp/nata-wc2/gates`.
  `env -u AI_AGENT .../nata-check --candidate .../nata --data /tmp/nata-wc2/gates
  --queries check/queries/aggregate`: **65/65**; again with
  `--force-aggregate-spill`: **65/65**; SQL **252/252**; scan **5/5**.
- Tests cover selected NULLs, masked invalid indices, dictionary-value NULLs,
  duplicate/reordered dictionaries, Unicode/empty values, caps and mixed-type
  fallback. A whole-SQL two-key fixture changes dictionaries across row groups
  and files at 1/4 threads, resident/forced-spill, normal/forced-collision paths.
- Production `nata_kernels::dictionary::append` is included by path in the
  existing isolated Kani crate. `wc2_dictionary::wc2_codes`: **verified**, all
  **3 covers satisfied**. Bounds: three arbitrary row addresses (including
  duplicates/permutations), arbitrary u32 codes/validity, radices 1..4, prior
  IDs 0..255. The independent formula checks every successful address and
  identifies exactly live invalid codes. An actual source mutation ignoring
  validity produces a completed failing proof (**1/1 caught**).
  Arrow ownership, canonical XXH3 internals, allocation and table probing are
  test-covered, not claimed proved. Checked integer/address failure boundaries
  outside the bounded proof are unit-tested.
- cargo-mutants **4/4 caught**, zero missed/timeouts/unviable: return None,
  return Some unconditionally, invert length check, invert code range check.
  Unmutated baseline passes. Restored source matches the checked implementation.
- Exact before/after CSV multiset gates (no tolerance) pass for SF1 Q1 and
  high-cardinality, 1/8 threads. Differential DuckDB gates precede timed arms.

Evidence: [checks, proofs, mutants and exact gates](raw/20261108-engine-perf-wc2/),
[raw local samples](raw/20261108-engine-perf-wc2/part1-screen/part1-samples.json),
[stats sidecars](raw/20261108-engine-perf-wc2/part1-stats/).

Nine alternating rounds per cell (before/after/identical-before, reversed on
odd rounds), warm SF1, whole CLI including startup/planning/decode/CSV, 8 GiB,
1/8 threads. **Unsettled screen only**, never a target or scaling verdict.
Use the observed control spread, including the dev VM's historical 0.83–1.98,
not a 10% local acceptance rule. The settled both-CPU matrix decides retention.

| Query | Threads | Before min / median s | After min / median s | Paired after/before | Control median [min,max] |
|---|---:|---:|---:|---:|---:|
| q1 | 1 | 1.110882 / 1.162604 | 0.893282 / 0.924232 | 0.801943 | 1.014106 [0.847480,1.083882] |
| q1 | 8 | 0.357000 / 0.407766 | 0.283955 / 0.337971 | 0.840314 | 1.041081 [0.676713,1.172731] |
| high_cardinality_tpch | 1 | 2.594095 / 2.696218 | 2.575197 / 2.660456 | 0.977623 | 0.987338 [0.910199,1.035669] |
| high_cardinality_tpch | 8 | 1.732235 / 1.850935 | 1.662262 / 1.825769 | 0.987290 | 0.971330 [0.933356,1.038678] |

No median regression exceeds the identical-baseline control spread. All native
invocations retain distinct `NATA_STATS_JSON` sidecars. Service wall medians from
the same nine timed invocations follow; they overlap at 8 threads and must not
be summed into CLI wall/CPU time. Package 0 remains `complete=false` with its
explicit missing-site/CPU/wake inventory. All local timed cells are in memory.

| Stage (service wall ms) | Q1 / 1t | Q1 / 8t | HC / 1t | HC / 8t |
|---|---:|---:|---:|---:|
| scan_decode | 233.315 → 227.575 | 514.021 → 486.164 | 232.751 → 227.973 | 547.354 → 498.527 |
| filter | 93.003 → 87.763 | 197.726 → 206.484 | 121.219 → 118.269 | 293.950 → 296.795 |
| expression | 170.613 → 168.158 | 327.307 → 320.940 | 316.941 → 311.233 | 446.614 → 439.075 |
| group_id | 266.159 → 45.373 | 541.563 → 93.599 | 893.968 → 859.315 | 1777.090 → 1709.435 |
| state_update | 225.343 → 225.945 | 490.542 → 479.708 | 111.560 → 110.163 | 222.696 → 210.888 |
| merge | 0.008 → 0.009 | 0.047 → 0.051 | 0.050 → 0.050 | 928.679 → 914.181 |
| finalize | 0.043 → 0.038 | 0.051 → 0.045 | 415.859 → 410.179 | 519.268 → 533.333 |
| output_sink | 0.087 → 0.066 | 0.099 → 0.100 | 742.468 → 717.515 | 832.070 → 825.763 |
| spill_write | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| spill_read | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| teardown | 0.213 → 0.184 | 0.295 → 0.246 | 16.939 → 35.385 | 26.251 → 27.313 |
| admission_wait | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| bind_metadata | 1.913 → 2.004 | 1.854 → 2.081 | 1.704 → 1.934 | 1.924 → 2.189 |

Reproduction: [exact commands and proof bounds](raw/20261108-engine-perf-wc2/reproduce.md).
The baseline and read-only candidate are separate immutable binaries. Disk free
remained above 20 GB. `scripts/check-leaks.sh --staged` precedes the commit.
The prepared `bench/aggregate-wc2.steps` will contain baseline, dictionary-only
and cumulative shared-state archives; no GCP operation is run here.

## W-C step 2, part 2: shared SUM/AVG state (2026-11-08)

**Kept for the parent's settled measurement; local evidence is UNSETTLED.**
Dictionary-only commit: `f774be08275353cf7bd23d97542ceda79e98d511`.
This part's local denominator is that immutable binary, not another session's
baseline timings. The GCP bundle compares both cumulative revisions against
branch base `709674d47891c340df63273d4e8e8e415743f830`.

Rule: the physical aggregate program maps equal typed input-expression nodes
to their first exact SUM/AVG consumer before scanning. Q1's seven consumers
have **five ExactSum owners**; the two other slots are memory-only aliases.
COUNT and extrema remain independent. Earlier consumer ownership preserves
update/merge error priority. ExactSum's count guard, wide promotion, update,
merge and rounding arithmetic are unchanged. Resident merges combine owner
states once and aliases remain aliases. No charge or bytes/group reduction is
claimed: logical state slots and conservative charges remain allocated.

Finalization expands aliases from the exact unfinalized owner before executing
every original consumer in slot order. Thus an AVG that fits cannot suppress
a SUM overflow, including when AVG precedes SUM, HAVING is false, or LIMIT is
zero. SUM still narrows only at group finalization; AVG divides the exact sum.
Typed expressions with different inputs/types never share.

**Spill format stays version 1.** The writer expands each alias to the same
full typed sum/count record the old implementation wrote; a test asserts
byte-for-byte equality with independent slots. It adds no disk tags or schema,
and neither canonical hashes nor hash/partition membership validation changes.
Replay still validates and merges each logical slot independently, including
expanded copies. This deliberately preserves validation/error behavior for
untrusted, individually valid records; no cross-slot equality is assumed from
disk. It also means replay still uses one state per logical consumer. The hot
partial accumulation/resident merge does the seven-to-five work; this change
makes no five-state spill-replay memory or codec-speed claim. No ADR amendment,
format extension or dependency was needed.

Checks (serial, four jobs, same release settings as part 1):

- `cargo test --locked --release -j 4 -p nata-kernels -p nata-exec`:
  **169 passed**, excluding child-process reruns. fmt, clippy all targets
  with `-D warnings`, locked release build of `nata`/`nata-check`: pass.
- Fresh step-2 fixtures from the recorded generation commands; aggregate
  **65/65**, forced spill **65/65**, SQL **252/252**, scan **5/5**.
- Physical-plan test asserts Q1's seven slots resolve to `[0,1,2,3,0,1,6]`:
  exactly five sums, separate COUNT, no alias for a changed input type.
- Independent expected-value tests cover exact cancellation, all-NULL state,
  non-NULL versus COUNT(*) counts, half-away negative AVG, and merging once.
  Planted two-max-DECIMAL(38) input yields a fitting AVG and overflowing SUM in
  either order, resident/forced spill, 1/4 threads. Exact positioned EvalError
  equality is asserted for AVG-before-SUM. Spill bytes equal the old expanded
  state; corrupting an expanded slot's count remains rejected.
- No numerical kernel changed or was added in part 2. The production dictionary
  proof and negative control from part 1 still cover the unchanged source.
  Existing wide ExactSum proofs/trust gaps are not upgraded by this change;
  planner aliases, Arrow and IPC remain covered by tests/mutations, not Kani.
- An initial focused mutation run had **3 caught, 1 missed**: inversion of the
  native Decimal64 NULL-mask condition was outside the then-current wide
  shared-state fixtures. A real SQL Decimal64 fixture now asserts SUM, AVG,
  COUNT(x) and COUNT(*) with NULLs plus a selected-out nonzero value, at 1/4
  threads and with/without spill. Full tests and gates were rerun; the resulting
  production binary is byte-identical to the earlier part-2 build (test-only
  strengthening). The initial outcome is retained, not counted as a pass.
- Final focused cargo-mutants: **4/4 caught**, zero missed, timeouts or
  unviable mutants; unmutated baseline passes. Controls invert owner/alias
  update choice, native NULL-mask choice, state construction and spill alias
  index validation. [Initial run](raw/20261108-engine-perf-wc2/part2-mutants-initial.json),
  [final run](raw/20261108-engine-perf-wc2/part2-mutants-final.json).
- Exact CSV multiset gates (no tolerance) pass for SF1 Q1/high-cardinality,
  1/8 threads before timing. The 12 DuckDB value gates also pass. No result,
  positioned error, hash or spill-format mismatch was observed.

The following **unsettled** numbers use nine alternating rounds and an
identical dictionary-only control, warm SF1, 8 GiB, whole CLI/CSV. Per-stage
values are same-invocation service-wall medians, not exclusive CPU time;
8-thread stages overlap. Package 0 remains explicitly incomplete. Raw samples,
quartiles, every exit, sidecars and source/binary hashes are retained in
[the evidence directory](raw/20261108-engine-perf-wc2/).

| Query | Threads | Before min / median s | After min / median s | Paired after/before | Control median [min,max] |
|---|---:|---:|---:|---:|---:|
| q1 | 1 | 0.927223 / 0.963090 | 0.865349 / 0.887777 | 0.916207 | 0.980640 [0.907280,1.132077] |
| q1 | 8 | 0.277209 / 0.309872 | 0.257932 / 0.286196 | 0.924999 | 1.007797 [0.908326,1.205710] |
| high_cardinality_tpch | 1 | 2.499047 / 2.568307 | 2.504930 / 2.623859 | 1.005921 | 1.012446 [0.885482,1.025396] |
| high_cardinality_tpch | 8 | 1.682122 / 1.769951 | 1.709128 / 1.834191 | 0.998686 | 1.024265 [0.921716,1.093155] |

No median regression exceeds the identical-control spread; no local rejection,
acceptance of a speed target, or scaling claim follows. Q1/1t state-update
service wall falls 229.910→170.581 ms in these same invocations.

| Stage (service wall ms) | Q1 / 1t | Q1 / 8t | HC / 1t | HC / 8t |
|---|---:|---:|---:|---:|
| scan_decode | 237.566 → 231.866 | 472.292 → 460.180 | 220.903 → 223.978 | 488.070 → 468.703 |
| filter | 92.036 → 91.021 | 196.037 → 186.507 | 114.649 → 114.570 | 291.851 → 286.805 |
| expression | 175.111 → 174.376 | 310.465 → 312.326 | 298.234 → 296.278 | 436.567 → 437.849 |
| group_id | 46.986 → 46.459 | 92.084 → 87.048 | 833.852 → 884.520 | 1700.503 → 1649.392 |
| state_update | 229.910 → 170.581 | 473.250 → 337.081 | 107.426 → 105.648 | 210.645 → 204.324 |
| merge | 0.009 → 0.009 | 0.043 → 0.046 | 0.049 → 0.050 | 951.133 → 904.330 |
| finalize | 0.043 → 0.041 | 0.054 → 0.044 | 368.167 → 395.077 | 533.279 → 563.867 |
| output_sink | 0.085 → 0.067 | 0.095 → 0.075 | 693.081 → 714.580 | 807.727 → 821.368 |
| spill_write | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| spill_read | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| teardown | 0.219 → 0.190 | 0.265 → 0.183 | 33.916 → 15.748 | 21.341 → 26.103 |
| admission_wait | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 | 0.000 → 0.000 |
| bind_metadata | 2.004 → 2.043 | 2.060 → 1.998 | 1.962 → 1.835 | 1.862 → 1.904 |

Disk remained above 20 GB free; stale-output deletion was not triggered.
[All exact reproduction commands](raw/20261108-engine-perf-wc2/reproduce.md),
[source identities](raw/20261108-engine-perf-wc2/identity.json),
[local comparison chart](raw/20261108-engine-perf-wc2/comparison.html).

### W-C step 2 settled GCP handoff (prepared only)

[`aggregate-wc2.steps`](../aggregate-wc2.steps) follows wc1 exactly: `# gate-exits`,
`tools` installs perf/time, `kept` refuses source-identical archives, topology,
profile pre-check, inert harness self-test, locked release gate build, fresh
fixtures, aggregate/spill/SQL/scan gates, SF1 fixture generation, then settled
five-round alternating primary/twin timing. All native invocations get distinct
persisted package-0 sidecars. Baseline, dictionary-only and cumulative shared
state use separate immutable archives and targets. Cells are Q1/high-cardinality,
1/8 threads, 8 GiB/64 MiB, on each CPU. No local result settles retention or any
frozen target. No cloud command below was run by the implementation agent.

Preparation from the committed branch (the final HEAD contains the checked
shared-state source and evidence):

```sh
WC2_BASE=709674d47891c340df63273d4e8e8e415743f830
WC2_DICTIONARY_SHA=f774be08275353cf7bd23d97542ceda79e98d511
WC2_SHARED_SHA=$(git rev-parse HEAD)
git merge-base --is-ancestor "$WC2_BASE" "$WC2_DICTIONARY_SHA"
git merge-base --is-ancestor "$WC2_DICTIONARY_SHA" "$WC2_SHARED_SHA"
WC2_SHIP=$(mktemp -d /tmp/nata-wc2-ship.XXXXXX)
git archive --format=tar --output "$WC2_SHIP/source.tar" "$WC2_SHARED_SHA"
sha256sum "$WC2_SHIP/source.tar" > "$WC2_SHIP/source.sha256"
mkdir "$WC2_SHIP/tree"
tar -xf "$WC2_SHIP/source.tar" -C "$WC2_SHIP/tree"
python3 scripts/bench-aggregate-perf1-heavy.py prepare --baseline "$WC2_BASE" \
  --fix dictionary="$WC2_DICTIONARY_SHA" --fix shared="$WC2_SHARED_SHA" \
  --bundle "$WC2_SHIP/tree/.aggregate-wc2-inputs"
sha256sum "$WC2_SHIP/tree/.aggregate-wc2-inputs/"*.tar.gz > "$WC2_SHIP/bundle.sha256"
git rev-parse "$WC2_SHARED_SHA" > "$WC2_SHIP/tree/.aggregate-wc2-commit"
cp bench/aggregate-wc2.steps "$WC2_SHIP/tree/heavy.steps"
```

Exact parent-run commands (independent CPU jobs, 80 GB data disk, two VM-hour
maximum each; currency bounds supplied by the owner at run time):

```sh
"$WC2_SHIP/tree/scripts/gcp/orchestrate.sh" plan --job heavy --machines c3d-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc2-x86
"$WC2_SHIP/tree/scripts/gcp/orchestrate.sh" plan --job heavy --machines n4a-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc2-arm
"$WC2_SHIP/tree/scripts/gcp/orchestrate.sh" run --job heavy --machines c3d-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc2-x86 --approved-usd "${WC2_APPROVED_X86_USD:?owner-approved bound required}"
"$WC2_SHIP/tree/scripts/gcp/orchestrate.sh" run --job heavy --machines n4a-highcpu-8 --data-gb 80 --max-vm-hours 2 --run-id wc2-arm --approved-usd "${WC2_APPROVED_ARM_USD:?owner-approved bound required}"
```

The heavy-runner inert self-test, job-chain tests and each prepared step's shell
syntax pass. A source-identical candidate, missing bundle or wrong baseline is
refused by the kept check. Staged leak scans precede both commits. No push,
merge, GCP, target/ADR change, new dependency or memory-charge reduction.
