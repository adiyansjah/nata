Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 61.3 / 61.4 | 134.1 / 134.4 | 2.19 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 62.2 / 62.6 | 85.0 / 85.3 | 1.36 [1.36, 1.37] | 0.63 | 1.01 |
| 2 + word-wise project | 59.8 / 60.0 | 72.8 / 73.3 | 1.22 [1.22, 1.22] | 0.54 | 0.97 |
| 3a + word-wise sink, per-row masks | 51.5 / 51.9 | 52.7 / 53.1 | 1.02 [1.02, 1.03] | 0.39 | 0.84 |
| 3 + word-wise sink, subtract NULL rows | 41.3 / 41.7 | 48.4 / 48.6 | 1.17 [1.16, 1.17] | 0.36 | 0.68 |
| 4 + null-rejecting filters | 41.3 / 41.8 | 48.5 / 48.7 | 1.17 [1.16, 1.17] | 0.36 | 0.68 |
| 5 + deferred overflow flag | 39.7 / 40.2 | 47.1 / 47.3 | 1.18 [1.18, 1.19] | 0.35 | 0.65 |
| 6 + wrapping sink adds | 39.7 / 40.0 | 46.7 / 46.9 | 1.18 [1.17, 1.18] | 0.35 | 0.65 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.5 / 15.5 | 34.0 / 34.2 | 2.20 [2.19, 2.22] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 15.8 / 16.0 | 21.4 / 21.5 | 1.35 [1.34, 1.36] | 0.63 | 1.02 |
| 2 + word-wise project | 14.9 / 15.2 | 18.3 / 18.5 | 1.22 [1.20, 1.24] | 0.54 | 0.97 |
| 3a + word-wise sink, per-row masks | 12.9 / 13.2 | 13.3 / 13.6 | 1.02 [1.02, 1.05] | 0.39 | 0.84 |
| 3 + word-wise sink, subtract NULL rows | 10.4 / 10.5 | 12.2 / 12.3 | 1.17 [1.16, 1.17] | 0.36 | 0.68 |
| 4 + null-rejecting filters | 10.5 / 10.6 | 12.2 / 12.3 | 1.16 [1.15, 1.17] | 0.36 | 0.68 |
| 5 + deferred overflow flag | 10.2 / 10.6 | 11.9 / 12.0 | 1.17 [1.12, 1.18] | 0.35 | 0.67 |
| 6 + wrapping sink adds | 10.0 / 10.3 | 11.8 / 12.0 | 1.17 [1.14, 1.17] | 0.35 | 0.66 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 17.3 / 17.7 | 44.3 / 44.8 | 2.54 [2.52, 2.56] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 17.0 / 17.2 | 18.2 / 18.4 | 1.07 [1.06, 1.08] | 0.41 | 0.98 |
| 2 + word-wise project | 17.0 / 17.1 | 18.2 / 18.4 | 1.07 [1.06, 1.08] | 0.41 | 0.97 |
| 3a + word-wise sink, per-row masks | 16.9 / 17.2 | 18.0 / 18.3 | 1.07 [1.07, 1.08] | 0.41 | 0.97 |
| 3 + word-wise sink, subtract NULL rows | 16.6 / 17.3 | 18.1 / 18.4 | 1.07 [1.06, 1.08] | 0.41 | 0.97 |
| 4 + null-rejecting filters | 16.9 / 17.2 | 18.0 / 18.3 | 1.06 [1.05, 1.08] | 0.41 | 0.98 |
| 5 + deferred overflow flag | 16.9 / 17.2 | 17.9 / 18.2 | 1.06 [1.05, 1.07] | 0.41 | 0.98 |
| 6 + wrapping sink adds | 16.8 / 17.2 | 18.0 / 18.2 | 1.07 [1.05, 1.07] | 0.41 | 0.97 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 4.6 / 4.6 | 11.5 / 11.6 | 2.49 [2.48, 2.50] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.4 / 4.5 | 4.8 / 4.8 | 1.07 [1.06, 1.08] | 0.42 | 0.97 |
| 2 + word-wise project | 4.4 / 4.5 | 4.7 / 4.8 | 1.06 [1.06, 1.08] | 0.42 | 0.97 |
| 3a + word-wise sink, per-row masks | 4.4 / 4.5 | 4.7 / 4.8 | 1.07 [1.06, 1.08] | 0.42 | 0.97 |
| 3 + word-wise sink, subtract NULL rows | 4.5 / 4.5 | 4.7 / 4.8 | 1.06 [1.05, 1.09] | 0.42 | 0.97 |
| 4 + null-rejecting filters | 4.4 / 4.5 | 4.7 / 4.8 | 1.05 [1.04, 1.07] | 0.41 | 0.97 |
| 5 + deferred overflow flag | 4.4 / 4.5 | 4.6 / 4.8 | 1.05 [1.05, 1.06] | 0.41 | 0.97 |
| 6 + wrapping sink adds | 4.5 / 4.5 | 4.6 / 4.8 | 1.06 [1.04, 1.06] | 0.41 | 0.97 |

