# Portability: x86-64 and ARM64

Principle: **one codebase, two first-class targets.** Nata must be correct on both x86-64 and ARM64 (aarch64), and measured on both. Neither is an afterthought. Code that is only fast or only correct on one is not done (see `way-of-working.md`, section 2).

Targets:

| Target | Why |
|---|---|
| `x86_64-unknown-linux-gnu` | Most servers; the default cloud fleet |
| `aarch64-unknown-linux-gnu` | AWS Graviton, Ampere, other ARM servers; cheaper per core |
| `aarch64-apple-darwin` | Developer laptops (Apple Silicon) |

## 1. Rules

1. **Scalar first.** Write plain, portable Rust over contiguous columnar buffers. LLVM autovectorizes this on both architectures. Do not write intrinsics until a benchmark shows the scalar kernel is the bottleneck.
2. **One reference kernel per operation.** Every optimized kernel has a scalar version that is the source of truth. Tests compare each optimized path against it.
3. **Runtime dispatch, not build-time tuning.** Release binaries run on any CPU of their architecture. Pick the fast path at startup with `is_x86_feature_detected!` / `is_aarch64_feature_detected!`.
4. **Never ship `-C target-cpu=native`.** It builds a binary that can crash on another machine of the same architecture. Use it only for local experiments, never in CI or releases.
5. **Intrinsics are isolated.** Architecture-specific code lives in a small `arch` module per crate, behind `#[cfg(target_arch = "...")]`, with a safe public function that does the dispatch. No `unsafe` intrinsics in operator code.
6. **No silent fallback to a slow path.** If a kernel falls back to scalar, `EXPLAIN ANALYZE` or the log must say so. Surprising slowness is a bug report, not a mystery.
7. **Same results everywhere.** The same query on the same data gives the same result on both architectures (see section 5).

## 2. Instruction sets

| | x86-64 | ARM64 |
|---|---|---|
| Baseline SIMD | SSE2 (128-bit) | NEON (128-bit) |
| Common wide SIMD | AVX2 (256-bit), on almost all servers since ~2015 | SVE/SVE2 on some chips (Graviton3/4); not on Apple Silicon |
| Optional | AVX-512, patchy: not on many consumer CPUs | none we rely on |
| Bit tricks | `popcnt`, BMI2 (`pdep`, `pext`) | `cnt`, no direct `pext` |
| Gather loads | AVX2 gather exists, often slow | none in NEON |
| CRC / hashing | `crc32`, `pclmulqdq` | CRC and crypto extensions |

Consequences:

- **Baseline we must support:** SSE2 and NEON. Everything works there.
- **Tier 2 (optional fast path):** AVX2 on x86-64. Add AVX-512 or SVE only with a measured win and an ADR.
- Do not assume a vector width. NEON is 128-bit and AVX2 is 256-bit, so a kernel tuned to one layout needs a separate design on the other, not a port.
- Do not design around `pext` or gather. If a Parquet bit-unpacking or filter kernel needs them, give it a different algorithm on ARM.
- Prefer a portable SIMD crate (for example `wide` or `pulp`) before raw `core::arch`. Check whether `std::simd` is stable on the pinned toolchain before using it.

## 3. Memory model and concurrency

- x86-64 is strongly ordered (TSO). ARM64 is weakly ordered. Code that is wrong can pass on x86-64 and fail on ARM64.
- Use the weakest ordering that is **correct**, and write down why: `Acquire` / `Release` for hand-off, `SeqCst` when unsure, `Relaxed` only for counters nobody synchronizes on.
- Lock-free structures and anything with `unsafe` shared state must be tested on ARM64 (see section 6). Passing on x86-64 proves nothing.
- Prefer channels, `Mutex` and existing crates over hand-rolled atomics. Hand-rolled atomics need an ADR.

## 4. Memory and layout

- **Cache line size is not 64 everywhere.** It is 64 bytes on most x86-64 and many ARM servers, but 128 bytes on Apple Silicon. Use `crossbeam_utils::CachePadded` instead of a hardcoded `64`.
- **Page size is not 4 KB everywhere.** Apple Silicon uses 16 KB, and some Linux ARM systems use 64 KB. Query the page size at runtime. Never hardcode it in allocators, `mmap` alignment, or spill file layouts.
- **Alignment.** Align column buffers to at least 64 bytes (Arrow's convention) so both architectures can use aligned vector loads. Do not rely on unaligned loads being free.
- **Endianness.** Both targets are little-endian. Do not write code that assumes it silently. Use `to_le_bytes` / `from_le_bytes` for on-disk and on-wire formats.
- **NUMA.** Large x86-64 servers often have several NUMA nodes. Do not assume uniform memory access in the scheduler (see ADR 0002).

## 5. Numeric determinism

- **Floating point.** Fused multiply-add (FMA) and different summation order can change the last bits. Do not use FMA or reorder floating-point reductions in a way that depends on the architecture. If a kernel does, results must still be identical to the reference kernel. Otherwise the tests and `nata-check` use an explicit tolerance and say so.
- **Integers.** Overflow, NULL and coercion behavior follows the strictness spec and is identical on both targets.
- **Ordering.** Sort, group and hash output order is defined by the spec, not by whatever a SIMD path happens to produce. Differential tests must not depend on incidental order.
- **Hash values** that reach disk, the network or a plan hash must be identical on both architectures. Pick an explicit, fixed hash for those. Do not use a hasher whose output differs by CPU feature.

## 6. Build, test and CI

- Both architectures run the full test suite natively in CI, not only cross-compiled. GitHub provides `ubuntu-24.04-arm` runners.
- Each CI job runs `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` per architecture.
- Run `nata-check` (differential against DuckDB) on both.
- Optimized-vs-scalar tests run once per available instruction set. A test must not skip silently when the CPU lacks a feature. It reports "skipped: no AVX2".
- Release builds are separate artifacts per target. Do not build a "universal" binary that depends on `target-cpu`.
- Dependencies: before adopting a crate for compression, hashing or Parquet, check that it has fast paths for both ARM64 and x86-64. Note the result in the ADR.

## 7. Benchmarks

- `nata-bench` prints the architecture, CPU model, core count, cache sizes, page size and detected features with every result.
- Results in `bench/results/` are recorded **per CPU**. A speedup on one architecture can be a regression on the other, and each must be reported.
- Compare like with like: do not claim "ARM is faster" from a laptop against a server. State cores, memory, warm or cold cache, and data size.
- A new optimized kernel lands only with a benchmark on **both** architectures. If it helps one and hurts the other, dispatch by architecture or drop it.

## 8. Definition of done additions

On top of `way-of-working.md`, a slice that adds or changes a kernel is done when:

1. The scalar reference and every optimized path give identical results in tests.
2. Tests pass on x86-64 and ARM64 in CI.
3. A benchmark result exists for both architectures, per CPU model.
4. Any `unsafe` or architecture-specific code is isolated in an `arch` module, with a comment on the invariant it relies on.
5. If a fast path is added or removed, a short ADR records why.

## 9. Open items

- [ ] Add an ARM64 job to `.github/workflows/ci.yml` (currently x86-64 `ubuntu-latest` only).
- [ ] Add `targets` to `rust-toolchain.toml` for the three targets above.
- [ ] Make `nata-bench` print CPU model, features, cache sizes and page size.
- [ ] Decide where kernels come from (ADR 3: arrow-rs, own, or staged). Check ARM64 coverage in arrow-rs kernels as part of that.
- [ ] Make `scripts/gen-tpch.sh` and the DuckDB install step in CI work on ARM64 (the CI step currently downloads the `linux-amd64` CLI).
