Engine only, batches in memory; rounds: 25 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 62.8 / 63.1 | 136.0 / 136.3 | 2.16 [2.16, 2.16] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 66.4 / 66.5 | 73.5 / 73.6 | 1.11 [1.11, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 73.1 / 73.3 | 73.5 / 73.7 | 1.01 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 63.4 / 63.6 | 64.7 / 64.9 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 53.2 / 53.4 | 60.0 / 60.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 53.2 / 53.5 | 60.1 / 60.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 43.9 / 44.1 | 50.4 / 50.7 | 1.15 [1.14, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 41.5 / 41.8 | 46.9 / 47.3 | 1.13 [1.13, 1.13] | 0.35 | 0.66 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.8 / 15.9 | 34.1 / 34.2 | 2.15 [2.14, 2.15] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.7 / 16.8 | 18.4 / 18.5 | 1.10 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 18.4 / 18.5 | 18.4 / 18.5 | 1.00 [1.00, 1.00] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 16.0 / 16.0 | 16.3 / 16.4 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 13.4 / 13.5 | 15.1 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 13.4 / 13.5 | 15.1 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 11.1 / 11.2 | 12.7 / 12.8 | 1.15 [1.14, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 10.5 / 10.6 | 11.9 / 12.0 | 1.13 [1.13, 1.13] | 0.35 | 0.67 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.5 / 14.2 | 36.9 / 39.6 | 2.80 [2.76, 3.05] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 13.5 / 14.6 | 14.5 / 16.4 | 1.11 [1.06, 1.18] | 0.41 | 1.08 |
| 2 + word-wise project | 13.5 / 15.2 | 14.8 / 16.4 | 1.08 [1.05, 1.16] | 0.41 | 1.08 |
| 3a + word-wise sink, per-row masks | 13.5 / 15.2 | 14.4 / 16.3 | 1.07 [1.05, 1.13] | 0.41 | 1.07 |
| 3 + word-wise sink, subtract NULL rows | 13.7 / 15.4 | 14.0 / 16.3 | 1.05 [1.05, 1.07] | 0.41 | 1.09 |
| 4 + null-rejecting filters | 13.5 / 15.3 | 14.6 / 16.1 | 1.04 [1.03, 1.08] | 0.40 | 1.08 |
| 5 + deferred overflow flag | 13.4 / 15.1 | 14.4 / 16.1 | 1.06 [1.03, 1.14] | 0.41 | 1.08 |
| 6 + wrapping sink adds | 13.6 / 15.4 | 13.9 / 16.1 | 1.05 [1.04, 1.10] | 0.41 | 1.08 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 3.6 / 3.7 | 9.6 / 9.9 | 2.64 [2.59, 2.67] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 3.9 / 4.0 | 3.9 / 4.1 | 1.02 [1.00, 1.03] | 0.42 | 1.08 |
| 2 + word-wise project | 3.9 / 4.0 | 3.9 / 4.1 | 1.02 [0.99, 1.05] | 0.41 | 1.07 |
| 3a + word-wise sink, per-row masks | 3.8 / 4.0 | 3.8 / 4.1 | 1.02 [1.00, 1.04] | 0.42 | 1.07 |
| 3 + word-wise sink, subtract NULL rows | 3.8 / 4.0 | 3.9 / 4.0 | 1.02 [0.99, 1.04] | 0.41 | 1.06 |
| 4 + null-rejecting filters | 3.8 / 4.0 | 3.9 / 4.0 | 1.01 [0.99, 1.04] | 0.41 | 1.07 |
| 5 + deferred overflow flag | 3.8 / 4.0 | 3.9 / 4.0 | 1.01 [1.00, 1.04] | 0.41 | 1.06 |
| 6 + wrapping sink adds | 3.8 / 4.0 | 3.8 / 4.1 | 1.02 [1.00, 1.05] | 0.41 | 1.06 |

