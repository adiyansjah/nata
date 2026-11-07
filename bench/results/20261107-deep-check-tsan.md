# 0004 F: TSan closeout

**Verdict: MET; R15 closed with the card's accepted Tokio I/O trust point.**
Full clean-VM workspace TSan exits 0; the planted Nata race is still reported.
No production code, dependency, limit or detector setting changes except the
suppression path.

**Rule (card 0004 F, unchanged):** "A TSan suppression file that names only
Tokio's epoll registration and readiness frames, with a written reason: the
kernel orders the `epoll_ctl`/`epoll_wait` handoff, and TSan does not model it.
Accepted as a trust point, like R10. No Nata frame may be suppressed"

**Check (unchanged):** "A race planted in Nata code is still reported with the
suppressions on (the known-bad version). The full TSan workspace run exits 0 on
a clean x86-64 VM"

**Measured numbers:**

| Check | Reports | Exit |
|---|---:|---:|
| Historical full TSan workspace, no suppression (`20261107-debt-a.md`) | 7 | 101 (affected binaries 66) |
| Fresh clean-VM full TSan workspace, suppression on: **55 binaries, 414 passing tests** | **0** | **0** |
| Local Tokio-only, no suppression | 5 | 66 |
| Local Tokio-only, suppression on | 0 | 0 |
| Local planted Nata test, suppression on | 1 | 66 (cargo and test process) |
| Clean-VM Tokio-only, no suppression | 4 | 66 |
| Clean-VM Tokio-only, suppression on | 0 | 0 |
| Clean-VM planted Nata test, suppression on | 1 | 66 (cargo and test process) |

Local trial uses only the existing Tokio-only reproducer, instrumented std and
the standalone planted test; never a local TSan workspace build. The complete
control gate exits 0 only for the expected failing/succeeding processes.

