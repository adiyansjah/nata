Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 62.9 / 63.3 | 136.0 / 136.3 | 2.16 [2.15, 2.16] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 66.3 / 66.6 | 73.4 / 73.7 | 1.11 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 73.2 / 73.4 | 73.6 / 73.7 | 1.00 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 63.4 / 63.6 | 64.8 / 65.0 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 53.2 / 53.3 | 60.0 / 60.2 | 1.13 [1.13, 1.13] | 0.44 | 0.84 |
| 4 + null-rejecting filters | 53.2 / 53.4 | 60.1 / 60.2 | 1.13 [1.13, 1.13] | 0.44 | 0.84 |
| 5 + deferred overflow flag | 43.9 / 44.1 | 50.5 / 50.6 | 1.15 [1.15, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 41.5 / 41.8 | 47.1 / 47.3 | 1.13 [1.13, 1.14] | 0.35 | 0.66 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.8 / 15.9 | 34.2 / 34.2 | 2.15 [2.15, 2.15] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.7 / 16.8 | 18.5 / 18.6 | 1.11 [1.11, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 18.4 / 18.5 | 18.5 / 18.6 | 1.01 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 16.0 / 16.1 | 16.3 / 16.4 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 13.4 / 13.5 | 15.2 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 13.4 / 13.5 | 15.2 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 11.1 / 11.2 | 12.8 / 12.8 | 1.15 [1.14, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 10.5 / 10.6 | 11.9 / 12.0 | 1.13 [1.13, 1.14] | 0.35 | 0.66 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.5 / 13.0 | 37.0 / 39.4 | 3.04 [2.74, 3.12] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 13.7 / 14.3 | 14.0 / 16.1 | 1.11 [1.04, 1.14] | 0.41 | 1.08 |
| 2 + word-wise project | 13.6 / 14.7 | 14.3 / 15.9 | 1.07 [1.04, 1.09] | 0.40 | 1.08 |
| 3a + word-wise sink, per-row masks | 13.4 / 15.2 | 14.1 / 15.7 | 1.06 [1.03, 1.12] | 0.40 | 1.07 |
| 3 + word-wise sink, subtract NULL rows | 13.6 / 15.3 | 14.1 / 15.9 | 1.04 [1.00, 1.06] | 0.41 | 1.09 |
| 4 + null-rejecting filters | 13.6 / 14.0 | 14.2 / 16.1 | 1.11 [1.05, 1.16] | 0.41 | 1.07 |
| 5 + deferred overflow flag | 13.5 / 13.9 | 14.0 / 15.9 | 1.10 [1.01, 1.17] | 0.41 | 1.07 |
| 6 + wrapping sink adds | 13.5 / 14.5 | 14.0 / 16.1 | 1.06 [1.04, 1.16] | 0.41 | 1.07 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 3.6 / 3.7 | 9.7 / 9.9 | 2.66 [2.64, 2.68] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 3.8 / 4.0 | 4.0 / 4.1 | 1.04 [1.03, 1.06] | 0.42 | 1.07 |
| 2 + word-wise project | 3.8 / 4.0 | 4.0 / 4.1 | 1.05 [1.04, 1.06] | 0.42 | 1.06 |
| 3a + word-wise sink, per-row masks | 3.8 / 3.9 | 4.0 / 4.1 | 1.04 [1.02, 1.05] | 0.41 | 1.06 |
| 3 + word-wise sink, subtract NULL rows | 3.8 / 3.9 | 3.9 / 4.1 | 1.03 [1.01, 1.06] | 0.41 | 1.06 |
| 4 + null-rejecting filters | 3.8 / 3.9 | 3.8 / 4.0 | 1.02 [1.01, 1.05] | 0.41 | 1.07 |
| 5 + deferred overflow flag | 3.8 / 3.9 | 4.0 / 4.0 | 1.02 [1.01, 1.06] | 0.41 | 1.06 |
| 6 + wrapping sink adds | 3.8 / 3.9 | 3.9 / 4.1 | 1.03 [1.02, 1.05] | 0.41 | 1.07 |

