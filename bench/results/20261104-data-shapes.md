# Do the results hold on other data? Skewed keys, string-heavy columns, many NULLs (2026-11-04)

Verification-plan item 20 (`docs/decisions/verification-plan.md`). Spike: `spikes/data-shapes/` (own Cargo workspace).
Everything verified so far ran on TPC-H SF1: mostly uniform keys, few distinct strings, no NULLs unless injected.
This item generates other data with DuckDB, runs the same queries through the engines of this repository, and
compares each answer with DuckDB's, exactly. TPC-DS is out of scope (its generator needs the network).

**Status: built and checked as far as the shared box allows; the engines have NOT been run on this data.** The run is
for GCP (section 4). Everything under "GCP results" is to be filled. Nothing below is a speed or correctness
result for the engines; section 5 says exactly what was and was not checked locally.

## 1. Design

**One script, one command.** `spikes/data-shapes/gcp-check.sh <summary-dir> [scratch-dir]` does everything:
build, generate a data set, check that the generated data has the intended shape, run every query of that data set
through DuckDB and through the engine at 1, 4 and `nproc` threads, compare, delete the data set, go on. It exits
non-zero if any query differs, crashes, times out, is refused when it should not be, or if a self-test misbehaves.

**Engines reused, nothing copied.**
- `spikes/hash-ops` (hash aggregate and inner hash join): used as a path dependency (library crate `hash_ops`), with
  the same release profile as that spike (`overflow-checks` on for the engine, off for the dependencies;
  `bench/results/20261104-hash-ops-correctness.md` section 6). New plans are written against its public operators
  (`run_pipeline`, `HashAgg`, `JoinBuild`, `Probe`, the filters). Binary `shapes`.
- `spikes/generic-ops` (the generic-operator engine): its `engine.rs`, `kernels.rs` and `source.rs` are included
  with `#[path]`. Binary `gshapes`. Its query shapes are Q1 (group by two dictionary keys, three sums, count) and Q6
  (filtered ungrouped sum); that engine supports at most 32 distinct key values, no NULL group keys (it refuses them
  with `NULL group key: not supported yet`) and no joins, so only the NULL data sets apply to it. The skewed and
  string-heavy sets cannot be run through it, by that engine's design, not by choice.

**One query table.** `src/qtable.rs` (hash-ops queries) and `src/gtable.rs` (generic-ops queries) hold every query as
one value that yields both the engine plan and the DuckDB SQL text (`shapes sql <name>`), so the plan and the SQL
cannot drift apart; DuckDB rejects any column name that is not in the generated table, which checked every column
name used by the plans (locally, section 5).

**Deterministic data.** No `random()`: every value is a function of the row number and a salt through DuckDB's
`hash()` (`sql/prelude.sql`). Zipf-like keys come from inverting the continuous power-law CDF (exponent 1.0 and
1.5); that is "Zipf-like", not the exact discrete Zipf distribution, and the checks below measure the shares that
result. Sizes derive from `N` (default 4,000,000), so the same SQL runs at `N = 10000` for validation.

