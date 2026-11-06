## R2: pruning on SF1 lineitem sorted by l_shipdate, 122,880-row groups, the 1994 predicate, 8 threads
row groups skipped: 41 of 49 (83.7%; R2 requires at least 60%): MET
bytes read pruned / unpruned: 9872047 / 60265360 = 16.4% (the card requires at most 40%): MET
wall: pruned 0.026 s, unpruned 0.048 s; rows returned 909455 (equal to DuckDB's in every run)

## Scalability, sf1, the 7 Q1 columns, --count (median of the rounds; the twin is the same arm again)
| threads | window | median s | rows/s (M) | speed-up vs 1 thread | efficiency | peak RSS MB | twin / arm |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0.243 | 24.7 | 1.00 | 1.00 | 15 | 1.02 |
| 1 | 2 | 0.257 | 23.4 | 0.95 | 0.95 | 15 | - |
| 2 | 2 | 0.158 | 38.0 | 1.54 | 0.77 | 21 | - |
| 2 | 4 | 0.122 | 49.2 | 1.99 | 1.00 | 23 | - |
| 4 | 4 | 0.088 | 68.2 | 2.76 | 0.69 | 35 | - |
| 4 | 8 | 0.074 | 81.1 | 3.28 | 0.82 | 39 | - |
| 8 | 8 | 0.080 | 75.0 | 3.04 | 0.38 | 61 | 0.99 |
| 8 | 16 | 0.078 | 76.9 | 3.12 | 0.39 | 80 | - |

8-thread speed-up (best window): 3.12x (the card requires at least 5x): NOT MET

## Scalability, sf10, the 7 Q1 columns, --count (median of the rounds; the twin is the same arm again)
| threads | window | median s | rows/s (M) | speed-up vs 1 thread | efficiency | peak RSS MB | twin / arm |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 2.222 | 27.0 | 1.00 | 1.00 | 18 | 1.01 |
| 1 | 2 | 2.331 | 25.7 | 0.95 | 0.95 | 18 | - |
| 2 | 2 | 1.227 | 48.9 | 1.81 | 0.91 | 25 | - |
| 2 | 4 | 0.935 | 64.2 | 2.38 | 1.19 | 28 | - |
| 4 | 4 | 0.644 | 93.1 | 3.45 | 0.86 | 39 | - |
| 4 | 8 | 0.455 | 131.8 | 4.88 | 1.22 | 50 | - |
| 8 | 8 | 0.480 | 125.0 | 4.63 | 0.58 | 71 | 0.98 |
| 8 | 16 | 0.326 | 184.0 | 6.82 | 0.85 | 97 | - |

8-thread speed-up (best window): 6.82x (the card requires at least 5x): MET
