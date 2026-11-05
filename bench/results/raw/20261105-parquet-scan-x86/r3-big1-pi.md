
# R3: /mnt/work/src/spikes/parquet-scan/data/big1-pi.parquet: 4 row groups of [1500304, 1500304, 1500304, 1500303] rows, page index: YES; morsels: 4 (row group) or 16 (quarter). 25 rounds, load at start 0.47

| threads | row group ms | quarter ms | row-group twin ms | throughput gain of the quarter (row group time / quarter time) | twin noise |
|---|---|---|---|---|---|
| 1 | 149.1 [146.5, 150.7] | 154.3 [149.1, 155.0] | 150.2 [145.8, 152.0] | 0.98 [0.97, 1.00] | 1.00 |
| 2 | 72.5 [71.6, 74.0] | 73.5 [73.0, 75.1] | 72.0 [71.1, 72.3] | 0.98 [0.97, 1.00] | 1.01 |
| 4 | 37.2 [36.4, 37.5] | 37.6 [37.2, 38.2] | 36.9 [36.5, 37.4] | 0.97 [0.96, 1.00] | 1.00 |
| 8 | 44.2 [39.5, 48.7] | 27.7 [27.2, 28.3] | 41.3 [38.5, 44.4] | 1.55 [1.39, 1.68] | 1.03 |

R3 (throughput half) on this file: the quarter's 8-thread gain is 1.55x (the rule wants at least 1.15x AND peak memory up by at most 25%: run `pscan mem` for both arms).
