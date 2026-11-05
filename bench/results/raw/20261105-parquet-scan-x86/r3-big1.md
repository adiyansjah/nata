
# R3: /mnt/work/src/spikes/parquet-scan/data/big1.parquet: 4 row groups of [1500519, 1500979, 1501976, 1497741] rows, page index: no; morsels: 4 (row group) or 16 (quarter). 25 rounds, load at start 0.48

| threads | row group ms | quarter ms | row-group twin ms | throughput gain of the quarter (row group time / quarter time) | twin noise |
|---|---|---|---|---|---|
| 1 | 127.8 [126.5, 129.7] | 279.4 [275.6, 282.5] | 128.2 [126.6, 129.9] | 0.46 [0.45, 0.46] | 1.00 |
| 2 | 68.8 [68.0, 69.8] | 149.1 [147.9, 151.3] | 69.0 [67.7, 69.9] | 0.46 [0.46, 0.47] | 1.00 |
| 4 | 39.8 [39.2, 40.1] | 88.7 [87.1, 90.7] | 39.5 [38.6, 40.7] | 0.44 [0.44, 0.45] | 1.00 |
| 8 | 44.2 [40.3, 49.4] | 70.7 [69.9, 72.3] | 42.0 [40.5, 47.5] | 0.64 [0.57, 0.70] | 1.03 |

R3 (throughput half) on this file: the quarter's 8-thread gain is 0.64x (the rule wants at least 1.15x AND peak memory up by at most 25%: run `pscan mem` for both arms).
