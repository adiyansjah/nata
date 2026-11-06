# Slice 4, step 4b: late materialization, footers ahead and page CRC (2026-11-06)

Job `slice4bench`, source commit `2914c5e`, script `crates/nata-bench/scan-bench.sh`, blocks `late footers crc`, five alternating rounds per arm with a twin control. Clean Debian 12 Spot VMs in `us-central1-a`, 8 vCPU and about 16 GB RAM each. ARM64 is Neoverse-N3 (8 cores); x86-64 is AMD EPYC 9B14 (4 cores, 2 threads/core). Rust 1.99.1 (repository toolchain), Arrow/Parquet 60.1.0, DuckDB 2.0.0 CLI, PyArrow 26.0.1. No pushes.

| run id | machine | counted VM duration | orchestrator VM cost estimate | approved / planned worst case | cleanup |
|---|---|---:|---:|---:|---|
| nata-1106-235337-d218 | n4a-highcpu-8 | 893 s (14m53s) | $0.024 | $0.14 / $0.138 | report: none left; separate status: empty instances and disks |
| nata-1106-235342-d3bf | c3d-highcpu-8 | 1152 s (19m12s) | $0.022 | $0.09 / $0.080 | report: none left; separate status: empty instances and disks |

The cost estimates are **VM-only**, at the last machine's rate times counted seconds; they exclude disks and IP. Each run used one VM, zero preemptions, eight completed steps. Plans were run immediately before the respective detached orchestrators; output logs are outside git under the operator's home. No retries were needed. Only run-labelled resources were used and removed. The unrelated bucket and service account were untouched.

Raw, redacted: `bench/results/raw/20261106-slice4bench-{arm64,x86}/` (CSV, generated summary, settle record, run output, machine record and orchestrator report). Project lines were removed; no keys, addresses or cloud identity are copied. Both benchmark outputs end `exit status: 0`; all 220 timed invocations per CPU returned DuckDB's row counts and exit 0. Each of the 13 settle points settled below 0.5 (none NOT SETTLED); starting one-minute loads are 0.42–0.48 ARM64 and 0.46–0.49 x86-64. The raw settle records also retain five- and fifteen-minute loads, which can be higher.

## Measurement limits

Wall time includes process start, footer reads, decoding, filtering and counting; SQL also includes binding, evaluation and CSV rendering into an `awk` row count. Local VM disk, warm page cache (explicit warm-ups), eight threads, window 16. SF1 has 6,001,215 lineitem rows; SF10 has 59,986,052. Seven Q1 columns, no aggregation. `pk` filters `l_partkey` (not projected); `sd` filters projected `l_shipdate`. Late arms disable pruning, isolating decoding. `10` and `100` are selectivity percentages, with exact quantile-derived counts checked against DuckDB. `twin` repeats early (late block), ahead1 (footers scan), old (footers SQL), or verifying (CRC).

Footers: 1,000 files, 2,048 rows/file, one row group/file; projection `l_orderkey,l_extendedprice`, predicate `l_quantity < 2`. The SQL new/old arms change BOTH binding and footer concurrency; their speed-up cannot be attributed to P10 alone. CRC: PyArrow copies lineitem with page checksums; compare the same source built with then-default `page-crc` and with `--no-default-features`, alternating. Plain-file arms are the no-checksum control. Checksummed and original files differ in layout; only comparisons WITHIN a block isolate CRC verification.

Times below are median [min, max] seconds of five rounds. The full spread, including twins, is retained rather than treating median-only verdict lines as a noise analysis.

## late

