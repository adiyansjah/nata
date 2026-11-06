
## P1 late materialization: `nata scan --count --no-prune`, the 7 Q1 columns, 8 threads, window 16 (median of the rounds; twin = the early arm again)
| block | rows | early s | late s | early / late | twin / early | bytes read early | bytes read late | verdict |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| late-sf1-pk-10 | 600153 | 0.061 | 0.063 | 0.97 | 1.00 | 81466908 | 81466908 | speed-up NOT MET (card: at least 1.2x) |
| late-sf1-pk-100 | 6001215 | 0.079 | 0.081 | 0.98 | 1.00 | 81466908 | 81466908 | slowdown MET (card: late/early at most 1.05) |
| late-sf1-sd-10 | 600472 | 0.040 | 0.062 | 0.65 | 1.00 | 53296442 | 62820380 | speed-up NOT MET (card: at least 1.2x; judged on the not-projected column, reported here) |
| late-sf1-sd-100 | 6001215 | 0.069 | 0.073 | 0.95 | 1.01 | 53296442 | 62820380 | slowdown NOT MET (card: late/early at most 1.05) |
| late-sf10-pk-10 | 5998626 | 0.415 | 0.466 | 0.89 | 1.00 | 830962780 | 830962780 | speed-up NOT MET (card: at least 1.2x) |
| late-sf10-pk-100 | 59986052 | 0.399 | 0.443 | 0.90 | 1.02 | 830962780 | 830962780 | slowdown NOT MET (card: late/early at most 1.05) |
| late-sf10-sd-10 | 6000939 | 0.239 | 0.388 | 0.62 | 0.99 | 532776459 | 627953013 | speed-up NOT MET (card: at least 1.2x; judged on the not-projected column, reported here) |
| late-sf10-sd-100 | 59986052 | 0.323 | 0.377 | 0.86 | 0.98 | 532776459 | 627953013 | slowdown NOT MET (card: late/early at most 1.05) |

## P2 footers read ahead, P10: 1,000 small files, 8 threads, window 16 (median of the rounds)
| block | arm | median s | min s | vs first arm | peak footers / open files (report) | verdict |
|---|---|---:|---:|---:|---|---|
| footers-scan | ahead1 | 0.064 | 0.064 | 1.00x | 8 / 8 |  |
| footers-scan | twin1 | 0.064 | 0.063 | 1.00x | 7 / 7 |  |
| footers-scan | ahead4 | 0.033 | 0.033 | 1.94x | 11 / 8 |  |
| footers-scan | ahead8 | 0.033 | 0.032 | 1.94x | 15 / 8 | speed-up 1.94x: MET (card: at least 1.3x) |
| footers-scan | ahead16 | 0.033 | 0.032 | 1.94x | 23 / 8 |  |
| footers-sql | old (all footers at bind, ahead 1) | 0.089 | - | 1.00x | - | |
| footers-sql | new (first footer at bind, ahead 8) | 0.037 | - | 2.41x | - | information (twin/old 1.00) |

## R4 page checksums, the cost: `nata scan --count`, the 7 Q1 columns, 8 threads, window 16; the verifying build against the build without the check (median of the rounds)
| block | rows | verifying s | without s | verifying / without | twin / verifying | verdict |
|---|---:|---:|---:|---:|---:|---|
| crc-sf1-ck | 6001215 | 0.095 | 0.091 | 1.04 | 1.00 | on a file WITH checksums: MET (card: at most 1.05) |
| crc-sf1-plain | 6001215 | 0.069 | 0.069 | 1.00 | 0.99 | control, a file without checksums |
| crc-sf10-ck | 59986052 | 0.713 | 0.701 | 1.02 | 1.01 | on a file WITH checksums: MET (card: at most 1.05) |
| crc-sf10-plain | 59986052 | 0.308 | 0.311 | 0.99 | 1.01 | control, a file without checksums |
