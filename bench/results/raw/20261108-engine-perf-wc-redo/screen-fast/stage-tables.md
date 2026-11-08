### q1, 1 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 209.160 | 203.806 |
| filter | 81.690 | 80.661 |
| expression | 253.152 | 147.533 |
| group_id | 226.432 | 226.155 |
| state_update | 212.403 | 194.128 |
| merge | 0.008 | 0.007 |
| finalize | 0.038 | 0.038 |
| output_sink | 0.078 | 0.075 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 0.207 | 0.192 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.643 | 1.373 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 520,660,560 | 118,332,372 |
| materialized_bytes | 520,660,560 | 357,996,716 |
| col_materializations | 187,598 | 211,044 |
| allocation_capacity_bytes | 118,332,436 | 357,996,780 |
| allocation_sites | 187,628 | 398,642 |
| grants | 76 | 76 |
| grant_bytes | 34,989,914 | 34,989,914 |
| releases | 76 | 76 |

### q1, 8 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 449.792 | 433.357 |
| filter | 174.480 | 173.649 |
| expression | 467.730 | 285.509 |
| group_id | 489.472 | 479.951 |
| state_update | 457.392 | 424.320 |
| merge | 0.131 | 0.042 |
| finalize | 0.046 | 0.048 |
| output_sink | 0.105 | 0.087 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 0.155 | 0.288 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 2.104 | 1.826 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 520,660,560 | 118,332,372 |
| materialized_bytes | 520,660,560 | 357,996,716 |
| col_materializations | 187,598 | 211,044 |
| allocation_capacity_bytes | 118,332,436 | 357,996,780 |
| allocation_sites | 187,628 | 398,642 |
| grants | 111 | 111 |
| grant_bytes | 155,695,317 | 155,695,317 |
| releases | 111 | 111 |

### high_cardinality_tpch, 1 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 264.410 | 212.302 |
| filter | 132.078 | 109.487 |
| expression | 318.786 | 287.197 |
| group_id | 750.819 | 816.444 |
| state_update | 105.964 | 98.396 |
| merge | 0.044 | 0.045 |
| finalize | 390.090 | 373.878 |
| output_sink | 714.453 | 701.727 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 16.459 | 17.454 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.648 | 1.836 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 345,511,118 | 144,347,024 |
| materialized_bytes | 345,511,118 | 287,845,560 |
| col_materializations | 303,728 | 327,174 |
| allocation_capacity_bytes | 91,097,705 | 234,596,241 |
| allocation_sites | 466,780 | 584,010 |
| grants | 1,394 | 1,394 |
| grant_bytes | 1,557,925,078 | 1,557,925,078 |
| releases | 1,394 | 1,394 |

### high_cardinality_tpch, 8 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 626.503 | 468.702 |
| filter | 340.273 | 267.477 |
| expression | 472.285 | 418.257 |
| group_id | 1479.009 | 1635.411 |
| state_update | 212.468 | 205.318 |
| merge | 919.987 | 901.100 |
| finalize | 532.222 | 521.235 |
| output_sink | 812.516 | 811.020 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 30.286 | 28.354 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.706 | 1.751 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 345,511,118 | 144,347,024 |
| materialized_bytes | 345,511,118 | 287,845,560 |
| col_materializations | 303,728 | 327,174 |
| allocation_capacity_bytes | 91,097,705 | 234,596,241 |
| allocation_sites | 466,780 | 584,010 |
| grants | 2,641 | 2,640 |
| grant_bytes | 3,009,457,589 | 3,009,455,221 |
| releases | 2,641 | 2,640 |
