# ARM64 run 2: kernels, pull versus push, and v2 on a quiet C4A Spot VM

Run id `nata-run2` (`scripts/gcp/orchestrate.sh run --job bench`), 2026-11-03 22:04-22:10 UTC (2026-11-04 local). Binaries and modes are the same spike code as the x86-64 results (`spikes/pipeline-model`: `kernels`, `pullpush 25`, `v2`), built on the VM.

## Machine, place and cost

| | |
|---|---|
| Machine type | `c4a-highcpu-8`, Spot, one VM; the chain contained only this type |
| Zone | us-central1-a |
| CPU | `Neoverse-V2`, `aarch64`, 8 vCPU, 15 GiB RAM, L2 16 MiB (8 instances), L3 80 MiB |
| OS | Debian 12, kernel 6.1.0-54-cloud-arm64 |
| Disks | 20 GB and 30 GB Hyperdisk Balanced, both deleted |
| VM time counted | 342 s, 1 VM, 0 preemptions |
| Estimated cost | about 0.015 USD for the VM (0.1572 USD/h x 342 s) plus a fraction of a cent for disks and IP; estimated from catalog prices, the bill was not checked. Cap was 0.20 USD |
| Left behind | nothing (verified by label after cleanup) |

This confirms C4A's core as Neoverse V2 (N4A, run 1, is Neoverse N3). The machine ran only the benchmark steps, one at a time; the orchestrator's state copy-back (a small `ssh` + `tar` every 120 s) ran alongside them. Run-to-run noise is far lower than on the shared local VM: the paired-ratio quartiles below are about ±1%, against ±3-5% there.

Step durations (UTC): toolchain 5 s, SF1 generation 3 s, build of the spike 76 s, one kernel benchmark 3 s, pull-versus-push 93 s, v2 12 s.

## Pull versus push on ARM64

Same method as `20261104-pull-vs-push-corrected.md`: 25 interleaved rounds, same morsel queue, a pull-against-pull control, three batch sizes, 1 to 8 threads.

| batch rows | threads | pull ms min / median | push ms min / median | push / pull, median of paired ratios [Q1, Q3] | control: pull2 / pull [Q1, Q3] |
|---:|---:|---|---|---|---|
| 8192 | 1 | 151 / 152 | 152 / 154 | 1.006 [1.001, 1.016] | 1.010 [1.007, 1.017] |
| 8192 | 2 | 78 / 80 | 77 / 78 | 0.978 [0.971, 0.992] | 1.000 [0.991, 1.009] |
| 8192 | 4 | 40 / 41 | 40 / 41 | 1.012 [0.996, 1.022] | 1.010 [0.997, 1.027] |
| 8192 | 8 | 22 / 22 | 22 / 22 | 0.995 [0.992, 1.003] | 1.008 [0.993, 1.015] |
| 1024 | 1 | 182 / 185 | 183 / 185 | 0.999 [0.996, 1.004] | 0.999 [0.993, 1.008] |
| 1024 | 2 | 93 / 94 | 93 / 95 | 1.006 [1.001, 1.009] | 1.006 [0.988, 1.012] |
| 1024 | 4 | 48 / 49 | 49 / 49 | 1.001 [0.997, 1.009] | 0.999 [0.990, 1.005] |
| 1024 | 8 | 26 / 26 | 26 / 26 | 1.007 [0.998, 1.012] | 1.005 [0.993, 1.010] |
| 256 | 1 | 277 / 280 | 277 / 281 | 0.999 [0.994, 1.007] | 0.997 [0.987, 1.003] |
| 256 | 2 | 142 / 143 | 142 / 143 | 1.001 [0.997, 1.005] | 0.999 [0.996, 1.006] |
| 256 | 4 | 74 / 74 | 73 / 74 | 0.999 [0.996, 1.003] | 0.999 [0.998, 1.002] |
| 256 | 8 | 39 / 40 | 40 / 40 | 1.001 [0.997, 1.007] | 1.000 [0.997, 1.003] |

Reading: push/pull lies between 0.978 and 1.012 in all 12 cells; the control lies between 0.997 and 1.010. At 256-row batches (about 12 microseconds per batch on one thread) the ratio is 0.999-1.001, so any dispatch difference is far below 1%. One cell (8192 rows, 2 threads) shows push 2.2% faster, outside that cell's control spread; it does not repeat in any other cell, so it is not evidence of a systematic effect. Scaling is near linear on this machine: 151 ms on one thread to 22 ms on eight (6.9x), against about 4x on the shared x86-64 VM.

