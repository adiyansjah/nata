# Compaction before arrow wrappers: what the copy costs (verification-plan item 18)

Status: **built and compile-checked only. Nothing has been run** (not the tests, not the benchmark): on the shared dev box only `cargo check --tests --examples` was allowed, and every run goes to GCP by the user's decision. **GCP results: to be filled.** Every number below that is not marked "design" is a placeholder in the sense that none exists yet.

Code: `spikes/compaction/` (throwaway, own Cargo workspace, `arrow = "60"` like the other spikes; delete once ADR 0003 is accepted). Question, from ADR 0003 decision 1 and the Consequences section ("every arrow-rs wrapper that cannot take a selection compacts first; that copy is unmeasured and may be the first thing to replace"): **when an operator needs a stock arrow kernel, what does materialising only the selected rows cost, compared with carrying the selection vector into our own selection-aware kernels, and at what selectivity (or how many functions applied to the compacted column) does the copy pay for itself?**

## Design

One cell = one column (type, number of rows `n`, 10% NULLs) + one selection (shape, selectivity). Every arm is a function of that cell only and returns a checksum.

| id | arm | what it measures |
|---|---|---|
| **A** | engine style | ONE function over the selected rows only, straight on the original column, no copy (our kernels: sorted-index loop with a validity bit per row, or a bitmap loop that ANDs the 64-bit validity word and walks the set bits; a full word of selected rows is a contiguous run) |
| **A3** | engine style, three functions | the same, three separate passes with three different functions |
| **Bt** | compact with `arrow::compute::take`, then ONE function densely | the cost the ADR calls "compaction before an arrow wrapper"; a bitmap selection is first converted to indices (cost counted) |
| **Bf** | compact with `arrow::compute::filter`, then ONE function densely | an index selection is first converted to a boolean mask (cost counted) |
| **Ct / Cf** | compact ONCE (take / filter), then THREE functions densely | amortisation: the more functions run on the compacted column, the less the copy matters |
| **D** | ONE function densely over ALL `n` rows, selection ignored | the control for selectivity: the cost of not selecting at all (and of computing garbage for rows the selection removed); its ns/row-selected is `n*d/k`. It is not a valid engine strategy for fallible functions (errors must be raised only for selected rows, ADR 0003 decision 4); it is a reference line |
| **Et / Ef** | the compaction alone (`take` / `filter`), nothing after | the copy cost isolated, without the function |

Axes (270 cells for the main run, plus a no-NULL control):

