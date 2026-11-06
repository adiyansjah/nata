# Clean-machine reproduction of the verification on x86-64, and the cost of `overflow-checks` on generic-ops (2026-11-06)

Run `verify-x86-settled` (job `verify`, one `c3d-highcpu-8` Spot VM, AMD EPYC 9B14, 4 cores x 2 threads), source `d021df7` with the
scripts of `c59602c` (the job now settles before each timed block). A first run the same day (`verify-x86`, $0.022) passed every gate but
started all eight timed blocks at a 1-minute load of 2.3 to 3.5, so its timings were sanity only and are not used. Raw outputs:
`bench/results/raw/20261106-verify-x86-settled/`.

## Gates (all passed; 36 steps)

- Workspace `cargo fmt --check`, `clippy -D warnings` and `cargo test --workspace` on a clean machine.
- Lint fixtures: 73 of 73 fail with the expected lint, 10 of 10 controls clean, 5 of 5 known gaps still pass clippy.
- Guarded expressions: the DuckDB diff and 22 of 22 guard mutations caught.
- The scheduler's loom model passes (9 of 9) and each of its 5 deliberate bugs is caught (`expect-fail: failed as expected`).
- Generic-ops `check` and `nullcheck` with `overflow-checks` on: same results as without, nothing panics.

These gates are architecture-independent in what they check (loom models SeqCst semantics; the lints are source-level), so this is a
clean-machine reproduction on x86-64 of the earlier ARM64 run (`bench/results/20261104-arm64-verify-run.md`), not new evidence about x86-64.

## Cost of `overflow-checks` on generic-ops (plan item 21, ADR 0003 decision 6)

The same code built with `overflow-checks` on and off, two alternations, 25 interleaved rounds each with a twin control, every block
after a settle (all eight started at 0.47 to 0.49; the twin control was 1.00 in every row). Medians in ms are whole numbers, so at
8 threads a 1 ms step is 3 to 8%.

| query | threads | end to end, generic: on / off (ms) | cost | engine only, generic: on / off (ms) | cost |
|---|---:|---|---:|---|---:|
| q1 | 1 | 152 / 144, 151 / 145 | +4.1% to +5.6% | 68 / 62 | +9.7% |
| q1 | 2 | 77 / 73, 77 / 74 | +4.1% to +5.5% | 34 / 31 | +9.7% |
| q1 | 4 | 40 / 38 | +5.3% | 17 / 16 | +6.2% |
| q1 | 8 | 31 / 28 | +10.7% | 14 / 12 | +16.7% |
| q6 | 1-8 | equal in every cell | +0.0% | equal in every cell | +0.0% |

The hand-fused loop pays more in the engine-only benchmark (q1 +8% to +26%, q6 +0% to +11%) because it does more checked arithmetic
per row, so the cost of generality shrinks slightly with the checks on, as on ARM64.

Reading: **with the checks on, the generic engine costs 4 to 6% on Q1 end to end at 1 to 4 threads and nothing on Q6, on x86-64**,
the same order as on ARM64 (3 to 5%, `bench/results/20261104-arm64-verify-run.md`). The 8-thread cells (+11% end to end, +17% engine
only) are 3 ms and 2 ms differences on 28 ms and 12 ms medians, at the resolution of the benchmark.

## What this does not show

The cost on code outside the spikes; any query other than Q1 and Q6 for generic-ops (the hash operators are in
`bench/results/20261105-timing-x86.md`); a finer timer than whole milliseconds; more than one run.
