# push-cost benchmark

- arch `x86_64`, 8 CPUs visible, rounds 25 (after 1 discarded warm-up round), rows per run 2^25 = 33554432
- 1-minute load average at start, before any benchmark thread ran: **0.83**

## Harness self-test

Arms: `control`, `control-twin` (same code), and `control+spinNns` = the control plus a busy-wait of N ns after every batch. Cell: cheap kernel, 1 stage, 1024 rows per batch, 33554432 rows per run, 25 interleaved rounds. Expected difference = the spin measured on its own on one thread. PASS needs: |median - expected| <= max(25% of expected, 15 ns), interquartile range not wider than the median, and the twin within max(10% of a control batch, 20 ns) of zero.

### 1 thread(s): control = 338 ns per batch per worker

| arm | injected (nominal ns) | injected (measured ns) | measured difference ns/batch, median [q1, q3] | tolerance | result |
|---|---|---|---|---|---|
| control-twin | 0 | 0 | -1.6 [-6.4, +0.2] | ±34 | PASS (no difference seen) |
| control+spin100ns | 100 | 141 | +139.5 [+136.6, +143.8] | ±35 | PASS |
| control+spin400ns | 400 | 453 | +439.0 [+434.9, +449.2] | ±113 | PASS |
| control+spin1600ns | 1600 | 1649 | +1650.7 [+1648.1, +1653.6] | ±412 | PASS |

### 4 thread(s): control = 432 ns per batch per worker

| arm | injected (nominal ns) | injected (measured ns) | measured difference ns/batch, median [q1, q3] | tolerance | result |
|---|---|---|---|---|---|
| control-twin | 0 | 0 | +1.3 [-4.2, +3.4] | ±43 | PASS (no difference seen) |
| control+spin100ns | 100 | 141 | +137.3 [+132.6, +142.2] | ±35 | PASS |
| control+spin400ns | 400 | 453 | +441.4 [+435.3, +446.2] | ±113 | PASS |
| control+spin1600ns | 1600 | 1649 | +1637.6 [+1634.0, +1644.4] | ±412 | PASS |

**Harness self-test: PASS - the harness measures an injected cost of 100 to 1600 ns per batch correctly**

## Fixed cost of one run (zero morsels)

Median of 31 interleaved runs, microseconds. This is included in every timing below.

| threads | control | static | dyn-direct | dyn | static-in-sched |
|---|---|---|---|---|---|
| 1 | 42 | 43 | 43 | 56 | 56 |
| 4 | 96 | 96 | 93 | 134 | 134 |

## Linear chain: control vs static vs dyn

Arms: see `src/arms.rs`. Overhead = (arm - control) / control per round, then median [q1, q3] over the rounds.

### cheap kernel, 1 stage(s), 1024 rows/batch, 1 thread(s)

