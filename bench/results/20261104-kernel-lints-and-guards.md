# Kernel strictness lints on a real crate, and guarded expressions (ADR 0003, decisions 4, 6 and 7)

Code: `crates/nata-kernels/` (new workspace crate). Toolchain: Rust and clippy 1.99.1, arrow-rs 60.1.0 (sub-crates `arrow-arith`, `arrow-array`, `arrow-buffer`, `arrow-cast`, `arrow-schema`, `arrow-select`), DuckDB 2.0.0. x86-64 only; the lints and the checked arithmetic do not depend on the architecture, but the timing table in section 3 does. The VM was shared and loaded (1-minute load 5.3 to 5.5 during the timing runs), so timings are indicative.

## Reproduce

```
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
python3 -I crates/nata-kernels/lint-fixtures/run.py          # 73 ban/lint fixtures, 10 controls, 5 gaps, sanity, real crate
python3 -I crates/nata-kernels/lint-fixtures/overflow_checks.py   # overflow-checks on/off, and where the setting must live
cargo test --release -p nata-kernels --test overflow_checks       # passes
cargo test --release -p nata-kernels --test overflow_checks --config 'profile.release.overflow-checks=false'   # must FAIL
python3 -I crates/nata-kernels/scripts/noise.py                # lint noise before and after
python3 -I crates/nata-kernels/scripts/guard_diff.py           # DuckDB differential + DuckDB probes
python3 -I crates/nata-kernels/scripts/mutate_guards.py        # 22 mutations of the new kernels
cargo run --release -p nata-kernels --example overflow_cost    # timing, repeat with --config profile.release.overflow-checks=false and a separate CARGO_TARGET_DIR
```

The scripts restore every file they touch. `Cargo.lock` at the root gained the arrow crates (unavoidable).

## 1. The strictness lints on a real crate (item 12)

### What is configured

- `crates/nata-kernels/clippy.toml`: `disallowed-methods` with the six functions of the ADR plus 46 more (section 1.4). It sits in the crate, not at the repository root, because the brief limited edits to the crate and the root `Cargo.toml`. A root-level `clippy.toml` was tried (with the first six entries) and `cargo clippy --workspace --all-targets -- -D warnings` stayed green for the other three crates: clippy does not warn about a path whose crate a given package does not depend on. Moving it to the root is a one-line decision for the orchestrator, and makes it apply to future kernel crates by default.
- `crates/nata-kernels/Cargo.toml` `[lints]`: the workspace lints repeated (Cargo cannot combine `workspace = true` with extra entries) plus `disallowed_methods`, `arithmetic_side_effects`, `cast_possible_truncation`, `cast_possible_wrap` and `allow_attributes_without_reason`, all `deny`. The last one makes every `#[allow]` carry a `reason`.
- Root `Cargo.toml`: `[profile.release] overflow-checks = true`, **workspace-wide, not per package**. See section 1.3 for why.
- `src/strict_cast.rs`: the one wrapper allowed to call `cast_with_options`, with `safe: false` and an explicit `#[allow(clippy::disallowed_methods, reason = ...)]`.

### 1.1 Fixtures: the lints can fail

`lint-fixtures/run.py` copies each fixture into a scratch crate that has the same `[lints]` table and the same `clippy.toml` (both copied from `crates/nata-kernels` at run time), runs `cargo clippy`, and requires clippy to fail **with the named lint code and message text**. A compile error or another lint does not count.

