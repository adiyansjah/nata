# ARM64 run 1: workspace tests and the operator-protocol suite on a Spot VM

First real run of `scripts/gcp/orchestrate.sh` (`--job ci`). Run id `nata-run1`, 2026-11-03 21:56-22:00 UTC (2026-11-04 local).

## Machine, place and cost (recorded as `way-of-working.md` section 8 asks)

| | |
|---|---|
| Machine type | `n4a-highcpu-8`, Spot, one VM (the fallback chain was `n4a-highcpu-8`, then `c4a-highcpu-8`; the first was used) |
| Zone | us-central1-a |
| CPU | `Neoverse-N3`, `aarch64`, 8 vCPU, 15 GiB RAM; L2 16 MiB (8 instances), L3 112 MiB |
| OS | Debian 12, kernel 6.1.0-54-cloud-arm64 |
| Disks | 20 GB Hyperdisk Balanced boot (deleted with the VM), 30 GB Hyperdisk Balanced data disk (deleted at the end) |
| Approved cap / worst-case bound | 0.20 USD / 0.197 USD |
| VM time counted | 183 seconds; 1 VM, 0 preemptions |
| Estimated cost | about 0.005 USD for the VM (0.0984 USD/h x 183 s), plus a fraction of a cent for disks and the external IP; **estimated from the catalog prices, not from the bill**, which was not checked |
| Left behind | nothing: the orchestrator listed all resources labelled `nata-run=nata-run1` after cleanup and found none |

Correction: the CPU of N4A is a **Neoverse N3**, observed on this machine. Earlier in this work N4A was described as the same Axion core as C4A (Neoverse V2). That was wrong; C4A's core was not observed and remains unverified.

## What ran, and the result

Source: the working tree at the time (uploaded once). Rust 1.99.1 from `rust-toolchain.toml`, built on the VM.

| step | result |
|---|---|
| `cargo test --workspace` (debug) | pass: 2 + 17 + 3 tests (`nata`, `nata-check` unit tests, `nata-check` fake-engine tests) |
| operator-protocol suite (`spikes/operator-trait`, release), run **6 times** | pass in all 6: 15 tests in `protocol.rs` (join, `LIMIT`, blocking exchange, 864-query stress, load smoke test, spill, cancellation, error) plus the deterministic shutdown-race test in `shutdown.rs`; no failure, panic or hang in any log |

Step durations (UTC, from the step markers): toolchain 6 s, DuckDB 1 s, workspace tests 4 s, each protocol-suite run about 15 s (13.7 to 14.5 s inside `cargo test`). The same suite takes about 6 s on the local x86-64 VM; the two machines differ in CPU, clock and noise, so this is not a speed comparison.

## What this changes

- **ARM64 correctness evidence** for the operator protocol: the scheduler, wakers, exchange, spill and the deterministic shutdown test pass on a weaker-memory-order machine, 6 of 6 repetitions. This is modest evidence: one machine, one CPU model, six repetitions, and no extra CPU contention beyond the suite's own.
- **Real-GCP facts established** (previously only assumed): Spot creation of an N4A VM works in this project despite `PREEMPTIBLE_CPUS = 0` and no N4A quota entry; Hyperdisk boot and data disks work; the startup script mounts the data disk; SSH with the throwaway instance key works; the upload, resume markers, state copy-back and label-based cleanup all work end to end; boot to finished took about 3 minutes.
- **Still not exercised on real GCP:** a real preemption (none happened), the machine fallback chain, moving zones, `kill` mid-run, and C4A. These are covered only by the fake-`gcloud` tests.
