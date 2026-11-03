# Strictness lint trial (ADR 0003, decision 6)

Code: `spikes/lint-check/` (throwaway; `clippy.toml`, one `main.rs` with a call for each ban, delete once ADR 0003 is accepted).

## What was run

- arrow 60.1.0, Rust and clippy 1.99.1, `cargo clippy`. Lints set to `deny` in `Cargo.toml`: `disallowed_methods`, `arithmetic_side_effects`, `cast_possible_truncation`, `cast_possible_wrap`. `disallowed-methods` paths are in `clippy.toml`.
- Runtime check of `overflow-checks`: a release build of `i32::MAX + 1` with the value hidden from the optimizer, with the setting on and off.
- x86-64 only. The lints are compile-time and do not depend on the architecture.

## Result

| call in the test file | flagged? |
|---|---|
| `add_wrapping`, `sub_wrapping`, `mul_wrapping` (called through `arrow::compute::kernels::numeric`) | yes |
| plain `sum` | yes |
| `arrow::compute::cast` (default options) | yes |
| `arrow::compute::cast_with_options` | yes |
| `add`, `sum_checked` (the strict ones) | no, as intended |
| `CastOptions::default()` and `..Default::default()` | **no**: the configured path `arrow_cast::cast::CastOptions::default` is reported as "does not refer to a reachable function"; a method of a trait impl cannot be named this way |
| a literal `safe: true` | **no**: a lint cannot see a field value |
| `x as i64` from `i128` | yes (`cast_possible_truncation`) |
| unchecked `+` and `*` on integers | yes (`arithmetic_side_effects`) |

Paths in `clippy.toml` use the defining crate (`arrow_arith::numeric::add_wrapping`), and a call through the `arrow::compute` re-export is still caught.

`overflow-checks`: release build with `overflow-checks = true` panics on `i32::MAX + 1` ("attempt to add with overflow"); with Cargo's default for release (off) it silently prints -2147483648.

## Reading

- Function-level bans work, including through re-exports. Banning both `cast` and `cast_with_options` covers the lenient cast path, so `CastOptions::default()` is only reachable through a function that is already banned; the single wrapper module that calls them must be exempted with an explicit `#[allow]`.
- The lint cannot see plain Rust arithmetic and casts; `arithmetic_side_effects`, `cast_possible_truncation`, `cast_possible_wrap` and `overflow-checks = true` do, and were confirmed to.
- Not covered: how noisy `arithmetic_side_effects` is on real kernel code (it flags every integer `+` and `*`, so kernel crates need deliberate `checked_*` use or a local allow with a stated invariant); `cast_possible_truncation` on legitimate narrowing casts; other lenient arrow entry points not in the list (for example casts inside other kernels); any other arrow-rs version.

## Precedent check for ADR 0001 (web search, secondary sources only)

- DuckDB: push-based pipelines (source, operators, sink) scheduled by an event-driven scheduler, with morsel-driven parallelism adapted from the HyPer paper. Stated consistently by several explainer blogs; not confirmed against DuckDB's own documentation or source.
- Presto/Trino: the Java `Operator` interface has `needsInput`, `addInput`, `getOutput` and `isBlocked` (a future). Confirmed in the Javadoc.
- Velox: a GitHub issue quotes an `isBlocked` returning a `BlockingReason` and filling a `ContinueFuture`, which matches. Velox's own `Operator.h` was not read.