| group | fixtures | outcome |
|---|---|---|
| `ban/` the six ADR functions (`add_wrapping`, `sub_wrapping`, `mul_wrapping`, `sum`, `cast`, `cast_with_options`) | 6 | all fail with `clippy::disallowed_methods` |
| `ban/` `cast_with_options` with a literal `safe: true` | 1 | fails (the function is banned; the field value is invisible to a lint) |
| `ban/` through the `arrow` re-exports (`arrow::compute::cast`, `cast_with_options`, `kernels::numeric::add_wrapping`, `sum`) | 4 | all fail |
| `ban/` extra arrow entry points (section 1.4) | 26 | all fail |
| `ban/` std `wrapping_*` and `saturating_*` on `i32`, `i64`, `i128` | 21 | all fail |
| `lints/` `arithmetic_side_effects`: `+ - * / %`, unary `-`, `+=`, and `x + 1` inside a closure given to arrow's `unary` | 8 | all fail |
| `lints/` `cast_possible_truncation`: `i64 as i32`, `usize as u32`, `f64 as i64`, `i128 as i64` | 4 | all fail |
| `lints/` `cast_possible_wrap`: `u64 as i64`, `u32 as i32` | 2 | all fail |
| `lints/` `allow_attributes_without_reason` | 1 | fails |
| `controls/` checked arrow functions (`add`, `sub`, `mul`, `div`, `sum_checked`, `product_checked`, `*_checked` methods), `checked_*`/`overflowing_*`, lossless and `try_from` casts, the wrapper pattern with `#[allow(.., reason)]`, `try_unary` | 10 | all clean (clippy exit 0, no warnings) |
| sanity: `ban/add_wrapping` with **no** `clippy.toml` | 1 | clean, so the ban comes from the file |
| real crate: `cargo clippy -p nata-kernels --all-targets -- -D warnings` | 1 | clean |

Result: **73 of 73 fixtures fail with the expected lint; 10 of 10 controls are clean.**

### 1.2 Known gaps (fixtures in `gaps/`, which must stay clean; if one starts failing, the lint improved)

- `<<` and `>>` are not flagged by `arithmetic_side_effects`. `overflow-checks` panics on a shift amount at or above the bit width (`1i32 << 40`), but **bits shifted out are silent in every mode** (`1i64 << 63` gives `i64::MIN` with checks on). A shift kernel must check for lost bits by hand.
- `abs()` and `pow()` are method calls the arithmetic lint does not see. Only `overflow-checks = true` makes `i32::MIN.abs()` and `i32::MAX.pow(2)` panic (section 1.3).
- `i64 as f64` loses precision above 2^53 and is not covered by the enabled cast lints (`cast_precision_loss` would, and was left off because SQL DOUBLE conversion is lossy by definition).
- `u64` and `usize` `wrapping_*` are not banned on purpose: hashing needs them. Review has to keep them out of value arithmetic.
- A literal `safe: true` is invisible to any lint; the protection is that the only function that takes `CastOptions` is banned.
- A well-formed `#[allow(.., reason = "..")]` can still be wrong. Every allow is an unproved claim unless a test or proof backs the stated range (the Kani work covers some).

### 1.3 `overflow-checks`

`lint-fixtures/overflow_checks.py` builds scratch crates in release, offline:

| operation | `overflow-checks = true` | `= false` |
|---|---|---|
| `i32::MAX + 1`, `i32::MIN - 1`, `i32::MAX * 2`, `i64::MAX + 1` | panic ("attempt to add/subtract/multiply with overflow") | silent wrap (-2147483648, 2147483647, -2, i64::MIN) |
| `-i32::MIN`, `i32::MIN.abs()` | panic | silent -2147483648 |
| `i32::MAX.pow(2)` | panic ("exponentiate") | silent 1 |
| `1i32 << 40` | panic ("shift left") | silent 256 |
| `1i64 << 63` (bits lost) | **no panic**, i64::MIN | same |
| `i64::MIN / -1`, `1 / 0` | panic | panic (always, in every profile) |
| `i64 as i32` truncation | silent (0) | silent |

In the real crate, `tests/overflow_checks.rs` (an unchecked `i32::MAX + 1` under `catch_unwind`) passes in `cargo test --release` and **fails** with `--config 'profile.release.overflow-checks=false'` ("it wrapped to Some(-2147483648)").

**Where the setting must live (surprise).** My first choice was a per-package entry, `[profile.release.package.nata-kernels] overflow-checks = true`. The scratch workspace with a kernel crate `k` and a caller `app` that has no setting showed it is not enough:

