Engine only, batches in memory; rounds: 25 (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: cpu

### q1 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 62.8 / 63.2 | 136.0 / 136.2 | 2.16 [2.15, 2.16] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 66.4 / 66.6 | 73.4 / 73.7 | 1.10 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 73.2 / 73.5 | 73.3 / 73.7 | 1.00 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 63.5 / 63.8 | 64.5 / 65.0 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 53.3 / 53.6 | 59.9 / 60.4 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 53.3 / 53.6 | 60.0 / 60.3 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 44.0 / 44.2 | 50.4 / 50.7 | 1.15 [1.15, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 41.6 / 41.9 | 47.0 / 47.4 | 1.13 [1.13, 1.13] | 0.35 | 0.66 |

### q1 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 63.1 / 63.3 | 136.1 / 136.3 | 2.15 [2.15, 2.16] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 66.6 / 66.7 | 73.6 / 73.8 | 1.11 [1.10, 1.11] | 0.54 | 1.05 |
| 2 + word-wise project | 73.2 / 73.5 | 73.7 / 73.9 | 1.01 [1.00, 1.01] | 0.54 | 1.16 |
| 3a + word-wise sink, per-row masks | 63.6 / 63.8 | 64.9 / 65.1 | 1.02 [1.02, 1.02] | 0.48 | 1.01 |
| 3 + word-wise sink, subtract NULL rows | 53.3 / 53.6 | 60.1 / 60.4 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 4 + null-rejecting filters | 53.3 / 53.6 | 60.2 / 60.4 | 1.13 [1.12, 1.13] | 0.44 | 0.85 |
| 5 + deferred overflow flag | 44.0 / 44.3 | 50.6 / 50.9 | 1.15 [1.15, 1.15] | 0.37 | 0.70 |
| 6 + wrapping sink adds | 41.8 / 42.0 | 47.2 / 47.6 | 1.13 [1.13, 1.14] | 0.35 | 0.66 |

### q6 , 1 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 12.7 / 14.1 | 37.0 / 39.8 | 2.81 [2.75, 2.89] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 13.8 / 15.3 | 14.3 / 16.3 | 1.07 [1.05, 1.16] | 0.41 | 1.08 |
| 2 + word-wise project | 13.6 / 15.1 | 14.6 / 16.2 | 1.07 [1.04, 1.08] | 0.41 | 1.07 |
| 3a + word-wise sink, per-row masks | 13.7 / 15.4 | 14.8 / 16.3 | 1.06 [1.05, 1.13] | 0.41 | 1.08 |
| 3 + word-wise sink, subtract NULL rows | 13.8 / 15.3 | 14.4 / 16.3 | 1.06 [1.05, 1.07] | 0.41 | 1.08 |
| 4 + null-rejecting filters | 13.7 / 15.2 | 14.1 / 16.0 | 1.04 [1.00, 1.09] | 0.41 | 1.08 |
| 5 + deferred overflow flag | 13.8 / 15.5 | 14.2 / 16.2 | 1.04 [1.02, 1.06] | 0.41 | 1.10 |
| 6 + wrapping sink adds | 13.8 / 15.4 | 14.2 / 16.2 | 1.05 [1.00, 1.12] | 0.41 | 1.08 |

### q6 , 4 thread(s)

| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |
|---|---|---|---|---|---|
| 0 old (generic-ops) | 14.0 / 14.4 | 38.1 / 38.9 | 2.68 [2.66, 2.72] | 1.00 | 1.00 |
| 1 bitmaps + mask filter | 14.7 / 15.6 | 15.4 / 16.0 | 1.03 [1.01, 1.04] | 0.41 | 1.08 |
| 2 + word-wise project | 15.1 / 15.5 | 15.0 / 15.8 | 1.02 [1.00, 1.06] | 0.41 | 1.08 |
| 3a + word-wise sink, per-row masks | 15.0 / 15.4 | 15.0 / 15.8 | 1.02 [1.00, 1.04] | 0.41 | 1.07 |
| 3 + word-wise sink, subtract NULL rows | 15.1 / 15.5 | 15.3 / 15.8 | 1.02 [1.00, 1.04] | 0.41 | 1.08 |
| 4 + null-rejecting filters | 14.6 / 15.5 | 15.0 / 15.8 | 1.02 [1.00, 1.04] | 0.41 | 1.07 |
| 5 + deferred overflow flag | 15.0 / 15.5 | 15.0 / 15.7 | 1.02 [1.00, 1.04] | 0.41 | 1.07 |
| 6 + wrapping sink adds | 15.0 / 15.4 | 15.0 / 15.7 | 1.01 [1.00, 1.04] | 0.40 | 1.07 |

