Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 60.4 / 60.5 | 132.8 / 133.0 | 2.20 [2.20, 2.20] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 62.1 / 62.2 | 70.1 / 70.3 | 1.13 [1.13, 1.13] | 0.53 | 1.03 |
| 2 + word-wise project | 59.1 / 59.3 | 60.4 / 60.6 | 1.02 [1.02, 1.02] | 0.46 | 0.98 |
| 3a + word-wise sink, per-row masks | 47.7 / 47.9 | 48.9 / 49.2 | 1.02 [1.02, 1.03] | 0.37 | 0.79 |
| 3 + word-wise sink, subtract NULL rows | 39.3 / 39.5 | 45.1 / 45.4 | 1.15 [1.15, 1.15] | 0.34 | 0.65 |
| 4 + null-rejecting filters | 39.3 / 39.6 | 45.1 / 45.4 | 1.15 [1.14, 1.15] | 0.34 | 0.65 |
| 5 + deferred overflow flag | 39.1 / 39.4 | 44.7 / 44.9 | 1.14 [1.14, 1.14] | 0.34 | 0.65 |
| 6 + wrapping sink adds | 39.0 / 39.3 | 44.6 / 44.9 | 1.14 [1.14, 1.14] | 0.34 | 0.65 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.2 / 15.3 | 33.4 / 33.4 | 2.18 [2.18, 2.19] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 15.7 / 15.7 | 17.7 / 17.8 | 1.13 [1.12, 1.13] | 0.53 | 1.03 |
| 2 + word-wise project | 14.9 / 15.0 | 15.3 / 15.3 | 1.02 [1.02, 1.02] | 0.46 | 0.98 |
| 3a + word-wise sink, per-row masks | 12.1 / 12.2 | 12.4 / 12.5 | 1.03 [1.02, 1.03] | 0.37 | 0.79 |
| 3 + word-wise sink, subtract NULL rows | 10.0 / 10.1 | 11.4 / 11.5 | 1.14 [1.14, 1.15] | 0.34 | 0.66 |
| 4 + null-rejecting filters | 10.0 / 10.1 | 11.4 / 11.5 | 1.14 [1.14, 1.14] | 0.34 | 0.66 |
| 5 + deferred overflow flag | 9.9 / 10.0 | 11.3 / 11.4 | 1.14 [1.13, 1.14] | 0.34 | 0.65 |
| 6 + wrapping sink adds | 9.9 / 10.0 | 11.3 / 11.4 | 1.13 [1.13, 1.14] | 0.34 | 0.65 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.5 / 14.3 | 36.5 / 39.1 | 2.74 [2.69, 2.91] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 14.5 / 16.2 | 14.8 / 16.5 | 1.02 [1.01, 1.06] | 0.43 | 1.14 |
| 2 + word-wise project | 14.4 / 15.1 | 14.6 / 16.5 | 1.05 [0.97, 1.12] | 0.42 | 1.13 |
| 3a + word-wise sink, per-row masks | 14.4 / 16.2 | 14.5 / 16.5 | 1.01 [1.00, 1.03] | 0.42 | 1.14 |
| 3 + word-wise sink, subtract NULL rows | 14.4 / 16.1 | 14.6 / 15.6 | 1.01 [0.94, 1.04] | 0.42 | 1.13 |
| 4 + null-rejecting filters | 14.6 / 16.3 | 14.5 / 16.5 | 1.01 [0.97, 1.04] | 0.42 | 1.13 |
| 5 + deferred overflow flag | 14.5 / 16.3 | 14.4 / 16.2 | 1.00 [0.96, 1.02] | 0.42 | 1.13 |
| 6 + wrapping sink adds | 14.5 / 16.1 | 14.5 / 16.4 | 1.02 [1.00, 1.03] | 0.42 | 1.14 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 3.7 / 3.7 | 9.5 / 9.7 | 2.57 [2.54, 2.60] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 4.1 / 4.2 | 4.0 / 4.1 | 0.99 [0.98, 0.99] | 0.43 | 1.12 |
| 2 + word-wise project | 4.1 / 4.2 | 4.0 / 4.1 | 0.98 [0.97, 0.99] | 0.43 | 1.11 |
| 3a + word-wise sink, per-row masks | 4.0 / 4.2 | 4.0 / 4.1 | 0.97 [0.97, 0.99] | 0.42 | 1.11 |
| 3 + word-wise sink, subtract NULL rows | 4.1 / 4.2 | 4.0 / 4.1 | 0.97 [0.97, 1.00] | 0.42 | 1.13 |
| 4 + null-rejecting filters | 4.1 / 4.2 | 4.0 / 4.1 | 0.98 [0.96, 0.99] | 0.42 | 1.11 |
| 5 + deferred overflow flag | 4.0 / 4.2 | 4.0 / 4.1 | 0.99 [0.98, 1.00] | 0.42 | 1.11 |
| 6 + wrapping sink adds | 4.1 / 4.2 | 3.9 / 4.1 | 0.97 [0.96, 0.99] | 0.42 | 1.11 |