| call from `app` into `k` | per-package entry on `k` | workspace-wide `[profile.release]` |
|---|---|---|
| `#[inline(never)] fn add` | panics | panics |
| plain small `fn add` (rustc inlines it across crates automatically) | **wraps** | panics |
| `#[inline(always)] fn add` | **wraps** | panics |
| generic `fn add<T: Add>` instantiated by the app | **wraps** | panics |

Small, inlined and generic kernel code is compiled in the calling crate and follows the **caller's** setting (verified; the cause is rustc's behaviour for cross-crate inlining, I did not read the compiler source). I therefore set it workspace-wide. A per-package entry would silently give no protection to kernel code called from `nata`, `nata-bench` or any other crate.

**Cost** (`examples/overflow_cost.rs`, 8192 rows, best of 7 repetitions, ns per row, two runs each, load 5.3 to 5.5; the second run of `mul, no NULLs` differs from the first by 2x in both settings, so that row is noise):

| kernel | checks on | checks off |
|---|---|---|
| `sum`, no NULLs (i128 accumulate) | 1.13 / 1.11 | 0.53 / 0.58 |
| `group_sums_dense` K=1 | 2.34 / 2.23 | 1.91 / 1.91 |
| `div`, no NULLs | 2.94 / 2.92 | 2.76 / 2.96 |
| `div`, NULLs, selection | 4.21 / 4.21 | 4.31 / 4.28 |
| `filter_range`, no NULLs | 0.87 / 0.87 | 0.91 / 0.87 |
| `mul`, no NULLs | 1.42 / 2.31 | 2.21 / 1.10 |
| `mul`, NULLs | 4.78 / 5.22 | 5.58 / 4.94 |

So the kernels already written with `overflowing_*` and `checked_*` pay nothing measurable; the accumulation loops (`sum`, grouped sums) cost about 2x and 20% because the checked i128 add blocks vectorization (reasoned, not examined in the assembly). Whether that matters end to end was not measured; the generic-ops benchmarks were not re-run with the setting on. x86-64 only.

### 1.4 Lenient arrow entry points not on the ADR list

Found by reading the public functions of `arrow-arith`, `arrow-cast`, `arrow-array`, `arrow-ord`, `arrow-select` and `arrow-buffer` 60.1.0. All the entries below were added to `clippy.toml` with a failing fixture each.

| entry point | why it is lenient |
|---|---|
| `arrow_arith::numeric::neg_wrapping` | wraps on MIN |
| `arrow_arith::aggregate::sum_array` | wrapping sum (also dictionary and run-end arrays) |
| `arrow_arith::aggregate::product` | wrapping product |
| `arrow_arith::arithmetic::multiply_fixed_point`, `multiply_fixed_point_dyn` | decimal multiply that wraps and rounds ("doesn't detect overflow", per its doc) |
| `arrow_arith::bitwise::bitwise_shift_left`, `bitwise_shift_right` | `wrapping_shl`: the shift amount wraps modulo the bit width |
| `arrow_array::PrimitiveArray::unary_opt` | a `None` from the closure silently becomes NULL |
| `arrow_array::ArrowNativeTypeOp::{add,sub,mul,div,mod,neg,pow,div_rem}_wrapping` (8) | the public trait behind every arrow integer and decimal op; **banning the trait method catches it on `i64` and `i128`** (fixtures use both) |
| `arrow_buffer::i256::wrapping_{abs,neg,add,sub,mul,div,rem,div_rem,pow}` (9) | Decimal256 inherent wrapping methods |
| std `wrapping_{add,sub,mul,neg}` and `saturating_{add,sub,mul}` on `i32`, `i64`, `i128` (21) | not arrow, but a closure given to `unary` or `binary` can call them and `arithmetic_side_effects` does not look at method calls |

Not added, with reasons:

