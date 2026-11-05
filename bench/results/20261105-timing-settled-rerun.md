# Settled re-run of the three blocks that missed the 0.5 settle rule (2026-11-05)

Runs `timing-arm64-r3` (`n4a-highcpu-8`, Neoverse-N3, 8 vCPU) and `timing-x86-r3` (`c3d-highcpu-8`, AMD EPYC 9B14, 8 vCPU), job `timing`, code commit
`2084acb` (settle before every timed block, `scripts/gcp/settle.sh`). Each: 42 steps, 1 incarnation, 0 preemptions, resources left after cleanup: none; ARM64
4,497 s of VM time, **$0.123**; x86-64 5,188 s, **$0.099**. Raw: `bench/gcp-runs/timing-{arm64,x86}-r3/results/` (not in git).

**Settle record.** Every timed phase of the three blocks started after a `SETTLE ... settled` line, none said NOT SETTLED:
NULL-path 13 of 13 phases on each machine (loads 0.46 to 0.49); push-cost start load 0.46 (ARM64) and 0.47 (x86-64), 52 of 52 cells labelled `quiet` on both
(other processes used 0.00 to 0.03 cores); Arrow end-to-end 0.47 / 0.47 and engine-only 0.48 / 0.46 (ARM64 / x86-64). The earlier runs had 1.4-2.0 (NULL-path
phases after the first), 0.72 and 0.83 (push-cost) and 1.47 (Arrow engine-only). **Caveat:** the machine idles at a 1-minute load of about 0.47, just under the
limit, and the 5-minute averages printed by the Arrow benchmark were 1.4 to 2.0 (the step before it was heavy); the rule in ADR 0006 is the 1-minute load only.
The hash-ops, perf and compaction blocks were run again as part of the bundle and are NOT compared here: the earlier values stand until someone does.

## 1. The word-wise NULL path (plan item 16)

Penalty = time with NULLs / time of the same engine with none; Q1, engine only (batches in memory), CPU time, 25 rounds, one thread.

| machine | NULLs | old engine | word-wise step 5 | word-wise step 6 |
|---|---:|---:|---:|---:|
| ARM64 | 1% | 2.29x | 1.06x | 1.06x |
| ARM64 | 10% | 2.15x | 1.15x | 1.13x |
| ARM64 | 50% | 1.54x | 1.28x | 1.24x |
| x86-64 | 1% | 2.15x | 1.04x | 1.07x |
| x86-64 | 10% | 2.01x | 1.14x | 1.13x |
| x86-64 | 50% | 1.40x | 1.27x | 1.21x |

Four threads on x86-64: step 5 1.04x / 1.12x / 1.25x, step 6 1.05x / 1.12x / 1.19x (1% / 10% / 50%). Q6 at 10% NULLs: ARM64 old 2.79x, step 5 1.05x, step 6 1.05x;
x86-64 old 2.36x, step 5 1.09x, step 6 1.07x. **The 1.3x target is met at 1%, 10% and 50% NULLs on both machines.** ARM64 matches the earlier values to within 0.01x
(they were right though labelled sanity). x86-64 differs from the earlier sanity values (step 5 at 50%: 1.30x then, 1.27x now; step 6: 1.23x then, 1.21x now). Margins are
small for step 5: the x86-64 interquartile range at 50% NULLs is [1.23, 1.32] with one thread, so a single round can exceed 1.3x; step 6 stays below it (upper quartile 1.25x).

## 2. Push cost (plan item 5)

Linear chain against the fused hand-written control, one thread unless stated (overhead % over the control; `dyn` = the real scheduler with a `Box<dyn Operator>` per stage; the
gap is `dyn` minus `static`, in ns per batch for one thread, in percentage points for four). Cheap kernel:

| chain | rows | ARM64 static / dyn | gap | x86-64 static / dyn | gap |
|---|---:|---|---:|---|---:|
| 1 stage, 1 thread | 1,024 | +24 / +45 % | 71 ns | +33 / +51 % | 64 ns |
| 1 stage, 1 thread | 8,192 | +76 / +80 % | 86 ns | +26 / +30 % | 107 ns |
| 3 stages, 1 thread | 1,024 | +110 / +231 % | 396 ns | +139 / +176 % | 122 ns |
| 3 stages, 1 thread | 8,192 | +178 / +176 % | noise | +121 / +130 % | 238 ns |
| 1 stage, 4 threads | 1,024 | +3 / +25 % | 23 points | +29 / +94 % | 65 points |
| 1 stage, 4 threads | 8,192 | +68 / +74 % | 7 points | +27 / +74 % | 47 points |
| 1 stage, 4 threads | 65,536 | +48 / +51 % | 3 points | +46 / +94 % | 48 points |