load1 0.83 -> 1.00 (includes this benchmark's own 1 threads), other processes used 0.04 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955456856260608 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.328 | 336 | - | - | - |
| control-twin | 0.327 | 335 | -0.3 [-1.0, +0.7] | -1 [-3, +2] | NOISY (IQR > difference) |
| static | 0.504 | 516 | +54.2 [+52.7, +55.9] | +182 [+177, +187] | slower |
| dyn-direct | 0.508 | 520 | +56.0 [+54.7, +57.4] | +187 [+183, +194] | slower |
| dyn | 0.564 | 577 | +73.3 [+70.9, +75.6] | +245 [+238, +251] | slower |
| static-in-sched | 0.568 | 581 | +73.1 [+71.7, +75.2] | +246 [+240, +249] | slower |

### cheap kernel, 1 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.00 -> 1.00 (includes this benchmark's own 4 threads), other processes used 0.20 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955456856260608 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.095 | 388 | - | - | - |
| control-twin | 0.095 | 391 | -0.2 [-0.8, +0.6] | -1 [-3, +2] | NOISY (IQR > difference) |
| static | 0.142 | 583 | +49.6 [+48.5, +52.7] | +193 [+188, +199] | slower |
| dyn-direct | 0.143 | 586 | +49.4 [+47.2, +52.1] | +191 [+186, +198] | slower |
| dyn | 0.210 | 859 | +120.9 [+117.2, +122.6] | +469 [+455, +478] | slower |
| static-in-sched | 0.207 | 849 | +118.4 [+116.2, +120.7] | +462 [+448, +474] | slower |

### cheap kernel, 1 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.00 -> 1.00 (includes this benchmark's own 1 threads), other processes used 0.04 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447196925952 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.335 | 2747 | - | - | - |
| control-twin | 0.336 | 2750 | -0.2 [-1.0, +0.5] | -5 [-28, +15] | NOISY (IQR > difference) |
| static | 0.500 | 4096 | +49.1 [+47.7, +50.3] | +1346 [+1318, +1379] | slower |
| dyn-direct | 0.504 | 4130 | +50.2 [+48.1, +50.9] | +1367 [+1340, +1398] | slower |
| dyn | 0.520 | 4256 | +54.3 [+52.5, +56.4] | +1494 [+1464, +1535] | slower |
| static-in-sched | 0.519 | 4248 | +54.3 [+53.0, +55.8] | +1499 [+1460, +1526] | slower |

### cheap kernel, 1 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.00 -> 1.00 (includes this benchmark's own 4 threads), other processes used 0.10 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447196925952 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.092 | 3014 | - | - | - |
| control-twin | 0.092 | 3003 | -0.0 [-1.0, +1.1] | -0 [-29, +34] | NOISY (IQR > difference) |
| static | 0.136 | 4451 | +47.7 [+47.0, +48.9] | +1444 [+1397, +1468] | slower |
| dyn-direct | 0.135 | 4438 | +47.3 [+46.4, +48.7] | +1434 [+1390, +1469] | slower |
| dyn | 0.180 | 5914 | +97.3 [+95.7, +97.7] | +2908 [+2854, +2963] | slower |
| static-in-sched | 0.180 | 5911 | +96.5 [+95.3, +97.7] | +2898 [+2883, +2924] | slower |

### cheap kernel, 1 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.00 -> 1.00 (includes this benchmark's own 1 threads), other processes used 0.12 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747426719744 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.383 | 25071 | - | - | - |
| control-twin | 0.380 | 24882 | -0.8 [-2.2, +1.2] | -200 [-560, +296] | NOISY (IQR > difference) |
| static | 0.621 | 40707 | +60.8 [+58.8, +64.7] | +15386 [+15047, +16066] | slower |
| dyn-direct | 0.626 | 41030 | +61.8 [+58.8, +66.7] | +15649 [+15052, +16560] | slower |
| dyn | 0.626 | 41018 | +62.9 [+61.2, +65.2] | +15924 [+15590, +16186] | slower |
| static-in-sched | 0.628 | 41179 | +63.9 [+60.3, +65.1] | +15895 [+15442, +16512] | slower |

### cheap kernel, 1 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.00 -> 1.00 (includes this benchmark's own 4 threads), other processes used 0.08 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747426719744 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.102 | 26822 | - | - | - |
| control-twin | 0.102 | 26827 | -0.2 [-1.4, +1.0] | -53 [-378, +273] | NOISY (IQR > difference) |
| static | 0.163 | 42738 | +59.2 [+58.2, +61.6] | +15923 [+15592, +16292] | slower |
| dyn-direct | 0.164 | 43119 | +61.4 [+58.9, +62.7] | +16442 [+15882, +16680] | slower |
| dyn | 0.215 | 56384 | +111.0 [+106.5, +114.0] | +29660 [+28804, +30184] | slower |
| static-in-sched | 0.215 | 56231 | +109.9 [+108.5, +112.6] | +29430 [+29215, +29907] | slower |

### cheap kernel, 3 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.00 -> 1.24 (includes this benchmark's own 1 threads), other processes used 0.05 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.340 | 348 | - | - | - |
| control-twin | 0.344 | 352 | +1.5 [+0.6, +2.6] | +5 [+2, +9] | NOISY (IQR > difference) |
| static | 0.863 | 883 | +155.2 [+152.3, +157.7] | +544 [+529, +548] | slower |
| dyn-direct | 0.879 | 900 | +159.5 [+156.8, +162.3] | +554 [+544, +563] | slower |
| dyn | 1.028 | 1053 | +205.0 [+197.7, +206.7] | +706 [+690, +723] | slower |
| static-in-sched | 0.951 | 974 | +180.3 [+177.4, +182.1] | +626 [+618, +635] | slower |

### cheap kernel, 3 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.24 -> 1.46 (includes this benchmark's own 4 threads), other processes used 0.09 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.101 | 414 | - | - | - |
| control-twin | 0.102 | 416 | +0.2 [-1.0, +1.6] | +1 [-4, +6] | NOISY (IQR > difference) |
| static | 0.233 | 956 | +130.7 [+129.8, +133.3] | +542 [+537, +546] | slower |
| dyn-direct | 0.237 | 972 | +136.6 [+132.7, +139.9] | +561 [+552, +570] | slower |
| dyn | 0.368 | 1506 | +263.8 [+258.6, +266.4] | +1089 [+1068, +1100] | slower |
| static-in-sched | 0.336 | 1374 | +233.6 [+225.8, +237.2] | +962 [+944, +982] | slower |

### cheap kernel, 3 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.46 -> 1.46 (includes this benchmark's own 1 threads), other processes used 0.03 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.334 | 2732 | - | - | - |
| control-twin | 0.334 | 2733 | -0.3 [-0.7, +0.3] | -8 [-19, +8] | NOISY (IQR > difference) |
| static | 0.835 | 6843 | +149.3 [+147.8, +150.7] | +4100 [+4042, +4131] | slower |
| dyn-direct | 0.842 | 6899 | +151.2 [+148.5, +153.8] | +4165 [+4053, +4203] | slower |
| dyn | 0.870 | 7125 | +159.8 [+157.6, +162.9] | +4367 [+4325, +4426] | slower |
| static-in-sched | 0.847 | 6936 | +154.5 [+152.3, +155.6] | +4194 [+4152, +4262] | slower |

### cheap kernel, 3 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.46 -> 1.46 (includes this benchmark's own 4 threads), other processes used 0.10 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.090 | 2960 | - | - | - |
| control-twin | 0.091 | 2992 | +0.2 [-0.9, +0.9] | +4 [-27, +28] | NOISY (IQR > difference) |
| static | 0.221 | 7233 | +143.4 [+141.6, +146.1] | +4253 [+4187, +4330] | slower |
| dyn-direct | 0.221 | 7253 | +146.1 [+141.5, +148.1] | +4292 [+4209, +4412] | slower |
| dyn | 0.302 | 9887 | +234.9 [+228.8, +238.2] | +6920 [+6749, +7048] | slower |
| static-in-sched | 0.295 | 9674 | +224.4 [+220.3, +230.5] | +6658 [+6456, +6798] | slower |

### cheap kernel, 3 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.46 -> 1.42 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.379 | 24848 | - | - | - |
| control-twin | 0.380 | 24891 | +0.9 [-2.3, +2.1] | +232 [-579, +519] | NOISY (IQR > difference) |
| static | 1.039 | 68108 | +175.1 [+172.1, +183.1] | +43253 [+42904, +44949] | slower |
| dyn-direct | 1.040 | 68131 | +176.6 [+175.2, +182.5] | +43389 [+42764, +44717] | slower |
| dyn | 1.053 | 69001 | +178.7 [+173.3, +184.0] | +44030 [+42564, +45174] | slower |
| static-in-sched | 1.048 | 68704 | +176.1 [+173.3, +182.6] | +43809 [+42791, +44599] | slower |

### cheap kernel, 3 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.42 -> 1.63 (includes this benchmark's own 4 threads), other processes used 0.14 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.108 | 28318 | - | - | - |
| control-twin | 0.099 | 26050 | -4.3 [-11.0, +0.5] | -1182 [-3114, +145] | NOISY (IQR > difference) |
| static | 0.289 | 75734 | +170.3 [+162.3, +180.5] | +47308 [+46498, +48930] | slower |
| dyn-direct | 0.285 | 74700 | +170.4 [+159.8, +182.2] | +46793 [+45980, +49948] | slower |
| dyn | 0.354 | 92829 | +235.2 [+219.0, +254.4] | +66244 [+63415, +69226] | slower |
| static-in-sched | 0.366 | 96003 | +241.8 [+227.0, +264.2] | +67677 [+66477, +72097] | slower |

### cheap kernel, 6 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.63 -> 1.50 (includes this benchmark's own 1 threads), other processes used 0.15 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955458533982208 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.352 | 360 | - | - | - |
| control-twin | 0.334 | 342 | -2.8 [-4.4, -0.2] | -10 [-16, -1] | NOISY (IQR > difference) |
| static | 1.388 | 1422 | +297.9 [+293.4, +301.6] | +1065 [+1024, +1078] | slower |
| dyn-direct | 1.378 | 1411 | +291.0 [+286.5, +304.4] | +1049 [+1024, +1066] | slower |
| dyn | 1.616 | 1655 | +367.5 [+357.5, +380.0] | +1301 [+1277, +1320] | slower |
| static-in-sched | 1.450 | 1485 | +317.8 [+313.4, +331.5] | +1138 [+1110, +1159] | slower |

### cheap kernel, 6 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.50 -> 1.50 (includes this benchmark's own 4 threads), other processes used 0.12 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955458533982208 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.107 | 440 | - | - | - |
| control-twin | 0.108 | 443 | -1.0 [-1.8, +0.3] | -5 [-8, +1] | NOISY (IQR > difference) |
| static | 0.357 | 1461 | +232.2 [+228.6, +235.2] | +1023 [+999, +1070] | slower |
| dyn-direct | 0.355 | 1455 | +231.5 [+228.9, +235.6] | +1014 [+1000, +1073] | slower |
| dyn | 0.562 | 2303 | +425.6 [+412.7, +440.1] | +1865 [+1827, +1980] | slower |
| static-in-sched | 0.500 | 2047 | +362.2 [+353.4, +373.6] | +1596 [+1567, +1666] | slower |

### cheap kernel, 6 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.50 -> 1.46 (includes this benchmark's own 1 threads), other processes used 0.02 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496448874647552 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.337 | 2764 | - | - | - |
| control-twin | 0.337 | 2761 | +0.3 [-0.9, +1.0] | +9 [-24, +27] | NOISY (IQR > difference) |
| static | 1.342 | 10991 | +297.6 [+292.1, +304.9] | +8263 [+7939, +8400] | slower |
| dyn-direct | 1.358 | 11127 | +300.5 [+297.2, +307.1] | +8350 [+8145, +8430] | slower |
| dyn | 1.397 | 11443 | +317.6 [+314.7, +323.5] | +8667 [+8549, +8827] | slower |
| static-in-sched | 1.358 | 11128 | +302.4 [+299.3, +304.7] | +8318 [+8066, +8420] | slower |

### cheap kernel, 6 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.46 -> 1.46 (includes this benchmark's own 4 threads), other processes used 0.08 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496448874647552 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.089 | 2909 | - | - | - |
| control-twin | 0.088 | 2890 | -0.9 [-1.6, +0.6] | -26 [-47, +17] | NOISY (IQR > difference) |
| static | 0.345 | 11290 | +289.0 [+285.6, +292.7] | +8388 [+8308, +8518] | slower |
| dyn-direct | 0.343 | 11245 | +285.3 [+282.6, +290.0] | +8306 [+8218, +8431] | slower |
| dyn | 0.467 | 15309 | +423.7 [+415.9, +427.8] | +12411 [+12220, +12436] | slower |
| static-in-sched | 0.464 | 15196 | +422.6 [+400.5, +426.5] | +12312 [+11984, +12441] | slower |

### cheap kernel, 6 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.46 -> 1.39 (includes this benchmark's own 1 threads), other processes used 0.03 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762749104441344 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.386 | 25318 | - | - | - |
| control-twin | 0.373 | 24463 | -2.2 [-3.9, -0.9] | -566 [-1015, -231] | NOISY (IQR > difference) |
| static | 1.727 | 113162 | +349.9 [+339.8, +355.5] | +87695 [+86043, +89567] | slower |
| dyn-direct | 1.693 | 110929 | +339.7 [+328.4, +350.8] | +85482 [+82827, +86969] | slower |
| dyn | 1.718 | 112560 | +344.0 [+337.0, +347.3] | +87492 [+84891, +89113] | slower |
| static-in-sched | 1.720 | 112740 | +349.9 [+340.9, +356.3] | +87200 [+85702, +89518] | slower |

### cheap kernel, 6 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.39 -> 1.39 (includes this benchmark's own 4 threads), other processes used 0.02 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762749104441344 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.113 | 29502 | - | - | - |
| control-twin | 0.099 | 25831 | -10.1 [-15.9, -5.3] | -3104 [-4778, -1622] | NOISY (IQR > difference) |
| static | 0.488 | 128027 | +331.5 [+318.8, +359.9] | +96505 [+95039, +100556] | slower |
| dyn-direct | 0.462 | 121171 | +308.6 [+286.6, +344.1] | +91624 [+87850, +97886] | slower |
| dyn | 0.575 | 150691 | +407.5 [+365.6, +439.1] | +120997 [+116801, +126051] | slower |
| static-in-sched | 0.602 | 157924 | +402.0 [+349.8, +468.4] | +126398 [+108814, +136697] | slower |

### medium kernel, 1 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.39 -> 1.33 (includes this benchmark's own 1 threads), other processes used 0.06 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 14636301070817198080 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.087 | 1113 | - | - | - |
| control-twin | 1.087 | 1113 | +0.1 [-0.8, +0.6] | +1 [-9, +6] | NOISY (IQR > difference) |
| static | 1.515 | 1551 | +39.6 [+38.8, +40.6] | +435 [+426, +451] | slower |
| dyn-direct | 1.521 | 1557 | +40.0 [+38.9, +40.6] | +445 [+426, +456] | slower |
| dyn | 1.619 | 1658 | +48.8 [+47.6, +50.0] | +542 [+529, +554] | slower |
| static-in-sched | 1.623 | 1662 | +49.4 [+48.8, +50.1] | +551 [+531, +559] | slower |

### medium kernel, 1 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.33 -> 1.33 (includes this benchmark's own 4 threads), other processes used 0.14 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 14636301070817198080 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.285 | 1169 | - | - | - |
| control-twin | 0.285 | 1167 | -0.5 [-1.1, +0.3] | -6 [-13, +3] | NOISY (IQR > difference) |
| static | 0.406 | 1663 | +41.8 [+40.4, +42.6] | +494 [+484, +502] | slower |
| dyn-direct | 0.409 | 1676 | +42.1 [+41.4, +43.8] | +499 [+487, +511] | slower |
| dyn | 0.569 | 2332 | +97.6 [+93.3, +100.5] | +1143 [+1084, +1188] | slower |
| static-in-sched | 0.553 | 2264 | +94.9 [+81.2, +97.7] | +1106 [+947, +1165] | slower |

### medium kernel, 1 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.33 -> 1.30 (includes this benchmark's own 1 threads), other processes used 0.04 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 2287251336015249408 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.068 | 8751 | - | - | - |
| control-twin | 1.069 | 8757 | +0.1 [-0.4, +0.6] | +9 [-37, +49] | NOISY (IQR > difference) |
| static | 1.506 | 12339 | +40.9 [+39.7, +42.4] | +3574 [+3457, +3715] | slower |
| dyn-direct | 1.505 | 12326 | +41.0 [+39.5, +43.0] | +3608 [+3487, +3725] | slower |
| dyn | 1.528 | 12517 | +43.5 [+41.7, +44.4] | +3784 [+3665, +3869] | slower |
| static-in-sched | 1.534 | 12570 | +42.9 [+42.3, +44.5] | +3786 [+3723, +3895] | slower |

### medium kernel, 1 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.30 -> 1.52 (includes this benchmark's own 4 threads), other processes used 0.15 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 2287251336015249408 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.281 | 9202 | - | - | - |
| control-twin | 0.280 | 9183 | -0.3 [-1.3, +0.3] | -28 [-116, +31] | NOISY (IQR > difference) |
| static | 0.395 | 12954 | +40.4 [+39.6, +41.8] | +3708 [+3667, +3853] | slower |
| dyn-direct | 0.400 | 13117 | +42.0 [+41.2, +43.8] | +3858 [+3772, +4005] | slower |
| dyn | 0.518 | 16964 | +83.4 [+82.5, +86.0] | +7781 [+7633, +7817] | slower |
| static-in-sched | 0.514 | 16849 | +82.7 [+80.9, +84.2] | +7681 [+7447, +7774] | slower |

### medium kernel, 1 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.52 -> 1.48 (includes this benchmark's own 1 threads), other processes used 0.11 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 10923728422425816576 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.112 | 72886 | - | - | - |
| control-twin | 1.099 | 72016 | -1.2 [-2.2, +0.3] | -843 [-1589, +236] | NOISY (IQR > difference) |
| static | 1.659 | 108696 | +48.4 [+47.0, +50.5] | +35352 [+34646, +36332] | slower |
| dyn-direct | 1.640 | 107492 | +48.2 [+46.3, +49.5] | +35124 [+33827, +36366] | slower |
| dyn | 1.657 | 108605 | +49.5 [+47.8, +51.5] | +35666 [+34652, +36933] | slower |
| static-in-sched | 1.645 | 107799 | +48.6 [+46.6, +52.6] | +35175 [+34199, +37904] | slower |

### medium kernel, 1 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.48 -> 1.68 (includes this benchmark's own 4 threads), other processes used 0.12 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 10923728422425816576 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.285 | 74830 | - | - | - |
| control-twin | 0.282 | 73872 | -2.1 [-3.0, -1.1] | -1509 [-2353, -806] | faster |
| static | 0.442 | 115744 | +54.5 [+52.2, +56.8] | +40744 [+39428, +42614] | slower |
| dyn-direct | 0.448 | 117396 | +56.1 [+52.4, +58.2] | +41864 [+40377, +43560] | slower |
| dyn | 1.296 | 339643 | +354.5 [+347.0, +418.1] | +264916 [+259630, +311507] | slower |
| static-in-sched | 1.283 | 336447 | +349.3 [+336.8, +364.9] | +261616 [+253636, +269495] | slower |

### medium kernel, 3 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.68 -> 1.42 (includes this benchmark's own 1 threads), other processes used 0.02 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.950 | 1996 | - | - | - |
| control-twin | 1.947 | 1994 | +0.3 [-0.8, +1.1] | +7 [-16, +23] | NOISY (IQR > difference) |
| static | 4.028 | 4124 | +106.6 [+102.5, +109.4] | +2116 [+2028, +2213] | slower |
| dyn-direct | 4.016 | 4112 | +105.5 [+103.4, +109.4] | +2108 [+2058, +2200] | slower |
| dyn | 4.465 | 4572 | +130.2 [+117.4, +134.1] | +2594 [+2354, +2661] | slower |
| static-in-sched | 4.186 | 4287 | +114.0 [+109.9, +117.4] | +2295 [+2182, +2323] | slower |

### medium kernel, 3 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.42 -> 1.62 (includes this benchmark's own 4 threads), other processes used 0.05 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.511 | 2093 | - | - | - |
| control-twin | 0.511 | 2093 | +0.2 [-0.7, +1.6] | +4 [-14, +34] | NOISY (IQR > difference) |
| static | 1.048 | 4293 | +104.0 [+102.6, +106.7] | +2185 [+2156, +2229] | slower |
| dyn-direct | 1.038 | 4250 | +103.5 [+101.6, +108.1] | +2149 [+2126, +2249] | slower |
| dyn | 1.349 | 5525 | +164.8 [+157.8, +174.2] | +3431 [+3311, +3592] | slower |
| static-in-sched | 1.254 | 5135 | +144.6 [+138.3, +154.6] | +3036 [+2935, +3262] | slower |

### medium kernel, 3 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.62 -> 1.49 (includes this benchmark's own 1 threads), other processes used 0.02 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.933 | 15834 | - | - | - |
| control-twin | 1.940 | 15889 | -0.3 [-0.5, +0.3] | -42 [-80, +51] | NOISY (IQR > difference) |
| static | 3.951 | 32371 | +105.0 [+103.4, +106.3] | +16533 [+16323, +16941] | slower |
| dyn-direct | 3.955 | 32401 | +104.7 [+102.6, +105.7] | +16477 [+16082, +16799] | slower |
| dyn | 4.012 | 32870 | +108.1 [+105.4, +110.5] | +17006 [+16657, +17463] | slower |
| static-in-sched | 3.994 | 32717 | +106.4 [+105.4, +108.7] | +16892 [+16647, +17114] | slower |

### medium kernel, 3 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.49 -> 1.61 (includes this benchmark's own 4 threads), other processes used 0.08 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.458 | 15024 | - | - | - |
| control-twin | 0.458 | 15022 | +0.2 [-0.4, +0.7] | +32 [-58, +111] | NOISY (IQR > difference) |
| static | 0.949 | 31109 | +106.1 [+105.2, +107.5] | +16043 [+15898, +16140] | slower |
| dyn-direct | 0.949 | 31112 | +107.2 [+105.8, +108.5] | +16074 [+15903, +16377] | slower |
| dyn | 1.154 | 37822 | +152.2 [+140.8, +161.5] | +22898 [+21323, +24244] | slower |
| static-in-sched | 1.181 | 38708 | +159.9 [+147.0, +163.4] | +23865 [+22238, +24740] | slower |

### medium kernel, 3 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.61 -> 1.40 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.937 | 126971 | - | - | - |
| control-twin | 1.938 | 127020 | -0.0 [-0.8, +0.6] | -43 [-1043, +812] | NOISY (IQR > difference) |
| static | 4.249 | 278440 | +118.6 [+116.1, +121.2] | +150647 [+148548, +152784] | slower |
| dyn-direct | 4.220 | 276555 | +116.7 [+113.3, +121.5] | +147659 [+143514, +153908] | slower |
| dyn | 4.348 | 284927 | +122.4 [+120.6, +885.2] | +153996 [+151788, +1117893] | NOISY (IQR > difference) |
| static-in-sched | 4.280 | 280492 | +118.7 [+116.4, +120.4] | +150871 [+149744, +153984] | slower |

### medium kernel, 3 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.40 -> 1.80 (includes this benchmark's own 4 threads), other processes used 0.05 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.510 | 133636 | - | - | - |
| control-twin | 0.505 | 132393 | -1.1 [-2.2, +0.1] | -1462 [-3131, +78] | NOISY (IQR > difference) |
| static | 1.167 | 305795 | +128.8 [+125.3, +131.2] | +172318 [+168324, +175120] | slower |
| dyn-direct | 1.146 | 300395 | +124.1 [+119.6, +128.4] | +166769 [+162741, +170971] | slower |
| dyn | 5.143 | 1348223 | +877.0 [+455.9, +925.0] | +1216689 [+623292, +1232428] | slower |
| static-in-sched | 1.378 | 361143 | +166.7 [+161.6, +181.9] | +225708 [+216115, +244378] | slower |

### medium kernel, 6 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.80 -> 1.53 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3637451306164387840 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 2.724 | 2789 | - | - | - |
| control-twin | 2.699 | 2764 | +0.1 [-0.2, +0.3] | +2 [-5, +9] | NOISY (IQR > difference) |
| static | 6.079 | 6225 | +124.0 [+122.2, +125.6] | +3498 [+3376, +3581] | slower |
| dyn-direct | 6.068 | 6214 | +125.1 [+122.5, +126.9] | +3477 [+3424, +3592] | slower |
| dyn | 6.585 | 6744 | +144.2 [+142.9, +146.1] | +3990 [+3938, +4214] | slower |
| static-in-sched | 6.263 | 6414 | +128.9 [+126.3, +130.7] | +3638 [+3507, +3767] | slower |

### medium kernel, 6 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.53 -> 1.72 (includes this benchmark's own 4 threads), other processes used 0.06 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3637451306164387840 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.681 | 2790 | - | - | - |
| control-twin | 0.681 | 2788 | -0.1 [-0.1, +0.0] | -2 [-4, +1] | NOISY (IQR > difference) |
| static | 1.495 | 6125 | +119.5 [+119.3, +119.7] | +3333 [+3330, +3340] | slower |
| dyn-direct | 1.501 | 6150 | +120.4 [+120.2, +121.2] | +3360 [+3356, +3382] | slower |
| dyn | 1.853 | 7588 | +172.0 [+165.9, +191.6] | +4800 [+4623, +5346] | slower |
| static-in-sched | 1.711 | 7009 | +151.2 [+149.1, +157.7] | +4229 [+4154, +4401] | slower |

### medium kernel, 6 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.72 -> 1.50 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 11521247186755477504 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 2.644 | 21657 | - | - | - |
| control-twin | 2.642 | 21646 | -0.0 [-0.1, +0.0] | -9 [-32, +8] | NOISY (IQR > difference) |
| static | 6.196 | 50761 | +134.3 [+133.7, +134.8] | +29112 [+28980, +29165] | slower |
| dyn-direct | 6.218 | 50935 | +135.2 [+134.9, +136.1] | +29278 [+29215, +29489] | slower |
| dyn | 6.120 | 50137 | +131.7 [+130.4, +132.9] | +28498 [+28240, +28805] | slower |
| static-in-sched | 6.239 | 51114 | +135.9 [+135.6, +136.7] | +29446 [+29365, +29595] | slower |

### medium kernel, 6 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.50 -> 1.70 (includes this benchmark's own 4 threads), other processes used 0.09 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 11521247186755477504 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.666 | 21810 | - | - | - |
| control-twin | 0.665 | 21807 | -0.0 [-0.2, +0.1] | -0 [-44, +21] | NOISY (IQR > difference) |
| static | 1.556 | 50987 | +133.8 [+133.3, +134.4] | +29178 [+29059, +29277] | slower |
| dyn-direct | 1.569 | 51409 | +135.8 [+134.4, +136.4] | +29600 [+29293, +29710] | slower |
| dyn | 1.724 | 56500 | +159.5 [+154.4, +167.7] | +34738 [+33671, +36915] | slower |
| static-in-sched | 1.752 | 57422 | +162.8 [+158.7, +180.7] | +35571 [+34580, +39402] | slower |

### medium kernel, 6 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.70 -> 1.43 (includes this benchmark's own 1 threads), other processes used 0.02 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7316847049341483008 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 2.694 | 176585 | - | - | - |
| control-twin | 2.697 | 176750 | +0.1 [-0.3, +0.2] | +178 [-449, +339] | NOISY (IQR > difference) |
| static | 6.648 | 435687 | +146.7 [+144.4, +148.2] | +259049 [+254998, +260342] | slower |
| dyn-direct | 6.593 | 432066 | +144.8 [+143.2, +146.2] | +255481 [+252947, +257546] | slower |
| dyn | 6.572 | 430707 | +144.2 [+143.0, +147.0] | +254334 [+251927, +257927] | slower |
| static-in-sched | 6.629 | 434413 | +146.7 [+145.2, +147.6] | +258563 [+256213, +259470] | slower |

### medium kernel, 6 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.43 -> 1.82 (includes this benchmark's own 4 threads), other processes used 0.06 cores, run started at load1 0.83: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7316847049341483008 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.691 | 181033 | - | - | - |
| control-twin | 0.678 | 177636 | -1.5 [-2.5, -0.2] | -2639 [-4518, -301] | NOISY (IQR > difference) |
| static | 1.891 | 495814 | +173.9 [+173.2, +175.9] | +314425 [+313596, +318176] | slower |
| dyn-direct | 1.849 | 484702 | +167.5 [+165.5, +171.6] | +302753 [+298807, +308754] | slower |
| dyn | 5.250 | 1376353 | +646.8 [+398.3, +674.0] | +1194859 [+718748, +1222769] | slower |
| static-in-sched | 2.073 | 543492 | +202.6 [+195.1, +210.5] | +362735 [+352719, +379450] | slower |

## Branching: 1 producer -> exchange -> 2 consumers, on 3 threads

`hand` = 3 OS threads and a Mutex/Condvar bounded queue calling the same kernels; `proto` = the real scheduler and exchange of spikes/operator-trait. Control = `hand`.

### branching, cheap kernel, 1024 rows/batch, exchange capacity 1

load1 1.82 -> 1.68 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 11.537 | 35441 | - | - | - |
| hand-twin | 11.511 | 35360 | -0.4 [-0.9, +0.6] | -140 [-336, +203] | NOISY (IQR > difference) |
| proto | 3.559 | 10935 | -69.1 [-69.4, -69.0] | -24467 [-24732, -24383] | faster |

### branching, cheap kernel, 1024 rows/batch, exchange capacity 4

load1 1.68 -> 1.71 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.884 | 8860 | - | - | - |
| hand-twin | 2.905 | 8923 | +0.2 [-0.8, +1.8] | +19 [-69, +154] | NOISY (IQR > difference) |
| proto | 3.292 | 10114 | +14.4 [+13.2, +15.8] | +1256 [+1169, +1379] | slower |

### branching, cheap kernel, 8192 rows/batch, exchange capacity 1

load1 1.71 -> 1.73 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.407 | 34590 | - | - | - |
| hand-twin | 1.401 | 34432 | -0.4 [-1.1, +0.1] | -151 [-392, +31] | NOISY (IQR > difference) |
| proto | 1.687 | 41452 | +19.5 [+19.3, +20.3] | +6762 [+6696, +6989] | slower |

### branching, cheap kernel, 8192 rows/batch, exchange capacity 4

load1 1.73 -> 1.75 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.406 | 34551 | - | - | - |
| hand-twin | 1.405 | 34528 | +0.1 [-0.6, +0.3] | +27 [-208, +89] | NOISY (IQR > difference) |
| proto | 1.690 | 41541 | +20.0 [+19.8, +21.0] | +6908 [+6854, +7249] | slower |

### branching, cheap kernel, 65536 rows/batch, exchange capacity 1

load1 1.75 -> 1.75 (includes this benchmark's own 3 threads), other processes used 0.11 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.605 | 118920 | - | - | - |
| hand-twin | 0.600 | 117910 | -0.9 [-2.9, +2.0] | -1071 [-3672, +2375] | NOISY (IQR > difference) |
| proto | 0.644 | 126588 | +4.7 [+3.8, +7.9] | +5541 [+4653, +9417] | slower |

### branching, cheap kernel, 65536 rows/batch, exchange capacity 4

load1 1.75 -> 1.85 (includes this benchmark's own 3 threads), other processes used 0.05 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.601 | 118217 | - | - | - |
| hand-twin | 0.605 | 119037 | +0.9 [-1.8, +3.9] | +1076 [-2112, +4446] | NOISY (IQR > difference) |
| proto | 0.644 | 126627 | +7.4 [+4.3, +10.1] | +8954 [+5001, +11913] | slower |

### branching, medium kernel, 1024 rows/batch, exchange capacity 1

load1 1.85 -> 2.11 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 5.826 | 17897 | - | - | - |
| hand-twin | 5.907 | 18146 | +1.7 [-0.8, +5.9] | +309 [-150, +1027] | NOISY (IQR > difference) |
| proto | 5.343 | 16414 | -8.8 [-10.8, -4.8] | -1572 [-1981, -831] | faster |

### branching, medium kernel, 1024 rows/batch, exchange capacity 4

load1 2.11 -> 2.17 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 4.085 | 12548 | - | - | - |
| hand-twin | 4.042 | 12418 | -1.1 [-2.7, +1.0] | -136 [-344, +126] | NOISY (IQR > difference) |
| proto | 5.414 | 16633 | +32.3 [+27.1, +33.4] | +4062 [+3390, +4207] | slower |

### branching, medium kernel, 8192 rows/batch, exchange capacity 1

load1 2.17 -> 2.16 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.285 | 56162 | - | - | - |
| hand-twin | 2.285 | 56153 | +0.2 [-0.8, +0.9] | +112 [-468, +482] | NOISY (IQR > difference) |
| proto | 2.523 | 62013 | +10.5 [+9.8, +11.3] | +5925 [+5494, +6355] | slower |

### branching, medium kernel, 8192 rows/batch, exchange capacity 4

load1 2.16 -> 2.23 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.292 | 56338 | - | - | - |
| hand-twin | 2.286 | 56172 | -0.1 [-1.3, +0.8] | -83 [-741, +454] | NOISY (IQR > difference) |
| proto | 2.525 | 62062 | +10.2 [+9.1, +11.1] | +5751 [+5097, +6203] | slower |

### branching, medium kernel, 65536 rows/batch, exchange capacity 1

load1 2.13 -> 2.13 (includes this benchmark's own 3 threads), other processes used 0.07 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.819 | 357699 | - | - | - |
| hand-twin | 1.830 | 359836 | +0.4 [-0.3, +1.1] | +1600 [-955, +3972] | NOISY (IQR > difference) |
| proto | 1.855 | 364676 | +1.3 [-0.3, +4.0] | +4784 [-1250, +13995] | NOISY (IQR > difference) |

### branching, medium kernel, 65536 rows/batch, exchange capacity 4

load1 2.13 -> 2.20 (includes this benchmark's own 3 threads), other processes used 0.03 cores, run started at load1 0.83: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.821 | 357940 | - | - | - |
| hand-twin | 1.830 | 359710 | +0.5 [-0.1, +1.3] | +1770 [-270, +4668] | NOISY (IQR > difference) |
| proto | 1.844 | 362561 | +1.2 [-0.0, +2.6] | +4306 [-121, +9250] | NOISY (IQR > difference) |

## Summary

Overhead of each arm against the hand-fused control, median of the paired per-round ratios, in percent. `?` = the interquartile range is wider than the overhead (not a result). `twin` is the control against itself: the noise floor of that cell. Cells not labelled quiet are marked `S` (SANITY ONLY).

| cell | control ns/row | static | dyn-direct | dyn | static-in-sched | twin | quiet |
|---|---|---|---|---|---|---|---|
| cheap L1 B1024 T1 | 0.328 | +54.2 | +56.0 | +73.3 | +73.1 | -0.3 ? | yes |
| cheap L1 B1024 T4 | 0.095 | +49.6 | +49.4 | +120.9 | +118.4 | -0.2 ? | yes |
| cheap L1 B8192 T1 | 0.335 | +49.1 | +50.2 | +54.3 | +54.3 | -0.2 ? | yes |
| cheap L1 B8192 T4 | 0.092 | +47.7 | +47.3 | +97.3 | +96.5 | -0.0 ? | yes |
| cheap L1 B65536 T1 | 0.383 | +60.8 | +61.8 | +62.9 | +63.9 | -0.8 ? | yes |
| cheap L1 B65536 T4 | 0.102 | +59.2 | +61.4 | +111.0 | +109.9 | -0.2 ? | yes |
| cheap L3 B1024 T1 | 0.340 | +155.2 | +159.5 | +205.0 | +180.3 | +1.5 ? | yes |
| cheap L3 B1024 T4 | 0.101 | +130.7 | +136.6 | +263.8 | +233.6 | +0.2 ? | yes |
| cheap L3 B8192 T1 | 0.334 | +149.3 | +151.2 | +159.8 | +154.5 | -0.3 ? | yes |
| cheap L3 B8192 T4 | 0.090 | +143.4 | +146.1 | +234.9 | +224.4 | +0.2 ? | yes |
| cheap L3 B65536 T1 | 0.379 | +175.1 | +176.6 | +178.7 | +176.1 | +0.9 ? | yes |
| cheap L3 B65536 T4 | 0.108 | +170.3 | +170.4 | +235.2 | +241.8 | -4.3 ? | yes |
| cheap L6 B1024 T1 | 0.352 | +297.9 | +291.0 | +367.5 | +317.8 | -2.8 ? | yes |
| cheap L6 B1024 T4 | 0.107 | +232.2 | +231.5 | +425.6 | +362.2 | -1.0 ? | yes |
| cheap L6 B8192 T1 | 0.337 | +297.6 | +300.5 | +317.6 | +302.4 | +0.3 ? | yes |
| cheap L6 B8192 T4 | 0.089 | +289.0 | +285.3 | +423.7 | +422.6 | -0.9 ? | yes |
| cheap L6 B65536 T1 | 0.386 | +349.9 | +339.7 | +344.0 | +349.9 | -2.2 ? | yes |
| cheap L6 B65536 T4 | 0.113 | +331.5 | +308.6 | +407.5 | +402.0 | -10.1 ? | yes |
| medium L1 B1024 T1 | 1.087 | +39.6 | +40.0 | +48.8 | +49.4 | +0.1 ? | yes |
| medium L1 B1024 T4 | 0.285 | +41.8 | +42.1 | +97.6 | +94.9 | -0.5 ? | yes |
| medium L1 B8192 T1 | 1.068 | +40.9 | +41.0 | +43.5 | +42.9 | +0.1 ? | yes |
| medium L1 B8192 T4 | 0.281 | +40.4 | +42.0 | +83.4 | +82.7 | -0.3 ? | yes |
| medium L1 B65536 T1 | 1.112 | +48.4 | +48.2 | +49.5 | +48.6 | -1.2 ? | yes |
| medium L1 B65536 T4 | 0.285 | +54.5 | +56.1 | +354.5 | +349.3 | -2.1 | yes |
| medium L3 B1024 T1 | 1.950 | +106.6 | +105.5 | +130.2 | +114.0 | +0.3 ? | yes |
| medium L3 B1024 T4 | 0.511 | +104.0 | +103.5 | +164.8 | +144.6 | +0.2 ? | yes |
| medium L3 B8192 T1 | 1.933 | +105.0 | +104.7 | +108.1 | +106.4 | -0.3 ? | yes |
| medium L3 B8192 T4 | 0.458 | +106.1 | +107.2 | +152.2 | +159.9 | +0.2 ? | yes |
| medium L3 B65536 T1 | 1.937 | +118.6 | +116.7 | +122.4 ? | +118.7 | -0.0 ? | yes |
| medium L3 B65536 T4 | 0.510 | +128.8 | +124.1 | +877.0 | +166.7 | -1.1 ? | yes |
| medium L6 B1024 T1 | 2.724 | +124.0 | +125.1 | +144.2 | +128.9 | +0.1 ? | yes |
| medium L6 B1024 T4 | 0.681 | +119.5 | +120.4 | +172.0 | +151.2 | -0.1 ? | yes |
| medium L6 B8192 T1 | 2.644 | +134.3 | +135.2 | +131.7 | +135.9 | -0.0 ? | yes |
| medium L6 B8192 T4 | 0.666 | +133.8 | +135.8 | +159.5 | +162.8 | -0.0 ? | yes |
| medium L6 B65536 T1 | 2.694 | +146.7 | +144.8 | +144.2 | +146.7 | +0.1 ? | yes |
| medium L6 B65536 T4 | 0.691 | +173.9 | +167.5 | +646.8 | +202.6 | -1.5 ? | yes |

| branching cell | hand ns/row | proto overhead % | twin | quiet |
|---|---|---|---|---|
| cheap B1024 cap1 | 11.537 | -69.1 | -0.4 ? | yes |
| cheap B1024 cap4 | 2.884 | +14.4 | +0.2 ? | yes |
| cheap B8192 cap1 | 1.407 | +19.5 | -0.4 ? | yes |
| cheap B8192 cap4 | 1.406 | +20.0 | +0.1 ? | yes |
| cheap B65536 cap1 | 0.605 | +4.7 | -0.9 ? | yes |
| cheap B65536 cap4 | 0.601 | +7.4 | +0.9 ? | yes |
| medium B1024 cap1 | 5.826 | -8.8 | +1.7 ? | yes |
| medium B1024 cap4 | 4.085 | +32.3 | -1.1 ? | yes |
| medium B8192 cap1 | 2.285 | +10.5 | +0.2 ? | yes |
| medium B8192 cap4 | 2.292 | +10.2 | -0.1 ? | yes |
| medium B65536 cap1 | 1.819 | +1.3 ? | +0.4 ? | yes |
| medium B65536 cap4 | 1.821 | +1.2 ? | +0.5 ? | yes |

All cells were measured with the run-start load average < 2, no other process using >= 0.5 core, and a validated harness: **quiet**.