- `arrow_cast::cast::{rescale_decimal, float_to_decimal_opt, single_float_to_decimal, num_cast}`: they return `Option`, so the failure is visible in the type and the caller must turn `None` into an error. They are the useful row-level building blocks for our own `strict_cast`.
- `arrow_arith::arity::{unary, binary}` and `PrimitiveArray::unary`: closures. The arithmetic lint reaches inside the closure (fixture `lints/arith_in_closure_passed_to_arrow` fails).
- `arrow_cast::parse::*`: return `Result`.
- `u64`/`usize` std wrappers: hashing (section 1.2).
- `CastOptions::default`: a trait-impl method; clippy reports the path as unreachable (as in the earlier trial). Not needed: the function that consumes it is banned.

**A lenient behaviour that cannot be banned: arrow's checked `add` on `Decimal128(38, 0)`** returns `Ok` for `10^38 - 1 + 1`, a value that is not a valid DECIMAL(38,0) (`tests/arrow_behaviour.rs`). `mul` does catch i128 overflow, but neither checks the declared precision. Decimal results therefore need our own precision check, as `decimal-rules.md` already requires; `validate_decimal_precision(38)` catches this value. Also: arrow's `rem` of `i64::MIN % -1` returns 0, while DuckDB 2.0.0 raises an overflow error for `%`.

### 1.5 Noise from `arithmetic_side_effects`, `cast_possible_truncation`, `cast_possible_wrap`

`scripts/noise.py`, on the old spike `kernels.rs` (no resolution), lints at `warn`:

| lint | library code | in `#[cfg(test)]` |
|---|---|---|
| `arithmetic_side_effects` | 10 | 2 |
| `cast_possible_truncation` | 2 | 1 |
| `cast_possible_wrap` | 0 | 2 |
| total | **12** | 5 |

They sit in `filter_range` (2 casts, 2 arithmetic), `sum` (3), `group_sums_dense` (2) and `group_sums_masked` (3). `mul` and `sub_const_left` produced none, because they use `overflowing_*`.

Resolution (current file: 0 diagnostics, including the new `div` kernels and `filter_ne`/`split_ne`):

- 5 `#[allow(..., reason = "...")]` in library code, one per function: three for the accumulation loops (`sum`, `group_sums_dense`, `group_sums_masked`; reason: i128 sums and u64 counts over fewer than 2^64 rows cannot overflow), two for the `usize as u32` row-index casts (`filter_range`, `split_ne`).
- The row-index casts are made sound by an `assert!(u32::try_from(rows).is_ok())` at function entry (one check per call, not per row), so the allow states a checked precondition.
- The test module has one `#[allow]` (test data generation).
- The new `div` and `div_const_left` needed none: `checked_div(..).unwrap_or(0)` plus the two flags.
- I did not rewrite the accumulation loops with `checked_add`: with `overflow-checks` on they already pay the check (section 1.3), and a second explicit check would add a branch with no new information.

**Honest reading.** Workable for this size: 12 warnings in about 200 lines of library code, all of one kind (accumulators). But the lint cannot do range analysis, so each allow is an unproved claim about row counts and value ranges, and the claim is only as good as the test or proof behind it (the 2^64 row bound is reasoned; the Kani work can cover it). Kernels written around `checked_*` and `overflowing_*` (mul, sub, div) are lint-free, so the noise concentrates in loops that accumulate, and will grow with the number of aggregate kernels. Not measured: noise on a larger crate.

## 2. Guarded expressions (item 13)

New in `crates/nata-kernels`: `div` and `div_const_left` (error on a zero divisor and on `i64::MIN / -1`; the error flags are masked by selection and by validity, as in `mul`), the guard filters `filter_ne` and `split_ne` (a NULL comparison drops the row, or sends it to the ELSE branch of a CASE), `merge_branches` and `constant` for CASE, and `arrow_path.rs` with three ways of calling arrow's checked `div`.

### 2.1 Tests

