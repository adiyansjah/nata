# push-cost benchmark

- arch `aarch64`, 8 CPUs visible, rounds 25 (after 1 discarded warm-up round), rows per run 2^25 = 33554432
- 1-minute load average at start, before any benchmark thread ran: **0.72**

## Harness self-test

Arms: `control`, `control-twin` (same code), and `control+spinNns` = the control plus a busy-wait of N ns after every batch. Cell: cheap kernel, 1 stage, 1024 rows per batch, 33554432 rows per run, 25 interleaved rounds. Expected difference = the spin measured on its own on one thread. PASS needs: |median - expected| <= max(25% of expected, 15 ns), interquartile range not wider than the median, and the twin within max(10% of a control batch, 20 ns) of zero.

### 1 thread(s): control = 339 ns per batch per worker

| arm | injected (nominal ns) | injected (measured ns) | measured difference ns/batch, median [q1, q3] | tolerance | result |
|---|---|---|---|---|---|
| control-twin | 0 | 0 | -0.5 [-3.4, +1.4] | ±34 | PASS (no difference seen) |
| control+spin100ns | 100 | 151 | +153.2 [+150.6, +154.4] | ±38 | PASS |
| control+spin400ns | 400 | 455 | +454.9 [+454.0, +456.2] | ±114 | PASS |
| control+spin1600ns | 1600 | 1658 | +1659.0 [+1657.1, +1663.9] | ±415 | PASS |

### 4 thread(s): control = 493 ns per batch per worker

| arm | injected (nominal ns) | injected (measured ns) | measured difference ns/batch, median [q1, q3] | tolerance | result |
|---|---|---|---|---|---|
| control-twin | 0 | 0 | +18.7 [-11.3, +32.9] | ±49 | PASS (no difference seen) |
| control+spin100ns | 100 | 151 | +158.4 [+144.7, +185.8] | ±38 | PASS |
| control+spin400ns | 400 | 455 | +460.4 [+443.3, +479.1] | ±114 | PASS |
| control+spin1600ns | 1600 | 1658 | +1689.3 [+1669.7, +1704.9] | ±415 | PASS |

**Harness self-test: PASS - the harness measures an injected cost of 100 to 1600 ns per batch correctly**

## Fixed cost of one run (zero morsels)

Median of 31 interleaved runs, microseconds. This is included in every timing below.

| threads | control | static | dyn-direct | dyn | static-in-sched |
|---|---|---|---|---|---|
| 1 | 33 | 34 | 34 | 48 | 48 |
| 4 | 79 | 80 | 84 | 93 | 89 |

## Linear chain: control vs static vs dyn

Arms: see `src/arms.rs`. Overhead = (arm - control) / control per round, then median [q1, q3] over the rounds.

### cheap kernel, 1 stage(s), 1024 rows/batch, 1 thread(s)

load1 0.74 -> 0.74 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955456856260608 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.336 | 344 | - | - | - |
| control-twin | 0.336 | 344 | -0.2 [-0.5, +0.2] | -1 [-2, +1] | NOISY (IQR > difference) |
| static | 0.417 | 427 | +23.9 [+23.6, +24.2] | +82 [+81, +83] | slower |
| dyn-direct | 0.420 | 431 | +25.1 [+24.5, +25.3] | +86 [+85, +87] | slower |
| dyn | 0.488 | 499 | +44.8 [+44.2, +45.4] | +155 [+153, +156] | slower |
| static-in-sched | 0.487 | 499 | +44.9 [+44.3, +45.3] | +155 [+153, +156] | slower |

### cheap kernel, 1 stage(s), 1024 rows/batch, 4 thread(s)