| block | arm | ARM64 median [min, max] s | x86-64 median [min, max] s |
|---|---|---:|---:|
| late-sf1-pk-10 | early | 0.037 [0.036, 0.039] | 0.061 [0.060, 0.062] |
| late-sf1-pk-10 | late | 0.037 [0.035, 0.040] | 0.063 [0.062, 0.066] |
| late-sf1-pk-10 | twin | 0.038 [0.037, 0.039] | 0.061 [0.060, 0.061] |
| late-sf1-pk-100 | early | 0.045 [0.044, 0.047] | 0.079 [0.073, 0.088] |
| late-sf1-pk-100 | late | 0.044 [0.044, 0.045] | 0.081 [0.074, 0.082] |
| late-sf1-pk-100 | twin | 0.046 [0.044, 0.048] | 0.079 [0.076, 0.081] |
| late-sf1-sd-10 | early | 0.026 [0.025, 0.029] | 0.040 [0.040, 0.041] |
| late-sf1-sd-10 | late | 0.034 [0.033, 0.035] | 0.062 [0.062, 0.063] |
| late-sf1-sd-10 | twin | 0.026 [0.025, 0.027] | 0.040 [0.040, 0.044] |
| late-sf1-sd-100 | early | 0.036 [0.034, 0.041] | 0.069 [0.065, 0.076] |
| late-sf1-sd-100 | late | 0.041 [0.037, 0.043] | 0.073 [0.064, 0.077] |
| late-sf1-sd-100 | twin | 0.037 [0.034, 0.045] | 0.070 [0.068, 0.074] |
| late-sf10-pk-10 | early | 0.232 [0.231, 0.233] | 0.415 [0.413, 0.418] |
| late-sf10-pk-10 | late | 0.274 [0.233, 0.279] | 0.466 [0.453, 0.492] |
| late-sf10-pk-10 | twin | 0.232 [0.226, 0.233] | 0.415 [0.412, 0.421] |
| late-sf10-pk-100 | early | 0.240 [0.231, 0.242] | 0.399 [0.388, 0.417] |
| late-sf10-pk-100 | late | 0.272 [0.259, 0.283] | 0.443 [0.390, 0.498] |
| late-sf10-pk-100 | twin | 0.240 [0.219, 0.254] | 0.406 [0.377, 0.408] |
| late-sf10-sd-10 | early | 0.143 [0.136, 0.148] | 0.239 [0.236, 0.244] |
| late-sf10-sd-10 | late | 0.196 [0.179, 0.198] | 0.388 [0.351, 0.422] |
| late-sf10-sd-10 | twin | 0.137 [0.136, 0.138] | 0.237 [0.234, 0.238] |
| late-sf10-sd-100 | early | 0.158 [0.155, 0.184] | 0.323 [0.274, 0.337] |
| late-sf10-sd-100 | late | 0.206 [0.204, 0.207] | 0.377 [0.367, 0.385] |
| late-sf10-sd-100 | twin | 0.158 [0.152, 0.180] | 0.316 [0.300, 0.328] |

## footers

| block | arm | ARM64 median [min, max] s | x86-64 median [min, max] s |
|---|---|---:|---:|
| footers-scan | ahead1 | 0.040 [0.040, 0.041] | 0.064 [0.064, 0.064] |
| footers-scan | ahead16 | 0.024 [0.023, 0.024] | 0.033 [0.032, 0.034] |
| footers-scan | ahead4 | 0.024 [0.023, 0.025] | 0.033 [0.033, 0.034] |
| footers-scan | ahead8 | 0.024 [0.022, 0.025] | 0.033 [0.032, 0.034] |
| footers-scan | twin1 | 0.041 [0.039, 0.041] | 0.064 [0.063, 0.064] |
| footers-sql | new | 0.026 [0.026, 0.028] | 0.037 [0.036, 0.038] |
| footers-sql | old | 0.055 [0.055, 0.056] | 0.089 [0.088, 0.089] |
| footers-sql | twin | 0.055 [0.054, 0.055] | 0.089 [0.089, 0.090] |

## crc

| block | arm | ARM64 median [min, max] s | x86-64 median [min, max] s |
|---|---|---:|---:|
| crc-sf1-ck | crc | 0.057 [0.056, 0.060] | 0.095 [0.094, 0.100] |
| crc-sf1-ck | nocrc | 0.054 [0.053, 0.056] | 0.091 [0.090, 0.094] |
| crc-sf1-ck | twin | 0.057 [0.055, 0.060] | 0.095 [0.092, 0.097] |
| crc-sf1-plain | crc | 0.038 [0.034, 0.049] | 0.069 [0.066, 0.071] |
| crc-sf1-plain | nocrc | 0.037 [0.034, 0.039] | 0.069 [0.067, 0.073] |
| crc-sf1-plain | twin | 0.038 [0.036, 0.040] | 0.068 [0.066, 0.074] |
| crc-sf10-ck | crc | 0.397 [0.392, 0.399] | 0.713 [0.696, 0.732] |
| crc-sf10-ck | nocrc | 0.378 [0.373, 0.383] | 0.701 [0.684, 0.705] |
| crc-sf10-ck | twin | 0.397 [0.382, 0.407] | 0.723 [0.713, 0.725] |
| crc-sf10-plain | crc | 0.152 [0.142, 0.173] | 0.308 [0.285, 0.313] |
| crc-sf10-plain | nocrc | 0.164 [0.138, 0.169] | 0.311 [0.297, 0.317] |
| crc-sf10-plain | twin | 0.166 [0.160, 0.169] | 0.311 [0.292, 0.319] |