- Unit tests in `kernels.rs`: `div` and `div_const_left` against a scalar reference over 6000 random cases each (edge values 0, -1, 1, MIN, MAX, NULLs, selections; the test also requires at least 500 errors and 500 successes so the generator reaches both), including which error is reported; hand-written cases for the unselected row, the NULL slot, and `MIN / -1`.
- `tests/guarded.rs`: 3 crafted tables (valid zero divisors; a valid `MIN / -1`; failures only in NULL slots, stored as dividend `MIN` and divisor 0 so a missing mask shows) x 9 queries x 3 executors = 81 runs, each compared with a row-by-row reference (plain Rust with short-circuit guards). The 9 queries: guarded filter, unguarded filter, CASE with three different guards (`b <> 0`, `b <> -1`, `a <> 0` with a constant dividend), two id-range selections, nested division with one and with two guards. 13 of the 27 table/query combinations are errors by design (the reference says error), 14 succeed. Eleven hand-derived expectations are asserted separately (for example `WHERE b <> 0 AND a // b > 1` on table 1 is exactly rows 0 and 9; the same table without the guard is a division-by-zero error; on table 2 the guard `b <> 0` does **not** protect the surviving `MIN / -1` row).
- The three executors: our kernels over the selection; arrow `div` after **compaction** (`take` the selected rows first); arrow `div` after **folding** the selection into the null buffers. All three agree with the reference on all 27 combinations.
- Why the rule exists (`tests/guarded.rs`): arrow's `div` over the whole array with no compaction returns `DivideByZero` for a row the guard removed, and `ArithmeticOverflow` for a removed `MIN / -1` row. After compaction or folding, the same call succeeds and agrees with our kernel row for row. Arrow already skips NULL slots by itself (its own validity handling), so the selection is the only thing it cannot see.

### 2.2 DuckDB differential (`scripts/guard_diff.py`)

The same tables and queries, written with integer division `//`, run in DuckDB 2.0.0 and compared with each of the three executors: **81 cases, 81 identical (42 result sets, 39 errors with the same category, division by zero or overflow), 0 different.** DuckDB's `WHERE a AND b` applies the conjuncts in order over the surviving rows (a NULL guard removes the row), and `CASE` does not evaluate the THEN branch for other rows, so the guards behave as in the Nata rule.

Surprise: DuckDB's integer `//` is **not** lenient. The existing `decimal-rules.md` section 9 says "`inf` for `/`; error for `%`". Probes (DuckDB 2.0.0, BIGINT):

| expression | DuckDB 2.0.0 | Nata rule | same? |
|---|---|---|---|
| `1 / 0` | `inf` (DOUBLE) | error | **no** (already in decimal-rules.md) |
| `MIN / -1` with `/` | 9.223372036854776e+18 (DOUBLE) | error | **no** |
| `7 / 2` | 3.5, type DOUBLE | exact decimal | **no** (already in decimal-rules.md) |
| `1 // 0` | error "Division by zero ... Use TRY(...)" | error | yes |
| `MIN // -1` | error "Overflow in division" | error | yes |
| `-7 // 2` | -3 (toward zero) | -3 | yes |
| `TRY(1 // 0)` | NULL | `TRY()` gives NULL | yes |
| `SET error_on_division_by_zero=false; 1 // 0` | NULL | no such setting | differs, a DuckDB option |
| `1 % 0` | error | error (decimal-rules.md) | yes |
| `MIN % -1` | error "Overflow in division" | not specified; arrow returns 0 | **open**: there is no Nata `%` kernel yet, so the rule needs a decision |
| `NULL // 0` (dividend NULL, divisor 0), `5 // NULL` | NULL, no error | NULL, no error | yes (in the 81 cases) |

Only the `/` rows are real differences; the `check/queries/decimal-differences/` files already cover those.

### 2.3 Mutation check (`scripts/mutate_guards.py`)

22 deliberate breakages, each applied alone to `src/kernels.rs` or `src/arrow_path.rs`, then `cargo test -p nata-kernels`. **22 of 22 caught.** One survivor in the first run, "merge_branches: later branch does not overwrite" (equivalent for disjoint branches), led to a new test of the overlap behaviour (`merge_branches_lets_a_later_part_win_on_an_overlap`) and is now caught. The script's output lists the failing tests per mutation.

