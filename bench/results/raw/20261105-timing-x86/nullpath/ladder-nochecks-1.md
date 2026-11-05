Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 61.1 / 61.3 | 134.1 / 134.4 | 2.19 [2.19, 2.20] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 62.3 / 62.7 | 84.1 / 84.4 | 1.35 [1.35, 1.35] | 0.63 | 1.02 |
| 2 + word-wise project | 58.9 / 59.3 | 71.4 / 71.8 | 1.21 [1.21, 1.21] | 0.53 | 0.97 |
| 3a + word-wise sink, per-row masks | 50.8 / 51.2 | 51.5 / 51.8 | 1.01 [1.01, 1.01] | 0.38 | 0.83 |
| 3 + word-wise sink, subtract NULL rows | 40.7 / 41.0 | 46.9 / 47.4 | 1.15 [1.15, 1.16] | 0.35 | 0.67 |
| 4 + null-rejecting filters | 40.6 / 41.0 | 47.1 / 47.3 | 1.15 [1.15, 1.16] | 0.35 | 0.67 |
| 5 + deferred overflow flag | 39.3 / 39.6 | 45.9 / 46.1 | 1.16 [1.16, 1.17] | 0.34 | 0.65 |
| 6 + wrapping sink adds | 39.3 / 39.6 | 45.5 / 45.7 | 1.16 [1.15, 1.16] | 0.34 | 0.64 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.5 / 15.6 | 34.0 / 34.2 | 2.21 [2.19, 2.21] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 15.8 / 15.9 | 21.3 / 21.5 | 1.35 [1.34, 1.36] | 0.63 | 1.02 |
| 2 + word-wise project | 14.9 / 15.0 | 18.2 / 18.4 | 1.23 [1.21, 1.23] | 0.54 | 0.96 |
| 3a + word-wise sink, per-row masks | 12.9 / 13.1 | 13.2 / 13.3 | 1.02 [1.01, 1.03] | 0.39 | 0.84 |
| 3 + word-wise sink, subtract NULL rows | 10.4 / 10.5 | 12.1 / 12.2 | 1.16 [1.16, 1.17] | 0.36 | 0.68 |
| 4 + null-rejecting filters | 10.4 / 10.5 | 12.1 / 12.2 | 1.16 [1.15, 1.17] | 0.36 | 0.68 |
| 5 + deferred overflow flag | 10.1 / 10.2 | 11.8 / 12.0 | 1.17 [1.17, 1.19] | 0.35 | 0.66 |
| 6 + wrapping sink adds | 10.1 / 10.2 | 11.8 / 11.8 | 1.16 [1.16, 1.17] | 0.35 | 0.66 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.3 / 17.7 | 44.3 / 44.6 | 2.52 [2.51, 2.54] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.9 / 17.3 | 18.1 / 18.4 | 1.06 [1.06, 1.07] | 0.41 | 0.98 |
| 2 + word-wise project | 16.2 / 17.2 | 18.0 / 18.5 | 1.07 [1.06, 1.08] | 0.41 | 0.97 |
| 3a + word-wise sink, per-row masks | 16.8 / 17.3 | 17.9 / 18.4 | 1.07 [1.06, 1.08] | 0.41 | 0.97 |
| 3 + word-wise sink, subtract NULL rows | 17.0 / 17.2 | 18.2 / 18.3 | 1.06 [1.06, 1.07] | 0.41 | 0.97 |
| 4 + null-rejecting filters | 16.9 / 17.2 | 18.0 / 18.2 | 1.06 [1.05, 1.07] | 0.41 | 0.97 |
| 5 + deferred overflow flag | 17.1 / 17.2 | 18.0 / 18.2 | 1.06 [1.06, 1.07] | 0.41 | 0.97 |
| 6 + wrapping sink adds | 17.1 / 17.2 | 17.9 / 18.2 | 1.06 [1.05, 1.06] | 0.41 | 0.97 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 4.5 / 4.6 | 11.3 / 11.4 | 2.50 [2.48, 2.51] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.4 / 4.5 | 4.7 / 4.8 | 1.05 [1.04, 1.06] | 0.42 | 0.99 |
| 2 + word-wise project | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.06, 1.07] | 0.42 | 0.98 |
| 3a + word-wise sink, per-row masks | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.06, 1.07] | 0.42 | 0.98 |
| 3 + word-wise sink, subtract NULL rows | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.05, 1.06] | 0.42 | 0.98 |
| 4 + null-rejecting filters | 4.4 / 4.5 | 4.6 / 4.7 | 1.05 [1.04, 1.06] | 0.41 | 0.98 |
| 5 + deferred overflow flag | 4.4 / 4.5 | 4.6 / 4.7 | 1.05 [1.03, 1.06] | 0.42 | 0.98 |
| 6 + wrapping sink adds | 4.4 / 4.5 | 4.7 / 4.7 | 1.06 [1.05, 1.07] | 0.41 | 0.98 |

