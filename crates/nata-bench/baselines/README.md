# Slice 4 regression baselines

`slice4-arm64.csv` and `slice4-x86.csv` describe the current SQL-slice code on
settled n4a-highcpu-8 / c3d-highcpu-8 Spot VMs. Raw runs, limits, code versions,
all numbers and deviations: `bench/results/20261106-slice4-close.md`.

The first baseline uses SQL rounds 0/2/4 and five primary scan rounds. Its
validation uses disjoint SQL rounds 1/3 and five scan twins from the same
alternating session. Future comparisons should reduce all five new rounds:

```sh
python3 crates/nata-bench/regression-summary.py RESULTS_DIR new.csv
crates/nata-bench/regress.sh crates/nata-bench/baselines/slice4-arm64.csv new.csv
```

Default tolerance is 20%; observed maximum median twin spreads are 5.104% ARM64
and 3.734% x86-64. The reducer rejects incomplete corpora/rounds and missing
resource probes; the comparator rejects missing, duplicate and invalid metrics.
There is no CI job because shared runners are noisy.

These are the first baselines for this comparator. The extra historical ARM64
SF1 scan comparison failed at +23% and remains open as P13; introducing the
current-code baseline does not resolve it. Preserve that record, the SF10
latency/scaling/resource misses, and the sanitizer failure. A later baseline
update requires an explained measurement and decision, never an automatic refresh
merely to accept a slower result.