## Decisions against the fixed card (part 5)

**P1 — leave off; retain the experimental switch.** Rule: “Keep it only if, on SF1 and SF10 and both CPU types, it is at least 1.2x faster than decoding every projected column at 10% selectivity and no slower than 1.05x at 100%; otherwise leave it off (default) and record why”. On the not-projected column, early/late at 10% is 1.00 / 0.85 ARM64 (SF1/SF10) and 0.97 / 0.89 x86-64. Even the range envelopes stay below 1.2. At 100%, the projected-column late/early ratios are 1.139 / 1.304 ARM64 and 1.058 / 1.167 x86-64; some SF1 spreads overlap 1.05, but the SF10 projected-column lower envelopes exceed it on both CPUs. The keep condition fails independently of those borderline cases. Default remains false; `--late-materialization` and its environment switch remain available.

Measured explanation: at 10% with `pk`, early and late read exactly the same byte counts (81,460,998 SF1; 830,968,817 SF10); selecting scattered rows saved no compressed bytes here. With `sd`, late reads more (62,816,657 versus 53,292,721 bytes SF1; 627,919,510 versus 532,742,971 SF10). The implementation decodes a filter column that is also projected twice. Reasoned: selection bookkeeping and that repeated decode explain why this attempt does not help this layout; no profile isolates their individual CPU costs. This does not rule out late materialization for other layouts or selectivities.

**P2 — keep, default to eight; P10 remains enabled.** Rule: “Keep it if a scan of 1,000 small files is at least 1.3x faster at 8 threads and the open-file and footer bounds of slice 3 still hold under the `ulimit` test”. Scan ahead1/ahead8 is 1.667x ARM64 and 1.939x x86-64. Conservative min(ahead1)/max(ahead8) is 1.60x / 1.88x: both clear 1.3, including the spread. Twins track the baseline (1.025x / 1.000x). At ahead8, peak footers/open files are 15/8 ARM64 and 15/8 x86-64, within K + running files (16) and one open file per worker (8). Bounds under `ulimit -n 256` are checked by the 10,000-file scan and SQL tests in validation below. Four and sixteen are information only, not used to choose a different threshold or default. The measured K=8 becomes the default in `ParallelConfig::new`, `ExecConfig::new`, CLI resolution and parallel-test helpers. Set `--footers-ahead 1` (or the environment variable to 1) to turn it off. Single-thread and small-window bounds still apply.

P10: first-footer-only binding stays the default, with `--bind-all-footers` retaining the old oracle. Combined SQL old/new medians improve 2.12x ARM64 and 2.41x x86-64; no separate P10 threshold was added after measuring. Deferred schemas are still checked as files are reached.

**R4 — NOT MET on ARM64 -> an option, OFF by default.** Rule: “Verify a page's CRC when the file has one (on by default; a corrupt page is an error naming the file and row group); its cost must stay under 5% on a file with checksums, or it becomes an option”. Verifying/without ratios on checksummed SF1/SF10 are 1.05556 / 1.05026 ARM64 (1.06x / about 1.05x, NOT MET), and 1.04396 / 1.01712 x86-64 (1.04x / 1.02x, MET). The ARM SF10 ratio rounds to 1.05 in the generated summary but exceeds the budget before rounding.

The paired measurements show a real cost: ARM SF1 verifying/without per round is 0.060/0.055, 0.056/0.053, 0.056/0.053, 0.057/0.054 and 0.057/0.056 seconds. The verifying build is slower in all five rounds on both SF1 and SF10. The verifying twin agrees with the verifying arm (medians 0.057 SF1 and 0.397 SF10), rather than the without-check arm (0.054 and 0.378). Independent min/max envelopes overlap 1.05, but pooling extrema loses the paired evidence; this is not a reason to override the card. The parent corrected that initial interpretation before this commit. No threshold changed.

Per the fixed rule, verification becomes a build-time option: `page-crc` stays available but is removed from default features in **both** `nata-parquet` and `nata`. Enable the CLI with `cargo build --release -p nata --features page-crc`, or the reader with `cargo build -p nata-parquet --features page-crc` (workspace syntax: `--features nata-parquet/page-crc`). The `nata` feature passes through to the reader. Without it, damage can silently decode as different values: this is the explicit trade-off recorded by the card, and the tests assert both states. `ci.yml`'s check job runs the checksum/CLI tests once with the feature on as well as the default workspace suite; nightly damaged-file fuzz explicitly enables it. Future `slice4bench` runs explicitly build the verifying arm with the feature, preserving the measured comparison after the default changes. The raw summary remains unchanged as measurement evidence.

