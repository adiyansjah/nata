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