**Comparison.** Same normalised text as `spikes/hash-ops/check.sh`: one line per group, key columns (NULL as `NULL`,
`DATE` keys as days since 1970-01-01), `count(*)`, then every `sum` (NULL if no input was non-NULL), joined by `|`,
sorted bytewise (`LC_ALL=C`). DuckDB prints with `-list -separator '|' -nullvalue NULL`. Sums are exact (`HUGEINT`
against the engine's `i128`). On a difference the report shows the first differing line of both sides.

**Data-shape checks (the data is what it claims to be).** After each generation a SQL check
(`sql/check_*.sql`, 176 checks over the 19 data sets) measures the data against stated bounds and the script fails if
one is outside: hot key share 30 percent (4-sigma bounds), Zipf shares, exactly 100 hot probe rows, string length
ranges, share of empty strings, share of non-ASCII and 4-byte characters, the number of strings that differ only in
the last byte (and only in the first byte), the NULL share of every column against the requested percentage, and, from
the Parquet metadata of the NULL-shape file, that `kr` and `vr` have a value in exactly one row group, that some
8192-row blocks of `kb_al` are all NULL, and that the row groups have the size the generator asked for.

### Data sets (N = 4,000,000; disk sizes are measured on the GCP run)

| tag | files (rows) | what it stresses |
|---|---|---|
| `skew` | `skew_fact` (4M), `skew_dim` (1M+1) | `k10` Zipf-like s=1.0 and `k15` s=1.5 over 1M keys (rank 1 holds about 5 and 29 percent); `khot`: key 0 holds 30 percent; `kuni`: uniform control; `l_shipmode`: 20 string values, Zipf 1.0, decoded as a dictionary column (the hash-ops reader maps that column name to `Dictionary<Int32, Utf8>`); the dimension has a dictionary payload `o_orderpriority` and `df = dk % 2` to keep or drop the hot key on the build side |
| `hot` | `hot_build` (2M), `hot_probe` (1M), `hh_a`, `hh_b` (20k each) | hot key on the BUILD side: a chain of 30 percent of the build rows (600k) probed by exactly 100 rows (60M joined rows), at two build sizes (500k and 2M) so ns per joined row can be compared; and hot on both sides (6k x 6k = 36M joined rows) |
| `str` | `str_fact` (3M) and five build sides | `s_hi` about 1.5M distinct 20-byte values; `s_long` 100 to 500 bytes, about 1M distinct; `s_empty` 40 percent `''`, 3 percent NULL; `s_utf8` 1 to 4 bytes per character including precomposed against combining forms of the same text; `s_last` and `s_first` strings that differ only in the last (first) byte; `s_len` differing only in length; `l_shipinstruct` about 300k distinct values read as a dictionary column that is as large as the data; rows in random order, so no row group has a small dictionary |
| `nul00 nul01 nul10 nul50 nul90 nul99 nul100` | `nul_pPP` (2M each), `nul_dimn`, `nul_sdim` | NULL percentage 0/1/10/50/90/99/100 independently in the keys (`BIGINT`, `INTEGER`, `DATE`, `VARCHAR` with `''` as well) AND the values; join partners also have NULL keys |
| `nulshape` | `nul_shape` (4M, row groups of 98,304) | NULL batches: `kb_al` NULL in whole aligned blocks of 8192 rows, `kb_un` in blocks of 5000 (straddling batches); `kr`/`vr` NULL in every row group but one; `kall`/`vall` NULL in every row |
| `li00 ... li100`, `linullkey` | `li_pPP` (1M each) | lineitem-shaped for the generic-ops engine: NULLs in the decimal columns and (half as often) in `l_shipdate`; `linullkey` also has NULL group keys |

### Queries (100 in total; names are the `name` column of `shapes list` / `gshapes list`)

- **Skew (14):** `sk_agg_{k10,k15,hot,uni}` (count, sums, group by the key), `sk_agg_mode_filt` (filter + dictionary
  key), `sk_agg_k15_mode` (composite int + dictionary key), `sk_join_{k10,k15}` (skewed probe, unique build, group by
  a build column), `sk_join_hot`, `sk_join_hot_df0`, `sk_join_hot_df1` (hot key kept / dropped on the build side;
  dictionary payload), `sk_join_k15_key` (group by the skewed probe key after the join), `sk_join_build_hot`,
  `sk_join_build_k15` (the fact is the build side: a chain of 1.2M rows).
- **Hot chain (3):** `hb_chain_small`, `hb_chain_big`, `hb_hot_hot`.
- **Strings (16):** nine group-bys (`st_agg_{hi,long,empty,utf8,last,first,len,instr,pair}`), two filtered
  (`st_agg_filt`, `st_agg_instr_filt`), five joins (`st_join_{long,hi,instr,utf8,empty_fanout}`: plain against plain,
  dictionary probe against plain build with 10 percent misses, a build side with 40 `''` rows and 3 NULL keys).
- **NULLs (6 per percentage, 42, plus 9 for the shape file):** `nl_agg_{kn,multi,ksi,filt}_pPP`,
  `nl_join_{kn,ks}_pPP`; `ns_agg_{kb_al,kb_un,kr,ksb,g_vb,kall,filt_vb}`, `ns_join_{kb,kr}`.
- **Generic-ops (16):** `li_q1_pPP`, `li_q6_pPP` for the seven percentages, `li_q1_nullkey` (must be REFUSED with
  `not supported yet`; anything else is a failure), `li_q6_nullkey`.

### Known-bad checks: a pass has to mean something

The harness is shown to report a difference, in four ways (the first was run locally; the others run in the GCP run):

1. **Comparison itself**, on a real DuckDB answer: nine perturbations of the text (identical copy, reordered lines,
   an appended character, a changed count, a deleted line, a duplicated line, a key replaced by NULL, a missing final
   newline, an empty file). The two that must not differ (identical copy, reordered lines, because order is not
   part of the result) are reported identical; the other seven are reported as differences.
2. **Whole harness, corrupted answers:** with `DS_NO_ENGINE=1 DS_STANDIN_PERTURB=appendchar` every answer is
   corrupted and the run reports every query as FAIL with the first differing line and exits 1 (run locally).
3. **The real engine, deliberately perturbed:** `SHAPES_PERTURB=count|sum|drop|key` makes `shapes` corrupt its own
   result before writing it; the harness requires "different" for all four (GCP).
4. **Real bugs in the engine (mutants):** four of the 30 mutants of `spikes/hash-ops/mutate.py` are applied to a private
   copy (`mutate.py --dry` applies the edit; the repository is untouched), built, and each must be reported as
   different on the query that needs it:
   `NULL group dropped from the output` on `nl_agg_kn_p10`; `merge loses the last thread's partial table` on
   `sk_agg_k10` (4 threads); `fan-out: only the first match of a chain is emitted` on `hb_hot_hot`;
   `SUM does not count its NULL inputs (non-NULL count wrong)` on `ns_agg_kall`. A mutant that comes back identical is
   a failure of the data set ("this data does not see the bug"). The first and last are bugs the TPC-H diff of
   `check.sh` cannot see at all (that data has no NULLs; `20261104-hash-ops-correctness.md` section 5).

### What to expect before the run (reasoned, not measured)

- `li_q1_nullkey` is refused (generic Q1 sink). All others should answer.
- A group-by on a string key keeps one copy of every distinct key per worker thread and a result copy for printing;
  the 1M-key 300-byte case (`st_agg_long`) is the memory stress. The join build uses 32-bit offsets for string
  columns: a build side or group-key store of 2 GiB or more is refused or errors, which none of these sets reaches.
- `hb_chain_big` against `hb_chain_small` should show about the same ns per joined row (the chain walk is linear);
  a ratio above 2 is printed as a warning and above 4 counts as a failure ("QUADRATIC?").
- Timings: engine time = Parquet decode + execution (not the printing of the result); DuckDB time = the whole CLI
  call including start-up, planning, printing. Both indicative only: they are single runs, not interleaved, and the
  DuckDB run uses all cores.

## 2. Files

`spikes/data-shapes/`: `gcp-check.sh` (the run), `Cargo.toml` + `Cargo.lock` (the locked versions of hash-ops),
`src/bin/shapes.rs`, `src/bin/gshapes.rs` (engine runners), `src/qtable.rs`, `src/gtable.rs` (query tables),
`src/sqltool.rs` (prints queries and SQL without building the engines; used by the DuckDB-only mode),
`sql/prelude.sql` (deterministic helpers), `sql/gen_*.sql` (generators), `sql/check_*.sql` (data-shape checks).
This file. No change anywhere else.

## 3. Disk, time, cost (estimates until the GCP run)

| | estimate | basis |
|---|---|---|
| disk | peak about 10 GB, under 15 GB; the script refuses to start with less than 14 GB free | reasoned: cargo target about 2.5 GB, the largest data set (strings) about 1.5 GB of Parquet, result texts up to about 1.5 GB at a time (five copies of the 300 MB `st_agg_long` text at worst); data sets are deleted as soon as their queries finish. The real figure is printed ("peak scratch disk") |
| time | 40 to 90 minutes on `n4a-highcpu-16` | reasoned: build 4 to 8 minutes, four mutant rebuilds 4 to 10 minutes, generation about 10 minutes, about 100 queries x (DuckDB + 3 engine runs). Each engine run has a 900 s timeout |
| cost | VM only: 1.5 to 2 hours x USD 0.1968 per hour (derived Spot rate for `n4a-highcpu-16` in `scripts/gcp/prices.env`, to be recomputed before the run) = about USD 0.30 to 0.40, plus disks and IP; the orchestrator's `plan` gives the bound | estimate |
| memory | 32 GiB (highcpu: 2 GiB per vCPU) should be enough | reasoned; the run reports peak RSS per query, and an out-of-memory kill is reported as CRASH, not hidden. If it happens, rerun the failing tag on `n4a-highmem-16` |

## 4. The GCP command

The job is not in `scripts/gcp/job.sh` (outside this task's scope). Proposed job entry, to be added by the
orchestrator (step name and `RESULTS` follow the pattern of the other jobs; the data disk of 30 GB is enough):

```bash
  datashapes)
    run data-shapes bash -c 'cd "$SRC" && spikes/data-shapes/gcp-check.sh "$RESULTS/data-shapes" "$WORK/ds-scratch"' ;;
```

On a clean Debian VM with `duckdb` on PATH and `rustup` installed (the existing `setup-rust` and `setup-duckdb` steps
do both), the command itself is:

```bash
cd <repo-checkout>        # rust-toolchain.toml at the root pins the toolchain
spikes/data-shapes/gcp-check.sh <results-dir> <scratch-base-dir>      # ARM64 first: n4a-highcpu-16
echo $?                    # 0 = everything identical and every self-test behaved; 1 = a difference/crash/timeout; 2 = harness error
```

Results to copy back: `<results-dir>/summary.txt` (the report), `results.tsv` (one row per query), `machine.txt`,
`build.log`, `failures/`. Then, if wanted, the same on x86-64 (for example `c3d-highcpu-16`, rate in `prices.env`).
Partial runs: `ONLY_TAGS="skew nul10" spikes/data-shapes/gcp-check.sh ...`; `DS_SKIP_MUTANTS=1` skips the four mutant
builds; `N=...` scales everything.

## 5. What was checked locally, and what was not

Shared 8-core box, DuckDB v2.0.0, nothing heavy: `scripts/local-heavy.sh cargo check --tests --offline --locked` for
the compile check, one at a time; DuckDB generation only at `N = 10000`.

| Check | Result |
|---|---|
| `cargo check --tests --offline --locked` in `spikes/data-shapes` | compiles, 0 warnings in the new code. `cargo clippy` reports 2 warnings, both in the included `spikes/generic-ops` sources (`type_complexity`, `doc_lazy_continuation`), not changed here |
| `Cargo.lock` | copied from `spikes/hash-ops`, plus the new package entry only (9 added lines), so the dependency versions are the locked ones of that spike |
| `sqltool list` | 100 queries: 84 for `shapes`, 16 for `gshapes` |
| `N=10000 DS_NO_ENGINE=1 gcp-check.sh` (generation, data-shape checks, all 100 SQL queries run in DuckDB, comparison plumbing; the "engine" is DuckDB's own answer) | 19 data sets generated; **176 of 176 data-shape checks pass** (after fixing one wrong expectation of mine: `s_hi` is 20 bytes, not 24); all 100 queries execute (so every column name and every SQL text in the query tables is valid); 9 of 9 text self-tests behave; exit 0; 12 seconds |
| same with `DS_STANDIN_PERTURB=appendchar ONLY_TAGS="skew nul10"` | 20 of 20 queries reported `FAIL`, each with the first differing line; exit 1; `failures/<query>.txt` written |
| `mutate.py --dry` for the four mutants | each edit applies exactly once to a private copy; `spikes/hash-ops` unchanged |

**Not checked, because it needs a run:** that `shapes` and `gshapes` link and run at all (only `cargo check` was done,
no binary was built); that any engine answer equals DuckDB's on any of these data sets; that the engines' plans are
wired correctly for the join column names (reasoned: every join output column used as a key or a sum was checked by
hand to be in the probe or build payload list); that the four mutants build and are caught; that `SHAPES_PERTURB`
is reported; the `/usr/bin/time` parsing, timeout handling and the CRASH/TIMEOUT/REFUSED classification (written, not
exercised); whether DuckDB's Parquet writer cuts the row groups of `nul_shape` exactly at 98,304 rows (the script
checks this from the metadata and fails the run if not; at `N = 10000` it did hold with groups of 2048);
the real disk, memory and time figures; ARM64 and x86-64 behaviour.

## GCP results: to be filled

Machine type, region, date, run id, cost: to be filled. Per data set: identical or first differing line; engine and
DuckDB wall times; peak RSS; any crash, timeout or quadratic warning; the four mutants and the engine-perturbation
self-test; real peak disk and wall time.


## GCP results (2026-11-04)

GCP run `nata-1104-172548-4307` (2026-11-04, `n4a-highcpu-16` Spot, Neoverse-N3, 16 vCPU, Debian 12 arm64; task of the `armcheck-retry` job, run concurrently with four other tasks, so no timing here is evidence).

**100 of 100 queries identical to DuckDB at every thread count (or refused as expected), 0 failing, 0 self-test failures, 0 data-shape check failures; 11 minutes, peak scratch disk 1.5 GB.** The known-bad checks behaved: the perturbed answers (`count`, `sum`, `drop`, `key`) and the four engine mutants (NULL group dropped, merge loses the last thread's table, fan-out emits only the first match, NULL count wrong) are all reported as differences (rows `selftest:...` below: result `caught`). A first attempt reported one data-shape check failure, and it was the CHECK that was wrong (it measured the empty-string share among non-NULL rows, 0.4124, instead of among all rows, 0.40); fixed in `sql/check_str.sql`. Timings are indicative only: the VM ran four other tasks at load 15-30, and DuckDB used all cores. Not covered by this run: x86-64, TPC-DS, and anything past SF-1-sized data.

Summary file (unedited):

```
date:      2026-11-04T10:27:15Z
host:      Linux 6.1.0-54-cloud-arm64 aarch64
cpu:        (16 cores)  Neoverse-N3
memory:    31.3 GiB
duckdb:    v2.0.0
commit:    unknown (+ uncommitted changes in the spikes)
scratch:   /mnt/work/armcheck-work/data-shapes/data-shapes.yAhBHt (92 GB free, need 14)
N=4000000 K=1000000 M=3000000 MH=2000000 ML=1000000 MI=300000 B=2000000 P=1000000 HH=20000 NP=2000000 NS=4000000 NL=1000000 row-group-size(nulshape)=98304 target-group=20
threads:   1 4 16    timeout/run: 900s   engine mode: real
note: /usr/bin/time not found; peak RSS is the engine's own VmHWM figure
building spikes/data-shapes (release, locked) ...
built mutant 1: NULL group dropped from the output
built mutant 2: merge loses the last thread's partial table
built mutant 3: fan-out: only the first match of a chain is emitted
built mutant 4: SUM does not count its NULL inputs (non-NULL count wrong)

=== data set: skew  (N=4000000)  10:31:27Z ===
generated in 5 s: skew_dim.parquet 4.1 MB; skew_fact.parquet 95.0 MB; 
  data ok            fact_rows = 4000000.0 (expected 4000000.0 .. 4000000.0)
  data ok            khot_share_of_key_0 = 0.2999575 (expected 0.29908348486100883 .. 0.30091651513899115)
  data ok            k15_share_of_rank_1 (Zipf 1.5: about 29%) = 0.2933965 (expected 0.2 .. 0.38)
  data ok            k10_share_of_rank_1 (Zipf 1.0: about ln2/ln K) = 0.05024525 (expected 0.03010299956639812 .. 0.08027466551039499)
  data ok            k10_share_of_top_100_ranks = 0.33448525 (expected 0.16702678114855357 .. 0.5010803434456608)
  data ok            k15_max_key_le_K = 999369.0 (expected 1.0 .. 1000000.0)
  data ok            k10_distinct_over_K = 0.520629 (expected 0.02 .. 1.0)
  data ok            kuni_distinct_over_min(K,rows) = 0.981639 (expected 0.5 .. 1.0)
  data ok            shipmode_distinct = 19.0 (expected 5.0 .. 20.0)
  data ok            dim_rows = 1000001.0 (expected 1000001.0 .. 1000001.0)
identical             sk_agg_k10                         groups=520629   duckdb=596ms engine: 1t=807.4ms 4t=479.3ms 16t=394.2ms peak_rss=231MB
  self-test CAUGHT   mutant 'merge loses the last thread's partial table' on sk_agg_k10: DIFFERENT: sk_agg_k10.txt sk_agg_k10.engine differ: byte 8, line 1; first differing line 1: duckdb=[100000|4|1644|128616] engine=[100000|3|1216|-28213]
identical             sk_agg_k15                         groups=38451    duckdb=316ms engine: 1t=266.7ms 4t=147.4ms 16t=173.5ms peak_rss=49MB
identical             sk_agg_hot                         groups=939011   duckdb=624ms engine: 1t=587.6ms 4t=581.8ms 16t=465.7ms peak_rss=332MB
identical             sk_agg_uni                         groups=981639   duckdb=429ms engine: 1t=569.9ms 4t=294.3ms 16t=417.2ms peak_rss=364MB
identical             sk_agg_mode_filt                   groups=19       duckdb=114ms engine: 1t=81.7ms 4t=35.5ms 16t=83.9ms peak_rss=37MB
identical             sk_agg_k15_mode                    groups=38480    duckdb=162ms engine: 1t=108.6ms 4t=61.3ms 16t=96.6ms peak_rss=31MB
identical             sk_join_k10                        groups=1000     duckdb=164ms engine: 1t=691.5ms 4t=199.0ms 16t=370.5ms peak_rss=89MB
identical             sk_join_k15                        groups=1000     duckdb=163ms engine: 1t=250.6ms 4t=94.6ms 16t=208.8ms peak_rss=71MB
identical             sk_join_hot                        groups=5        duckdb=187ms engine: 1t=810.1ms 4t=232.2ms 16t=297.8ms peak_rss=68MB
identical             sk_join_hot_df0                    groups=5        duckdb=190ms engine: 1t=540.9ms 4t=170.0ms 16t=261.4ms peak_rss=56MB
identical             sk_join_hot_df1                    groups=5        duckdb=165ms engine: 1t=438.9ms 4t=135.6ms 16t=253.0ms peak_rss=49MB
identical             sk_join_k15_key                    groups=38451    duckdb=167ms engine: 1t=223.1ms 4t=78.8ms 16t=154.3ms peak_rss=63MB
identical             sk_join_build_hot                  groups=1000     duckdb=125ms engine: 1t=781.5ms 4t=308.9ms 16t=316.8ms peak_rss=208MB
identical             sk_join_build_k15                  groups=1000     duckdb=116ms engine: 1t=324.9ms 4t=248.1ms 16t=308.3ms peak_rss=197MB
  self-test ok       text perturbation 'same': identical, as it must be (order is not part of the result)
  self-test ok       text perturbation 'reorder': identical, as it must be (order is not part of the result)
  self-test ok       text perturbation 'appendchar': reported as a difference
  self-test ok       text perturbation 'count': reported as a difference
  self-test ok       text perturbation 'delline': reported as a difference
  self-test ok       text perturbation 'dupline': reported as a difference
  self-test ok       text perturbation 'nullkey': reported as a difference
  self-test ok       text perturbation 'dropnl': reported as a difference
  self-test ok       text perturbation 'empty': reported as a difference
  self-test ok       engine output perturbed ('count'): reported as a difference (DIFFERENT: self.exp self.engine differ: byte 13, line 1; first differing line 1: duckdb=[MODE_01|461609|230273396|-203314931] engine=[MODE_01|461610|230273396|-203314931])
  self-test ok       engine output perturbed ('sum'): reported as a difference (DIFFERENT: self.exp self.engine differ: byte 24, line 1; first differing line 1: duckdb=[MODE_01|461609|230273396|-203314931] engine=[MODE_01|461609|230273397|-203314931])
  self-test ok       engine output perturbed ('drop'): reported as a difference (DIFFERENT: EOF on self.engine after byte 592, line 18; first differing line 19: duckdb=[MODE_19|34286|17031540|-8456584] engine=[])
  self-test ok       engine output perturbed ('key'): reported as a difference (DIFFERENT: self.exp self.engine differ: byte 8, line 1; first differing line 1: duckdb=[MODE_01|461609|230273396|-203314931] engine=[MODE_01x|461609|230273396|-203314931])
--- skew done in 50 s

=== data set: hot  (N=4000000)  10:32:17Z ===
generated in 0 s: hh_a.parquet 0.2 MB; hh_b.parquet 0.2 MB; hot_build.parquet 17.6 MB; hot_probe.parquet 9.8 MB; 
  data ok            hot_build_rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            hot_build_share_of_key_0 = 0.300808 (expected 0.2987038518603184 .. 0.3012961481396816)
  data ok            hot_probe_rows_with_key_0 (exactly 100) = 100.0 (expected 100.0 .. 100.0)
  data ok            hh_a_share_of_key_0 = 0.3027 (expected 0.28703851860318425 .. 0.31296148139681573)
  data ok            hh_b_share_of_key_0 = 0.30495 (expected 0.28703851860318425 .. 0.31296148139681573)
  data ok            hh_join_rows_over_(0.3 HH)^2 = 1.0259168333333333 (expected 0.7 .. 3.0)
identical             hb_chain_small                     groups=77       duckdb=302ms engine: 1t=514.1ms 4t=188.4ms 16t=214.1ms peak_rss=47MB
identical             hb_chain_big                       groups=77       duckdb=347ms engine: 1t=1765.5ms 4t=662.5ms 16t=517.4ms peak_rss=139MB
identical             hb_hot_hot                         groups=100      duckdb=492ms engine: 1t=839.5ms 4t=843.9ms 16t=886.5ms peak_rss=7MB
  self-test CAUGHT   mutant 'fan-out: only the first match of a chain is emitted' on hb_hot_hot: DIFFERENT: hb_hot_hot.txt hb_hot_hot.engine differ: byte 5, line 1; first differing line 1: duckdb=[0|0|367922|180094806|182420013] engine=[0|0|78|35777|38781]
--- hot done in 9 s

=== data set: str  (N=4000000)  10:32:26Z ===
generated in 17 s: str_dim.parquet 48.4 MB; str_dim2.parquet 9.8 MB; str_dim3.parquet 1.4 MB; str_dim4.parquet 1.3 MB; str_fact.parquet 306.6 MB; str_small.parquet 0.0 MB; 
  data ok            fact_rows = 3000000.0 (expected 3000000.0 .. 3000000.0)
  data ok            s_hi_distinct = 1554834.0 (expected 1087617.7757921983 .. 2000000.0)
  data ok            s_hi_length = 20.0 (expected 20.0 .. 20.0)
  data ok            s_long_min_length = 100.0 (expected 100.0 .. 500.0)
  data ok            s_long_max_length = 500.0 (expected 450.0 .. 500.0)
  data ok            s_long_distinct = 950396.0 (expected 570127.7589792816 .. 1000000.0)
  data ok            s_empty_share_empty = 0.40003 (expected 0.3988686291501016 .. 0.40113137084989847)
  data ok            s_empty_share_null = 0.029968 (expected 0.029599999999999998 .. 0.0304)
  data ok            s_utf8_share_non_ascii (every row) = 1.0 (expected 1.0 .. 1.0)
  data ok            s_utf8_share_with_combining_acute = 0.14310666666666666 (expected 0.08 .. 0.2)
  data ok            s_utf8_share_with_4_byte_chars = 0.26543666666666665 (expected 0.05 .. 0.3)
  data ok            s_last_distinct_prefixes = 2000.0 (expected 1800.0 .. 2000.0)
  data ok            s_last_distinct_over_prefixes (differ only in the last byte) = 31.0 (expected 31.0 .. 62.0)
  data ok            s_first_distinct_over_tails (differ only in the first byte) = 31.0 (expected 31.0 .. 62.0)
  data ok            s_len_distinct = 300.0 (expected 270.0 .. 300.0)
  data ok            shipinstruct_distinct = 299981.0 (expected 150000.0 .. 300000.0)
  data ok            str_dim_rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            str_dim2_rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            str_dim3_rows = 270000.0 (expected 270000.0 .. 270000.0)
  data ok            str_dim4_rows_unique = 0.0 (expected 0.0 .. 0.0)
  data ok            str_small_empty_rows = 40.0 (expected 20.0 .. 60.0)
identical             st_agg_hi                          groups=1554834  duckdb=1055ms engine: 1t=1276.4ms 4t=432.4ms 16t=494.2ms peak_rss=715MB
identical             st_agg_long                        groups=950396   duckdb=1323ms engine: 1t=1760.3ms 4t=798.6ms 16t=1012.6ms peak_rss=1671MB
identical             st_agg_empty                       groups=1002     duckdb=89ms engine: 1t=114.8ms 4t=68.0ms 16t=83.3ms peak_rss=13MB
identical             st_agg_utf8                        groups=147000   duckdb=412ms engine: 1t=587.4ms 4t=266.7ms 16t=387.0ms peak_rss=218MB
identical             st_agg_last                        groups=62000    duckdb=423ms engine: 1t=667.3ms 4t=329.5ms 16t=397.1ms peak_rss=351MB
identical             st_agg_first                       groups=62000    duckdb=449ms engine: 1t=673.9ms 4t=321.1ms 16t=355.1ms peak_rss=348MB
identical             st_agg_len                         groups=300      duckdb=106ms engine: 1t=210.8ms 4t=77.8ms 16t=151.2ms peak_rss=52MB
identical             st_agg_instr                       groups=299981   duckdb=375ms engine: 1t=659.7ms 4t=362.7ms 16t=507.0ms peak_rss=332MB
identical             st_agg_pair                        groups=1846992  duckdb=988ms engine: 1t=898.4ms 4t=499.6ms 16t=734.6ms peak_rss=911MB
identical             st_agg_filt                        groups=1056263  duckdb=477ms engine: 1t=378.2ms 4t=434.8ms 16t=321.1ms peak_rss=537MB
identical             st_agg_instr_filt                  groups=294576   duckdb=344ms engine: 1t=417.5ms 4t=220.9ms 16t=287.7ms peak_rss=286MB
identical             st_join_long                       groups=1000     duckdb=553ms engine: 1t=3016.1ms 4t=963.3ms 16t=736.8ms peak_rss=1006MB
identical             st_join_hi                         groups=1000     duckdb=274ms engine: 1t=1355.4ms 4t=375.5ms 16t=434.5ms peak_rss=184MB
identical             st_join_instr                      groups=777      duckdb=243ms engine: 1t=995.5ms 4t=257.2ms 16t=263.3ms peak_rss=136MB
identical             st_join_utf8                       groups=100      duckdb=240ms engine: 1t=656.2ms 4t=163.8ms 16t=180.7ms peak_rss=61MB
identical             st_join_empty_fanout               groups=13       duckdb=161ms engine: 1t=1015.9ms 4t=703.1ms 16t=367.7ms peak_rss=20MB
--- str done in 124 s

=== data set: nul00  (N=4000000)  10:34:30Z ===
generated in 2 s: nul_dimn.parquet 4.0 MB; nul_p00.parquet 41.3 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_ki = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_kd = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_ks = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_v = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_w = 0.0 (expected 0.0 .. 0.0)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.0498395 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p00                      groups=864572   duckdb=464ms engine: 1t=325.5ms 4t=420.1ms 16t=379.6ms peak_rss=358MB
identical             nl_agg_multi_p00                   groups=1999980  duckdb=937ms engine: 1t=368.1ms 4t=264.4ms 16t=615.1ms peak_rss=871MB
identical             nl_agg_ksi_p00                     groups=1767553  duckdb=774ms engine: 1t=602.9ms 4t=619.1ms 16t=530.4ms peak_rss=747MB
identical             nl_agg_filt_p00                    groups=5000     duckdb=184ms engine: 1t=79.9ms 4t=63.6ms 16t=76.2ms peak_rss=23MB
identical             nl_join_kn_p00                     groups=97       duckdb=205ms engine: 1t=896.6ms 4t=295.3ms 16t=251.5ms peak_rss=68MB
identical             nl_join_ks_p00                     groups=7        duckdb=144ms engine: 1t=162.5ms 4t=151.4ms 16t=195.9ms peak_rss=38MB
--- nul00 done in 73 s

=== data set: nul01  (N=4000000)  10:35:43Z ===
generated in 1 s: nul_dimn.parquet 4.0 MB; nul_p01.parquet 41.4 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.010043 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            null_share_ki = 0.010058 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            null_share_kd = 0.0099015 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            null_share_ks = 0.010029 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            null_share_v = 0.009821 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            null_share_w = 0.0099835 (expected 0.009718575054410594 .. 0.010281424945589406)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.05034440402799678 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p01                      groups=861877   duckdb=736ms engine: 1t=724.2ms 4t=590.6ms 16t=403.4ms peak_rss=358MB
identical             nl_agg_multi_p01                   groups=1999954  duckdb=1215ms engine: 1t=635.6ms 4t=231.6ms 16t=602.1ms peak_rss=885MB
identical             nl_agg_ksi_p01                     groups=1750499  duckdb=1632ms engine: 1t=820.5ms 4t=579.8ms 16t=505.9ms peak_rss=709MB
identical             nl_agg_filt_p01                    groups=5001     duckdb=132ms engine: 1t=65.4ms 4t=64.9ms 16t=63.0ms peak_rss=21MB
identical             nl_join_kn_p01                     groups=97       duckdb=118ms engine: 1t=540.3ms 4t=180.4ms 16t=245.3ms peak_rss=68MB
identical             nl_join_ks_p01                     groups=7        duckdb=117ms engine: 1t=111.1ms 4t=116.4ms 16t=144.7ms peak_rss=27MB
--- nul01 done in 74 s

=== data set: nul10  (N=4000000)  10:36:57Z ===
generated in 1 s: nul_dimn.parquet 4.0 MB; nul_p10.parquet 39.5 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.1004825 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            null_share_ki = 0.10014 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            null_share_kd = 0.10009 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            null_share_ks = 0.100178 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            null_share_v = 0.0994655 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            null_share_w = 0.100162 (expected 0.09915147186257615 .. 0.10084852813742386)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.055388176772739495 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p10                      groups=834713   duckdb=441ms engine: 1t=362.7ms 4t=362.5ms 16t=381.7ms peak_rss=357MB
  self-test CAUGHT   mutant 'NULL group dropped from the output' on nl_agg_kn_p10: DIFFERENT: EOF on nl_agg_kn_p10.engine after byte 17100560, line 834712; first differing line 834713: duckdb=[NULL|200965|100431929|-30572128] engine=[]
identical             nl_agg_multi_p10                   groups=1997288  duckdb=822ms engine: 1t=303.2ms 4t=632.5ms 16t=597.0ms peak_rss=868MB
identical             nl_agg_ksi_p10                     groups=1573790  duckdb=678ms engine: 1t=373.6ms 4t=514.5ms 16t=517.3ms peak_rss=653MB
identical             nl_agg_filt_p10                    groups=5001     duckdb=125ms engine: 1t=61.4ms 4t=39.1ms 16t=64.7ms peak_rss=27MB
identical             nl_join_kn_p10                     groups=97       duckdb=113ms engine: 1t=503.8ms 4t=264.6ms 16t=245.5ms peak_rss=67MB
identical             nl_join_ks_p10                     groups=7        duckdb=102ms engine: 1t=75.8ms 4t=97.3ms 16t=189.7ms peak_rss=30MB
--- nul10 done in 45 s

=== data set: nul50  (N=4000000)  10:37:42Z ===
generated in 1 s: nul_dimn.parquet 4.0 MB; nul_p50.parquet 27.3 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.5003475 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            null_share_ki = 0.4996525 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            null_share_kd = 0.4996525 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            null_share_ks = 0.4996525 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            null_share_v = 0.4996525 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            null_share_w = 0.5003475 (expected 0.4985857864376269 .. 0.5014142135623731)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.09960977120900974 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p50                      groups=632342   duckdb=365ms engine: 1t=240.9ms 4t=284.5ms 16t=251.6ms peak_rss=260MB
identical             nl_agg_multi_p50                   groups=1484291  duckdb=814ms engine: 1t=442.9ms 4t=577.4ms 16t=568.1ms peak_rss=632MB
identical             nl_agg_ksi_p50                     groups=873837   duckdb=438ms engine: 1t=217.5ms 4t=310.2ms 16t=366.1ms peak_rss=354MB
identical             nl_agg_filt_p50                    groups=1        duckdb=102ms engine: 1t=33.2ms 4t=37.8ms 16t=66.7ms peak_rss=12MB
identical             nl_join_kn_p50                     groups=97       duckdb=104ms engine: 1t=335.7ms 4t=225.7ms 16t=245.9ms peak_rss=56MB
identical             nl_join_ks_p50                     groups=7        duckdb=125ms engine: 1t=88.5ms 4t=84.0ms 16t=118.8ms peak_rss=20MB
--- nul50 done in 31 s

=== data set: nul90  (N=4000000)  10:38:13Z ===
generated in 0 s: nul_dimn.parquet 4.0 MB; nul_p90.parquet 14.9 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.900382 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            null_share_ki = 0.899806 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            null_share_kd = 0.8997785 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            null_share_ks = 0.8998505 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            null_share_v = 0.89957 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            null_share_w = 0.8998835 (expected 0.8991514718625762 .. 0.9008485281374239)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.0 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p90                      groups=180846   duckdb=150ms engine: 1t=73.4ms 4t=71.0ms 16t=105.6ms peak_rss=86MB
identical             nl_agg_multi_p90                   groups=203845   duckdb=215ms engine: 1t=131.7ms 4t=140.5ms 16t=168.3ms peak_rss=98MB
identical             nl_agg_ksi_p90                     groups=172689   duckdb=156ms engine: 1t=86.5ms 4t=119.8ms 16t=115.0ms peak_rss=89MB
identical             nl_agg_filt_p90                    groups=1        duckdb=66ms engine: 1t=17.4ms 4t=35.2ms 16t=79.7ms peak_rss=11MB
identical             nl_join_kn_p90                     groups=97       duckdb=114ms engine: 1t=182.6ms 4t=129.7ms 16t=200.1ms peak_rss=53MB
identical             nl_join_ks_p90                     groups=7        duckdb=101ms engine: 1t=21.6ms 4t=96.0ms 16t=119.4ms peak_rss=7MB
--- nul90 done in 7 s

=== data set: nul99  (N=4000000)  10:38:20Z ===
generated in 0 s: nul_dimn.parquet 4.0 MB; nul_p99.parquet 8.8 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 0.99003 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            null_share_ki = 0.9899995 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            null_share_kd = 0.990027 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            null_share_ks = 0.990049 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            null_share_v = 0.9900065 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            null_share_w = 0.990122 (expected 0.9897185750544106 .. 0.9902814249455894)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.0 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p99                      groups=19753    duckdb=101ms engine: 1t=49.0ms 4t=41.6ms 16t=73.3ms peak_rss=15MB
identical             nl_agg_multi_p99                   groups=35384    duckdb=117ms engine: 1t=57.6ms 4t=58.7ms 16t=89.9ms peak_rss=29MB
identical             nl_agg_ksi_p99                     groups=17544    duckdb=97ms engine: 1t=45.1ms 4t=72.9ms 16t=87.4ms peak_rss=15MB
identical             nl_agg_filt_p99                    groups=1        duckdb=72ms engine: 1t=12.3ms 4t=12.7ms 16t=24.2ms peak_rss=7MB
identical             nl_join_kn_p99                     groups=97       duckdb=100ms engine: 1t=67.2ms 4t=73.3ms 16t=137.0ms peak_rss=57MB
identical             nl_join_ks_p99                     groups=7        duckdb=72ms engine: 1t=20.7ms 4t=105.0ms 16t=146.8ms peak_rss=10MB
--- nul99 done in 3 s

=== data set: nul100  (N=4000000)  10:38:23Z ===
generated in 1 s: nul_dimn.parquet 4.0 MB; nul_p100.parquet 7.6 MB; nul_sdim.parquet 0.0 MB; 
  data ok            rows = 2000000.0 (expected 2000000.0 .. 2000000.0)
  data ok            null_share_kn = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_ki = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_kd = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_ks = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_v = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_w = 1.0 (expected 1.0 .. 1.0)
  data ok            empty_string_share_of_non_null_ks (0.05; none if all NULL) = 0.05 (expected 0.0 .. 0.15)
  data ok            nul_dimn_null_key_rows = 100000.0 (expected 100000.0 .. 100000.0)
  data ok            nul_sdim_rows_with_empty_and_null_key = 10.0 (expected 10.0 .. 10.0)
identical             nl_agg_kn_p100                     groups=1        duckdb=54ms engine: 1t=44.8ms 4t=18.1ms 16t=46.2ms peak_rss=8MB
identical             nl_agg_multi_p100                  groups=1        duckdb=71ms engine: 1t=46.7ms 4t=38.1ms 16t=58.9ms peak_rss=7MB
identical             nl_agg_ksi_p100                    groups=1        duckdb=70ms engine: 1t=38.4ms 4t=35.2ms 16t=63.3ms peak_rss=7MB
identical             nl_agg_filt_p100                   groups=0        duckdb=45ms engine: 1t=9.4ms 4t=14.0ms 16t=23.5ms peak_rss=5MB
identical             nl_join_kn_p100                    groups=0        duckdb=88ms engine: 1t=77.1ms 4t=56.6ms 16t=134.9ms peak_rss=56MB
identical             nl_join_ks_p100                    groups=0        duckdb=88ms engine: 1t=7.9ms 4t=25.5ms 16t=70.7ms peak_rss=6MB
--- nul100 done in 3 s

=== data set: nulshape  (N=4000000)  10:38:26Z ===
generated in 1 s: nul_dimn.parquet 4.0 MB; nul_sdim.parquet 0.0 MB; nul_shape.parquet 33.5 MB; 
  data ok            rows = 4000000.0 (expected 4000000 .. 4000000)
  data ok            row_groups = 41.0 (expected 3 .. 1000000)
  data ok            row_group_rows_of_the_first_group = 98304.0 (expected 98304 .. 98304)
  data ok            row_groups_where_kr_has_a_value (exactly 1) = 1.0 (expected 1 .. 1)
  data ok            row_groups_where_vr_has_a_value (exactly 1) = 1.0 (expected 1 .. 1)
  data ok            row_groups_where_kall_has_a_value (none) = 0.0 (expected 0 .. 0)
  data ok            kr_non_null_rows = 49152.0 (expected 49151 .. 49153)
  data ok            all_NULL_aligned_8192_blocks_of_kb_al = 163.0 (expected 1 .. 1000000)
  data ok            all_valid_aligned_8192_blocks_of_kb_al = 326.0 (expected 1 .. 1000000)
  data ok            all_NULL_5000_blocks_of_kb_un = 267.0 (expected 1 .. 1000000)
  data ok            kall_null_share = 1.0 (expected 1 .. 1)
  data ok            vall_null_share = 1.0 (expected 1 .. 1)
  data ok            v_null_share = 0.0 (expected 0 .. 0)
identical             ns_agg_kb_al                       groups=1001     duckdb=89ms engine: 1t=58.4ms 4t=53.3ms 16t=76.4ms peak_rss=12MB
identical             ns_agg_kb_un                       groups=1001     duckdb=111ms engine: 1t=61.8ms 4t=23.1ms 16t=73.5ms peak_rss=15MB
identical             ns_agg_kr                          groups=1001     duckdb=66ms engine: 1t=82.0ms 4t=45.7ms 16t=63.1ms peak_rss=11MB
identical             ns_agg_ksb                         groups=1001     duckdb=91ms engine: 1t=85.7ms 4t=30.0ms 16t=74.7ms peak_rss=15MB
identical             ns_agg_g_vb                        groups=10       duckdb=70ms engine: 1t=108.4ms 4t=38.7ms 16t=85.2ms peak_rss=10MB
identical             ns_agg_kall                        groups=1        duckdb=68ms engine: 1t=81.6ms 4t=41.6ms 16t=83.1ms peak_rss=11MB
  self-test CAUGHT   mutant 'SUM does not count its NULL inputs (non-NULL count wrong)' on ns_agg_kall: DIFFERENT: ns_agg_kall.txt ns_agg_kall.engine differ: byte 25, line 1; first differing line 1: duckdb=[NULL|4000000|1998031848|NULL] engine=[NULL|4000000|1998031848|0]
identical             ns_agg_filt_vb                     groups=10       duckdb=70ms engine: 1t=32.6ms 4t=21.6ms 16t=36.4ms peak_rss=11MB
identical             ns_join_kb                         groups=97       duckdb=94ms engine: 1t=134.6ms 4t=89.5ms 16t=156.5ms peak_rss=59MB
identical             ns_join_kr                         groups=97       duckdb=101ms engine: 1t=76.4ms 4t=48.5ms 16t=102.5ms peak_rss=58MB
--- nulshape done in 5 s

=== data set: li00  (N=4000000)  10:38:31Z ===
generated in 1 s: li_p00.parquet 11.8 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_l_discount = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_l_shipdate (P/2) = 0.0 (expected 0.0 .. 0.0)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 272546.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p00                          groups=6        duckdb=112ms engine: 1t=41.3ms 4t=14.5ms 16t=41.5ms peak_rss=13MB
identical             li_q6_p00                          groups=1        duckdb=83ms engine: 1t=21.8ms 4t=21.6ms 16t=17.8ms peak_rss=8MB
--- li00 done in 1 s

=== data set: li01  (N=4000000)  10:38:32Z ===
generated in 1 s: li_p01.parquet 11.9 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.010218 (expected 0.009602005025157352 .. 0.010397994974842648)
  data ok            null_share_l_discount = 0.010089 (expected 0.009602005025157352 .. 0.010397994974842648)
  data ok            null_share_l_shipdate (P/2) = 0.005 (expected 0.004717865280406682 .. 0.005282134719593318)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 269763.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p01                          groups=6        duckdb=97ms engine: 1t=57.0ms 4t=27.2ms 16t=32.4ms peak_rss=13MB
identical             li_q6_p01                          groups=1        duckdb=106ms engine: 1t=22.1ms 4t=14.8ms 16t=25.0ms peak_rss=10MB
--- li01 done in 2 s

=== data set: li10  (N=4000000)  10:38:34Z ===
generated in 0 s: li_p10.parquet 11.5 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.1002 (expected 0.0988 .. 0.10120000000000001)
  data ok            null_share_l_discount = 0.099957 (expected 0.0988 .. 0.10120000000000001)
  data ok            null_share_l_shipdate (P/2) = 0.050107 (expected 0.049128220211291866 .. 0.05087177978870814)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 245249.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p10                          groups=6        duckdb=103ms engine: 1t=41.8ms 4t=40.2ms 16t=56.0ms peak_rss=13MB
identical             li_q6_p10                          groups=1        duckdb=86ms engine: 1t=21.1ms 4t=18.0ms 16t=26.4ms peak_rss=11MB
--- li10 done in 1 s

=== data set: li50  (N=4000000)  10:38:35Z ===
generated in 0 s: li_p50.parquet 8.9 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.499932 (expected 0.498 .. 0.502)
  data ok            null_share_l_discount = 0.499932 (expected 0.498 .. 0.502)
  data ok            null_share_l_shipdate (P/2) = 0.250043 (expected 0.24826794919243111 .. 0.2517320508075689)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 135889.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p50                          groups=6        duckdb=103ms engine: 1t=42.0ms 4t=20.1ms 16t=46.1ms peak_rss=16MB
identical             li_q6_p50                          groups=1        duckdb=96ms engine: 1t=16.5ms 4t=12.7ms 16t=15.0ms peak_rss=8MB
--- li50 done in 1 s

=== data set: li90  (N=4000000)  10:38:36Z ===
generated in 1 s: li_p90.parquet 6.4 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.900213 (expected 0.8988 .. 0.9012)
  data ok            null_share_l_discount = 0.899978 (expected 0.8988 .. 0.9012)
  data ok            null_share_l_shipdate (P/2) = 0.449914 (expected 0.44801002512578675 .. 0.4519899748742133)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 27163.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p90                          groups=6        duckdb=73ms engine: 1t=23.7ms 4t=15.4ms 16t=15.5ms peak_rss=9MB
identical             li_q6_p90                          groups=1        duckdb=66ms engine: 1t=10.1ms 4t=5.9ms 16t=13.4ms peak_rss=7MB
--- li90 done in 1 s

=== data set: li99  (N=4000000)  10:38:37Z ===
generated in 1 s: li_p99.parquet 5.2 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.989999 (expected 0.9896020050251574 .. 0.9903979949748426)
  data ok            null_share_l_discount = 0.990149 (expected 0.9896020050251574 .. 0.9903979949748426)
  data ok            null_share_l_shipdate (P/2) = 0.494873 (expected 0.4930001000025001 .. 0.4969998999974999)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 2656.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p99                          groups=6        duckdb=75ms engine: 1t=20.8ms 4t=12.1ms 16t=12.0ms peak_rss=11MB
identical             li_q6_p99                          groups=1        duckdb=52ms engine: 1t=9.5ms 4t=5.6ms 16t=9.2ms peak_rss=8MB
--- li99 done in 1 s

=== data set: li100  (N=4000000)  10:38:38Z ===
generated in 0 s: li_p100.parquet 5.0 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_l_discount = 1.0 (expected 1.0 .. 1.0)
  data ok            null_share_l_shipdate (P/2) = 0.499932 (expected 0.498 .. 0.502)
  data ok            null_share_l_returnflag (PK) = 0.0 (expected 0.0 .. 0.0)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 0.0 (expected 0.0 .. 1000000.0)
identical             li_q1_p100                         groups=6        duckdb=73ms engine: 1t=19.5ms 4t=13.5ms 16t=20.2ms peak_rss=8MB
identical             li_q6_p100                         groups=1        duckdb=50ms engine: 1t=7.1ms 4t=5.9ms 16t=10.6ms peak_rss=5MB
--- li100 done in 1 s

=== data set: linullkey  (N=4000000)  10:38:39Z ===
generated in 0 s: li_nullkey.parquet 11.8 MB; 
  data ok            rows = 1000000.0 (expected 1000000.0 .. 1000000.0)
  data ok            null_share_l_quantity = 0.1002 (expected 0.0988 .. 0.10120000000000001)
  data ok            null_share_l_discount = 0.099957 (expected 0.0988 .. 0.10120000000000001)
  data ok            null_share_l_shipdate (P/2) = 0.050107 (expected 0.049128220211291866 .. 0.05087177978870814)
  data ok            null_share_l_returnflag (PK) = 0.100044 (expected 0.0988 .. 0.10120000000000001)
  data ok            distinct_returnflag_non_null = 3.0 (expected 3.0 .. 3.0)
  data ok            q6_discount_in_range_rows (rows reach the sum when P is small) = 245249.0 (expected 0.0 .. 1000000.0)
REFUSED-OK            li_q1_nullkey                      groups=0        duckdb=86ms engine: 1t=nams 4t=nams 16t=nams peak_rss=0MB
identical             li_q6_nullkey                      groups=1        duckdb=96ms engine: 1t=21.2ms 4t=16.4ms 16t=33.7ms peak_rss=12MB
--- linullkey done in 1 s

=== hot key on the build side: time per joined row (chain 30% of the build rows) ===
  threads=1: build chain B/4 -> 15213800 joined rows, 33.8 ns/row; chain B -> 60859985 joined rows, 29.0 ns/row; ratio 0.86  [ok]
  threads=4: build chain B/4 -> 15213800 joined rows, 12.4 ns/row; chain B -> 60859985 joined rows, 10.9 ns/row; ratio 0.88  [ok]
  threads=16: build chain B/4 -> 15213800 joined rows, 14.1 ns/row; chain B -> 60859985 joined rows, 8.5 ns/row; ratio 0.60  [ok]

=== summary ===
queries run: 100; identical to DuckDB at every thread count (or refused as expected): 100; failing: 0; self-test failures: 0; data-shape check failures: 0
peak scratch disk: 1 GB (1526 MB); wall time 11 min
RESULT: ALL IDENTICAL to DuckDB; self-tests behaved
```

Result table (`results.tsv`, unedited):

```
query	tag	result	duckdb_ms	engine_ms	rss_mb	groups	out_rows	detail
sk_agg_k10	skew	ok: 1:identical 4:identical 16:identical	596	394.2	231	520629	4000000	
selftest:mutant:merge loses the last thread's partial table	skew	caught						DIFFERENT: sk_agg_k10.txt sk_agg_k10.engine differ: byte 8, line 1; first differing line 1: duckdb=[100000|4|1644|128616] engine=[100000|3|1216|-28213]
sk_agg_k15	skew	ok: 1:identical 4:identical 16:identical	316	173.5	49	38451	4000000	
sk_agg_hot	skew	ok: 1:identical 4:identical 16:identical	624	465.7	332	939011	4000000	
sk_agg_uni	skew	ok: 1:identical 4:identical 16:identical	429	417.2	364	981639	4000000	
sk_agg_mode_filt	skew	ok: 1:identical 4:identical 16:identical	114	83.9	37	19	2000364	
sk_agg_k15_mode	skew	ok: 1:identical 4:identical 16:identical	162	96.6	31	38480	4000000	
sk_join_k10	skew	ok: 1:identical 4:identical 16:identical	164	370.5	89	1000	4000000	
sk_join_k15	skew	ok: 1:identical 4:identical 16:identical	163	208.8	71	1000	4000000	
sk_join_hot	skew	ok: 1:identical 4:identical 16:identical	187	297.8	68	5	4000000	
sk_join_hot_df0	skew	ok: 1:identical 4:identical 16:identical	190	261.4	56	5	2599414	
sk_join_hot_df1	skew	ok: 1:identical 4:identical 16:identical	165	253.0	49	5	1400586	
sk_join_k15_key	skew	ok: 1:identical 4:identical 16:identical	167	154.3	63	38451	4000000	
sk_join_build_hot	skew	ok: 1:identical 4:identical 16:identical	125	316.8	208	1000	4000000	
sk_join_build_k15	skew	ok: 1:identical 4:identical 16:identical	116	308.3	197	1000	4000000	
hb_chain_small	hot	ok: 1:identical 4:identical 16:identical	302	214.1	47	77	15213800	
hb_chain_big	hot	ok: 1:identical 4:identical 16:identical	347	517.4	139	77	60859985	
hb_hot_hot	hot	ok: 1:identical 4:identical 16:identical	492	886.5	7	100	36933006	
selftest:mutant:fan-out: only the first match of a chain is emitted	hot	caught						DIFFERENT: hb_hot_hot.txt hb_hot_hot.engine differ: byte 5, line 1; first differing line 1: duckdb=[0|0|367922|180094806|182420013] engine=[0|0|78|35777|38781]
st_agg_hi	str	ok: 1:identical 4:identical 16:identical	1055	494.2	715	1554834	3000000	
st_agg_long	str	ok: 1:identical 4:identical 16:identical	1323	1012.6	1671	950396	3000000	
st_agg_empty	str	ok: 1:identical 4:identical 16:identical	89	83.3	13	1002	3000000	
st_agg_utf8	str	ok: 1:identical 4:identical 16:identical	412	387.0	218	147000	3000000	
st_agg_last	str	ok: 1:identical 4:identical 16:identical	423	397.1	351	62000	3000000	
st_agg_first	str	ok: 1:identical 4:identical 16:identical	449	355.1	348	62000	3000000	
st_agg_len	str	ok: 1:identical 4:identical 16:identical	106	151.2	52	300	3000000	
st_agg_instr	str	ok: 1:identical 4:identical 16:identical	375	507.0	332	299981	3000000	
st_agg_pair	str	ok: 1:identical 4:identical 16:identical	988	734.6	911	1846992	3000000	
st_agg_filt	str	ok: 1:identical 4:identical 16:identical	477	321.1	537	1056263	1500514	
st_agg_instr_filt	str	ok: 1:identical 4:identical 16:identical	344	287.7	286	294576	1205156	
st_join_long	str	ok: 1:identical 4:identical 16:identical	553	736.8	1006	1000	3000000	
st_join_hi	str	ok: 1:identical 4:identical 16:identical	274	434.5	184	1000	3000000	
st_join_instr	str	ok: 1:identical 4:identical 16:identical	243	263.3	136	777	2699896	
st_join_utf8	str	ok: 1:identical 4:identical 16:identical	240	180.7	61	100	3000000	
st_join_empty_fanout	str	ok: 1:identical 4:identical 16:identical	161	367.7	20	13	48099631	
nl_agg_kn_p00	nul00	ok: 1:identical 4:identical 16:identical	464	379.6	358	864572	2000000	
nl_agg_multi_p00	nul00	ok: 1:identical 4:identical 16:identical	937	615.1	871	1999980	2000000	
nl_agg_ksi_p00	nul00	ok: 1:identical 4:identical 16:identical	774	530.4	747	1767553	2000000	
nl_agg_filt_p00	nul00	ok: 1:identical 4:identical 16:identical	184	76.2	23	5000	1000079	
nl_join_kn_p00	nul00	ok: 1:identical 4:identical 16:identical	205	251.5	68	97	2000000	
nl_join_ks_p00	nul00	ok: 1:identical 4:identical 16:identical	144	195.9	38	7	507966	
nl_agg_kn_p01	nul01	ok: 1:identical 4:identical 16:identical	736	403.4	358	861877	2000000	
nl_agg_multi_p01	nul01	ok: 1:identical 4:identical 16:identical	1215	602.1	885	1999954	2000000	
nl_agg_ksi_p01	nul01	ok: 1:identical 4:identical 16:identical	1632	505.9	709	1750499	2000000	
nl_agg_filt_p01	nul01	ok: 1:identical 4:identical 16:identical	132	63.0	21	5001	990096	
nl_join_kn_p01	nul01	ok: 1:identical 4:identical 16:identical	118	245.3	68	97	1979914	
nl_join_ks_p01	nul01	ok: 1:identical 4:identical 16:identical	117	144.7	27	7	507878	
nl_agg_kn_p10	nul10	ok: 1:identical 4:identical 16:identical	441	381.7	357	834713	2000000	
selftest:mutant:NULL group dropped from the output	nul10	caught						DIFFERENT: EOF on nl_agg_kn_p10.engine after byte 17100560, line 834712; first differing line 834713: duckdb=[NULL|200965|100431929|-30572128] engine=[]
nl_agg_multi_p10	nul10	ok: 1:identical 4:identical 16:identical	822	597.0	868	1997288	2000000	
nl_agg_ksi_p10	nul10	ok: 1:identical 4:identical 16:identical	678	517.3	653	1573790	2000000	
nl_agg_filt_p10	nul10	ok: 1:identical 4:identical 16:identical	125	64.7	27	5001	899686	
nl_join_kn_p10	nul10	ok: 1:identical 4:identical 16:identical	113	245.5	67	97	1799035	
nl_join_ks_p10	nul10	ok: 1:identical 4:identical 16:identical	102	189.7	30	7	506956	
nl_agg_kn_p50	nul50	ok: 1:identical 4:identical 16:identical	365	251.6	260	632342	2000000	
nl_agg_multi_p50	nul50	ok: 1:identical 4:identical 16:identical	814	568.1	632	1484291	2000000	
nl_agg_ksi_p50	nul50	ok: 1:identical 4:identical 16:identical	438	366.1	354	873837	2000000	
nl_agg_filt_p50	nul50	ok: 1:identical 4:identical 16:identical	102	66.7	12	1	499867	
nl_join_kn_p50	nul50	ok: 1:identical 4:identical 16:identical	104	245.9	56	97	999305	
nl_join_ks_p50	nul50	ok: 1:identical 4:identical 16:identical	125	118.8	20	7	502926	
nl_agg_kn_p90	nul90	ok: 1:identical 4:identical 16:identical	150	105.6	86	180846	2000000	
nl_agg_multi_p90	nul90	ok: 1:identical 4:identical 16:identical	215	168.3	98	203845	2000000	
nl_agg_ksi_p90	nul90	ok: 1:identical 4:identical 16:identical	156	115.0	89	172689	2000000	
nl_agg_filt_p90	nul90	ok: 1:identical 4:identical 16:identical	66	79.7	11	1	100145	
nl_join_kn_p90	nul90	ok: 1:identical 4:identical 16:identical	114	200.1	53	97	199236	
nl_join_ks_p90	nul90	ok: 1:identical 4:identical 16:identical	101	119.4	7	7	1019	
nl_agg_kn_p99	nul99	ok: 1:identical 4:identical 16:identical	101	73.3	15	19753	2000000	
nl_agg_multi_p99	nul99	ok: 1:identical 4:identical 16:identical	117	89.9	29	35384	2000000	
nl_agg_ksi_p99	nul99	ok: 1:identical 4:identical 16:identical	97	87.4	15	17544	2000000	
nl_agg_filt_p99	nul99	ok: 1:identical 4:identical 16:identical	72	24.2	7	1	9905	
nl_join_kn_p99	nul99	ok: 1:identical 4:identical 16:identical	100	137.0	57	97	19940	
nl_join_ks_p99	nul99	ok: 1:identical 4:identical 16:identical	72	146.8	10	7	99	
nl_agg_kn_p100	nul100	ok: 1:identical 4:identical 16:identical	54	46.2	8	1	2000000	
nl_agg_multi_p100	nul100	ok: 1:identical 4:identical 16:identical	71	58.9	7	1	2000000	
nl_agg_ksi_p100	nul100	ok: 1:identical 4:identical 16:identical	70	63.3	7	1	2000000	
nl_agg_filt_p100	nul100	ok: 1:identical 4:identical 16:identical	45	23.5	5	0	0	
nl_join_kn_p100	nul100	ok: 1:identical 4:identical 16:identical	88	134.9	56	0	0	
nl_join_ks_p100	nul100	ok: 1:identical 4:identical 16:identical	88	70.7	6	0	0	
ns_agg_kb_al	nulshape	ok: 1:identical 4:identical 16:identical	89	76.4	12	1001	4000000	
ns_agg_kb_un	nulshape	ok: 1:identical 4:identical 16:identical	111	73.5	15	1001	4000000	
ns_agg_kr	nulshape	ok: 1:identical 4:identical 16:identical	66	63.1	11	1001	4000000	
ns_agg_ksb	nulshape	ok: 1:identical 4:identical 16:identical	91	74.7	15	1001	4000000	
ns_agg_g_vb	nulshape	ok: 1:identical 4:identical 16:identical	70	85.2	10	10	4000000	
ns_agg_kall	nulshape	ok: 1:identical 4:identical 16:identical	68	83.1	11	1	4000000	
selftest:mutant:SUM does not count its NULL inputs (non-NULL count wrong)	nulshape	caught						DIFFERENT: ns_agg_kall.txt ns_agg_kall.engine differ: byte 25, line 1; first differing line 1: duckdb=[NULL|4000000|1998031848|NULL] engine=[NULL|4000000|1998031848|0]
ns_agg_filt_vb	nulshape	ok: 1:identical 4:identical 16:identical	70	36.4	11	10	1000245	
ns_join_kb	nulshape	ok: 1:identical 4:identical 16:identical	94	156.5	59	97	2664704	
ns_join_kr	nulshape	ok: 1:identical 4:identical 16:identical	101	102.5	58	97	49152	
li_q1_p00	li00	ok: 1:identical 4:identical 16:identical	112	41.5	13	6	0	
li_q6_p00	li00	ok: 1:identical 4:identical 16:identical	83	17.8	8	1	0	
li_q1_p01	li01	ok: 1:identical 4:identical 16:identical	97	32.4	13	6	0	
li_q6_p01	li01	ok: 1:identical 4:identical 16:identical	106	25.0	10	1	0	
li_q1_p10	li10	ok: 1:identical 4:identical 16:identical	103	56.0	13	6	0	
li_q6_p10	li10	ok: 1:identical 4:identical 16:identical	86	26.4	11	1	0	
li_q1_p50	li50	ok: 1:identical 4:identical 16:identical	103	46.1	16	6	0	
li_q6_p50	li50	ok: 1:identical 4:identical 16:identical	96	15.0	8	1	0	
li_q1_p90	li90	ok: 1:identical 4:identical 16:identical	73	15.5	9	6	0	
li_q6_p90	li90	ok: 1:identical 4:identical 16:identical	66	13.4	7	1	0	
li_q1_p99	li99	ok: 1:identical 4:identical 16:identical	75	12.0	11	6	0	
li_q6_p99	li99	ok: 1:identical 4:identical 16:identical	52	9.2	8	1	0	
li_q1_p100	li100	ok: 1:identical 4:identical 16:identical	73	20.2	8	6	0	
li_q6_p100	li100	ok: 1:identical 4:identical 16:identical	50	10.6	5	1	0	
li_q1_nullkey	linullkey	ok: 1:refused-as-expected 4:refused-as-expected 16:refused-as-expected	86		0			
li_q6_nullkey	linullkey	ok: 1:identical 4:identical 16:identical	96	33.7	12	1	0
```
