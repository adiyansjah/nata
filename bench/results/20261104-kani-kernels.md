# Kani proofs of the generic-ops kernels (ADR 0003, item 11)

Run 2026-11-04 on a GCP Spot VM: `n4a-highmem-8` (8 vCPU Neoverse-N3, **64 GB**), Debian 13 arm64 (Kani 0.68.0 / CBMC 6.11.0 need glibc 2.39; Debian 12 has 2.36), us-central1-a, run id `kani-arm64-c`. 1,445 VM-seconds, estimated cost 0.061 USD (catalog prices, the highmem rate derived from core and RAM prices; the bill was not checked). Nothing was left on GCP. The harnesses were written by a delegated agent (cancelled before it wrote a report); the orchestrator read their design and the full output.

## Result

**62 of 62 harnesses VERIFIED (`SUCCESSFUL`), 0 failed, 0 timed out (900 s cap each).** **Correction (2026-11-04, later):** this line first said "every `cover` check satisfied". That was not supported: `run1.sh` kept only the first summary line and counted `Status: UNSATISFIED`, a string Kani does not print for covers (it prints `UNSATISFIABLE`), so its "cover UNSATISFIED: 0" column is always 0. Whether the covers (the non-vacuity checks) were satisfied in this run is therefore NOT recorded; the raw output was not kept. The proofs themselves (`SUCCESSFUL`, 0 failed checks) stand, and the 15/15 mutation result shows they are not vacuous, but the per-harness cover evidence needs a re-run with the fixed counter (`spikes/kani-nata-kernels/run.sh`, planned on GCP; it covers the same families). Raw table: the run's `results/kani-harnesses.txt` (copied below in summary form; one line per harness with verification time and the number of checks, e.g. '0 of 963 failed (7 unreachable)').

