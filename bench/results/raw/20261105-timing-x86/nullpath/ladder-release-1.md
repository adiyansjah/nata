Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 67.5 / 67.7 | 145.6 / 148.0 | 2.19 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 71.9 / 72.5 | 90.7 / 91.0 | 1.26 [1.25, 1.26] | 0.61 | 1.07 |
| 2 + word-wise project | 75.1 / 75.7 | 85.1 / 85.4 | 1.13 [1.13, 1.13] | 0.58 | 1.12 |
| 3a + word-wise sink, per-row masks | 62.8 / 63.4 | 63.8 / 64.4 | 1.02 [1.01, 1.02] | 0.44 | 0.94 |
| 3 + word-wise sink, subtract NULL rows | 53.3 / 55.4 | 60.4 / 62.3 | 1.13 [1.12, 1.13] | 0.42 | 0.82 |
| 4 + null-rejecting filters | 53.7 / 55.4 | 60.9 / 62.3 | 1.13 [1.12, 1.13] | 0.42 | 0.82 |
| 5 + deferred overflow flag | 44.6 / 46.1 | 51.1 / 53.3 | 1.15 [1.15, 1.16] | 0.36 | 0.68 |
| 6 + wrapping sink adds | 41.1 / 43.1 | 47.2 / 48.8 | 1.14 [1.13, 1.14] | 0.33 | 0.64 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.1 / 17.2 | 36.8 / 37.4 | 2.16 [2.14, 2.23] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 18.3 / 18.4 | 23.0 / 23.5 | 1.26 [1.25, 1.28] | 0.62 | 1.07 |
| 2 + word-wise project | 19.1 / 19.3 | 21.5 / 21.8 | 1.13 [1.12, 1.14] | 0.58 | 1.12 |
| 3a + word-wise sink, per-row masks | 16.1 / 16.2 | 16.3 / 16.4 | 1.01 [1.00, 1.02] | 0.44 | 0.94 |
| 3 + word-wise sink, subtract NULL rows | 13.7 / 14.0 | 15.5 / 15.7 | 1.12 [1.11, 1.13] | 0.42 | 0.81 |
| 4 + null-rejecting filters | 13.8 / 14.0 | 15.5 / 15.7 | 1.12 [1.11, 1.15] | 0.42 | 0.81 |
| 5 + deferred overflow flag | 11.5 / 11.7 | 13.2 / 13.5 | 1.14 [1.14, 1.17] | 0.36 | 0.68 |
| 6 + wrapping sink adds | 10.8 / 10.9 | 12.2 / 12.6 | 1.13 [1.12, 1.16] | 0.33 | 0.63 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.6 / 17.8 | 45.6 / 45.9 | 2.58 [2.57, 2.60] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 17.0 / 17.5 | 18.2 / 18.6 | 1.06 [1.05, 1.07] | 0.41 | 0.98 |
| 2 + word-wise project | 16.9 / 17.3 | 18.3 / 18.6 | 1.07 [1.06, 1.09] | 0.41 | 0.98 |
| 3a + word-wise sink, per-row masks | 16.8 / 17.4 | 18.2 / 18.5 | 1.07 [1.06, 1.08] | 0.40 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 17.0 / 17.3 | 18.3 / 18.5 | 1.07 [1.06, 1.08] | 0.40 | 0.97 |
| 4 + null-rejecting filters | 17.0 / 17.4 | 18.0 / 18.3 | 1.06 [1.05, 1.06] | 0.40 | 0.97 |
| 5 + deferred overflow flag | 17.0 / 17.3 | 18.2 / 18.4 | 1.06 [1.05, 1.08] | 0.40 | 0.98 |
| 6 + wrapping sink adds | 17.3 / 17.4 | 18.1 / 18.4 | 1.06 [1.05, 1.06] | 0.40 | 0.98 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 4.6 / 4.6 | 11.1 / 11.3 | 2.43 [2.41, 2.45] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.05, 1.07] | 0.43 | 0.98 |
| 2 + word-wise project | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.06, 1.07] | 0.43 | 0.98 |
| 3a + word-wise sink, per-row masks | 4.5 / 4.5 | 4.7 / 4.8 | 1.06 [1.05, 1.07] | 0.43 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.05, 1.07] | 0.43 | 0.97 |
| 4 + null-rejecting filters | 4.5 / 4.5 | 4.7 / 4.8 | 1.05 [1.05, 1.06] | 0.42 | 0.97 |
| 5 + deferred overflow flag | 4.4 / 4.5 | 4.7 / 4.8 | 1.05 [1.05, 1.06] | 0.42 | 0.98 |
| 6 + wrapping sink adds | 4.5 / 4.5 | 4.7 / 4.8 | 1.06 [1.04, 1.06] | 0.42 | 0.97 |