## Runner fix and validation

Before these runs, `test-steps.sh` could report an unrelated `sleep 30`, including a survivor of its own SIGKILL scenario. More seriously, `lib.sh` used `pkill -P` on only one generation: nested workers survived SIGTERM. Commit `2914c5e` puts each step in a session/process group with `setsid` and signals that group (TERM then KILL), waits for its leader, records interruption and exits 143. This handles the runner's TERM/INT cancellation path; an external timeout must signal that path. There is no built-in per-step timeout, and no claim to contain descendants that deliberately create their own sessions. SIGKILL itself cannot run a cleanup trap.

With an unrelated `sleep 30` deliberately running, the original global check exits 1 while the revised check exits 0, both using the fixed runner. This demonstrates the test-isolation bug independently. The revised test keeps multiple descendant generations alive and checks its recorded worker PID, ignoring a dead zombie awaiting system reaping. It exits 1 with “the slow step's child was killed too” against the original `lib.sh` from `2914c5e^`, and passes against the fix. Every `scripts/gcp/test/*.sh` was run: 10 scripts pass; `test-icebergpolaris.sh` has one stale expected-mode-list assertion, and `test-icebergspikes.sh` has that assertion plus four startup-package pattern assertions. The actual mode list now includes `gcs`, and the startup case also includes `slice4bench`; these unrelated failures were left outside this change. Both real runs installed PyArrow successfully.

Final-tree validation is rerun after the R4 default correction; results follow below. Preliminary suite runs were superseded by the default changes and the on-disk scratch fix below. Cancellation is not a test failure attributed to the code. Local validation scratch uses the home directory; `AI_AGENT` is unset for DuckDB and differential invocations.

On-disk validation harness fix: the damaged-file fuzzer repeatedly truncated the same dirty file, waiting for writeback on this host. A local scratch probe (20 writes/reads of 184,233 bytes) took 3.875 s with truncation and 0.010 s when unlinking before recreation. The fuzzer now unlinks the previous case before writing the next one at the same path; every byte mutation, 3,000 cases per shape, both thread counts, seed and oracle are unchanged. This avoids relying on a RAM disk. These probe times explain harness I/O only, not engine performance. The full debug fuzzer passed in 49.13 s before the R4 default correction; both feature states are revalidated afterwards.

## Final validation after the R4 default correction

All local commands below used `TMPDIR=$HOME/nata-scratch` on disk, with `AI_AGENT` unset. No reduced fuzz-case count was used.

| check | result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace` (CRC off, footers ahead 8) | pass; includes both opt-in guards, both 10,000-file `ulimit -n 256` checks, and the full damaged-file fuzz (59.59 s) |
| `cargo test --release -p nata-parquet -p nata` (CRC off) | pass; default checksum expectations and full fuzz (53.18 s) |
| `NATA_CRC_BUILD=1 cargo test --release -p nata-parquet -p nata --features nata/page-crc --test checksums --test fuzz_files --test scan_cli --test sql_cli` | pass; full enabled fuzz (48.52 s), damaged-page errors, and enabled guards |
| `NATA_CRC_BUILD=1 cargo test -p nata-parquet -p nata --features nata/page-crc --test checksums --test scan_cli --test sql_cli` (the new CI command) | pass |
| `cargo build --release -p nata -p nata-check`, then `nata-check --data data/tpch-sf0.01 --queries check/queries/{scan,sql,sql-differences}` | pass: 5/5, 251/251, 37/37; zero mismatch, error or unsupported |
| `scripts/gcp/test/test-job-chains.sh` after the explicit benchmark feature change | pass |
| workflow YAML parse; shell syntax of job and benchmark | pass |
| staged leak scan and `git diff --check` | pass |

The CRC default mutants now target accidental **on** defaults, matching the new opt-in guard expectations (the old off-default mutant results remain historical in step 4a). These retargeted mutants were not rerun in this step; the actual default and enabled test commands above were run. CI/nightly workflow edits were checked locally, not run on GitHub (nothing pushed); the nightly 30,000-case job was not run locally. The two unrelated stale GCP-test failures described above remain. The slice-wide benchmark/regression closeout beyond these carried-in items remains outside step 4b.