| family | harnesses | bound | longest |
|---|---:|---|---:|
| `primitive_overflowing_mul_i64_is_exact` (std's 64-bit `overflowing_mul` equals the exact product with an overflow flag, for ALL i64 pairs) | 1 | none (all inputs) | 28 s |
| `primitive_overflowing_sub_i64_is_exact`, `primitive_overflowing_mul_i16_is_exact` | 2 | none | 3 s |
| `mul_logic_matches_the_spec` (error iff some selected valid row overflows; output values and validity on selected rows; the real loop structure of `mul`, with the multiply replaced by a free pure stand-in so the proof holds whatever the primitive computes) | 5 | rows 0..4, any selection, any validity representation (All / Bytes / Bits) | 18 s |
| `sub_const_left_logic_matches_the_spec` (same, stand-in subtraction) | 5 | rows 0..4 | 12 s |
| `sub_const_left_exact_matches_the_spec` (the REAL `overflowing_sub`, spec in i128) | 5 | rows 0..4 | 15 s |
| `filter_range_i64` / `filter_range_i32` (output = exactly the in-order selected, valid rows with lo <= x <= hi; NULL never passes) | 26 | rows up to 4, selection shapes none and concrete lengths | 6 s |
| `each_visits_the_selection_in_order` | 5 | rows 0..4, any slice | 1 s |
| `sum_matches_the_spec` (exact i128 sum of valid selected values and the count) | 5 | rows 0..4 | 55 s |
| `group_sums_dense_matches_the_spec` | 4 | rows 0..3, 2 groups | 68 s |
| `group_sums_masked_matches_the_spec` (sums, counts, per-column NULL counts) | 4 | rows 0..3, 2 groups | **456 s** |

## What this establishes and what it does not

- **Established, within the bounds:** for the real source of `spikes/generic-ops/src/kernels.rs` (included by path, not copied), the arithmetic kernels raise an error exactly when some selected AND valid row overflows, never for an unselected or NULL row; outputs and validity are right on selected rows; filters, sums and grouped sums equal their specifications for arbitrary values, validity and selections. The 64-bit multiplication primitive is proved exact for all inputs, which closes the trust point the harness author had flagged ('a 64x64 multiplier miter, which CBMC may not finish'; it finished in 28 s with 64 GB).
- **Composition argument (not itself checked by a tool):** the logic proofs treat the arithmetic primitive abstractly and the primitive proofs show std's operation is exact, so together they cover the concrete `mul`. The abstraction is a stand-in function with an independent result and flag per call, which covers every behaviour a correct primitive could have.
- **Bounds:** rows <= 4 (grouped sums <= 3 rows and 2 groups, because the masked proof costs about 8 minutes at 3 rows); column slices at least as long as `rows`; selections sorted, unique and < rows (the engine's contract; `each` is proved for arbitrary slices). The i128 sums cannot overflow below 2^64 rows (n x 2^63 < 2^127): that is an argument, not a proof.
- **Replaced by stubs:** `format!` inside the overflow error constructor (CBMC does not finish on it), so the error MESSAGE text is not proved. `NullBuffer` is a small stand-in for arrow's type.
- **Not covered:** the `div` kernels and the arrow wrappers of `crates/nata-kernels` (that crate holds a COPY of the kernels, so these proofs do not cover it until the harnesses point at it; drift is possible); threading; performance; anything about `arrow`.
- **Mutation check of the proofs: done, see the next section.**

## Reproduce

```
scripts/gcp/orchestrate.sh run --job kani --approved-usd 0.44 --run-id kani-arm64-c    # with IMAGE_FAMILY=debian-13-arm64 MACHINES=n4a-highmem-8 MAX_VM_HOURS=2.5
# or by hand on a machine with Kani 0.68 and 64 GB:
cd spikes/generic-ops/kani && ./run.sh 900
```

## Findings of the run itself

Two failed attempts (108 s and 222 s, 0.005 and 0.009 USD) found: the install step needs the toolchain pinned (the VM has no default), and Kani's prebuilt binaries need glibc 2.39 (so `IMAGE_FAMILY` was added to the orchestrator). Locally, with about 13 GB available, CBMC ran out of memory on the 64-bit multiplication proofs and the symbolic-size harnesses; the high-memory VM did not.

## Mutation check of the proofs (run `kani-mut-arm64-b`, 2026-11-04)

Each of the 15 deliberate breakages of `spikes/generic-ops/mutate.py` is applied to a temp copy of `kernels.rs` and the relevant harnesses are run, cheapest first, until one fails verification (`spikes/generic-ops/kani/mutate_kani.py`). Same machine as above (`n4a-highmem-8`, Debian 13 arm64, 64 GB); 708 VM-seconds, estimated cost 0.030 USD; nothing left on GCP. **15 of 15 mutants are caught by a real harness; 0 survived; 0 invalid.** The unmutated copy verifies.

| mutant | caught by |
|---|---|
| filter ignores NULLs | `filter_range_i64_rows4_none` |
| sum ignores NULLs | `sum_matches_the_spec_rows4` |
| mul error not masked by validity | `mul_logic_matches_the_spec_rows4` |
| sub error not masked by validity | `sub_const_left_logic_matches_the_spec_rows4` |
| mul output validity always true | `mul_logic_matches_the_spec_rows4` |
| mul validity ignores the second operand | `mul_logic_matches_the_spec_rows4` |
| masked group sums do not skip NULL values | `group_sums_masked_matches_the_spec_rows3` |
| masked group sums miscount NULLs | `group_sums_masked_matches_the_spec_rows3` |
| dense group count wrong | `group_sums_dense_matches_the_spec_rows3` |
| masked group count wrong | `group_sums_masked_matches_the_spec_rows3` |
| filter ignores the incoming selection (masked path) | `filter_range_i64_rows4_s0` |
| filter ignores the incoming selection (dense path) | `filter_range_i64_rows4_s0` |
| mul overflow never reported (all-valid path) | `mul_logic_matches_the_spec_rows4` |
| mul overflow never reported (masked path) | `mul_logic_matches_the_spec_rows4` |
| filter lower bound off by one | `filter_range_i64_rows4_none` |

### Correction (2026-11-04): the first mutation run was partly invalid

The first run (`kani-mut-arm64`, 696 s, 0.030 USD) reported 5 mutants (the four filter ones and 'sum ignores NULLs') as 'CAUGHT by COMPILE ERROR'. That was a bug in the script: the filter harness name it used did not exist, Kani returned no result, and the script counted 'no result' as a catch; the 'sum' mutant was also routed to the multiplication harnesses because its name contains 'mul'. Those five rows were not evidence. The script now aborts on an unknown harness name, reports 'no result' as INVALID (never a catch), and routes mutants by name prefix; the table above is from the fixed script. The other 10 results of the first run agree with the second.

Limits unchanged: a mutation check shows these 15 breakages are caught, not that every possible breakage is.
