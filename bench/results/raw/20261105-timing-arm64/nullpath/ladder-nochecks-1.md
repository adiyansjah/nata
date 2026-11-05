Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 60.3 / 60.6 | 132.6 / 132.8 | 2.19 [2.19, 2.20] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 61.7 / 62.1 | 69.9 / 70.1 | 1.13 [1.13, 1.13] | 0.53 | 1.02 |
| 2 + word-wise project | 59.0 / 59.4 | 60.1 / 60.5 | 1.02 [1.02, 1.02] | 0.45 | 0.98 |
| 3a + word-wise sink, per-row masks | 47.5 / 48.1 | 48.9 / 49.1 | 1.02 [1.02, 1.03] | 0.37 | 0.79 |
| 3 + word-wise sink, subtract NULL rows | 39.1 / 39.6 | 44.8 / 45.3 | 1.15 [1.14, 1.15] | 0.34 | 0.65 |
| 4 + null-rejecting filters | 39.2 / 39.7 | 45.1 / 45.3 | 1.14 [1.14, 1.15] | 0.34 | 0.65 |
| 5 + deferred overflow flag | 38.9 / 39.4 | 44.4 / 44.8 | 1.14 [1.13, 1.15] | 0.34 | 0.65 |
| 6 + wrapping sink adds | 39.0 / 39.3 | 44.4 / 44.8 | 1.14 [1.14, 1.14] | 0.34 | 0.65 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.2 / 15.3 | 33.3 / 33.4 | 2.18 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 15.6 / 15.7 | 17.6 / 17.7 | 1.13 [1.12, 1.13] | 0.53 | 1.03 |
| 2 + word-wise project | 14.9 / 15.0 | 15.2 / 15.3 | 1.02 [1.02, 1.02] | 0.46 | 0.98 |
| 3a + word-wise sink, per-row masks | 12.0 / 12.2 | 12.4 / 12.5 | 1.02 [1.02, 1.03] | 0.37 | 0.80 |
| 3 + word-wise sink, subtract NULL rows | 10.0 / 10.0 | 11.4 / 11.5 | 1.14 [1.14, 1.15] | 0.34 | 0.66 |
| 4 + null-rejecting filters | 10.0 / 10.1 | 11.4 / 11.5 | 1.14 [1.14, 1.15] | 0.34 | 0.66 |
| 5 + deferred overflow flag | 9.9 / 10.0 | 11.3 / 11.4 | 1.14 [1.13, 1.14] | 0.34 | 0.65 |
| 6 + wrapping sink adds | 9.9 / 10.0 | 11.3 / 11.4 | 1.14 [1.13, 1.14] | 0.34 | 0.65 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.8 / 13.3 | 36.7 / 39.2 | 2.83 [2.74, 2.98] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 14.7 / 16.3 | 15.9 / 16.6 | 1.02 [1.00, 1.12] | 0.43 | 1.14 |
| 2 + word-wise project | 14.7 / 16.4 | 15.2 / 16.6 | 1.01 [0.95, 1.03] | 0.42 | 1.15 |
| 3a + word-wise sink, per-row masks | 14.5 / 16.1 | 14.7 / 16.5 | 1.01 [0.99, 1.04] | 0.42 | 1.15 |
| 3 + word-wise sink, subtract NULL rows | 14.6 / 16.1 | 14.8 / 16.7 | 1.01 [0.99, 1.12] | 0.42 | 1.15 |
| 4 + null-rejecting filters | 14.4 / 16.3 | 14.6 / 16.7 | 1.03 [0.95, 1.03] | 0.42 | 1.14 |
| 5 + deferred overflow flag | 14.5 / 15.5 | 15.0 / 16.5 | 1.02 [1.02, 1.11] | 0.42 | 1.14 |
| 6 + wrapping sink adds | 14.5 / 16.3 | 15.0 / 16.5 | 1.01 [0.93, 1.04] | 0.42 | 1.15 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 3.6 / 3.8 | 9.5 / 9.8 | 2.62 [2.55, 2.69] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.1 / 4.3 | 4.1 / 4.3 | 1.01 [0.99, 1.02] | 0.43 | 1.13 |
| 2 + word-wise project | 4.0 / 4.2 | 4.1 / 4.2 | 0.99 [0.98, 1.02] | 0.43 | 1.11 |
| 3a + word-wise sink, per-row masks | 4.0 / 4.2 | 4.0 / 4.2 | 0.99 [0.97, 1.01] | 0.43 | 1.13 |
| 3 + word-wise sink, subtract NULL rows | 4.0 / 4.2 | 4.0 / 4.2 | 1.00 [0.99, 1.03] | 0.43 | 1.12 |
| 4 + null-rejecting filters | 4.0 / 4.2 | 4.0 / 4.2 | 0.98 [0.98, 1.02] | 0.42 | 1.12 |
| 5 + deferred overflow flag | 4.0 / 4.2 | 4.1 / 4.2 | 0.99 [0.97, 1.01] | 0.42 | 1.12 |
| 6 + wrapping sink adds | 4.0 / 4.2 | 4.0 / 4.2 | 1.02 [0.99, 1.04] | 0.43 | 1.10 |

