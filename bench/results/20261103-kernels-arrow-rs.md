# Kernels: arrow-rs semantics and cost (ADR 0003)

Code: `spikes/pipeline-model/` (`spike-pipeline-model <file> kernels`; throwaway, delete once ADR 0003 is accepted).

## What was run

- arrow-rs / parquet 60.1.0, Rust 1.99.1, release build, single thread.
- CPU: 12th Gen Intel(R) Core(TM) i3-1220P, 8 vCPUs under KVM, 15.6 GiB RAM, Linux 6.12.113. x86-64 only; nothing was run on ARM64.
- Expression: `p * (100 - d)` (the TPC-H Q1 discounted price at scale 2), over 6,001,215 rows in 8192-row batches. Data is synthetic and uniformly random (`p` below 10,500,000, `d` below 11), not the real column.
- Timing: 15 timed rounds after 1 warm-up, variants alternating inside each round, min and median of the wall time for all 6M rows. Results are discarded after a `black_box`. The numbers below are ranges over 5 runs of 2 builds of the same code.
- Arrow variants allocate their result and one temporary per batch. Hand variants write into one reused output buffer, which is a design difference of the API as much as of the kernel.

## Semantics (what the kernels do on bad input)

| operation | result |
|---|---|
| `add(i32::MAX, 1)` | error: `Overflow happened on: 2147483647 + 1` |
| `add_wrapping(i32::MAX, 1)` | silently -2147483648 |
| `cast` i64 5e9 to i32, default `CastOptions` | **silently NULL** (`safe = true` is the default) |
| same, `CastOptions { safe: false }` | error |
| `cast` 'abc' to i32, default / `safe: false` | silently NULL / error |
| `1 / 0` (i32) | error: divide by zero |
| `sum([i64::MAX, 1])`, plain `sum` | **silently wraps** to -9223372036854775808 |
| same, `sum_checked` | error |
| Decimal128(15,2) 99999.99 * 99999.99 | Decimal128(31,4), value 99999980000001 (exact) |
| Decimal128(38,0) 10^37 * 100 | error: overflow |

So strict behaviour exists, but the lenient entry points (`*_wrapping`, plain `sum`, default `CastOptions`) are the ones a casual call reaches.

## Cost, ns per row (all 6M rows, min over rounds, range over runs)

| variant | ns/row | vs checked i64 hand loop |
|---|---:|---:|
| arrow i64, checked (two kernels, two temporary arrays) | 1.98-2.52 | 2.4-3.0x slower |
| arrow i64, wrapping | 1.03-1.08 | 1.3x slower |
| hand loop i64, checked, one pass | 0.80-0.84 | 1.0 |
| hand loop i64, wrapping, one pass | 0.70-0.74 | 0.9 |
| hand loop i64, checked, fused with the sum (no output array) | 0.75-0.79 | 0.9 |
| arrow i64, checked, 10% NULLs in each input | 2.43-2.73 | |
| hand loop i64, checked, 10% NULLs, per-row branch on validity | 3.10-3.50 | slower than arrow |
| hand loop i64, checked, 10% NULLs, branchless (overflow flag masked by validity) | 1.57-1.71 | about 1.6x faster than arrow |
| arrow Decimal128(15,2), checked (two kernels) | 5.15-9.57 | 6-12x slower |
| hand loop on Decimal128 storage, i128 checked, one pass | 3.15-3.39 | 4x slower |
| hand loop on Decimal128 storage, narrowed to i64, checked | 1.43-1.52 | 1.8x slower |

## Reading, and limits