Readings. (1) `dyn-direct` equals `static` in the cheap cells on both machines (virtual dispatch itself costs nothing). (2) **ARM64 reproduces the earlier record closely** (static +24%, dyn +45% for one
cheap stage at 1,024 rows; the exchange cells below match to within a few points, except that the slower cells now read up to +35% where the earlier record had +29%). (3) **New, x86-64 only: the scheduler's cost does not disappear into the noise at large batches with four threads.** On
ARM64 the gap shrinks from 23 points at 1,024 rows to 3 at 65,536; on x86-64 it stays at 47 to 65 points. With one thread it is small on both (64 to 238 ns per batch). The cause was not investigated (cross-thread wake-ups are a candidate; so is the machine: a `c3d-highcpu-8` is 4 cores with 2 hardware threads each, so 4 worker threads can share cores, while the `n4a` has one thread per core; both untested). The earlier statement "disappears into the noise from
8,192 rows" holds for ARM64 and for one thread, not for x86-64 with four threads. (4) The medium-kernel 65,536-row `dyn` cells are an outlier on both machines (+110% to +422%, the per-stage
vector allocation noted before), and cheap chains of 3 and 6 stages are dominated by the lack of fusion, not by the protocol.

The real exchange (1 producer, 2 consumers) against a hand-written blocking queue (negative = the exchange is faster):

| kernel | rows | capacity | ARM64 | x86-64 |
|---|---:|---:|---:|---:|
| cheap | 1,024 | 1 | -76% | -68% |
| cheap | 1,024 | 4 | +8% | +14% |
| cheap | 8,192 | 1 / 4 | +4% / +13% | +19% / +20% |
| cheap | 65,536 | 1 / 4 | +0.7% / +0.2% (noise) | +10% / +9% (noise) |
| medium | 1,024 | 1 | -54% | -23% |
| medium | 1,024 | 4 | +35% | +17% |
| medium | 8,192 | 1 / 4 | +6% / +5% | +7% / +9% |

The exchange wins when a hand-written queue pays a thread wake-up per batch (capacity 1, 1,024 rows) and is 4 to 35% slower in the other small-batch cells; it is indistinguishable from the queue at
65,536 rows on ARM64, and within the noise at best on x86-64. Same pattern as before; x86-64 is the worse machine for the 8,192-row cells.

## 3. Arrow pipeline (ADR 0001): protocol scheduler against the generic-ops engine

Correctness gate first (tests and the DuckDB diff, digit for digit) passed on both. Ratio protocol / generic-ops, median [q1, q3], 25 rounds; the control column (protocol again / protocol) has medians of
0.96 to 1.04 everywhere, i.e. the noise floor is a few percent.

| block | query | threads | ARM64 | x86-64 |
|---|---|---:|---|---|
| end to end | Q1 | 1 / 2 / 4 / 8 | 1.01 / 1.01 / 1.01 / 1.05 | 0.99 / 1.02 / 1.13 / 1.06 |
| end to end | Q6 | 1 / 2 / 4 / 8 | 1.01 / 1.01 / 1.03 / 1.12 | 1.01 / 1.00 / 1.13 / 1.11 |
| engine only | Q1 | 1 / 2 / 4 / 8 | 1.01 / 1.01 / 1.02 / 1.09 | 1.01 / 1.00 / 1.14 (noisy) / 1.04 |
| engine only | Q6 | 1 / 2 / 4 / 8 | 1.02 / 1.02 / 1.04 / 1.17 | 1.02 / 1.02 / 1.22 / 1.10 |

The protocol scheduler is within about 1 to 4% of the generic engine up to two threads on both machines and 3 to 22% slower at four and eight threads (Q6 engine-only at 4 threads on x86-64: 1.22x; at
8 threads on ARM64 1.17x; these runs take 2 to 19 ms and sit near the noise). It agrees with the push-cost finding above: the cost of the protocol shows up with several threads, more on x86-64.
ARM64 matches the earlier values (engine-only Q1 1.01 to 1.02 at 1 to 4 threads; Q6 1.02 to 1.04, before 1.03 to 1.08, within noise).

## What this does not show

One VM per architecture, one run each; 25 rounds per cell. No hardware counters, so the reason for the x86-64 four-thread cost is open. The hash-ops, perf and compaction results of this bundle are not compared
with the earlier ones. The machine idled at about 0.47 load, close to the 0.5 limit.