- **Selectivity:** 0.1%, 1%, 10%, 50%, 90%, 100% (k = round(n * s), at least 1; so 0.1% of 1,024 rows is one row, which measures the fixed per-call cost).
- **Selection shape:** `random idx` (k distinct rows chosen uniformly, carried as a sorted `u32` index vector); `4 runs idx` (the k rows in at most 4 long runs, carried as sorted `u32` indices); `bitmap` (k random rows, carried as `u64` words). The shape fixes both the set of rows and the native form of the selection, because the native form decides which conversion a compaction method has to pay.
- **Batch size:** 1,024 / 8,192 / 65,536 rows.
- **Column type:** `Int64`; `Decimal64(18,2)` (the spikes' narrow decimal; same 8-byte storage as Int64, kept because it is the type ADR 0003 prescribes); `Decimal128(38,4)` (16 bytes); `Utf8` (4-24 random bytes, mean 14); `Dictionary<Int32, Utf8>` with 256 distinct values (take/filter copy only the 4-byte keys and share the dictionary).
- **NULLs:** 10% in the main run (random positions; NULL slots hold non-zero garbage data on purpose, so a kernel that lets them in changes the result); 0% in the control run (no null buffer at all; 8,192 rows).
- **Functions** (wrapping `u64` per-row results summed over selected valid rows; F0 cheapest, F2 heaviest): numeric - the value / a multiply-shift hash / an xor-shift-multiply-rotate mix; strings - the byte length / the first byte / FNV-1a over all bytes (about 1 ns per byte). Numeric rows are masked branch-free (`row & -valid`); string rows are skipped by a branch (computing a hash for a NULL slot and discarding it would be a strawman). The "one function" arms use F0, the cheapest, so B/A is the pessimistic case for compaction; C/A3 uses all three.

How the timing is done (docs/way-of-working.md section 6):

1. **Check before timing.** Before a cell is timed, every arm is run once and compared with an independent scalar reference (row by row through arrow's own accessors, sharing only the pure per-value functions). A mismatch aborts the run with a non-zero exit.
2. **Interleaved rounds, rotating order.** All 9 arms of a cell are timed in the same process, round after round; the starting arm rotates by one each round so none is always first or last. 25 rounds by default, one discarded warm-up round. Each arm's repetitions per timed sample are calibrated so that a sample lasts at least 100 us.
3. **Statistics.** Per arm: median [lower quartile, upper quartile] of ns per SELECTED row over the rounds. Ratios (B*/A, C*/A3, D/A) are paired per round (the two arms' times from the same round), then median [q1, q3]. B* / C* = the faster of take/filter by median. The raw numbers are in the CSV.
4. **Load.** The 1-minute load average is read at the start, between cells and at the end; at 2 or more the tables are labelled **SANITY ONLY**, as are all numbers when `gcp-check.sh` sees a load of 2 or more at any checkpoint (the benchmark itself is one thread and contributes 1.0).
5. **Break-even.** For each (type, shape, size): the lowest selectivity at which the median paired ratio is <= 1, interpolated linearly in log10(selectivity) between the two grid points around the crossing (an estimate, only six grid points), or "never" with the best ratio seen. `n*` per cell = the number of functions at which compact-then-dense equals selection-aware, from `A`, `A3`, the better B and its C: `n* = 1 + (B - A) / (a - d)`, `a = (A3 - A)/2`, `d = (C - B)/2` (a linear extrapolation of the marginal cost of one more function; `-` when a function costs no less under the selection than densely, i.e. compaction never catches up).
6. **The harness can fail.** `compaction-bench selftest` runs the real arm A next to an identical twin (must come out "same", difference within 3% + 40 ns) and next to copies with a busy-wait of 1, 5 and 20 us per call injected (must come out "slower" by that amount, within 10% + 60 ns, same checksum). It uses the same `measure` and paired statistics as the benchmark. `gcp-check.sh` exits non-zero if it fails, before any benchmark is run.

## Tests (written, not yet run; `spikes/compaction/tests/arms_agree.rs`)

- **Every arm equals the scalar reference** for every type x shape x 8 sizes (1, 2, 63, 64, 65, 200, 1,000, 4,097 rows) x NULL density (0, 3, 10, 50, 100%) x the six selectivities: 3,600 cells.
- **Empty selection** (all types, shapes, NULL densities 0/30/100%): every arm gives 0 rows / checksum 0.
- **Awkward selections**: first row only, last row only, rows 63/64, whole words, one row short of a word, all rows, every other row.
- **NULLs under the selection with garbage in their slots**: asserted (the number of selected NULL rows is in a stated range, and ignoring validity changes the result for every type).
- **Compaction content**: `take` and `filter` results hold exactly the selected rows, in order, with the same values and NULLs, for all types and shapes.
- **Selection shapes**: sorted, distinct, k rows, at most 4 runs, bitmap and index forms and both conversions agree.
- **The checks can fail (mutation):** five deliberate breakages of the selection-aware kernel (validity ignored; bitmap path without the validity word; index path drops the last index; bitmap path drops the last word; dense path with validity mishandles the partial last word) must each make the agreement check fail, for every type (several seeds, because a breakage can hide when the affected row is a NULL). Checksums are also asserted to depend on the data.
- **Harness:** the injected-cost self-test (as above), the interleaving order (rotates by one per round), the paired-ratio verdict, the break-even arithmetic (interpolation, "at or below", "never"), and a tiny grid end to end (report tables, CSV row count, the SANITY ONLY label).

## What compiled here

`scripts/local-heavy.sh bash -c 'cd spikes/compaction && cargo check --tests --examples'` finished with no errors and no warnings (dev profile, arrow 60.1.0, rustc 1.99.1 from `rust-toolchain.toml`). No test, benchmark or release build was run. A release build with LTO was never attempted here; compile errors in release-only code paths (none expected: there are no `cfg`s) and every behaviour below are therefore unverified.

## Exact GCP run

Run on a clean Debian VM, once on **ARM64** (`n4a-highcpu-8`, Spot, us-central1) and once on **x86-64** (`c3d-highcpu-8`, Spot, us-central1), from a checkout or tarball of the repository (the toolchain is pinned by `rust-toolchain.toml` at the repo root; the script installs rustup with no default toolchain if missing, and needs `build-essential`):

```bash
sudo apt-get update && sudo apt-get install -y build-essential curl ca-certificates   # if the image lacks a C compiler
cd <repo-root>
spikes/compaction/gcp-check.sh ~/compaction-results 25      # arg 1: results directory, arg 2: rounds (default 25)
echo "exit status: $?"
```

Exit status is non-zero if the build, a unit test or the harness self-test fails (no benchmark is started then). Copy `~/compaction-results/` back (`machine.txt`, `summary.txt`, `selftest.md`, `compaction.md`, `compaction.csv`, `compaction-nonull.md`, `compaction-nonull.csv`, `tests.txt`, `build.txt`) and record machine type, region and date next to the numbers (way-of-working section 8).

**Expected duration (a reasoned estimate, nothing was timed):** build about 3-6 min on 8 vCPUs (arrow in release, then the test binary), unit tests 1-3 min (single test thread), self-test under 10 s, the two benchmark passes 1-5 min together; **about 10-15 min of VM time per architecture**, plus boot. The benchmark is cheap, so if the first result is noisy, `gcp-check.sh <dir> 101` repeats it with 101 rounds at little cost. **Cost estimate:** Spot rates in `scripts/gcp/prices.env` (fetched 2026-11-03/04): `n4a-highcpu-8` $0.0984/h, `c3d-highcpu-8` $0.0687/h; at one hour each that is about $0.17 for both before disk and external IP (cents); to be recomputed and confirmed with the human before the run. Delete the VMs when the results are copied. Run nothing else on the VM during the timed phase (the script waits up to 5 minutes for its own build's load to decay and warns when the load average is 2 or more).

Local re-run for a quick look at one cell, on a quiet machine only: `target/release/compaction-bench bench --types int64 --shapes random --sizes 8192 --sels 10,50 --rounds 9`.

## How to read the output

- `compaction.md`, section **Break-even selectivity**: one row per (type, selection shape, batch size) and three columns. `B/A <= 1 at`: the selectivity from which compact-with-take-or-filter-then-one-function beats one selection-aware pass; `C/A3 <= 1 at`: the same with three functions on the compacted column against three selection-aware passes (the amortisation case); `D/A <= 1 at`: from where a dense pass over all rows is cheaper than the selection-aware pass (where carrying the selection stops paying, for infallible functions only). "never (best 1.8x at 100%)" means compaction was slower at every grid point and how close it got.
- Detail tables (one per type and size): rows = shape x selectivity; columns = the nine arms in ns per selected row, `median [q1,q3]`; then `B*/A`, `C*/A3`, `D/A` as paired `median [q1,q3]` (a ratio above 1 with the quartile range above 1 = compaction slower in at least 75% of rounds); `n*` as defined above.
- `Et`/`Ef` give the copy cost alone; `B - E` is roughly the dense function on the compacted column.
- A value for 0.1% of 1,024 rows (one row) is the fixed cost of a call (allocation, dispatch, arrow's `take` setup) divided by 1; read it as an overhead floor, not as a per-row cost.
- `compaction-nonull.md` (no null buffer, 8,192 rows) is the control for how much the NULL handling (validity bits in `A`, null-buffer construction in `take`/`filter`) contributes.
- `selftest.md` must say `SELF-TEST PASSED`; `summary.txt` says whether the load stayed below 2.

## Limits to keep next to the numbers (known before any run)

- **One column is compacted, not a whole batch.** A batch with c columns that all need compaction pays about c times the copy; only the kernel's input columns need copying, and that is what is measured.
- **Warm, in-cache batches, allocation on every call, no buffer pooling.** 65,536 rows of Decimal128 or Utf8 is 1-2 MB, which may exceed a core's L2; cold-cache and pooled-allocator behaviour are not measured. The output of every compaction is freed at the end of the call (the allocator's steady state is what is timed).
- **The selection-aware kernels are mine, simple, scalar, and not tuned beyond the null-path lessons** (word-wise validity, run of 64 for a full word). A faster engine kernel moves the break-even towards compaction never paying; a slower one the opposite. Dense passes are plain loops that LLVM may or may not vectorise (not inspected).
- **`filter` at 100% selectivity with a bitmap is zero-copy** (arrow returns a slice when the mask is all true), so Bf/Cf at 100% for the bitmap shape is not a copy; the engine would not compact an unselected batch anyway. For index shapes the 100% selection is the identity vector and `take` copies everything.
- **Arrow's `filter` and `take` implementations are used as shipped (arrow 60.1.0), with their own strategy choice (slices, indices, all, none).** No hand-written compaction was tried; that would be the next step if compaction turns out to dominate.
- **The functions are synthetic.** Real stock arrow kernels (string functions, casts, sort) have their own per-row costs (a `cast` or a `LIKE` may cost 10-100 ns per row, far above F0); `n*` and the three-function arms show where that moves the break-even, but no real arrow kernel is timed here.
- **No joins, exchanges or downstream effects** (e.g. a compacted batch may be faster for later operators or take less memory); only the cost of the compaction step and one or three functions after it.
- **Break-even values are interpolated between six selectivities**, with one data set per cell (a fixed seed), not several random draws; the noise shown is round-to-round, not data-to-data.
- ARM64 and x86-64 may differ (ADR 0003: arrow's kernels have no architecture-specific code and rely on autovectorisation). Both are planned; either one alone must be labelled with its architecture.

## GCP results: to be filled

(machine types, regions, dates, load, the break-even tables per architecture, what limited the numbers, the conclusion for ADR 0003 - when to compact - and any correction.)
