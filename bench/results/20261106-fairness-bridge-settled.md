# Fairness overhead and the tokio-to-pool bridge on settled machines (2026-11-06)

Runs `cpack-settled-arm64` (`n4a-highcpu-8`, Neoverse-N3, $0.025) and `cpack-settled-x86` (`c3d-highcpu-8`, AMD EPYC 9B14, 4 cores x 2
threads, $0.017), job `cpack`, VM time only. The first runs of these measurements (`bench/results/20261104-cpack-arm64.md`,
`-cpack-x86.md`) started at a 1-minute load of 4 to 8, so by ADR 0006 rule 7 they were sanity only (plan rows 6 and 22). This time the
job waited for a 1-minute load below 0.5 before each measurement (commit `69af56d`); the SETTLE lines (raw `settle-*.log`):

| measurement | ARM64 | x86-64 |
|---|---|---|
| fairness overhead | settled, 0.46 after 145 s | settled, 0.47 after 155 s |
| bridge latency | settled, 0.48 after 160 s | settled, 0.47 after 160 s |
| bridge memory | settled, 0.45 after 55 s | settled, 0.47 after 55 s |
| bridge idle | settled, 0.42 after 0 s | settled, 0.40 after 0 s |

Every step of both runs passed (21 of 21). Raw outputs: `bench/results/raw/20261106-cpack-settled-{arm64,x86}/`.

## 1. Cost of the cooperative time slice (plan item 22, ADR 0002)

Paired wall ratio against the unlimited-slice arm A, 20 rounds; A' is A again (the noise control). **Caveat:** only the first block of the
benchmark started below 0.5; it runs its blocks back to back and its own work lifts the 1-minute average to 0.67-1.44 for the later blocks
(no other process; printed per block in the raw file). By the strict rule the later blocks are sanity only; they agree with the first.

| block (tasks, morsel rows) | arm | ARM64 ratio to A (min..max) | x86-64 ratio to A (min..max) |
|---|---|---|---|
| 2 tasks, 4,096 (settled block) | A' (control) | 0.999 (0.966..1.018) | 0.999 (0.898..1.198) |
| | default (32 morsels / 1 ms) | 1.008 (0.974..1.022) | 1.004 (0.875..1.176) |
| | slice 1 morsel | 1.035 (0.998..1.068) | 0.994 (0.876..1.170) |
| 8 tasks, 4,096 | slice 1 morsel | 1.172 (1.134..1.193) | 1.339 (1.103..1.428) |
| | slice 4 morsels | 1.032 (1.010..1.044) | 1.077 (0.918..1.242) |
| | slice 16 morsels | 1.002 (0.986..1.015) | 1.016 (0.870..1.222) |
| 8 tasks, 65,536 | default | 0.998 (0.981..1.021) | 0.986 (0.971..1.061) |
| 8 tasks, 1,048,576 | default | 1.003 (0.973..1.032) | 1.001 (0.979..1.025) |

Reading. **The default slice (32 morsels or 1 ms) costs nothing measurable** on either machine (0.986 to 1.008, inside the twin's spread).
Yielding after every morsel is what costs: +17% (ARM64) and +34% (x86-64) with 8 tasks on 2 workers and 4,096-row morsels, because a
yield then happens 32,762 times per run; at 16 morsels per slice it is back within noise. x86-64 is noisier (the twin spans 0.90..1.20).

## 2. The bridge (plan item 6 and 7, ADR 0002)

Latency of I/O tasks (microseconds, pooled samples, `lag` = ready to polled):

| arm | ARM64 lag p50 / p99 | x86-64 lag p50 / p99 |
|---|---|---|
| idle | 10 / 18 | 15 / 24 |
| CPU pool saturated | 10 / 18 | 16 / 23 |
| OS threads spinning | 9 / 17 | 16 / 22 |
| tokio saturated, 1 ms tasks | 550 / 1,704 | 538 / 1,713 |
| tokio saturated, 10 ms tasks | 5,157 / 16,888 | 5,335 / 18,384 |

**A saturated CPU pool does not delay I/O** (same lag as idle). A saturated tokio runtime does, by about the length of the blocking tasks
(this is the reason CPU work does not run on tokio). Memory bound: respected in every bounded configuration on both machines (peak
chunks equal the bound exactly, 3 to 50 chunks); the unbounded control grew to 880 MiB (ARM64) and 761 MiB (x86-64). Idle CPU: the real
bridge (waker stored, re-queued by the producer) uses 0.000 CPU seconds per second on both; the known-bad busy re-queue uses 1.26 (ARM64)
and 1.98 (x86-64), so the measurement can see the bug. Stress (`bridge-stress.txt`) and the unit tests passed on both.

## What this does not show

Two pool workers and 8 vCPUs only; a synthetic workload (the fairness spike's sums and the bridge's chunk producers), not a real query mix;
one run per machine; the later fairness blocks are sanity only by the strict settle rule (see the caveat above).
