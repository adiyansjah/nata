# Card 0008 package 0 — implementation checks, 2026-11-08

**Incomplete; not a performance certification.** Slice 5 remains pinned to
`bb946ab`; its targets, matrix, resource gates and manual noise judgment are
unchanged. No paid job or reference timing was run here. No dependency was added.

## Verdict engine and shared timer

Rule: explicit `0008-v1` policy, five paired alternating rounds, primary/twin
noise `median(abs(primary/twin-1))`, largest arm noise ≤5%, median AND four of
five within the cell's ceiling (or above its scaling floor). Missing cells are
NOT RUN, failed reference UNASSESSED, noisy/unsettled cells UNRESOLVED. These
proposal values still require the card's reference freeze. No slice-5 rebaseline.

Design: both existing runners use `scripts/bench_verdict.py`'s nanosecond
pipe-EOF timer (b383422 method), process-group deadline, full output and process
CPU/RSS. The closeout runner's default calls the extracted, unchanged slice-5
summary. `--policy 0008-v1` attaches a separate verdict requiring complete
same-run stats. An offline manifest evaluator supports cell-specific ceilings,
floors and dimensions; retains every sample and attached stats. No geometric mean.
Pinned affinity chooses one sibling per physical core before using SMT siblings;
the unpinned arm remains a separate session/dimension.

Measured checks: nine verdict unittests pass; eight deliberate Python verdict
mutants CAUGHT (slowdown, noise, vote count, missing stats, drops, floor direction,
round pairing and failed denominator). Both full inert harness self-tests pass
(36 SF1 + 36 SF10 + 5 SF100 cells; five-source/480-arm heavy simulation).
The two completion-timer tests pass with the smoke child's terminal timestamp
flushed immediately before `_exit`; interpreter teardown had produced 5.0–5.4 ms
exit residuals against the existing 5 ms bound. No timer bound was loosened.

Commands:

```sh
python3 -m unittest discover -s scripts -p test_bench_verdict.py
python3 scripts/check-verdict-mutants.py
python3 -m unittest discover -s scripts -p test_bench_aggregate_closeout.py
python3 scripts/bench-aggregate-closeout.py --self-test
python3 scripts/bench-aggregate-perf1-heavy.py self-test --work /tmp/package0-harness
python3 scripts/bench_verdict.py --manifest MANIFEST.json --timings TIMINGS.json --out VERDICTS.json
```

Verdict: control checks MET; production T6/T7 **NOT RUN** until stats completeness,
both-CPU profiling, the target freeze and settled reference runs. `cargo-mutants`
cannot mutate Python; the eight-source-mutation screen above is its applicable
counterpart. Rust stats mutation results are recorded separately below.
