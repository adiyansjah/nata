| query | threads | ours median s | DuckDB median s | ours / DuckDB | ours speed-up vs 1 thread | ours efficiency | DuckDB speed-up | DuckDB efficiency |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| q1 | 8 | 1.668 | 2.135 | 0.78 | 1.00 | 1.00 | 1.00 | 1.00 |
| q1 | 16 | 0.883 | 1.195 | 0.74 | 1.89 | 0.94 | 1.79 | 0.89 |
| q1 | 32 | 0.501 | 0.791 | 0.63 | 3.33 | 0.83 | 2.70 | 0.67 |
| q6 | 8 | 0.906 | 1.099 | 0.82 | 1.00 | 1.00 | 1.00 | 1.00 |
| q6 | 16 | 0.502 | 0.673 | 0.75 | 1.80 | 0.90 | 1.63 | 0.82 |
| q6 | 32 | 0.305 | 0.530 | 0.58 | 2.97 | 0.74 | 2.07 | 0.52 |

Outputs compared with DuckDB: 18, mismatches: 0
