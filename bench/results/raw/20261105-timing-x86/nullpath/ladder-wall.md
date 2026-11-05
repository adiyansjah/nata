Engine only, batches in memory; rounds: 25 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 67.4 / 67.7 | 145.8 / 147.5 | 2.18 [2.17, 2.18] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 72.0 / 72.2 | 91.8 / 92.2 | 1.28 [1.27, 1.28] | 0.63 | 1.07 |
| 2 + word-wise project | 74.6 / 74.9 | 86.1 / 87.3 | 1.17 [1.16, 1.17] | 0.59 | 1.11 |
| 3a + word-wise sink, per-row masks | 62.5 / 63.0 | 66.0 / 66.5 | 1.06 [1.05, 1.06] | 0.45 | 0.93 |
| 3 + word-wise sink, subtract NULL rows | 53.1 / 54.8 | 62.7 / 64.3 | 1.17 [1.17, 1.18] | 0.44 | 0.81 |
| 4 + null-rejecting filters | 52.7 / 54.8 | 62.6 / 64.2 | 1.17 [1.17, 1.18] | 0.44 | 0.81 |
| 5 + deferred overflow flag | 44.0 / 45.7 | 51.6 / 53.3 | 1.17 [1.16, 1.17] | 0.36 | 0.67 |
| 6 + wrapping sink adds | 41.1 / 42.5 | 47.5 / 48.8 | 1.15 [1.15, 1.15] | 0.33 | 0.63 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.1 / 17.2 | 36.6 / 37.2 | 2.16 [2.15, 2.17] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 18.2 / 18.4 | 22.9 / 23.0 | 1.25 [1.22, 1.26] | 0.62 | 1.07 |
| 2 + word-wise project | 19.0 / 19.1 | 21.5 / 21.6 | 1.13 [1.10, 1.16] | 0.58 | 1.11 |
| 3a + word-wise sink, per-row masks | 15.9 / 16.0 | 16.2 / 16.3 | 1.02 [1.01, 1.06] | 0.44 | 0.93 |
| 3 + word-wise sink, subtract NULL rows | 13.7 / 13.9 | 15.3 / 15.7 | 1.13 [1.08, 1.17] | 0.42 | 0.81 |
| 4 + null-rejecting filters | 13.6 / 13.8 | 15.4 / 15.7 | 1.14 [1.11, 1.17] | 0.42 | 0.80 |
| 5 + deferred overflow flag | 11.4 / 11.5 | 13.1 / 13.4 | 1.15 [1.13, 1.18] | 0.36 | 0.67 |
| 6 + wrapping sink adds | 10.6 / 10.8 | 12.0 / 12.3 | 1.13 [1.12, 1.15] | 0.33 | 0.63 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.3 / 17.6 | 43.8 / 44.1 | 2.51 [2.50, 2.52] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.8 / 17.4 | 18.0 / 18.5 | 1.06 [1.06, 1.07] | 0.42 | 0.99 |
| 2 + word-wise project | 16.9 / 17.2 | 18.1 / 18.5 | 1.07 [1.07, 1.08] | 0.42 | 0.98 |
| 3a + word-wise sink, per-row masks | 17.0 / 17.2 | 18.2 / 18.4 | 1.07 [1.07, 1.08] | 0.42 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 16.9 / 17.3 | 18.2 / 18.5 | 1.07 [1.07, 1.08] | 0.42 | 0.98 |
| 4 + null-rejecting filters | 16.9 / 17.2 | 17.9 / 18.3 | 1.06 [1.05, 1.06] | 0.41 | 0.98 |
| 5 + deferred overflow flag | 17.0 / 17.3 | 18.0 / 18.3 | 1.06 [1.06, 1.07] | 0.41 | 0.98 |
| 6 + wrapping sink adds | 17.0 / 17.2 | 18.0 / 18.3 | 1.06 [1.06, 1.07] | 0.42 | 0.98 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 4.6 / 4.6 | 11.2 / 11.3 | 2.44 [2.41, 2.46] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.5 / 4.6 | 4.8 / 4.9 | 1.07 [1.06, 1.08] | 0.43 | 0.98 |
| 2 + word-wise project | 4.4 / 4.5 | 4.7 / 4.9 | 1.07 [1.06, 1.08] | 0.43 | 0.98 |
| 3a + word-wise sink, per-row masks | 4.4 / 4.6 | 4.7 / 4.9 | 1.06 [1.06, 1.08] | 0.43 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 4.4 / 4.6 | 4.8 / 4.9 | 1.07 [1.06, 1.08] | 0.43 | 0.98 |
| 4 + null-rejecting filters | 4.5 / 4.6 | 4.7 / 4.8 | 1.05 [1.05, 1.07] | 0.42 | 0.98 |
| 5 + deferred overflow flag | 4.5 / 4.5 | 4.7 / 4.8 | 1.05 [1.05, 1.07] | 0.42 | 0.98 |
| 6 + wrapping sink adds | 4.5 / 4.5 | 4.7 / 4.8 | 1.05 [1.05, 1.07] | 0.42 | 0.98 |