| # | mutation | caught by (first tests listed) |
|---|---|---|
| 1 | `div`: error flags ignore the selection | `a_zero_divisor_in_an_unselected_row_is_not_an_error`, `div_matches_the_scalar_reference...` |
| 2 | `div`: zero-divisor flag not masked by validity | `a_zero_divisor_in_a_null_slot_is_not_an_error`, `div_matches...` |
| 3 | `div`: overflow flag not masked by validity | `div_matches...`, `min_divided_by_minus_one...` |
| 4 | `div`: validity ignores the divisor | `a_zero_divisor_in_a_null_slot...`, `div_matches...` |
| 5 | `div`: zero check dropped | `a_zero_divisor_in_an_unselected_row...`, `div_const_left_follows_the_same_rules` (+2) |
| 6 | `div`: `MIN / -1` check dropped | `div_const_left_follows_the_same_rules`, `div_matches...` (+2) |
| 7 | `div`: output validity always true | `a_zero_divisor_in_a_null_slot...`, `div_matches...` |
| 8 | `div`: rounds toward minus infinity | `div_matches...`, `min_divided_by_minus_one...` |
| 9 | `div`: overflow reported ahead of zero | `div_matches...`, `div_const_left_matches...` |
| 10 | `div_const_left`: zero flag not masked by validity | `div_const_left_follows_the_same_rules`, `div_const_left_matches...` |
| 11 | `div_const_left`: ignores the selection | same two |
| 12 | `div_const_left`: operands swapped | same two |
| 13 | `split_ne`: a NULL comparison counts as TRUE | `filter_ne_drops_nulls...`, `split_ne_sends_null_comparisons...` |
| 14 | `split_ne`: NULL comparisons vanish from ELSE | `split_ne_sends_null_comparisons...` |
| 15 | `split_ne`: ignores the incoming selection | `filter_ne_drops_nulls...`, `split_ne_sends...` |
| 16 | `split_ne`: `==` instead of `<>` | same two |
| 17 | `merge_branches`: branch NULLs become valid | `merge_branches_places_each_part...` |
| 18 | `merge_branches`: later part does not overwrite | `merge_branches_lets_a_later_part_win_on_an_overlap` (added) |
| 19 | arrow wrapper: compaction skipped | 9 tests, including `arrow_without_compaction_errors_on_a_row_the_guard_removed` |
| 20 | arrow wrapper: selection not folded | 8 tests |
| 21 | arrow wrapper: selection dropped from the null-buffer union | 8 tests |
| 22 | arrow wrapper: validity bytes inverted | 9 tests |

Mutations 19 to 22 are caught through the executors in `guarded.rs`; mutations 13 to 16 are caught only by the two small unit tests listed, not by the crafted-table queries.

## 3. Limits and what is not proved

- Not formal: no Kani proof or exhaustive check of `div` here. The random tests cover 6000 cases of up to 40 rows each; the edge values are drawn with high probability but not exhaustively.
- Only `i64`. Decimal, `i32` and other widths, and a `%` kernel, are not written.
- The DuckDB differential covers one table shape and 9 queries, DuckDB 2.0.0 only. It agrees on errors by category, not by message.
- The arrow wrapper cost (`take` or the null-buffer fold) was not measured.
- `clippy.toml` only reaches code that a lint can see: raw operators on non-integer-like newtypes, `unsafe` (forbidden), and anything inside a macro from another crate are not examined.
- The ban list is tied to arrow 60.1.0; a new arrow version can add a lenient function. `run.py` has no check for that, only a manual re-read of the public functions.
- The overflow-checks cost table is from a loaded shared VM. The `sum` and grouped-sum differences are large and repeated; the `mul` row is not usable.
- The lint fixtures need the arrow crates in the local cargo cache (they build offline).
- `lint-fixtures/run.py` and the scripts use Python 3 from the host; they are developer tools, not part of the build.
