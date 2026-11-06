
## P1 late materialization: `nata scan --count --no-prune`, the 7 Q1 columns, 8 threads, window 16 (median of the rounds; twin = the early arm again)
| block | rows | early s | late s | early / late | twin / early | bytes read early | bytes read late | verdict |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| late-sf1-pk-10 | 600153 | 0.037 | 0.037 | 1.00 | 1.03 | 81460998 | 81460998 | speed-up NOT MET (card: at least 1.2x) |
| late-sf1-pk-100 | 6001215 | 0.045 | 0.044 | 1.02 | 1.02 | 81460998 | 81460998 | slowdown MET (card: late/early at most 1.05) |
| late-sf1-sd-10 | 600472 | 0.026 | 0.034 | 0.76 | 1.00 | 53292721 | 62816657 | speed-up NOT MET (card: at least 1.2x; judged on the not-projected column, reported here) |
| late-sf1-sd-100 | 6001215 | 0.036 | 0.041 | 0.88 | 1.03 | 53292721 | 62816657 | slowdown NOT MET (card: late/early at most 1.05) |
| late-sf10-pk-10 | 5998626 | 0.232 | 0.274 | 0.85 | 1.00 | 830968817 | 830968817 | speed-up NOT MET (card: at least 1.2x) |
| late-sf10-pk-100 | 59986052 | 0.240 | 0.272 | 0.88 | 1.00 | 830968817 | 830968817 | slowdown NOT MET (card: late/early at most 1.05) |
| late-sf10-sd-10 | 6000939 | 0.143 | 0.196 | 0.73 | 0.96 | 532742971 | 627919510 | speed-up NOT MET (card: at least 1.2x; judged on the not-projected column, reported here) |
| late-sf10-sd-100 | 59986052 | 0.158 | 0.206 | 0.77 | 1.00 | 532742971 | 627919510 | slowdown NOT MET (card: late/early at most 1.05) |

## P2 footers read ahead, P10: 1,000 small files, 8 threads, window 16 (median of the rounds)
| block | arm | median s | min s | vs first arm | peak footers / open files (report) | verdict |
|---|---|---:|---:|---:|---|---|
| footers-scan | ahead1 | 0.040 | 0.040 | 1.00x | 5 / 5 |  |
| footers-scan | twin1 | 0.041 | 0.039 | 0.98x | 5 / 6 |  |
| footers-scan | ahead4 | 0.024 | 0.023 | 1.67x | 11 / 8 |  |
| footers-scan | ahead8 | 0.024 | 0.022 | 1.67x | 15 / 8 | speed-up 1.67x: MET (card: at least 1.3x) |
| footers-scan | ahead16 | 0.024 | 0.023 | 1.67x | 23 / 8 |  |
| footers-sql | old (all footers at bind, ahead 1) | 0.055 | - | 1.00x | - | |
| footers-sql | new (first footer at bind, ahead 8) | 0.026 | - | 2.12x | - | information (twin/old 1.00) |

## R4 page checksums, the cost: `nata scan --count`, the 7 Q1 columns, 8 threads, window 16; the verifying build against the build without the check (median of the rounds)
| block | rows | verifying s | without s | verifying / without | twin / verifying | verdict |
|---|---:|---:|---:|---:|---:|---|
| crc-sf1-ck | 6001215 | 0.057 | 0.054 | 1.06 | 1.00 | on a file WITH checksums: NOT MET -> becomes an option (card: at most 1.05) |
| crc-sf1-plain | 6001215 | 0.038 | 0.037 | 1.03 | 1.00 | control, a file without checksums |
| crc-sf10-ck | 59986052 | 0.397 | 0.378 | 1.05 | 1.00 | on a file WITH checksums: NOT MET -> becomes an option (card: at most 1.05) |
| crc-sf10-plain | 59986052 | 0.152 | 0.164 | 0.93 | 1.09 | control, a file without checksums |
