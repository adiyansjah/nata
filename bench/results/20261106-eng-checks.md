# Engineering checks on the real crates (plan item 34, 2026-11-06)

Runs `eng-checks-x86` ($0.059) and `eng-checks-x86-r2` ($0.063), job `eng-checks`, one `c3d-highcpu-8` Spot VM each (AMD EPYC 9B14),
code `8514d59`+ and `d9e99e5`+ (the second with the two fixes below). Raw: `bench/results/raw/20261106-eng-checks-x86{,-r2}/`.
Crates under test: `nata-sched`, `nata-parquet`, `nata-scan`, `nata-kernels` (the scan and the kernels of slices 2 and 3).

| check | first run | fix | second run |
|---|---|---|---|
| `cargo-audit` (RustSec, 142 crates in Cargo.lock) | no vulnerability | - | no vulnerability |
| `cargo-deny` (`deny.toml`: licenses, bans, sources, advisories) | FAIL: 3 "wildcard" path dependencies between workspace crates | the crates are marked `publish = false` and `allow-wildcard-paths = true` (`c6e4005`); also run in CI now | advisories ok, bans ok, licenses ok, sources ok |
| AddressSanitizer, every test of the four crates (`-Zbuild-std`, nightly-2026-08-21) | FAIL in one test: the fuzz test counted 3 live threads where 2 were expected | a test race, not a scan defect: the test's own per-case helper thread was not joined before the count; it is joined now (`0119e67`); no AddressSanitizer report in either run | all 25 test binaries pass, no report |
| ThreadSanitizer, the same tests (including the 5,000 cancel scenarios, the loom-model crate's std tests and the parallel differential) | pass, 0 warnings | - | pass, 0 warnings |
| Miri: `nata-scan`'s 35 pruning rule tests, `nata-kernels`' 20 unit tests | pass, no undefined behaviour | - | pass, no undefined behaviour |
| unsafe inventory (`scripts/unsafe-inventory.py`, textual, an upper bound) | 0 `unsafe` in the workspace crates; 8,183 keywords in 56 of 78 dependency packages of `nata`; 0 in the trees of `nata-sched` and `nata-scan` | - | same |

Duplicate versions reported as warnings (not errors): `getrandom` (3), `syn` (2), `r-efi` (2).

What this does not show: Miri on the integration tests of `nata-parquet` (Parquet decoding under Miri is far too slow) and on property tests
with many cases; sanitizers on ARM64; MemorySanitizer (needs every dependency rebuilt with it); `cargo geiger`'s own numbers (not installable
offline; the inventory is textual). The benchmark regression harness of the card is a slice 4 deliverable and is not part of these runs.
