### q1, 1 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 202.940 | 201.357 |
| filter | 80.221 | 80.818 |
| expression | 246.725 | 146.885 |
| group_id | 220.435 | 237.049 |
| state_update | 207.220 | 285.471 |
| merge | 0.008 | 0.007 |
| finalize | 0.037 | 0.037 |
| output_sink | 0.075 | 0.065 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 0.243 | 0.214 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.740 | 1.941 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 520,660,560 | 118,332,372 |
| materialized_bytes | 520,660,560 | 357,996,716 |
| col_materializations | 187,598 | 211,044 |
| allocation_capacity_bytes | 118,332,436 | 357,996,780 |
| allocation_sites | 187,628 | 398,642 |
| grants | 76 | 76 |
| grant_bytes | 34,989,824 | 34,989,824 |
| releases | 76 | 76 |

### q1, 8 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 443.020 | 439.335 |
| filter | 176.130 | 174.672 |
| expression | 475.922 | 294.332 |
| group_id | 503.427 | 492.855 |
| state_update | 458.635 | 622.200 |
| merge | 0.043 | 0.040 |
| finalize | 0.046 | 0.049 |
| output_sink | 0.085 | 0.086 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 0.158 | 0.292 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.898 | 2.034 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 520,660,560 | 118,332,372 |
| materialized_bytes | 520,660,560 | 357,996,716 |
| col_materializations | 187,598 | 211,044 |
| allocation_capacity_bytes | 118,332,436 | 357,996,780 |
| allocation_sites | 187,628 | 398,642 |
| grants | 111 | 111 |
| grant_bytes | 155,695,227 | 155,695,227 |
| releases | 111 | 111 |

### high_cardinality_tpch, 1 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 265.399 | 213.165 |
| filter | 134.599 | 114.337 |
| expression | 318.783 | 287.967 |
| group_id | 747.218 | 831.368 |
| state_update | 107.197 | 138.600 |
| merge | 0.041 | 0.039 |
| finalize | 372.755 | 358.405 |
| output_sink | 704.157 | 696.180 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 15.465 | 11.009 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 1.713 | 1.712 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 345,511,118 | 144,347,024 |
| materialized_bytes | 345,511,118 | 287,845,560 |
| col_materializations | 303,728 | 327,174 |
| allocation_capacity_bytes | 91,097,705 | 234,596,241 |
| allocation_sites | 466,780 | 584,010 |
| grants | 1,394 | 1,394 |
| grant_bytes | 1,557,924,988 | 1,557,924,988 |
| releases | 1,394 | 1,394 |

### high_cardinality_tpch, 8 threads, 8192 MiB

| Stage (service wall ms) | Before | After |
|---|---:|---:|
| scan_decode | 620.395 | 469.658 |
| filter | 345.307 | 281.767 |
| expression | 481.538 | 445.623 |
| group_id | 1511.950 | 1666.952 |
| state_update | 219.811 | 273.806 |
| merge | 936.270 | 919.498 |
| finalize | 529.010 | 541.112 |
| output_sink | 853.778 | 834.869 |
| spill_write | 0.000 | 0.000 |
| spill_read | 0.000 | 0.000 |
| teardown | 29.230 | 28.769 |
| admission_wait | 0.000 | 0.000 |
| bind_metadata | 2.029 | 1.915 |

| Counter | Before | After |
|---|---:|---:|
| copied_bytes | 345,511,118 | 144,347,024 |
| materialized_bytes | 345,511,118 | 287,845,560 |
| col_materializations | 303,728 | 327,174 |
| allocation_capacity_bytes | 91,097,705 | 234,596,241 |
| allocation_sites | 466,780 | 584,010 |
| grants | 2,641 | 2,640 |
| grant_bytes | 3,009,457,499 | 3,009,391,771 |
| releases | 2,641 | 2,640 |
