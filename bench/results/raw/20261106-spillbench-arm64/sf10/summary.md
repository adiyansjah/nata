FAIL run join-inner t=8 g64 round 1: status FAILED
FAIL run join-inner t=8 g64 round 2: status FAILED
FAIL run join-inner t=8 g64 round 3: status FAILED
FAIL run join-left t=8 g64 round 1: status FAILED
FAIL run join-left t=8 g64 round 2: status FAILED
FAIL run join-left t=8 g64 round 3: status FAILED
FAIL run q13 t=8 g64 round 1: status FAILED
FAIL run q13 t=8 g64 round 2: status FAILED
FAIL run q13 t=8 g64 round 3: status FAILED
| query | threads | arm | median s | median peak RSS MB | spilled MB | spill files | most open | arm / mem median [q1, q3] | verdict |
|---|---:|---|---:|---:|---:|---:|---:|---|---|
| join-inner | 8 | mem | 4.451 | 1227 | 0 | 0 | 0 | - | - |
| join-inner | 8 | mem2 | 4.497 | 1223 | 0 | 0 | 0 | 1.00 [1.00, 1.01] | control |
| join-inner | 8 | g256 | 0.621 | 639 | 241 | 64 | 58 | 0.14 [0.14, 0.14] | faster |
| join-inner | 8 | duckdb | 0.174 | 226 | - | - | - | 0.04 [0.04, 0.04] | faster |
| join-left | 8 | mem | 4.499 | 1236 | 0 | 0 | 0 | - | - |
| join-left | 8 | mem2 | 4.517 | 1240 | 0 | 0 | 0 | 1.01 [0.99, 1.01] | control |
| join-left | 8 | g256 | 0.632 | 645 | 241 | 64 | 58 | 0.14 [0.14, 0.14] | faster |
| join-left | 8 | duckdb | 0.186 | 232 | - | - | - | 0.04 [0.04, 0.04] | faster |
| q13 | 8 | mem | 5.336 | 1284 | 0 | 0 | 0 | - | - |
| q13 | 8 | mem2 | 5.334 | 1295 | 0 | 0 | 0 | 1.00 [1.00, 1.00] | control |
| q13 | 8 | g256 | 1.506 | 625 | 241 | 64 | 60 | 0.28 [0.28, 0.28] | faster |
| q13 | 8 | duckdb | 0.296 | 669 | - | - | - | 0.06 [0.06, 0.06] | faster |
