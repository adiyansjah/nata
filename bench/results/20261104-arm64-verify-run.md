# Clean-machine reproduction of the verification on ARM64 (job `verify`)

Run 2026-11-04 on a GCP Spot VM, `n4a-highcpu-8` (8 vCPU Neoverse-N3, 16 GB, Debian 12 arm64, kernel 6.1), us-central1-a. Source uploaded as a tarball of the working tree (uncommitted changes included), Rust installed from `rust-toolchain.toml` (1.99.1), DuckDB 2.0.0. The job is `verify` in `scripts/gcp/job.sh`; each step stops the job on failure.

## Runs and cost (estimates from catalog prices; the bill was not checked)

| run id | result | VM time | VM cost |
|---|---|---:|---:|
| `verify-arm64` | failed at the lint fixtures (no default toolchain on the VM) | 212 s | 0.006 USD |
| `verify-arm64-b` | failed at the lint fixtures (`--offline` could not resolve crates the workspace build never downloaded) | 237 s | 0.006 USD |
| `verify-arm64-c` | **finished, 28 steps, 0 failures** | 767 s | 0.021 USD |

Each run had its own approval and a cap of 0.20 USD; nothing was left on GCP after any of them.

## What the two failed runs found (both: "works only on the author's machine")

1. The fixture runners ran `cargo clippy` in scratch directories outside the repository. `rustup` does not see the repository's `rust-toolchain.toml` there, and the VM has no default toolchain, so all 73 fixtures reported 'no default is configured'. Fix: the runners pin the repository's channel via `RUSTUP_TOOLCHAIN`.
2. Then every fixture failed with a resolution error in `--offline` mode: the fixtures need arrow crates that the workspace build had not downloaded. Reproduced locally with an empty `CARGO_HOME` (0 of 73), fixed by making `--offline` opt-in (`NATA_OFFLINE=1`), confirmed locally on an empty registry (73 of 73 in 54 s) before the next run.

## Results of the finished run

| step | result |
|---|---|
| `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace` | pass |
| lint fixtures | 73 of 73 fail with the expected lint; 10 of 10 controls clean; 5 of 5 known gaps still pass clippy |
| guarded-expression differential against DuckDB | 81 of 81 identical (42 result sets, 39 errors), 0 different |
| guard mutations | 22 of 22 caught, 0 survived |
| scheduler tests (default build) | pass |
| loom model on the fixed scheduler | 9 of 9 pass (100 s) |
| loom, 5 deliberate bugs | each caught: a deadlock for `bug-lost-shutdown`, `bug-wake-ignored-while-running`, `bug-park-ignores-notified` and `bug-cancel-does-not-wake`; the never-run-twice assertion (`src/sched.rs:289`) for `bug-wake-enqueues-running` |
| generic-ops with `overflow-checks` on | `check` and `nullcheck` give the same results; nothing panics |

The loom and lint results are architecture-independent (the model checks SeqCst semantics; the lints are source-level), so this is a clean-machine reproduction of the local result, not new ARM64 evidence.

## Cost of `overflow-checks` on the generic-operator experiment (ADR 0003 item 21)

The same code built with and without `[profile.release] overflow-checks = true`, benchmarked as 2 alternations of off, on (25 rounds, interleaved arms with a control, the two binaries alternated). The two alternations agree to within 1 ms in every cell. Load average at the start of each benchmark was 2.3-3.3 (it still included the preceding builds), so these are quiet-machine numbers but not perfectly idle.

Engine only (batches in memory), 1 thread, min ms:

| query | generic off | generic on | fused off | fused on | generic/fused off | generic/fused on |
|---|---:|---:|---:|---:|---:|---:|
| Q1 | 61 | 63-64 | 31 | 38 | 1.96 | 1.66 |
| Q6 | 13-14 | 13-14 | 19-20 | 21 | 0.74-0.76 | 0.72 |

End to end (Parquet decode included), 1 thread: Q1 generic 121-122 off, 125 on; fused 95 off, 103 on; ratio 1.28 off, 1.21 on. Q6 is unchanged at 0.85. At 8 threads Q1 end to end: ratio 1.20 off, 1.15 on.

Reading: with the checks on, the generic engine slows by about 3-5% and the hand-fused loop by 8-22% (it does more checked arithmetic per row), so the cost of generality shrinks slightly. The kernel micro-benchmark's 'sum about 2x slower' (`bench/results/20261104-kernel-lints-and-guards.md`) does not carry over to this query shape. One architecture, one data set, two queries; x86-64 not measured.

## Limits

ARM64 N3 only. The `kani`, `hashops` and `decimal` jobs were not part of this run. Costs are estimates.
