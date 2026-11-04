# Kani proofs of the real `crates/nata-kernels` (division, guards, merge): harnesses and local results

Date 2026-11-04. Written by a delegated test agent. Everything here ran on the shared dev machine (x86-64, 16 GB, load 8-17 from other agents) unless it says 'to be filled by the GCP run'. **The full set has NOT been run: the heavy harnesses go to the 64 GB GCP VM (section 'For the GCP run').** The kernel crate was not edited (`git diff -- crates/nata-kernels/src` is empty).

Code: `spikes/kani-nata-kernels/` (own cargo project with a `[workspace]` table; `src/lib.rs` has the harnesses, `arrow-stub/` the one stubbed type, `run.sh` / `run1.sh` the runners, `mutate_kani.py` the mutation check). It includes `crates/nata-kernels/src/kernels.rs` by `#[path]` (never a copy, so no drift). `arrow_path.rs` and `strict_cast.rs` (the arrow wrappers) are NOT included: they call real arrow code and are not covered by these proofs.

## Result in one paragraph

No kernel bug was found: every harness that finished verified. Locally (cheap set, rows <= 2): 41 harnesses `SUCCESSFUL`, 0 failing checks, peak memory 474 MB; the mutation check caught 14 of 14 mutants whose relevant harnesses are cheap (the other 4 kernel mutants and the 4 arrow-wrapper mutants were not run: see below). Not yet proved: rows 3-4 of everything, the 64-bit division primitive, the exact (no-stand-in) division proofs, `split_ne` / `filter_ne` / the CASE pipeline above 0 rows (cost: see 'Local results'), grouped sums at the full bound, and 4 of the 18 kernel mutants (the `split_ne` ones) were not run. All of that is in the GCP list.

## Harnesses (105) and bounds