load1 0.74 -> 0.74 (includes this benchmark's own 4 threads), other processes used 0.01 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955456856260608 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.121 | 495 | - | - | - |
| control-twin | 0.126 | 516 | +3.8 [-3.2, +6.1] | +18 [-16, +29] | NOISY (IQR > difference) |
| static | 0.128 | 524 | +3.7 [+0.0, +7.0] | +18 [+0, +33] | NOISY (IQR > difference) |
| dyn-direct | 0.128 | 523 | +2.6 [+0.3, +8.6] | +14 [+2, +42] | NOISY (IQR > difference) |
| dyn | 0.163 | 668 | +31.2 [+27.2, +37.6] | +159 [+138, +185] | slower |
| static-in-sched | 0.166 | 678 | +37.4 [+28.3, +39.5] | +185 [+143, +195] | slower |

### cheap kernel, 1 stage(s), 8192 rows/batch, 1 thread(s)

load1 0.74 -> 0.68 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447196925952 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.315 | 2579 | - | - | - |
| control-twin | 0.315 | 2582 | +0.1 [-0.1, +0.3] | +3 [-2, +8] | NOISY (IQR > difference) |
| static | 0.547 | 4479 | +73.6 [+73.4, +74.0] | +1898 [+1894, +1907] | slower |
| dyn-direct | 0.547 | 4483 | +73.9 [+73.5, +74.0] | +1905 [+1898, +1907] | slower |
| dyn | 0.556 | 4552 | +76.5 [+76.3, +76.7] | +1975 [+1970, +1976] | slower |
| static-in-sched | 0.557 | 4560 | +76.7 [+76.4, +77.1] | +1978 [+1973, +1986] | slower |

### cheap kernel, 1 stage(s), 8192 rows/batch, 4 thread(s)

load1 0.68 -> 0.68 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447196925952 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.085 | 2795 | - | - | - |
| control-twin | 0.085 | 2789 | -0.2 [-0.7, +0.6] | -5 [-19, +17] | NOISY (IQR > difference) |
| static | 0.141 | 4618 | +65.4 [+64.2, +66.2] | +1827 [+1803, +1839] | slower |
| dyn-direct | 0.141 | 4636 | +66.0 [+65.1, +66.9] | +1845 [+1828, +1862] | slower |
| dyn | 0.146 | 4780 | +71.5 [+69.9, +72.7] | +1994 [+1967, +2018] | slower |
| static-in-sched | 0.146 | 4789 | +71.2 [+70.1, +71.8] | +1986 [+1968, +2017] | slower |

### cheap kernel, 1 stage(s), 65536 rows/batch, 1 thread(s)

load1 0.68 -> 0.71 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747426719744 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.381 | 24978 | - | - | - |
| control-twin | 0.380 | 24925 | -0.3 [-1.6, +0.4] | -64 [-395, +108] | NOISY (IQR > difference) |
| static | 0.582 | 38173 | +53.0 [+52.1, +53.9] | +13217 [+13052, +13383] | slower |
| dyn-direct | 0.588 | 38525 | +53.7 [+52.0, +55.4] | +13486 [+12918, +13898] | slower |
| dyn | 0.587 | 38492 | +53.9 [+53.5, +55.5] | +13531 [+13242, +13904] | slower |
| static-in-sched | 0.588 | 38520 | +54.2 [+52.5, +55.0] | +13510 [+13063, +13780] | slower |

### cheap kernel, 1 stage(s), 65536 rows/batch, 4 thread(s)

load1 0.71 -> 0.71 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747426719744 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.098 | 25580 | - | - | - |
| control-twin | 0.097 | 25487 | -0.8 [-1.2, +0.3] | -197 [-317, +79] | NOISY (IQR > difference) |
| static | 0.142 | 37208 | +45.9 [+45.1, +46.5] | +11704 [+11518, +11843] | slower |
| dyn-direct | 0.142 | 37242 | +46.4 [+44.9, +46.8] | +11799 [+11507, +11941] | slower |
| dyn | 0.144 | 37829 | +48.4 [+47.7, +48.9] | +12330 [+12203, +12515] | slower |
| static-in-sched | 0.145 | 37934 | +48.3 [+46.8, +50.2] | +12396 [+12084, +12725] | slower |

### cheap kernel, 3 stage(s), 1024 rows/batch, 1 thread(s)

load1 0.71 -> 0.71 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.321 | 329 | - | - | - |
| control-twin | 0.322 | 330 | -0.1 [-1.3, +0.3] | -0 [-5, +1] | NOISY (IQR > difference) |
| static | 0.680 | 697 | +109.5 [+108.6, +146.4] | +361 [+357, +478] | slower |
| dyn-direct | 0.682 | 698 | +109.3 [+107.9, +116.3] | +361 [+356, +382] | slower |
| dyn | 1.081 | 1107 | +230.9 [+226.7, +242.3] | +771 [+763, +793] | slower |
| static-in-sched | 0.753 | 771 | +131.8 [+130.2, +162.4] | +436 [+432, +538] | slower |

### cheap kernel, 3 stage(s), 1024 rows/batch, 4 thread(s)

load1 0.71 -> 0.81 (includes this benchmark's own 4 threads), other processes used 0.05 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.105 | 432 | - | - | - |
| control-twin | 0.103 | 420 | -1.9 [-4.8, +1.6] | -8 [-21, +7] | NOISY (IQR > difference) |
| static | 0.195 | 800 | +89.3 [+81.7, +94.0] | +380 [+354, +404] | slower |
| dyn-direct | 0.201 | 821 | +90.7 [+81.2, +97.6] | +390 [+350, +411] | slower |
| dyn | 0.301 | 1234 | +185.9 [+172.4, +198.1] | +802 [+748, +846] | slower |
| static-in-sched | 0.230 | 941 | +118.8 [+114.5, +127.4] | +503 [+488, +541] | slower |

### cheap kernel, 3 stage(s), 8192 rows/batch, 1 thread(s)

load1 0.81 -> 0.81 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.349 | 2862 | - | - | - |
| control-twin | 0.348 | 2855 | -0.0 [-0.3, +0.3] | -0 [-8, +9] | NOISY (IQR > difference) |
| static | 0.943 | 7728 | +170.4 [+169.4, +172.1] | +4867 [+4845, +4890] | slower |
| dyn-direct | 0.942 | 7718 | +169.6 [+168.9, +171.9] | +4855 [+4845, +4864] | slower |
| dyn | 0.934 | 7648 | +167.1 [+166.6, +171.5] | +4784 [+4764, +4858] | slower |
| static-in-sched | 0.951 | 7792 | +172.2 [+171.5, +177.1] | +4928 [+4907, +5012] | slower |

### cheap kernel, 3 stage(s), 8192 rows/batch, 4 thread(s)

load1 0.81 -> 1.07 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.088 | 2898 | - | - | - |
| control-twin | 0.088 | 2900 | +0.2 [-0.8, +1.0] | +7 [-24, +29] | NOISY (IQR > difference) |
| static | 0.241 | 7894 | +171.8 [+169.4, +173.9] | +4995 [+4931, +5031] | slower |
| dyn-direct | 0.241 | 7901 | +172.0 [+169.7, +174.0] | +4988 [+4958, +5030] | slower |
| dyn | 0.241 | 7892 | +172.3 [+170.1, +175.2] | +4992 [+4935, +5040] | slower |
| static-in-sched | 0.245 | 8019 | +177.4 [+175.6, +179.0] | +5129 [+5091, +5169] | slower |

### cheap kernel, 3 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.07 -> 1.07 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.368 | 24112 | - | - | - |
| control-twin | 0.367 | 24062 | +0.1 [-0.6, +1.5] | +19 [-159, +353] | NOISY (IQR > difference) |
| static | 1.022 | 67008 | +186.2 [+170.5, +198.8] | +43438 [+41889, +47340] | slower |
| dyn-direct | 1.053 | 68986 | +190.2 [+177.3, +194.6] | +44585 [+43462, +47810] | slower |
| dyn | 1.002 | 65639 | +175.2 [+161.7, +179.6] | +41932 [+39575, +42850] | slower |
| static-in-sched | 1.070 | 70124 | +194.1 [+178.5, +199.5] | +46027 [+44558, +48517] | slower |

### cheap kernel, 3 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.07 -> 1.22 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.101 | 26547 | - | - | - |
| control-twin | 0.097 | 25420 | -1.6 [-6.0, +0.6] | -419 [-1610, +169] | NOISY (IQR > difference) |
| static | 0.273 | 71658 | +172.2 [+167.3, +175.1] | +45320 [+44460, +46061] | slower |
| dyn-direct | 0.275 | 72165 | +171.8 [+168.0, +178.8] | +45610 [+44551, +46827] | slower |
| dyn | 0.252 | 66125 | +150.9 [+148.6, +153.3] | +39828 [+39238, +40733] | slower |
| static-in-sched | 0.278 | 72833 | +177.0 [+168.6, +183.5] | +46413 [+45590, +47867] | slower |

### cheap kernel, 6 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.22 -> 1.20 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955458533982208 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.334 | 342 | - | - | - |
| control-twin | 0.333 | 341 | +0.2 [-0.8, +0.7] | +1 [-3, +2] | NOISY (IQR > difference) |
| static | 1.701 | 1742 | +409.9 [+407.9, +412.7] | +1399 [+1395, +1404] | slower |
| dyn-direct | 1.700 | 1741 | +410.0 [+408.3, +412.2] | +1401 [+1395, +1406] | slower |
| dyn | 1.765 | 1807 | +429.9 [+427.7, +432.4] | +1466 [+1463, +1469] | slower |
| static-in-sched | 1.745 | 1787 | +423.7 [+420.1, +426.2] | +1446 [+1438, +1450] | slower |

### cheap kernel, 6 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.20 -> 1.20 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3550955458533982208 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.122 | 498 | - | - | - |
| control-twin | 0.121 | 496 | -1.0 [-4.3, +4.5] | -5 [-22, +22] | NOISY (IQR > difference) |
| static | 0.349 | 1429 | +190.9 [+181.9, +198.9] | +944 [+906, +997] | slower |
| dyn-direct | 0.357 | 1463 | +197.7 [+181.2, +206.3] | +964 [+928, +1013] | slower |
| dyn | 0.492 | 2015 | +301.7 [+286.9, +315.4] | +1508 [+1454, +1541] | slower |
| static-in-sched | 0.402 | 1648 | +229.2 [+216.9, +241.2] | +1137 [+1109, +1196] | slower |

### cheap kernel, 6 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.20 -> 1.19 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496448874647552 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.306 | 2504 | - | - | - |
| control-twin | 0.305 | 2498 | -0.4 [-0.8, +0.4] | -11 [-20, +10] | NOISY (IQR > difference) |
| static | 1.569 | 12850 | +413.2 [+411.7, +416.1] | +10351 [+10336, +10367] | slower |
| dyn-direct | 1.570 | 12858 | +414.0 [+411.3, +416.5] | +10360 [+10335, +10372] | slower |
| dyn | 1.459 | 11954 | +376.3 [+374.7, +380.9] | +9460 [+9385, +9491] | slower |
| static-in-sched | 1.580 | 12947 | +417.8 [+414.9, +420.3] | +10458 [+10420, +10470] | slower |

### cheap kernel, 6 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.19 -> 1.17 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18425496448874647552 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.089 | 2912 | - | - | - |
| control-twin | 0.088 | 2894 | -0.5 [-2.0, +0.6] | -13 [-58, +18] | NOISY (IQR > difference) |
| static | 0.395 | 12945 | +344.4 [+341.1, +349.7] | +10029 [+9986, +10109] | slower |
| dyn-direct | 0.399 | 13071 | +348.0 [+343.4, +352.7] | +10134 [+10073, +10206] | slower |
| dyn | 0.382 | 12526 | +329.7 [+326.9, +333.0] | +9597 [+9567, +9668] | slower |
| static-in-sched | 0.406 | 13292 | +356.5 [+351.5, +359.3] | +10372 [+10246, +10464] | slower |

### cheap kernel, 6 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.17 -> 1.16 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762749104441344 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.374 | 24501 | - | - | - |
| control-twin | 0.370 | 24267 | -0.7 [-1.4, +1.1] | -163 [-365, +289] | NOISY (IQR > difference) |
| static | 1.845 | 120901 | +387.3 [+375.7, +410.4] | +95486 [+89807, +100175] | slower |
| dyn-direct | 1.926 | 126208 | +404.0 [+391.9, +428.4] | +100865 [+97097, +103965] | slower |
| dyn | 1.607 | 105330 | +327.2 [+319.3, +339.8] | +80435 [+79966, +81544] | slower |
| static-in-sched | 1.912 | 125276 | +412.9 [+386.3, +429.2] | +101493 [+96878, +104541] | slower |

### cheap kernel, 6 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.16 -> 1.39 (includes this benchmark's own 4 threads), other processes used 0.02 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7213762749104441344 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.101 | 26455 | - | - | - |
| control-twin | 0.096 | 25158 | -5.4 [-8.1, -3.2] | -1473 [-2240, -833] | faster |
| static | 0.479 | 125601 | +370.2 [+356.2, +400.9] | +98351 [+95750, +101968] | slower |
| dyn-direct | 0.493 | 129268 | +384.7 [+371.7, +413.4] | +102316 [+99400, +109388] | slower |
| dyn | 0.406 | 106305 | +299.7 [+287.3, +307.0] | +79155 [+78720, +79847] | slower |
| static-in-sched | 0.528 | 138402 | +416.6 [+382.8, +447.3] | +111843 [+102542, +116349] | slower |

### medium kernel, 1 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.39 -> 1.35 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 14636301070817198080 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.134 | 1161 | - | - | - |
| control-twin | 1.134 | 1161 | +0.0 [-0.0, +0.1] | +0 [-0, +1] | NOISY (IQR > difference) |
| static | 1.382 | 1415 | +22.0 [+21.8, +22.0] | +255 [+254, +255] | slower |
| dyn-direct | 1.381 | 1415 | +21.9 [+21.8, +22.0] | +254 [+253, +255] | slower |
| dyn | 1.486 | 1522 | +31.1 [+31.0, +31.2] | +361 [+360, +363] | slower |
| static-in-sched | 1.487 | 1523 | +31.1 [+31.1, +31.3] | +362 [+361, +363] | slower |

### medium kernel, 1 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.35 -> 1.35 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 14636301070817198080 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.330 | 1351 | - | - | - |
| control-twin | 0.329 | 1349 | +0.7 [-0.7, +1.4] | +9 [-9, +19] | NOISY (IQR > difference) |
| static | 0.370 | 1516 | +12.7 [+11.7, +13.7] | +172 [+158, +182] | slower |
| dyn-direct | 0.371 | 1518 | +12.9 [+11.6, +14.4] | +170 [+158, +192] | slower |
| dyn | 0.410 | 1677 | +24.6 [+22.9, +27.7] | +333 [+312, +367] | slower |
| static-in-sched | 0.406 | 1664 | +24.4 [+22.0, +24.9] | +322 [+296, +335] | slower |

### medium kernel, 1 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.35 -> 1.30 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 2287251336015249408 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.140 | 9341 | - | - | - |
| control-twin | 1.141 | 9344 | +0.0 [-0.1, +0.1] | +3 [-5, +10] | NOISY (IQR > difference) |
| static | 1.474 | 12079 | +29.2 [+29.1, +29.4] | +2730 [+2725, +2743] | slower |
| dyn-direct | 1.476 | 12091 | +29.4 [+29.3, +29.5] | +2752 [+2743, +2757] | slower |
| dyn | 1.490 | 12208 | +30.6 [+30.5, +30.7] | +2859 [+2850, +2871] | slower |
| static-in-sched | 1.489 | 12195 | +30.5 [+30.5, +30.6] | +2854 [+2850, +2860] | slower |

### medium kernel, 1 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.30 -> 1.30 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 2287251336015249408 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.292 | 9576 | - | - | - |
| control-twin | 0.292 | 9574 | -0.2 [-0.4, +0.3] | -18 [-36, +28] | NOISY (IQR > difference) |
| static | 0.374 | 12256 | +28.0 [+27.3, +28.3] | +2685 [+2633, +2702] | slower |
| dyn-direct | 0.374 | 12269 | +28.1 [+27.6, +28.3] | +2694 [+2639, +2706] | slower |
| dyn | 0.381 | 12482 | +30.3 [+29.6, +30.9] | +2901 [+2846, +2959] | slower |
| static-in-sched | 0.381 | 12478 | +30.1 [+29.4, +30.7] | +2873 [+2818, +2930] | slower |

### medium kernel, 1 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.30 -> 1.25 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 10923728422425816576 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.179 | 77276 | - | - | - |
| control-twin | 1.178 | 77184 | -0.1 [-0.3, +0.0] | -45 [-236, +31] | NOISY (IQR > difference) |
| static | 1.598 | 104748 | +35.6 [+34.0, +37.4] | +27558 [+26309, +28998] | slower |
| dyn-direct | 1.653 | 108357 | +40.3 [+38.3, +41.4] | +31148 [+29563, +32029] | slower |
| dyn | 1.668 | 109320 | +41.6 [+40.2, +42.9] | +32131 [+31186, +33114] | slower |
| static-in-sched | 1.680 | 110117 | +42.3 [+40.1, +43.6] | +32780 [+30899, +33687] | slower |

### medium kernel, 1 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.25 -> 1.25 (includes this benchmark's own 4 threads), other processes used 0.01 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 10923728422425816576 count 29360128

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.304 | 79775 | - | - | - |
| control-twin | 0.299 | 78377 | -1.8 [-2.2, -0.9] | -1412 [-1727, -737] | faster |
| static | 0.411 | 107648 | +35.1 [+34.4, +36.2] | +27925 [+27467, +28885] | slower |
| dyn-direct | 0.407 | 106653 | +33.7 [+33.2, +34.4] | +26884 [+26409, +27581] | slower |
| dyn | 0.412 | 107922 | +35.0 [+34.5, +35.6] | +27853 [+27529, +28322] | slower |
| static-in-sched | 0.412 | 107934 | +35.4 [+34.5, +36.2] | +28257 [+27397, +28726] | slower |

### medium kernel, 3 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.25 -> 1.20 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.561 | 1599 | - | - | - |
| control-twin | 1.575 | 1612 | +0.1 [-1.3, +1.6] | +1 [-21, +26] | NOISY (IQR > difference) |
| static | 3.096 | 3170 | +98.7 [+94.4, +115.7] | +1562 [+1513, +1874] | slower |
| dyn-direct | 3.416 | 3498 | +117.2 [+99.8, +121.2] | +1891 [+1643, +1930] | slower |
| dyn | 3.510 | 3594 | +124.1 [+121.1, +126.4] | +1982 [+1972, +2017] | slower |
| static-in-sched | 3.274 | 3353 | +113.8 [+103.1, +126.2] | +1784 [+1640, +2016] | slower |

### medium kernel, 3 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.20 -> 1.42 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.410 | 1681 | - | - | - |
| control-twin | 0.410 | 1679 | -0.0 [-0.9, +0.6] | -0 [-15, +9] | NOISY (IQR > difference) |
| static | 0.782 | 3202 | +90.7 [+89.6, +92.2] | +1524 [+1514, +1537] | slower |
| dyn-direct | 0.789 | 3230 | +93.1 [+91.1, +94.8] | +1562 [+1531, +1591] | slower |
| dyn | 0.919 | 3765 | +124.6 [+121.9, +128.2] | +2083 [+2053, +2131] | slower |
| static-in-sched | 0.829 | 3396 | +101.7 [+100.3, +103.6] | +1710 [+1686, +1740] | slower |

### medium kernel, 3 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.42 -> 1.33 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 3.399 | 27848 | - | - | - |
| control-twin | 3.391 | 27779 | -0.2 [-2.4, +2.9] | -62 [-688, +818] | NOISY (IQR > difference) |
| static | 3.197 | 26191 | -5.8 [-7.2, -4.8] | -1610 [-2031, -1331] | faster |
| dyn-direct | 3.203 | 26241 | -5.7 [-7.2, -4.8] | -1568 [-2026, -1318] | faster |
| dyn | 3.241 | 26552 | -4.5 [-7.1, -3.7] | -1252 [-2033, -1029] | faster |
| static-in-sched | 3.209 | 26292 | -5.4 [-6.6, -4.6] | -1513 [-1876, -1264] | faster |

### medium kernel, 3 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.33 -> 1.54 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.811 | 26583 | - | - | - |
| control-twin | 0.809 | 26525 | -0.0 [-0.4, +0.1] | -12 [-102, +30] | NOISY (IQR > difference) |
| static | 0.806 | 26418 | -0.7 [-0.9, -0.5] | -172 [-244, -136] | faster |
| dyn-direct | 0.808 | 26467 | -0.5 [-0.8, -0.1] | -141 [-218, -35] | NOISY (IQR > difference) |
| dyn | 0.823 | 26954 | +1.4 [+1.1, +1.6] | +364 [+299, +429] | slower |
| static-in-sched | 0.810 | 26537 | -0.0 [-0.3, +0.3] | -5 [-85, +90] | NOISY (IQR > difference) |

### medium kernel, 3 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.54 -> 1.36 (includes this benchmark's own 1 threads), other processes used 0.01 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 3.986 | 261204 | - | - | - |
| control-twin | 3.987 | 261284 | +0.0 [-0.3, +0.6] | +12 [-655, +1492] | NOISY (IQR > difference) |
| static | 4.314 | 282727 | +7.8 [+6.6, +9.2] | +20398 [+17180, +24017] | slower |
| dyn-direct | 4.343 | 284603 | +8.9 [+7.5, +10.8] | +23324 [+19678, +28187] | slower |
| dyn | 10.125 | 663546 | +154.9 [+149.3, +160.7] | +403030 [+391443, +420292] | slower |
| static-in-sched | 4.357 | 285552 | +9.2 [+8.2, +10.4] | +24018 [+21447, +27193] | slower |

### medium kernel, 3 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.36 -> 1.76 (includes this benchmark's own 4 threads), other processes used 0.09 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.986 | 258444 | - | - | - |
| control-twin | 0.982 | 257315 | -0.3 [-0.7, -0.0] | -860 [-1925, -16] | NOISY (IQR > difference) |
| static | 1.085 | 284354 | +10.7 [+9.1, +13.0] | +27504 [+23741, +33565] | slower |
| dyn-direct | 1.081 | 283247 | +10.0 [+8.5, +11.2] | +25737 [+22204, +28957] | slower |
| dyn | 1.992 | 522111 | +102.0 [+94.0, +178.7] | +263686 [+243091, +461841] | slower |
| static-in-sched | 1.121 | 293739 | +13.6 [+11.8, +15.4] | +35096 [+30884, +39606] | slower |

### medium kernel, 6 stage(s), 1024 rows/batch, 1 thread(s)

load1 1.76 -> 1.50 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3637451306164387840 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 2.242 | 2295 | - | - | - |
| control-twin | 2.241 | 2295 | -0.2 [-0.7, +0.5] | -4 [-15, +11] | NOISY (IQR > difference) |
| static | 5.728 | 5866 | +155.3 [+151.3, +157.3] | +3570 [+3489, +3598] | slower |
| dyn-direct | 5.745 | 5883 | +155.8 [+152.6, +157.5] | +3589 [+3498, +3607] | slower |
| dyn | 5.424 | 5554 | +142.4 [+139.0, +144.8] | +3254 [+3201, +3301] | slower |
| static-in-sched | 5.841 | 5981 | +160.7 [+158.0, +161.9] | +3691 [+3630, +3715] | slower |

### medium kernel, 6 stage(s), 1024 rows/batch, 4 thread(s)

load1 1.50 -> 1.70 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 3637451306164387840 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.590 | 2419 | - | - | - |
| control-twin | 0.589 | 2414 | -0.3 [-0.9, +0.4] | -7 [-22, +9] | NOISY (IQR > difference) |
| static | 1.333 | 5460 | +125.3 [+123.0, +126.9] | +3035 [+2968, +3056] | slower |
| dyn-direct | 1.338 | 5479 | +126.0 [+122.8, +127.6] | +3051 [+2963, +3077] | slower |
| dyn | 1.437 | 5886 | +143.9 [+142.5, +145.4] | +3479 [+3449, +3521] | slower |
| static-in-sched | 1.387 | 5682 | +134.6 [+133.4, +136.6] | +3265 [+3244, +3297] | slower |

### medium kernel, 6 stage(s), 8192 rows/batch, 1 thread(s)

load1 1.70 -> 1.46 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 11521247186755477504 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 4.000 | 32771 | - | - | - |
| control-twin | 3.899 | 31937 | -0.9 [-4.0, +2.8] | -299 [-1307, +904] | NOISY (IQR > difference) |
| static | 5.271 | 43181 | +31.7 [+30.0, +35.6] | +10382 [+10007, +11343] | slower |
| dyn-direct | 5.269 | 43167 | +32.1 [+29.4, +35.9] | +10528 [+9812, +11405] | slower |
| dyn | 5.200 | 42601 | +30.1 [+27.9, +33.9] | +9876 [+9309, +10788] | slower |
| static-in-sched | 5.284 | 43285 | +32.4 [+29.9, +36.1] | +10605 [+9975, +11485] | slower |

### medium kernel, 6 stage(s), 8192 rows/batch, 4 thread(s)

load1 1.46 -> 1.66 (includes this benchmark's own 4 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 11521247186755477504 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 0.924 | 30262 | - | - | - |
| control-twin | 0.926 | 30330 | +0.1 [-0.5, +0.7] | +31 [-153, +226] | NOISY (IQR > difference) |
| static | 1.298 | 42546 | +40.6 [+40.1, +40.9] | +12327 [+12149, +12357] | slower |
| dyn-direct | 1.298 | 42535 | +40.6 [+40.2, +40.9] | +12275 [+12212, +12349] | slower |
| dyn | 1.311 | 42954 | +41.9 [+41.5, +42.1] | +12660 [+12590, +12728] | slower |
| static-in-sched | 1.306 | 42782 | +41.3 [+40.8, +41.6] | +12527 [+12384, +12593] | slower |

### medium kernel, 6 stage(s), 65536 rows/batch, 1 thread(s)

load1 1.66 -> 1.37 (includes this benchmark's own 1 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7316847049341483008 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 4.866 | 318914 | - | - | - |
| control-twin | 4.861 | 318563 | -0.2 [-0.4, +0.1] | -553 [-1313, +302] | NOISY (IQR > difference) |
| static | 7.370 | 483000 | +52.1 [+50.1, +54.0] | +165641 [+160590, +171887] | slower |
| dyn-direct | 7.553 | 494972 | +55.6 [+52.2, +58.1] | +176459 [+166752, +185627] | slower |
| dyn | 6.559 | 429832 | +34.3 [+32.1, +159.5] | +109707 [+102359, +511810] | NOISY (IQR > difference) |
| static-in-sched | 7.560 | 495479 | +56.1 [+52.6, +58.2] | +177549 [+167807, +185132] | slower |

### medium kernel, 6 stage(s), 65536 rows/batch, 4 thread(s)

load1 1.37 -> 1.77 (includes this benchmark's own 4 threads), other processes used 0.01 cores, run started at load1 0.72: **quiet**

checksum (all 6 arms identical, equal to the scalar reference): sum 7316847049341483008 count 12582912

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| control (control) | 1.209 | 316805 | - | - | - |
| control-twin | 1.195 | 313255 | -1.1 [-1.4, -0.5] | -3472 [-4386, -1727] | faster |
| static | 1.957 | 512917 | +62.9 [+61.2, +65.4] | +198343 [+193306, +207119] | slower |
| dyn-direct | 1.968 | 515961 | +63.4 [+60.1, +65.5] | +200284 [+190268, +208330] | slower |
| dyn | 3.332 | 873579 | +176.0 [+168.9, +181.0] | +557070 [+535298, +567616] | slower |
| static-in-sched | 2.035 | 533435 | +68.5 [+65.7, +72.6] | +217383 [+205697, +230628] | slower |

## Branching: 1 producer -> exchange -> 2 consumers, on 3 threads

`hand` = 3 OS threads and a Mutex/Condvar bounded queue calling the same kernels; `proto` = the real scheduler and exchange of spikes/operator-trait. Control = `hand`.

### branching, cheap kernel, 1024 rows/batch, exchange capacity 1

load1 1.77 -> 1.32 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 12.426 | 38173 | - | - | - |
| hand-twin | 12.519 | 38458 | +0.1 [-0.4, +1.7] | +22 [-151, +628] | NOISY (IQR > difference) |
| proto | 2.908 | 8933 | -76.8 [-77.2, -76.3] | -29352 [-29791, -28885] | faster |

### branching, cheap kernel, 1024 rows/batch, exchange capacity 4

load1 1.32 -> 1.30 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 3550955457225359360 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.442 | 7501 | - | - | - |
| hand-twin | 2.413 | 7413 | -1.6 [-4.8, +0.8] | -127 [-360, +55] | NOISY (IQR > difference) |
| proto | 2.564 | 7876 | +3.7 [+1.6, +7.1] | +286 [+124, +524] | NOISY (IQR > difference) |

### branching, cheap kernel, 8192 rows/batch, exchange capacity 1

load1 1.30 -> 1.35 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.840 | 20638 | - | - | - |
| hand-twin | 0.854 | 20981 | +1.2 [-1.6, +7.1] | +234 [-341, +1398] | NOISY (IQR > difference) |
| proto | 0.843 | 20719 | +1.0 [-2.4, +4.7] | +214 [-507, +945] | NOISY (IQR > difference) |

### branching, cheap kernel, 8192 rows/batch, exchange capacity 4

load1 1.35 -> 1.35 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18425496447566024704 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.771 | 18959 | - | - | - |
| hand-twin | 0.776 | 19081 | +1.3 [-0.5, +2.2] | +251 [-87, +404] | NOISY (IQR > difference) |
| proto | 0.858 | 21094 | +11.3 [+9.3, +14.4] | +2144 [+1769, +2702] | slower |

### branching, cheap kernel, 65536 rows/batch, exchange capacity 1

load1 1.35 -> 1.40 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.588 | 115535 | - | - | - |
| hand-twin | 0.586 | 115124 | +1.5 [-2.7, +3.9] | +1611 [-3186, +4428] | NOISY (IQR > difference) |
| proto | 0.578 | 113689 | -0.6 [-3.9, +2.6] | -695 [-4667, +2960] | NOISY (IQR > difference) |

### branching, cheap kernel, 65536 rows/batch, exchange capacity 4

load1 1.40 -> 1.40 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 7213762747795818496 count 33554432

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 0.577 | 113484 | - | - | - |
| hand-twin | 0.576 | 113339 | +0.7 [-3.3, +4.9] | +857 [-3672, +5547] | NOISY (IQR > difference) |
| proto | 0.576 | 113220 | -0.5 [-2.6, +1.9] | -606 [-2946, +2087] | NOISY (IQR > difference) |

### branching, medium kernel, 1024 rows/batch, exchange capacity 1

load1 1.40 -> 1.19 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 11.832 | 36349 | - | - | - |
| hand-twin | 11.890 | 36525 | +0.9 [-1.6, +2.5] | +324 [-585, +867] | NOISY (IQR > difference) |
| proto | 5.129 | 15755 | -56.4 [-57.9, -55.7] | -20806 [-21027, -20021] | faster |

### branching, medium kernel, 1024 rows/batch, exchange capacity 4

load1 1.19 -> 1.31 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18360248225261912064 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 3.971 | 12199 | - | - | - |
| hand-twin | 3.979 | 12223 | -0.2 [-3.2, +2.5] | -29 [-391, +302] | NOISY (IQR > difference) |
| proto | 5.152 | 15826 | +28.7 [+27.5, +31.2] | +3503 [+3355, +3794] | slower |

### branching, medium kernel, 8192 rows/batch, exchange capacity 1

load1 1.31 -> 1.36 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.977 | 48595 | - | - | - |
| hand-twin | 1.978 | 48617 | +0.3 [-0.6, +1.3] | +150 [-294, +636] | NOISY (IQR > difference) |
| proto | 2.083 | 51190 | +5.6 [+4.9, +6.8] | +2682 [+2402, +3293] | slower |

### branching, medium kernel, 8192 rows/batch, exchange capacity 4

load1 1.36 -> 1.42 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 6904249261301358592 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 1.974 | 48518 | - | - | - |
| hand-twin | 1.977 | 48578 | +0.4 [-0.6, +1.5] | +199 [-278, +728] | NOISY (IQR > difference) |
| proto | 2.077 | 51033 | +5.9 [+4.2, +6.4] | +2870 [+2071, +3097] | slower |

### branching, medium kernel, 65536 rows/batch, exchange capacity 1

load1 1.42 -> 1.59 (includes this benchmark's own 3 threads), other processes used 0.00 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.037 | 400497 | - | - | - |
| hand-twin | 2.039 | 400847 | -0.2 [-1.9, +2.1] | -627 [-7637, +8438] | NOISY (IQR > difference) |
| proto | 2.027 | 398539 | -0.3 [-1.6, +0.8] | -1254 [-6338, +2989] | NOISY (IQR > difference) |

### branching, medium kernel, 65536 rows/batch, exchange capacity 4

load1 1.59 -> 1.62 (includes this benchmark's own 3 threads), other processes used 0.02 cores, run started at load1 0.72: **quiet**

checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum 18343659772654502912 count 20971520

| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |
|---|---|---|---|---|---|
| hand (control) | 2.023 | 397718 | - | - | - |
| hand-twin | 2.012 | 395486 | -0.2 [-2.3, +1.1] | -672 [-9336, +4529] | NOISY (IQR > difference) |
| proto | 1.999 | 393034 | -1.3 [-2.2, +1.7] | -5122 [-8989, +6686] | NOISY (IQR > difference) |

## Summary

Overhead of each arm against the hand-fused control, median of the paired per-round ratios, in percent. `?` = the interquartile range is wider than the overhead (not a result). `twin` is the control against itself: the noise floor of that cell. Cells not labelled quiet are marked `S` (SANITY ONLY).

| cell | control ns/row | static | dyn-direct | dyn | static-in-sched | twin | quiet |
|---|---|---|---|---|---|---|---|
| cheap L1 B1024 T1 | 0.336 | +23.9 | +25.1 | +44.8 | +44.9 | -0.2 ? | yes |
| cheap L1 B1024 T4 | 0.121 | +3.7 ? | +2.6 ? | +31.2 | +37.4 | +3.8 ? | yes |
| cheap L1 B8192 T1 | 0.315 | +73.6 | +73.9 | +76.5 | +76.7 | +0.1 ? | yes |
| cheap L1 B8192 T4 | 0.085 | +65.4 | +66.0 | +71.5 | +71.2 | -0.2 ? | yes |
| cheap L1 B65536 T1 | 0.381 | +53.0 | +53.7 | +53.9 | +54.2 | -0.3 ? | yes |
| cheap L1 B65536 T4 | 0.098 | +45.9 | +46.4 | +48.4 | +48.3 | -0.8 ? | yes |
| cheap L3 B1024 T1 | 0.321 | +109.5 | +109.3 | +230.9 | +131.8 | -0.1 ? | yes |
| cheap L3 B1024 T4 | 0.105 | +89.3 | +90.7 | +185.9 | +118.8 | -1.9 ? | yes |
| cheap L3 B8192 T1 | 0.349 | +170.4 | +169.6 | +167.1 | +172.2 | -0.0 ? | yes |
| cheap L3 B8192 T4 | 0.088 | +171.8 | +172.0 | +172.3 | +177.4 | +0.2 ? | yes |
| cheap L3 B65536 T1 | 0.368 | +186.2 | +190.2 | +175.2 | +194.1 | +0.1 ? | yes |
| cheap L3 B65536 T4 | 0.101 | +172.2 | +171.8 | +150.9 | +177.0 | -1.6 ? | yes |
| cheap L6 B1024 T1 | 0.334 | +409.9 | +410.0 | +429.9 | +423.7 | +0.2 ? | yes |
| cheap L6 B1024 T4 | 0.122 | +190.9 | +197.7 | +301.7 | +229.2 | -1.0 ? | yes |
| cheap L6 B8192 T1 | 0.306 | +413.2 | +414.0 | +376.3 | +417.8 | -0.4 ? | yes |
| cheap L6 B8192 T4 | 0.089 | +344.4 | +348.0 | +329.7 | +356.5 | -0.5 ? | yes |
| cheap L6 B65536 T1 | 0.374 | +387.3 | +404.0 | +327.2 | +412.9 | -0.7 ? | yes |
| cheap L6 B65536 T4 | 0.101 | +370.2 | +384.7 | +299.7 | +416.6 | -5.4 | yes |
| medium L1 B1024 T1 | 1.134 | +22.0 | +21.9 | +31.1 | +31.1 | +0.0 ? | yes |
| medium L1 B1024 T4 | 0.330 | +12.7 | +12.9 | +24.6 | +24.4 | +0.7 ? | yes |
| medium L1 B8192 T1 | 1.140 | +29.2 | +29.4 | +30.6 | +30.5 | +0.0 ? | yes |
| medium L1 B8192 T4 | 0.292 | +28.0 | +28.1 | +30.3 | +30.1 | -0.2 ? | yes |
| medium L1 B65536 T1 | 1.179 | +35.6 | +40.3 | +41.6 | +42.3 | -0.1 ? | yes |
| medium L1 B65536 T4 | 0.304 | +35.1 | +33.7 | +35.0 | +35.4 | -1.8 | yes |
| medium L3 B1024 T1 | 1.561 | +98.7 | +117.2 | +124.1 | +113.8 | +0.1 ? | yes |
| medium L3 B1024 T4 | 0.410 | +90.7 | +93.1 | +124.6 | +101.7 | -0.0 ? | yes |
| medium L3 B8192 T1 | 3.399 | -5.8 | -5.7 | -4.5 | -5.4 | -0.2 ? | yes |
| medium L3 B8192 T4 | 0.811 | -0.7 | -0.5 ? | +1.4 | -0.0 ? | -0.0 ? | yes |
| medium L3 B65536 T1 | 3.986 | +7.8 | +8.9 | +154.9 | +9.2 | +0.0 ? | yes |
| medium L3 B65536 T4 | 0.986 | +10.7 | +10.0 | +102.0 | +13.6 | -0.3 ? | yes |
| medium L6 B1024 T1 | 2.242 | +155.3 | +155.8 | +142.4 | +160.7 | -0.2 ? | yes |
| medium L6 B1024 T4 | 0.590 | +125.3 | +126.0 | +143.9 | +134.6 | -0.3 ? | yes |
| medium L6 B8192 T1 | 4.000 | +31.7 | +32.1 | +30.1 | +32.4 | -0.9 ? | yes |
| medium L6 B8192 T4 | 0.924 | +40.6 | +40.6 | +41.9 | +41.3 | +0.1 ? | yes |
| medium L6 B65536 T1 | 4.866 | +52.1 | +55.6 | +34.3 ? | +56.1 | -0.2 ? | yes |
| medium L6 B65536 T4 | 1.209 | +62.9 | +63.4 | +176.0 | +68.5 | -1.1 | yes |

| branching cell | hand ns/row | proto overhead % | twin | quiet |
|---|---|---|---|---|
| cheap B1024 cap1 | 12.426 | -76.8 | +0.1 ? | yes |
| cheap B1024 cap4 | 2.442 | +3.7 ? | -1.6 ? | yes |
| cheap B8192 cap1 | 0.840 | +1.0 ? | +1.2 ? | yes |
| cheap B8192 cap4 | 0.771 | +11.3 | +1.3 ? | yes |
| cheap B65536 cap1 | 0.588 | -0.6 ? | +1.5 ? | yes |
| cheap B65536 cap4 | 0.577 | -0.5 ? | +0.7 ? | yes |
| medium B1024 cap1 | 11.832 | -56.4 | +0.9 ? | yes |
| medium B1024 cap4 | 3.971 | +28.7 | -0.2 ? | yes |
| medium B8192 cap1 | 1.977 | +5.6 | +0.3 ? | yes |
| medium B8192 cap4 | 1.974 | +5.9 | +0.4 ? | yes |
| medium B65536 cap1 | 2.037 | -0.3 ? | -0.2 ? | yes |
| medium B65536 cap4 | 2.023 | -1.3 ? | -0.2 ? | yes |

All cells were measured with the run-start load average < 2, no other process using >= 0.5 core, and a validated harness: **quiet**.