- Decimal arithmetic is where arrow-rs is clearly expensive: at least 5 ns/row, against 3.2-3.4 for a one-pass `i128` loop and 0.8 for native `i64`. Storage width explains about 4x (3.2 to 0.8); one pass without temporaries explains about 1.6x (5.2 to 3.2).
- Arrow's Decimal128 time varied from 5.2 to 9.6 ns/row between builds of identical code. The cause was not found. A guess that the 128 KB temporaries hit glibc's `mmap` threshold was tested in a build where the slow case did not appear (`MALLOC_MMAP_THRESHOLD_` changed nothing), so it is untested. The decimal conclusion holds across the whole range.
- NULLs erase the hand loop's advantage unless it is written to be branch-free: the obvious version (branch on validity) was slower than arrow. The 1.6x advantage with NULLs needs the masked overflow flag.
- Hand loops here are lower bounds for a real kernel: no selection vector, a single expression, one reused output buffer, fixed types, 10% NULLs at random positions only.
- Arrow wrapping is within 1.3x of a hand loop; checked arrow is 2.4-3x slower. The cost of strictness is mostly in arrow's checked path, not in checking itself (hand checked is 0.80 against 0.70 wrapping).
- Not measured: ARM64; multiple threads; strings, comparisons, filters or casts; real column data; arrow's Decimal64 kernels.

## arrow-rs and ARM64 (source inspection, not a measurement)

arrow-rs 60.1 kernels (`arrow-arith`, `arrow-cast`, `arrow-select`, `arrow-ord`) contain no architecture-specific code and no NEON or AVX intrinsics; they are plain Rust that depends on LLVM autovectorization. The only architecture-specific code in the arrow crates is allocation alignment constants per target (`arrow-buffer/src/alloc/alignment.rs`), a `PREFERRED_VECTOR_SIZE` constant in `arrow-arith/src/aggregate.rs` chosen at compile time (16 bytes by default, 32 with AVX, 64 with AVX-512), and some bit utilities. The `parquet` crate has none. So there is no ARM64 fast path to rely on or lose; performance on ARM64 is whatever LLVM produces and is not measured here.

## Follow-up: Decimal64, allocation, and fused versus unfused (after independent review)

The independent review asked for three things the first run did not measure. Same machine and method (15 rounds, alternating, min over rounds); ns per row, range over 3 runs of one build unless stated.

| variant | ns/row |
|---|---:|
| arrow i64, checked (two kernels) | 1.87-2.20 |
| arrow **Decimal64**(15,2), checked (two kernels); result type `Decimal64(18, 4)` | 1.87-2.23 |
| arrow Decimal128(15,2), checked (two kernels) | 4.50 (this build; 4.5-9.6 across all builds) |
| hand i64, one fused pass, reused output | 0.75-0.78 |
| hand i64, one fused pass, allocates an `Int64Array` per batch | 0.74 |
| hand i64, **two** primitive passes (sub, then mul), reused temp and output, checked per row | 1.20-1.25 |
| hand i64, two passes, overflow flag per pass (branch-free) | 1.21-1.26 |
| hand, narrowed to i64, allocates a `Decimal128Array(31, 4)` per batch | 1.34-1.36 |
| hand loop on Decimal128 storage, `i128` checked, one pass | 2.72 (2.7-3.4 across builds) |

What this shows:

- **Arrow's `Decimal64` kernels are about as fast as its `i64` kernels** (about 2 ns/row), and 2-5x faster than its `Decimal128` kernels. The large decimal gap in the first run was Decimal128, not decimals in general. Its multiply of (15,2) by (16,2) returns `Decimal64(18, 4)`: the result precision is capped at 18 instead of promoted to 31, which differs from the usual SQL rule and from what a `Decimal128` kernel returns.
- **Owning the primitives and fusing are separate gains.** Two own primitive passes (as a vectorized interpreter would run them) take 1.2-1.26 ns/row, 1.6-1.8x faster than arrow's two checked kernels. One fused pass takes 0.75-0.78, another 1.6x. The 0.77 figure is a loop specialized to this one expression, which an expression compiler could produce but a generic interpreter would not. The first run's "2.4-3x" mixed both gains.
- **Output allocation is not the explanation.** Allocating a fresh output array per batch cost the same as reusing a buffer (0.74 against 0.75-0.78), because building an Arrow array from a `Vec` does not copy. The first run's caveat that the gap might be a buffer-reuse effect does not apply here.
- Still not measured: ARM64, threads, real data, and selection-aware kernels.
