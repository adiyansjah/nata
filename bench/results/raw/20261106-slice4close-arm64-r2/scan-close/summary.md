## R2: pruning on SF1 lineitem sorted by l_shipdate, 122,880-row groups, the 1994 predicate, 8 threads
row groups skipped: 41 of 49 (83.7%; R2 requires at least 60%): MET
bytes read pruned / unpruned: 9875085 / 60283511 = 16.4% (the card requires at most 40%): MET
wall: pruned 0.016 s, unpruned 0.031 s; rows returned 909455 (equal to DuckDB's in every run)

## Scalability, sf1, the 7 Q1 columns, --count (median of the rounds; the twin is the same arm again)
| threads | window | median s | rows/s (M) | speed-up vs 1 thread | efficiency | peak RSS MB | twin / arm |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0.171 | 35.1 | 1.00 | 1.00 | 14 | 0.97 |
| 1 | 2 | 0.139 | 43.2 | 1.23 | 1.23 | 14 | - |
| 2 | 2 | 0.084 | 71.4 | 2.04 | 1.02 | 20 | - |
| 2 | 4 | 0.068 | 88.3 | 2.51 | 1.26 | 21 | - |
| 4 | 4 | 0.053 | 113.2 | 3.23 | 0.81 | 32 | - |
| 4 | 8 | 0.041 | 146.4 | 4.17 | 1.04 | 38 | - |
| 8 | 8 | 0.047 | 127.7 | 3.64 | 0.45 | 59 | 0.98 |
| 8 | 16 | 0.040 | 150.0 | 4.28 | 0.53 | 81 | - |

8-thread speed-up (best window): 4.28x (the card requires at least 5x): NOT MET

## Scalability, sf10, the 7 Q1 columns, --count (median of the rounds; the twin is the same arm again)
| threads | window | median s | rows/s (M) | speed-up vs 1 thread | efficiency | peak RSS MB | twin / arm |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 1.272 | 47.2 | 1.00 | 1.00 | 17 | 1.00 |
| 1 | 2 | 0.912 | 65.8 | 1.39 | 1.39 | 17 | - |
| 2 | 2 | 0.734 | 81.7 | 1.73 | 0.87 | 24 | - |
| 2 | 4 | 0.502 | 119.5 | 2.53 | 1.27 | 26 | - |
| 4 | 4 | 0.336 | 178.5 | 3.79 | 0.95 | 38 | - |
| 4 | 8 | 0.252 | 238.0 | 5.05 | 1.26 | 42 | - |
| 8 | 8 | 0.222 | 270.2 | 5.73 | 0.72 | 68 | 0.97 |
| 8 | 16 | 0.171 | 350.8 | 7.44 | 0.93 | 111 | - |

8-thread speed-up (best window): 7.44x (the card requires at least 5x): MET