`scripts/tsan-suppressions.txt` contains one entry:
`race:tokio::runtime::io::driver::dispatch_event`. All seven historical reports
contain that readiness frame: `ScheduledIo::set_readiness`/`wake` versus
`RegistrationSet::allocate` -> `Handle::add_source` ->
`Registration::new_with_interest_and_handle`. Suppressing only this shared frame
avoids extra allocation/registration patterns. No Nata, std or wildcard entry.
**Reasoned trust point:** the kernel orders ScheduledIo pointer publication
through epoll; TSan does not model that handoff. The no-Nata Tokio program reproduces the reports.
[Tokio discussion 4746](https://github.com/tokio-rs/tokio/discussions/4746) shows
the matching report; it is not an upstream proof that Tokio is race-free. This
is the card's accepted trust point, like R10, with a finite known-bad control.

The workspace has `unsafe_code = "forbid"`. `scripts/tsan-planted/` is a separate,
dependency-free, test-only workspace, never linked into Nata. Its contents are
compiled only under `cfg(all(test, sanitize = "thread"))`. Two threads cross a
start barrier then write the same `UnsafeCell<u64>` without synchronization.
The deliberately wrong `Sync` impl and volatile writes are confined to that
test. TSan reports both accesses through
`<nata_tsan_planted::DeliberateRace>::nata_unsynchronized_write`
(`scripts/tsan-planted/planted.rs:17`), with summary
`core::ptr::write_volatile::<u64>` and test-process exit 66, despite the active
suppression. Volatile preserves the writes; it does not order them.

**Reproduce** (on-disk scratch; controls are cheap, workspace mode belongs on Spot):

```bash
mkdir -p "$HOME/tmp-0004f"
TMPDIR="$HOME/tmp-0004f" scripts/gcp/test/test-job-chains.sh
TMPDIR="$HOME/tmp-0004f" bash scripts/gcp/test/test-tsan-wiring.sh
TMPDIR="$HOME/tmp-0004f" WORK="$HOME/tmp-0004f" CARGO_BUILD_JOBS=2 scripts/gcp/tsan-closeout.sh controls
```

The `heavy` job ships this gitignored `heavy.steps`:

```text
tsan-tools rustup toolchain install nightly-2026-08-21 --profile minimal --component rust-src
tsan-controls scripts/gcp/tsan-closeout.sh controls
tsan-workspace scripts/gcp/tsan-closeout.sh workspace
```

`workspace` uses the eng-checks fixture/native RSS companion and exact command:
`NATA_INSTRUMENTED=1 NATA_RSS_BINARY="$WORK/target-rss/release/nata" RUSTFLAGS="-Zsanitizer=thread" RUSTDOCFLAGS="-Zsanitizer=thread" CARGO_TARGET_DIR="$WORK/target-thread" cargo +nightly-2026-08-21 test -Zbuild-std --target x86_64-unknown-linux-gnu --workspace --lib --tests --no-fail-fast`.
`TSAN_OPTIONS` preserves existing options and appends
`suppressions=$SRC/scripts/tsan-suppressions.txt`. Automatic suppression wiring
is confined to eng-checks' thread step; address/Miri/other jobs are unchanged.
The standalone closeout runner explicitly enables it for these measurements.
`heavy` avoids repeating already-certified ASan, Miri and audits. A failing
planted process is required; a failing workspace process fails its heavy step.
Read the `.exit` files: the generic heavy job itself continues after failures.

## GCP run and identity

2026-11-07, Debian 12, us-central1-a, x86-64 Spot; no service account or scopes.
Plans precede creation. First plan: `nata-0004f-tsan`, c3d-highcpu-32, 1.1 h,
$0.330 worst case. GCP rejects the nonexistent machine type; **no VM created**.
Stopped immediately; its 100 GiB labeled data disk was deleted. Status lists
no instances or disks. This consumes the initial attempt; exactly one retry.

The retry `nata-0004f-tsan2` uses verified c3d-highcpu-16 (16 vCPU, 32 GiB RAM),
20 GiB boot + 100 GiB data, two VM-hours maximum, at most one preemption.
CPU-bound builds need 16 cores; the data disk holds both native and TSan targets.
Printed worst case **$0.320**, below the owner's **$0.35** cap. Prices are the
repo's 2026-11-04 Cloud Billing Catalog rates; today's refresh returned HTTP 429,
so they are not freshly confirmed. Compute is an estimate, not a billing invoice;
disk and ephemeral IPv4 are included in the plan, egress assumed negligible.

```bash
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 2 --data-gb 100 --max-preemptions 1 --run-id nata-0004f-tsan2
TMPDIR="$HOME/tmp-0004f" PULL_SECONDS=30 scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-16 --max-vm-hours 2 --data-gb 100 --max-preemptions 1 --run-id nata-0004f-tsan2 --approved-usd 0.35
scripts/gcp/orchestrate.sh status --run-id nata-0004f-tsan2
```

Measured code tree (`scripts/code-tree.sh`; source/tests/scripts/manifests/lock,
docs and results excluded):
`238c8be94665c5176c3883ceb2b306f52a0550be496ae9354902f4f67a0ab0df`.
The clean VM's `results/code-tree.txt` and an independent SSH read after all
compilation match this hash. The code is frozen before shipment; the committed
code matches it.

**Measured GCP outcome:** finished, **1 VM, 0 preemptions, 1,642 counted VM
seconds (27 min 22 s)**. Compute estimate **$0.063** at $0.1374/h. A rough
compute + boot/data disk + ephemeral IPv4 estimate is **$0.072**, including
about $0.0002 for the first attempt's disk; this is not a billing invoice.
All three heavy-step `.exit` files are 0; workspace exit is independently 0.
Cleanup verified at **2026-11-07 05:04:27 UTC**. Subsequent
`orchestrate.sh status --run-id nata-0004f-tsan2` prints only `instances:` and
`disks:`, with no rows; the first run's status is empty too. Only run-labeled
instances/disks were managed; no bucket or IAM operations. Raw evidence remains
in this worktree's gitignored `bench/gcp-runs/nata-0004f-tsan2/results/`
(`workspace.txt`, `planted.txt`, `tokio-unsuppressed.txt`, `tokio-suppressed.txt`, `*-exit.txt`,
`*.exit`, `code-tree.txt`); local trial logs are in `$HOME/tmp-0004f/`.

**Checks: PASS.** Workspace and planted-crate `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, job-chain test, TSan
wiring test, the TSan controls and `scripts/check-leaks.sh --staged`.