One harness per concrete row count (`_rowsN`), because a symbolic `rows` makes the `Vec` allocations in the kernels run CBMC out of memory (same design as `bench/results/20261104-kani-kernels.md`). All values, validity representations (`All` / `Bytes` / `Bits`) and selections are symbolic. Column slices have at least `rows` entries; selections for arithmetic, `filter_range`, sums and grouped sums are sorted, unique and < rows (the engine's contract). `split_ne`, `filter_ne` and `merge_branches` are proved for ANY sequence of indexes < rows (unsorted, duplicates, length up to 4), which is stronger.

| family | harnesses | rows | what it proves |
|---|---:|---|---|
| `each_visits_the_selection_in_order` | 5 | 0..4 | `each` visits the slice in order, or `0..rows` |
| `mul_logic_matches_the_spec` | 5 | 0..4 | `mul` (logic, stand-in primitive) |
| `sub_const_left_logic_matches_the_spec`, `sub_const_left_exact_matches_the_spec` | 10 | 0..4 | `sub_const_left` (stand-in, and the real `overflowing_sub`) |
| `div_logic_matches_the_spec`, `div_const_left_logic_matches_the_spec` | 10 | 0..4 | `div`, `div_const_left` (logic, stand-in for `checked_div`) |
| `div_exact_matches_the_spec`, `div_const_left_exact_matches_the_spec` | 6 | 0..2 | the same with the REAL 64-bit `checked_div` (no abstraction) |
| `split_ne_partitions_the_selection`, `filter_ne_matches_the_spec` | 10 | 0..4 | the guard split and filter |
| `merge_branches_matches_the_spec` | 5 | 0..4 | two parts, overlaps allowed |
| `case_guarded_division_matches_the_spec` | 5 | 0..4 | `CASE WHEN x <> 0 THEN a / x ELSE 0 END` from the real `split_ne`, `div`, `constant`, `merge_branches` |
| `sum_matches_the_spec` | 5 | 0..4 | `sum` |
| `group_sums_dense_matches_the_spec`, `group_sums_masked_matches_the_spec` | 8 | 0..3, 2 groups | grouped sums, NULL counts |
| `filter_range_i64_*`, `filter_range_i32_*` | 26 | 0..4, selection none or concrete length | `filter_range` (concrete selection length: it allocates `vec![0; sel.len()]`) |
| `primitive_overflowing_{sub_i64,mul_i64,mul_i16}_is_exact` | 3 | all inputs | as in the earlier run (the 64-bit multiplication one is GCP-only here) |
| `primitive_checked_div_{i8,i16,i32,i64}_is_exact` | 4 | all inputs | `checked_div` is None exactly for zero divisor and MIN / -1, else the exact truncating quotient |
| `characterisation_of_truncating_division_{i8,i16}` | 2 | all inputs | the division-free spec used at 64 bits equals `/` |
| `div_zero_message_text` | 1 | 1 row | the error text is exactly `division by zero` for `div` and `div_const_left` |

## Specifications (written without calling the function under test)

- **`div` / `div_const_left`** (ADR 0003 decision 4). With `live(i) = selected(i) AND valid(i)` (validity of both operands for `div`): an error is raised exactly when some live row has a zero divisor or is `MIN / -1`. Its kind: `division by zero` (checked by length 16; the exact text in `div_zero_message_text`) if some live row has a zero divisor, otherwise the overflow error (the overflow error is a stub, `OVF`). Never for an unselected row or a NULL slot. On success: `rows` values; each selected row's validity is `live`-validity, and a valid row's value is the quotient; unselected rows are untouched (0, not valid). In the exact variants a selected NULL slot whose division would fail holds 0 (the comment on `div_step`). The quotient is specified in the logic harnesses as the stand-in's value, and in the exact harnesses by the characterisation `a = q*b + r, |r| < |b|, r = 0 or sign(r) = sign(a)` in i128 (unique for b != 0, proved equal to `/` at 8 and 16 bits).
- **`split_ne`**: walking the input sequence in order, with `p(i) = valid(i) AND col[i] <> value`, each element equals the next unread element of `yes` if `p(i)`, of `no` otherwise, and both outputs are exactly consumed (so: no row lost, none duplicated, order kept, NULL comparisons go to `no`). Also `yes.len() + no.len()` = input length; every `yes` row satisfies `p` (a guard row never reaches the guarded branch); every `no` row fails `p`; the branches therefore share no row.
- **`filter_ne`**: the in-order rows of the input sequence where `p(i)`.
- **`merge_branches`** (two parts, arbitrary index sequences and branch outputs): output position `i` holds the value and validity byte of the LAST part whose sequence contains `i` (validity 1 if that part has no validity vector); a row in no part is NULL; output has `rows` entries and always a validity vector. This is 'reassembles in order' for the disjoint split, and 'later part wins' for overlaps (documented in the source).
- **CASE pipeline**: an error iff some selected row with `x` valid, `x <> 0`, `a` valid, `a = MIN`, `x = -1`, and it is NEVER `division by zero`; otherwise unselected rows are NULL, rows with `x` NULL or 0 are valid 0, others are NULL if `a` is NULL and the quotient (stand-in) otherwise.
- `mul`, `sub_const_left`, `filter_range`, `sum`, grouped sums: as in `bench/results/20261104-kani-kernels.md`.

## Division at 64 bits: the two-part argument and the trust points

CBMC cannot cheaply check the real 64-bit `checked_div` inside every loop proof, so as for multiplication:

1. **Primitive.** `primitive_checked_div_i64_is_exact` (all 64-bit pairs; None exactly for zero / `MIN/-1`; otherwise the characterisation holds) is GCP-only. Locally done: 8-bit (0.4 s) and the characterisation at 8 bits. The 16-bit primitive proof (spec by a 32-bit division) timed out at 300 s on the loaded machine; it, the 32-bit one, and the 16-bit characterisation are in the GCP list.
2. **Logic.** The `*_logic_*` harnesses stub `i64::checked_div` by `fake_checked_div`, an arbitrary function that is deliberately NOT tied to 'None iff zero or MIN/-1' (so the kernel's error flags, computed by plain comparisons `b == 0` and `a == MIN && b == -1`, are shown not to depend on the primitive). Their value spec is the stand-in's value.
3. **No abstraction at 1-2 rows:** `div_exact_*` and `div_const_left_exact_*` use the real `checked_div` with the characterisation as spec (GCP-only).

**Trust points, honestly:** (a) until `primitive_checked_div_i64_is_exact` finishes on GCP, 'std's 64-bit `checked_div` is exact' rests on the argument that std's `checked_div` is one macro for all widths and is proved at 8 bits (reasoned, not proved at 64); the composition of (1) and (2) is itself an argument, not a tool check; (b) the overflow error is a stub, so the text `overflow in ...` and `format!` are not proved; the zero-divisor text is; (c) `NullBuffer` is a small stand-in for arrow's type; (d) the error kind is checked by message length in the large harnesses (16 vs 3 bytes), the exact text only in `div_zero_message_text`; (e) the arrow wrappers are not covered; (f) the bounds below.

## Local results (cheap set: `scripts/local-heavy.sh spikes/kani-nata-kernels/run.sh 300 --local`)

All 41 `SUCCESSFUL`, 0 failing checks, cap 300 s (from `/tmp` logs of one run; the run's output is not kept in the repo). Verification time and peak RSS of the largest process (`/usr/bin/time`). 'covers' is Kani's `N of M cover properties satisfied`; the unreachable ones are covers I gated by the minimum row count at which they can hold (for example 'a zero divisor and an overflow in the same batch' needs 2 rows), so a harness at the largest local row count shows all of its covers satisfied except where noted.

| harness | time | peak RSS | covers |
|---|---:|---:|---|
| `primitive_overflowing_sub_i64_is_exact` | 0.1 s | 152 MB | - |
| `primitive_overflowing_mul_i16_is_exact` | 5.3 s | 153 MB | - |
| `primitive_checked_div_i8_is_exact` | 0.4 s | 154 MB | 2 of 2 |
| `characterisation_of_truncating_division_i8` | 6.0 s | 179 MB | 1 of 1 |
| `div_zero_message_text` | 1.5 s | 217 MB | - |
| `each_visits_the_selection_in_order_rows0/1/2` | 0.8 / 0.9 / 0.9 s | 274 MB | 1 of 1 |
| `mul_logic_matches_the_spec_rows0/1/2` | 1.6 / 10.3 / 13.4 s | 326 / 403 / 452 MB | rows2: 3 of 3 |
| `sub_const_left_logic_matches_the_spec_rows0/1/2` | 1.3 / 7.6 / 10.6 s | 328 / 350 / 400 MB | rows2: 3 of 3 |
| `sub_const_left_exact_matches_the_spec_rows0/1/2` | 1.3 / 10.9 / 16.8 s | 326 / 346 / 358 MB | rows2: 3 of 3 |
| `div_logic_matches_the_spec_rows0/1/2` | 1.8 / 11.6 / 14.2 s | 330 / 382 / 474 MB | rows2: 5 of 5 |
| `div_const_left_logic_matches_the_spec_rows0/1/2` | 1.5 / 8.6 / 10.5 s | 328 / 344 / 431 MB | rows2: 5 of 5 |
| `split_ne_partitions_the_selection_rows0` | 2.8 s | 314 MB | rows 0: none reachable (see below) |
| `filter_ne_matches_the_spec_rows0` | 2.1 s | 307 MB | rows 0: none reachable |
| `merge_branches_matches_the_spec_rows0/1/2` | 2.0 / 10.9 / 17.4 s | 332 / 332 / 356 MB | rows2: 1 of 2 (the other needs 3 rows) |
| `case_guarded_division_matches_the_spec_rows0` | 3.6 s | 341 MB | rows 0: none reachable |
| `sum_matches_the_spec_rows0/1/2` | 0.7 / 4.6 / 12.3 s | 282 / 281 / 292 MB | rows2: 1 of 1 |
| `filter_range_i64_rows{0,1,2}_{none,s0..}` (9) | 0.6-4.5 s | 316 MB | with a non-empty selection: 2 of 2 |

Also run, by a mistake in my own filter (grouped sums were still in the `--local` set until afterwards; they stayed within 1.1 GB and 200 s, but they count as heavy under the rule and are now excluded from `--local`): `group_sums_dense_matches_the_spec_rows0/1/2` SUCCESSFUL in 1.7 / 11.7 / 27.6 s; `group_sums_masked_matches_the_spec_rows0/1/2` SUCCESSFUL in 6.1 / 59.8 / 196.7 s (peak 333 / 969 / 1062 MB). Their cover counts were not recorded (the counter then in `run1.sh` was wrong, see Findings). `filter_ne_matches_the_spec_rows1` and `rows2` SUCCESSFUL in an earlier batch, before the rule, 245 s each at load 10-17 (quiet machine: rows2 in 56 s, **3.2 GB**), so they exceed the 2 GB rule and moved to the GCP list. **Timed out at 300 s (load 10-17, so not a verdict): `split_ne_partitions_the_selection_rows1` and `rows2`, `case_guarded_division_matches_the_spec_rows1` and `rows2`, `primitive_checked_div_i16_is_exact`.** These are GCP-only.

**What the local run does not show (non-vacuity gap):** for `split_ne`, `filter_ne`, the CASE pipeline and the 3-row cover of `merge_branches`, the `cover!` checks are only satisfiable at rows >= 2 (or 3), which is above what runs locally. Their non-vacuity is therefore NOT shown locally; the GCP run must show `covers satisfied: N of N` at rows 2-4 for them. (`merge_branches` is shown non-vacuous by its two caught mutants.)

## Mutation check of the proofs (`mutate_kani.py`)

The script parses the 22 mutations from `crates/nata-kernels/scripts/mutate_guards.py` (no copy), applies each to a temp copy of `kernels.rs` (the real file is never written), and runs the relevant harnesses cheapest first. Verdicts: `CAUGHT` (a harness returns a failed verification with at least one failing check; the unmutated copy is verified first), `SURVIVED`, `INVALID` (no Kani result, compile error, unknown harness, timeout, pattern not found), `SKIPPED` (the 4 mutants of `arrow_path.rs`, which these proofs do not include). It checks that every harness name it can use exists in `src/lib.rs` before running, and prints a `SUMMARY` line.

Local, through the lock: `mutate_kani.py --rows 2 --cap 300 "div:" "div_const_left" "merge_branches"` (cheap harnesses only): **`SUMMARY: caught 14 of 14 in scope; survived 0; invalid 0`**. (The 1-row version of the same run took 7 min 46 s wall, including waiting for other agents' heavy commands.) All 14 caught by the `*_logic_*` or `merge_branches` harness at rows 2:
the 7 of `div:` that change flags, selection, validity, zero check, MIN/-1 check; `div: rounds toward minus infinity` (see the caveat); `div: overflow reported ahead of zero`; 3 of `div_const_left:`; 2 of `merge_branches:`.

- **At 1 row the same run reported 13 of 14: `div: overflow reported ahead of zero` SURVIVED.** That is a bound effect, not a hole in the spec: the preference between the two error kinds only shows when one batch has a zero divisor in one row and `MIN / -1` in another, which needs 2 rows (`cover!(want_zero && want_ovf)` is gated to rows >= 2 for that reason). At 2 rows it is caught.
- **Caveat on `div: rounds toward minus infinity`:** the mutation replaces the primitive call, and the logic harness catches it only because the stand-in is no longer called (weak evidence). The strong catch is by `div_exact_*` (real `checked_div` vs `checked_div_euclid`); `--heavy` puts that harness first for this mutant. Not run locally.
- **Self-tests that a non-catch is never a catch:** `mutate_kani.py --rows 2 --inject-bad-harness no_such_harness_rows2 "div: zero check dropped"` printed `INVALID | ... no usable result for no_such_harness_rows2 (NONE ...); NOT a catch` and `SUMMARY: caught 0 of 1 in scope; survived 0; invalid 1`. `--rows 9` (names that do not exist) aborted before running: `harness names not found in src/lib.rs (wrong --rows?)`, exit 1. A mutant of `arrow_path.rs` printed `SKIPPED`. The `--heavy` path and `run1.sh`'s 'cover gap' flag were not exercised locally.
- **Not run locally:** the 4 `guard split_ne` mutants (their relevant harnesses are above the local limits) and, with `--heavy`, the exact-division harnesses.

## Findings of this work

- **No bug in the kernels** was found by any harness that finished.
- **Tooling bug in my first runner, fixed:** `VERIFICATION:- FAILED` is also printed when CBMC is killed (`CBMC failed with status 15` once on this shared machine), so a naive script counts a killed proof as a catch (the same class of error as the first mutation run of the earlier report). `run1.sh` now reports FAILED only with at least one `Status: FAILURE` check; otherwise `NO RESULT (...)`.
- **Cover accounting:** Kani 0.68 prints covers as `SATISFIED` / `UNSATISFIABLE` / `UNREACHABLE`, never `UNSATISFIED`. My first counter grepped `UNSATISFIED`, so it always said 0 and proved nothing; the reliable signal is the line `N of M cover properties satisfied`. `run1.sh` now prints it and flags `SUCCESSFUL but COVER GAP` if any cover is `UNSATISFIABLE`. The older runner `spikes/generic-ops/kani/run1.sh` has the same `UNSATISFIED` grep; I did not read the raw output of that earlier run, so whether 'every `cover` check satisfied' in `20261104-kani-kernels.md` rests on that counter is unchecked (worth re-checking the raw `results/` of that run).
- **Cost:** `Vec::push` with a data-dependent count (`split_ne`) is much more expensive for CBMC than the preallocated writes in the arithmetic kernels; unwinding a symbolic-length slice loop to a bound of 7-8 made it worse. Every loop in these harnesses runs at most 4 times, so `unwind(5)` is used (Kani reports a failing unwinding assertion if that were too small).

## For the GCP run (to be run by the orchestrator on a 64 GB high-memory VM; Debian 13, Kani 0.68)

Setup as in `20261104-kani-kernels.md` (Kani 0.68.0 needs glibc 2.39, so Debian 13; the toolchain pinned; `python3`; optionally the `time` package for the memory column). Nothing here needs network beyond the Kani install.

```
cd spikes/kani-nata-kernels
./run.sh 900 2>&1 | tee kani-harnesses.txt                     # all 105 harnesses, one at a time, 900 s cap each
./run.sh 3600 '<names that timed out>' 2>&1 | tee kani-heavy.txt  # re-run only the timeouts with a longer cap
./mutate_kani.py --rows 4 --cap 900 --heavy 2>&1 | tee kani-mutants.txt   # 18 kernel mutants (+4 skipped); needs the rows-4 and exact harnesses to exist (they do)
```

Expected duration and memory are guesses (reasoned from the local numbers and the earlier GCP run, not measured here); each is to be replaced by the GCP run.

| harnesses | why not local | expected (guess) | measured |
|---|---|---|---|
| `primitive_checked_div_i64_is_exact` | 64-bit divider miter plus a 128-bit multiply in the spec; the 64-bit multiplication miter ran out of memory at about 13 GB locally | minutes to tens of minutes, several GB to 20+ GB; may need the 3600 s cap | to be filled by the GCP run |
| `primitive_checked_div_i16_is_exact`, `_i32_`, `characterisation_of_truncating_division_i16`, `primitive_overflowing_mul_i64_is_exact` | timed out at 300 s locally (i16); mul64 took 28 s with 64 GB before | seconds to minutes | to be filled by the GCP run |
| `div_exact_matches_the_spec_rows0..2`, `div_const_left_exact_matches_the_spec_rows0..2` | the real 64-bit divide and a 64x64 multiply per row | minutes each at rows 1-2, GBs | to be filled by the GCP run |
| `split_ne_partitions_the_selection_rows1..4`, `filter_ne_matches_the_spec_rows1..4`, `case_guarded_division_matches_the_spec_rows1..4` | over 300 s locally (rows >= 1) or 3.2 GB (`filter_ne` rows 2); the likeliest to need the longer cap | tens of minutes at rows 3-4 | to be filled by the GCP run; must show `covers satisfied: N of N` |
| `div_logic_*`, `div_const_left_logic_*`, `merge_branches_*`, `mul_logic_*`, `sub_const_left_*`, `sum_*`, `each_*` at rows 3, 4 | rows 3-4 are above the local limit | seconds to minutes each (rows 2 take 10-17 s locally; the earlier GCP run took 12-55 s at rows 4) | to be filled by the GCP run |
| `filter_range_i64_rows3..4_*`, `filter_range_i32_rows4_*` | rows 3-4 | seconds each (earlier GCP run: 6 s) | to be filled by the GCP run |
| `group_sums_dense_*` rows 0..3, `group_sums_masked_*` rows 0..3 | grouped sums are heavy | earlier GCP run: dense rows3 68 s, masked rows3 456 s | to be filled by the GCP run |
| the full mutation run, 18 kernel mutants | each mutant runs up to a few harnesses at rows 4 | to be filled by the GCP run | to be filled by the GCP run |

Expected total for the full `run.sh`: to be filled by the GCP run (the earlier set of 62 harnesses took 1,445 VM-seconds on `n4a-highmem-8`; this set has 105 harnesses including the unmeasured ones above, so plan for a multiple of that). What to check in its output: every line `SUCCESSFUL |` (no `COVER GAP`, no `NO RESULT`, no `TIMEOUT`); `covers satisfied` complete at the largest row count of each family; in the mutation output `SUMMARY: caught 18 of 18 in scope; survived 0; invalid 0; skipped (out of scope) 4`; the `rounds toward minus infinity` mutant caught by `div_exact_*` and not only by the stand-in.

## Limits

Bounds: rows <= 4 (grouped sums <= 3 rows, 2 groups, `div_exact` <= 2 rows); column slices at least `rows` long; selections for arithmetic, sums and `filter_range` sorted, unique and < rows; `split_ne` / `filter_ne` / `merge_branches` index sequences of length <= 4 with entries < rows (an index >= rows is an out-of-bounds panic and is excluded by assumption, as the engine's contract); two parts in `merge_branches`; `i128` sums cannot overflow below 2^64 rows (argument, not proof). Not proved: the 64-bit division primitive (until the GCP run), the text of the overflow error, anything about the arrow wrappers (`arrow_path.rs`, `strict_cast.rs`), threading, performance. The 22 mutants are a sample of possible breakages, not all of them; mutants of `arrow_path.rs` are out of scope of these proofs. The local timings were taken under load 8-17 and vary by 2-4x with the load.
