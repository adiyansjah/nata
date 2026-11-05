Engine only, batches in memory; rounds: 25 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: cpu

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 67.4 / 67.6 | 145.5 / 147.8 | 2.19 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 71.8 / 72.0 | 90.6 / 90.9 | 1.26 [1.26, 1.26] | 0.61 | 1.07 |
| 2 + word-wise project | 74.9 / 75.2 | 84.8 / 85.4 | 1.13 [1.13, 1.14] | 0.58 | 1.11 |
| 3a + word-wise sink, per-row masks | 62.5 / 63.2 | 63.2 / 64.2 | 1.02 [1.01, 1.02] | 0.43 | 0.93 |
| 3 + word-wise sink, subtract NULL rows | 53.0 / 55.2 | 60.1 / 62.1 | 1.13 [1.12, 1.13] | 0.42 | 0.82 |
| 4 + null-rejecting filters | 53.3 / 55.2 | 60.4 / 62.1 | 1.13 [1.12, 1.13] | 0.42 | 0.82 |
| 5 + deferred overflow flag | 44.2 / 46.1 | 50.7 / 53.1 | 1.15 [1.15, 1.15] | 0.36 | 0.68 |
| 6 + wrapping sink adds | 41.3 / 42.9 | 47.1 / 48.6 | 1.13 [1.13, 1.14] | 0.33 | 0.63 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 68.3 / 68.7 | 146.3 / 147.5 | 2.15 [2.14, 2.15] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 72.5 / 73.2 | 91.4 / 92.0 | 1.26 [1.25, 1.26] | 0.62 | 1.06 |
| 2 + word-wise project | 75.6 / 76.0 | 85.7 / 86.2 | 1.13 [1.13, 1.14] | 0.59 | 1.11 |
| 3a + word-wise sink, per-row masks | 63.3 / 63.8 | 64.3 / 65.0 | 1.02 [1.01, 1.02] | 0.44 | 0.93 |
| 3 + word-wise sink, subtract NULL rows | 54.3 / 55.1 | 61.3 / 62.3 | 1.13 [1.12, 1.14] | 0.42 | 0.80 |
| 4 + null-rejecting filters | 54.6 / 55.2 | 61.7 / 62.0 | 1.12 [1.11, 1.13] | 0.42 | 0.80 |
| 5 + deferred overflow flag | 45.5 / 46.1 | 52.3 / 53.2 | 1.15 [1.13, 1.18] | 0.36 | 0.67 |
| 6 + wrapping sink adds | 42.7 / 43.4 | 48.4 / 49.0 | 1.13 [1.12, 1.15] | 0.33 | 0.63 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.5 / 17.8 | 43.5 / 44.2 | 2.47 [2.45, 2.50] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.8 / 17.5 | 18.3 / 18.6 | 1.06 [1.05, 1.08] | 0.42 | 0.98 |
| 2 + word-wise project | 16.9 / 17.3 | 18.1 / 18.6 | 1.07 [1.07, 1.08] | 0.42 | 0.97 |
| 3a + word-wise sink, per-row masks | 16.9 / 17.4 | 18.1 / 18.6 | 1.07 [1.07, 1.08] | 0.42 | 0.97 |
| 3 + word-wise sink, subtract NULL rows | 17.0 / 17.4 | 18.3 / 18.6 | 1.08 [1.07, 1.09] | 0.42 | 0.97 |
| 4 + null-rejecting filters | 17.0 / 17.3 | 18.0 / 18.5 | 1.07 [1.05, 1.08] | 0.42 | 0.97 |
| 5 + deferred overflow flag | 17.1 / 17.3 | 18.1 / 18.5 | 1.07 [1.06, 1.07] | 0.42 | 0.97 |
| 6 + wrapping sink adds | 16.9 / 17.3 | 18.2 / 18.4 | 1.07 [1.06, 1.07] | 0.42 | 0.97 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 18.1 / 18.4 | 44.5 / 45.0 | 2.45 [2.44, 2.46] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 17.7 / 18.0 | 19.0 / 19.2 | 1.07 [1.06, 1.07] | 0.43 | 0.98 |
| 2 + word-wise project | 17.6 / 18.0 | 18.9 / 19.2 | 1.06 [1.06, 1.07] | 0.43 | 0.98 |
| 3a + word-wise sink, per-row masks | 17.7 / 17.9 | 18.9 / 19.2 | 1.07 [1.06, 1.07] | 0.43 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 17.6 / 17.9 | 18.8 / 19.1 | 1.07 [1.06, 1.08] | 0.43 | 0.98 |
| 4 + null-rejecting filters | 17.6 / 18.0 | 18.7 / 19.1 | 1.06 [1.05, 1.07] | 0.42 | 0.98 |
| 5 + deferred overflow flag | 17.6 / 18.0 | 18.7 / 19.0 | 1.06 [1.05, 1.06] | 0.42 | 0.98 |
| 6 + wrapping sink adds | 17.4 / 18.0 | 18.6 / 19.0 | 1.06 [1.05, 1.07] | 0.42 | 0.98 |

