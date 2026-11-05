Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 67.2 / 67.7 | 145.2 / 147.9 | 2.18 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 71.4 / 71.9 | 90.6 / 90.9 | 1.26 [1.26, 1.27] | 0.61 | 1.06 |
| 2 + word-wise project | 74.8 / 75.2 | 85.2 / 85.5 | 1.14 [1.13, 1.14] | 0.58 | 1.11 |
| 3a + word-wise sink, per-row masks | 62.3 / 62.9 | 63.8 / 64.3 | 1.02 [1.02, 1.03] | 0.44 | 0.93 |
| 3 + word-wise sink, subtract NULL rows | 53.0 / 54.8 | 60.2 / 62.2 | 1.14 [1.13, 1.14] | 0.42 | 0.81 |
| 4 + null-rejecting filters | 53.3 / 54.9 | 60.6 / 62.3 | 1.14 [1.13, 1.14] | 0.42 | 0.81 |
| 5 + deferred overflow flag | 43.9 / 45.6 | 51.0 / 52.9 | 1.16 [1.16, 1.16] | 0.36 | 0.67 |
| 6 + wrapping sink adds | 41.1 / 42.3 | 47.2 / 48.5 | 1.14 [1.14, 1.15] | 0.33 | 0.63 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.0 / 17.1 | 36.9 / 37.0 | 2.17 [2.16, 2.18] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 18.1 / 18.2 | 22.9 / 23.1 | 1.27 [1.26, 1.27] | 0.62 | 1.06 |
| 2 + word-wise project | 19.0 / 19.1 | 21.6 / 21.7 | 1.14 [1.13, 1.14] | 0.59 | 1.12 |
| 3a + word-wise sink, per-row masks | 15.9 / 16.1 | 16.1 / 16.2 | 1.01 [1.01, 1.02] | 0.44 | 0.94 |
| 3 + word-wise sink, subtract NULL rows | 13.7 / 13.8 | 15.5 / 15.5 | 1.13 [1.13, 1.13] | 0.42 | 0.81 |
| 4 + null-rejecting filters | 13.7 / 13.8 | 15.5 / 15.6 | 1.13 [1.12, 1.14] | 0.42 | 0.81 |
| 5 + deferred overflow flag | 11.3 / 11.5 | 13.1 / 13.4 | 1.16 [1.16, 1.18] | 0.36 | 0.67 |
| 6 + wrapping sink adds | 10.6 / 10.7 | 12.2 / 12.3 | 1.14 [1.13, 1.15] | 0.33 | 0.63 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.5 / 17.7 | 44.4 / 44.7 | 2.53 [2.51, 2.54] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 17.1 / 17.3 | 18.5 / 18.7 | 1.08 [1.08, 1.09] | 0.42 | 0.98 |
| 2 + word-wise project | 17.1 / 17.2 | 18.6 / 18.7 | 1.09 [1.09, 1.10] | 0.42 | 0.97 |
| 3a + word-wise sink, per-row masks | 17.1 / 17.3 | 18.4 / 18.8 | 1.09 [1.09, 1.09] | 0.42 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 17.0 / 17.3 | 18.6 / 18.8 | 1.09 [1.07, 1.09] | 0.42 | 0.98 |
| 4 + null-rejecting filters | 16.9 / 17.3 | 18.2 / 18.5 | 1.08 [1.07, 1.08] | 0.41 | 0.98 |
| 5 + deferred overflow flag | 17.2 / 17.3 | 18.4 / 18.6 | 1.08 [1.07, 1.08] | 0.42 | 0.98 |
| 6 + wrapping sink adds | 17.1 / 17.3 | 18.4 / 18.5 | 1.07 [1.07, 1.08] | 0.42 | 0.98 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 4.6 / 4.6 | 11.3 / 11.4 | 2.45 [2.44, 2.47] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.5 / 4.6 | 4.9 / 4.9 | 1.07 [1.07, 1.08] | 0.43 | 0.99 |
| 2 + word-wise project | 4.4 / 4.6 | 4.8 / 5.0 | 1.08 [1.07, 1.08] | 0.43 | 0.98 |
| 3a + word-wise sink, per-row masks | 4.4 / 4.6 | 4.8 / 5.0 | 1.09 [1.07, 1.09] | 0.44 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 4.4 / 4.6 | 4.8 / 4.9 | 1.08 [1.08, 1.09] | 0.43 | 0.98 |
| 4 + null-rejecting filters | 4.5 / 4.5 | 4.8 / 4.9 | 1.07 [1.07, 1.08] | 0.43 | 0.98 |
| 5 + deferred overflow flag | 4.5 / 4.6 | 4.8 / 4.9 | 1.07 [1.06, 1.08] | 0.43 | 0.98 |
| 6 + wrapping sink adds | 4.5 / 4.6 | 4.8 / 4.9 | 1.08 [1.06, 1.08] | 0.43 | 0.98 |

