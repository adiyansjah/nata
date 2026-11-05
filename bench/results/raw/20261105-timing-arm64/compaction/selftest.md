## Harness self-test (Int64, 8192 rows, 10% random idx, 10% NULLs; arm A = 772 ns per call; 41 rounds; load 0.49 -> 0.49)

An arm with a known extra cost injected per call must be detected as slower by that amount; an identical twin must come out the same.

| arm | injected ns/call | measured difference to A (paired median) | tolerance | verdict | expected | ok |
|---|---:|---:|---:|---|---|---|
| A-twin | 0 | -0 | +-63 | same | same | yes |
| A+1000ns | 1000 | 1045 | +-160 | slower | slower | yes |
| A+5000ns | 5000 | 5065 | +-560 | slower | slower | yes |
| A+20000ns | 20000 | 20064 | +-2060 | slower | slower | yes |

SELF-TEST PASSED
