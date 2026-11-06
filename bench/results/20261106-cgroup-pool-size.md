# The scan's worker pool under cgroup CPU limits (plan item 9, 2026-11-06)

Run `cgroupcheck-arm64` (job `cgroupcheck`, `n4a-highcpu-8`, Neoverse-N3, 8 vCPU, cgroup v2, $0.006), code `77a7aca`, script
`scripts/cgroup-check.sh`. `nata scan --count` WITHOUT `--threads` (so the pool takes its default, `std::thread::available_parallelism`) on
TPC-H SF1 `lineitem`, started inside a systemd scope with a CPU quota (cgroup v2 `cpu.max`), under an affinity mask, or both; the pool size
is read from the scan's own report. Raw: `bench/results/raw/20261106-cgroupcheck-arm64/`.

| limit | pool threads | allowed | result |
|---|---:|---|---|
| none | 8 | all 8 CPUs | PASS |
| quota 100% | 1 | 1 | PASS |
| quota 200% | 2 | 2 | PASS |
| quota 350% | 3 | 3 or 4 | PASS (rounded down) |
| quota 600% | 6 | 6 | PASS |
| affinity mask of 3 CPUs (`taskset -c 0-2`) | 3 | 3 | PASS |
| quota 200% and a mask of 6 CPUs | 2 | 1 or 2 | PASS (the smaller limit wins) |

The row count was 6,001,215 in every case. The check can fail: the report gives the pool's real size (an explicit `--threads 4` under a
100% quota reports 4), so a pool sized to the machine's cores would report 8 under every quota. **The default pool respects cgroup CPU
quotas and affinity masks (rounding a fractional quota down).** ADR 0002's "sized to the available cores (cgroup aware)" holds for the
scan's scheduler.

What this does not show: cgroup v1 (Debian 12 and 13 use v2); a quota that changes while the process runs (read once at start); a
container runtime's own limits beyond cpu.max and the mask; x86-64 (the mechanism is the kernel's and std's, not the architecture's).
