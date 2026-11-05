Engine only, batches in memory; rounds: 13 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: wall

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 63.1 / 63.2 | 135.9 / 136.2 | 2.15 [2.15, 2.16] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 66.4 / 66.6 | 73.5 / 73.7 | 1.11 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 73.3 / 73.5 | 73.5 / 73.7 | 1.00 [1.00, 1.00] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 63.5 / 63.7 | 64.5 / 64.9 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 53.3 / 53.4 | 60.1 / 60.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 53.2 / 53.5 | 60.1 / 60.3 | 1.13 [1.13, 1.13] | 0.44 | 0.84 |
| 5 + deferred overflow flag | 43.9 / 44.0 | 50.4 / 50.7 | 1.15 [1.15, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 41.4 / 41.8 | 47.2 / 47.4 | 1.13 [1.13, 1.14] | 0.35 | 0.66 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 15.9 / 16.0 | 34.1 / 34.3 | 2.15 [2.14, 2.15] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 16.7 / 16.8 | 18.5 / 18.6 | 1.11 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 18.4 / 18.5 | 18.5 / 18.6 | 1.00 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 16.0 / 16.1 | 16.3 / 16.4 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 13.5 / 13.5 | 15.2 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 13.5 / 13.5 | 15.1 / 15.2 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 11.1 / 11.2 | 12.8 / 12.8 | 1.15 [1.14, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 10.5 / 10.6 | 11.9 / 12.0 | 1.13 [1.13, 1.13] | 0.35 | 0.66 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.7 / 13.5 | 36.7 / 39.6 | 2.83 [2.73, 2.95] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 13.8 / 14.6 | 14.8 / 15.6 | 1.06 [1.03, 1.11] | 0.41 | 1.09 |
| 2 + word-wise project | 13.7 / 14.5 | 14.4 / 15.5 | 1.08 [1.03, 1.11] | 0.39 | 1.08 |
| 3a + word-wise sink, per-row masks | 13.8 / 14.6 | 14.4 / 16.0 | 1.08 [1.02, 1.11] | 0.41 | 1.08 |
| 3 + word-wise sink, subtract NULL rows | 14.3 / 14.6 | 14.4 / 15.6 | 1.05 [0.98, 1.12] | 0.41 | 1.09 |
| 4 + null-rejecting filters | 13.8 / 14.6 | 14.3 / 15.2 | 1.05 [1.01, 1.07] | 0.40 | 1.11 |
| 5 + deferred overflow flag | 14.0 / 14.6 | 14.2 / 15.1 | 1.04 [0.98, 1.07] | 0.39 | 1.09 |
| 6 + wrapping sink adds | 14.3 / 15.2 | 14.1 / 15.2 | 1.03 [0.98, 1.07] | 0.39 | 1.09 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 3.6 / 3.7 | 9.7 / 9.9 | 2.66 [2.64, 2.68] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 3.9 / 4.0 | 4.1 / 4.2 | 1.03 [1.02, 1.05] | 0.42 | 1.08 |
| 2 + word-wise project | 3.8 / 4.0 | 3.9 / 4.1 | 1.03 [1.01, 1.05] | 0.42 | 1.08 |
| 3a + word-wise sink, per-row masks | 3.8 / 4.0 | 3.8 / 4.1 | 1.03 [1.03, 1.04] | 0.42 | 1.07 |
| 3 + word-wise sink, subtract NULL rows | 3.8 / 3.9 | 4.0 / 4.1 | 1.04 [1.03, 1.04] | 0.42 | 1.07 |
| 4 + null-rejecting filters | 3.9 / 4.0 | 4.0 / 4.1 | 1.03 [1.02, 1.04] | 0.41 | 1.08 |
| 5 + deferred overflow flag | 3.9 / 4.0 | 3.9 / 4.1 | 1.02 [0.98, 1.03] | 0.41 | 1.08 |
| 6 + wrapping sink adds | 3.8 / 4.0 | 3.9 / 4.0 | 1.03 [1.01, 1.05] | 0.41 | 1.07 |

