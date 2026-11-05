
# R3: /mnt/work/src/spikes/parquet-scan/data/big1-pi.parquet: 4 row groups of [1500304, 1500304, 1500304, 1500303] rows, page index: YES; morsels: 4 (row group) or 16 (quarter). 25 rounds, load at start 0.48

| threads | row group ms | quarter ms | row-group twin ms | throughput gain of the quarter (row group time / quarter time) | twin noise |
|---|---|---|---|---|---|
| 1 | 109.4 [109.0, 110.0] | 113.6 [113.0, 114.2] | 110.1 [109.1, 110.6] | 0.96 [0.96, 0.97] | 1.00 |
| 2 | 55.5 [55.3, 56.1] | 57.7 [57.2, 58.1] | 55.6 [55.2, 56.1] | 0.96 [0.96, 0.97] | 1.00 |
| 4 | 28.5 [28.3, 29.0] | 29.1 [28.9, 29.5] | 28.9 [28.7, 29.4] | 0.98 [0.97, 1.00] | 0.98 |
| 8 | 28.9 [28.4, 29.1] | 16.0 [15.8, 16.4] | 28.9 [28.6, 29.4] | 1.80 [1.73, 1.84] | 1.00 |

R3 (throughput half) on this file: the quarter's 8-thread gain is 1.80x (the rule wants at least 1.15x AND peak memory up by at most 25%: run `pscan mem` for both arms).
