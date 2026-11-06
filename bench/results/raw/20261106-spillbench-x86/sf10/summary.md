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
| join-inner | 8 | mem | 2.304 | 1226 | 0 | 0 | 0 | - | - |
| join-inner | 8 | mem2 | 2.307 | 1228 | 0 | 0 | 0 | 1.00 [1.00, 1.00] | control |
| join-inner | 8 | g256 | 0.732 | 641 | 241 | 64 | 56 | 0.32 [0.32, 0.32] | faster |
| join-inner | 8 | duckdb | 0.221 | 242 | - | - | - | 0.10 [0.09, 0.10] | faster |
| join-left | 8 | mem | 2.321 | 1239 | 0 | 0 | 0 | - | - |
| join-left | 8 | mem2 | 2.290 | 1240 | 0 | 0 | 0 | 0.99 [0.96, 0.99] | control |
| join-left | 8 | g256 | 0.751 | 651 | 241 | 64 | 54 | 0.31 [0.31, 0.32] | faster |
| join-left | 8 | duckdb | 0.224 | 246 | - | - | - | 0.10 [0.09, 0.10] | faster |
| q13 | 8 | mem | 2.989 | 1250 | 0 | 0 | 0 | - | - |
| q13 | 8 | mem2 | 3.027 | 1255 | 0 | 0 | 0 | 0.99 [0.98, 1.02] | control |
| q13 | 8 | g256 | 1.488 | 624 | 241 | 64 | 60 | 0.50 [0.49, 0.50] | faster |
| q13 | 8 | duckdb | 0.396 | 703 | - | - | - | 0.13 [0.13, 0.13] | faster |
