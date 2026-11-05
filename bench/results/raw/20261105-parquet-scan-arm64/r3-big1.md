
# R3: /mnt/work/src/spikes/parquet-scan/data/big1.parquet: 4 row groups of [1500519, 1500979, 1501976, 1497741] rows, page index: no; morsels: 4 (row group) or 16 (quarter). 25 rounds, load at start 0.48

| threads | row group ms | quarter ms | row-group twin ms | throughput gain of the quarter (row group time / quarter time) | twin noise |
|---|---|---|---|---|---|
| 1 | 96.8 [96.3, 97.4] | 244.3 [243.4, 244.7] | 97.2 [96.6, 97.6] | 0.40 [0.39, 0.40] | 1.00 |
| 2 | 48.7 [48.5, 49.3] | 121.5 [121.1, 122.4] | 49.0 [48.6, 50.0] | 0.40 [0.40, 0.40] | 1.00 |
| 4 | 25.5 [25.3, 26.6] | 63.5 [62.9, 64.1] | 26.2 [25.5, 27.3] | 0.41 [0.40, 0.42] | 1.00 |
| 8 | 25.7 [25.1, 26.3] | 35.4 [34.6, 36.0] | 26.5 [25.3, 27.2] | 0.73 [0.71, 0.75] | 0.99 |

R3 (throughput half) on this file: the quarter's 8-thread gain is 0.73x (the rule wants at least 1.15x AND peak memory up by at most 25%: run `pscan mem` for both arms).