## v2 (the hand-fused query) on ARM64

min / median ms, 15 timed runs, the same four variants as the x86-64 table.

| variant | 1 thread | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| Decimal128 | 101 / 101 | 51 / 52 | 27 / 27 | 15 / 15 |
| reader returns Decimal64 | 91 / 91 | 47 / 47 | 24 / 24 | 13 / 14 |
| file held in memory (Decimal128) | 95 / 95 | 49 / 51 | 25 / 27 | 14 / 14 |
| Decimal64 and file in memory | 85 / 85 | 43 / 43 | 23 / 23 | 12 / 12 |

The result rows are identical to DuckDB's exact decimals. Unlike on the noisy x86-64 VM, Decimal64 (about 10% faster) and holding the file in memory (about 6% faster again) are visible and consistent here. These are single-machine timings; no ARM-versus-x86 speed claim is made, because the machines differ in cores, clock and noise.

## Kernels on ARM64

Semantics are identical to the x86-64 results (arrow's checked and lenient entry points behave the same). Cost, ns per row (three runs, min over 15 rounds each):

| variant | ns/row |
|---|---:|
| arrow i64, checked (two kernels) | 2.41 |
| arrow Decimal64, checked | 2.42-2.43 |
| arrow Decimal128, checked | 2.75-2.77 |
| own i64, one fused pass, reused output | 0.90-0.93 |
| own i64, one pass, allocating the output array | 0.86-0.89 |
| own i64, two primitive passes | 1.64-1.71 |
| own, narrowed to i64, building a `Decimal128(31,4)` array | 0.96-0.97 |
| own `i128` loop on Decimal128 storage | 1.44-1.46 |
| NULLs (10%): arrow / own with a per-row branch / own branch-free | 2.31-2.36 / 2.37-2.39 / 1.77-1.79 |

Ratios, x86-64 (shared VM, 3 runs of one build) against ARM64 (this run):

| ratio | x86-64 | ARM64 |
|---|---:|---:|
| arrow checked i64 / own two passes (gain from owning the primitive kernels) | 1.6-1.8x | 1.4-1.5x |
| own two passes / own one fused pass (gain from fusing) | 1.5-1.7x | 1.8-1.9x |
| arrow checked i64 / own fused (both gains) | 2.5-2.9x | 2.6-2.7x |
| arrow Decimal128 / own narrowed loop building Decimal128 | 3.3-3.4x | 2.8-2.9x |
| arrow Decimal128 / arrow Decimal64 | 2.0-2.4x | 1.1x |
| own per-row-branch NULL loop / arrow with NULLs (above 1 = worse than arrow) | 1.1-1.4x | 1.0x |
| arrow with NULLs / own branch-free NULL loop | 1.4-1.7x | 1.3x |

Reading:
- **The conclusions of ADR 0003 hold on ARM64, with smaller margins.** Fusing and a branch-free NULL strategy still help; arrow's Decimal128 is still the dearest arrow kernel. Owning the primitive kernels is 1.4-1.5x faster than arrow's two checked kernels, which sits right at the ADR's 1.5x replacement threshold, against 1.6-1.8x on x86-64.
- **The large Decimal128 gap seen on x86-64 does not appear:** arrow's Decimal128 is only 1.1x slower than its Decimal64 here (2.0-2.4x on x86-64), and the earlier unexplained build-to-build swing in arrow's Decimal128 time is absent on this machine.
- **The naive per-row-branch NULL loop is no worse than arrow on ARM64** (it was 1.1-1.4x worse on x86-64), so the branch-free requirement is a x86-64 finding; the branch-free loop is still the faster of the two (1.3x).
- 128-bit integer arithmetic is cheap on this CPU (the `i128` loop at 1.45 ns/row against 2.7-3.4 on x86-64).

## Limits

- One machine and one CPU model per architecture, in different environments: the x86-64 numbers come from a shared VM with ±3-5% noise and the ARM64 numbers from a quiet Spot VM. Ratios within a machine are compared; absolute times are not.
- Synthetic data for the kernels (no selection vectors, one expression), one query shape for the pipeline. A Spot VM was never preempted during these runs.
