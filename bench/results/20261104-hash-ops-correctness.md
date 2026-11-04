# Hash aggregate and hash join spike: correctness (2026-11-04)

Spike: `spikes/hash-ops/` (general hash aggregate and inner hash join on Arrow batches with a selection vector;
extends `20261104-generic-ops-x86.md`). This file records the correctness work that had to come before any speed
claim (`docs/way-of-working.md` sections 5 and 6): the failures the property tests found, the fixes, the checks that
show the fixes hold, and what could not be checked locally. **It contains no speed result.** Every number marked
**to be filled** comes from the GCP run. (Update, later the same day: the mutation part of the GCP run is done, see section 3; the benchmark and memory numbers are still to be filled.)

Local machine for everything below: shared Linux x86-64 box, 8 cores, load average 1 to 50 while the work ran
(other agents were building and model-checking). Nothing here depends on timing except the build and run times,
which are given as loaded-machine figures and are not evidence of speed.

## 1. The five original failures

Before any change, `cargo test --release` gave 10 passing kernel tests and 5 failing property tests. Three
different causes. The "trace" lines are reasoned from the code; the confirmation is that the one-line fix flips the
named tests and that re-introducing the bug as a mutant is caught (section 5).

### 1a. Root cause A (engine): the NULL flag of the very first group key was lost

Tests: `a_null_group_key_is_its_own_group_and_empty_string_is_not_null`,
`many_groups_across_batches_and_threads_match_the_reference`, `aggregate_matches_the_scalar_reference` (case 0),
`join_then_aggregate_matches_the_reference` (case 7).

- Symptom: `duplicate group key in a result ([S("")] / [I(0)])`; the engine returned
  `{[Null,Null,Null]: 299, [I(0), S(""), S("")]: 1}` where the reference has `{[Null,Null,Null]: 300}`; the NULL group
  split in two.
- Where: `KStore::push` in `src/keys.rs`. The store keeps a per-column "is NULL" byte vector that stays empty until
  the first NULL key. The old code was
  `if null && nulls.is_empty() { nulls.resize(len, 0) }  if !nulls.is_empty() { nulls.push(null as u8) }`.
  When the first group ever stored is the NULL group, `len` is 0, `resize(0, 0)` leaves the vector empty, and the
  second `if` then skips the push: the NULL flag is never recorded.
- Minimal input: one column of group keys `[NULL, NULL]` (and, for the duplicate: `[NULL, ""]`, the
  `a_null_group_key...` test uses `NULL, "", NULL, "x"`). Trace: group 0 is stored with the value bytes of the
  NULL slot and no flag, so it prints as `""` (or `0`). The second NULL row compares equal to nothing
  (`stored not-NULL != row NULL`) and opens group 1, which does get a flag. Two groups for one key. For `[NULL, ""]`
  the `""` row matches the unflagged group 0 and the two are merged. Across threads the same key then appears
  twice in the merged result, which `Output::add` rejects.
- Fix: `if null { if nulls.is_empty() { nulls.resize(len, 0) } nulls.push(1) } else if !nulls.is_empty() { nulls.push(0) }`
  (invariant stated in a comment: empty, or exactly one byte per stored group).
- Held by: the four tests above pass; mutant "first group has a NULL key: NULL flag lost (the original bug)" is
  CAUGHT (section 5).

### 1b. Root cause B (test): a wrong expected count

Test: `string_keys_join_across_dictionary_and_plain_representations` expected 5 rows, the engine gave 6.
Hand count, independent of the engine: build keys `a, b, b, ""`, probe keys `b, "", c, a, b`. Probe `b` (two rows)
matches the two build `b` rows: 4. Probe `""` matches the build `""`: 1. Probe `a` matches build `a`: 1. Probe `c`
matches nothing. Total 6. The test's own failure message ("b matches twice for each of two probe rows, plus '' and a")
adds up to 6, and the assertion just before it (`got == want`, engine against the nested-loop reference) passed
for all four dictionary/plain combinations. The constant was wrong, not the engine. Changed to 6 with the hand
count in a comment. No other test was edited or weakened.

### 1c. Root cause C (engine): dictionary lookup before the NULL test

Test: `aggregate_matches_the_scalar_reference`, which after fix A stopped on a panic hidden behind the first
assertion: `index out of bounds: the len is 0 but the index is 0` at `src/engine.rs:172` (`FilterStrIn`).

- Cause: `ok[keys[i] as usize] & !kc.is_null(i)` uses the non-short-circuit `&`, so the dictionary is indexed with
  the key stored under a NULL slot. Arrow leaves that key unspecified; here the column was all NULL, the dictionary
  empty and the key 0.
- Same pattern in `Step::FlagIn` (`!kc.is_null(i) & (ok[keys[i] as usize] != negate)`).
- Fix: `&&` in both places.
- Held by: mutants "string filter looks up the dictionary before the NULL test (original bug)" and
  "string flag looks up ..." CAUGHT (the second one was a survivor, section 4).

## 2. What else changed in the spike

- `src/join.rs`: `JoinTable::stats()` (`BuildStats`: rows, rows in the fullest of the 64 partitions, key comparisons made
  while building), an `u64` increment per comparison in the build loop. Used by the hash-quality test (section 4).
- `src/testkit.rs`: the random tables now put **junk under every NULL slot** (non-zero, often huge integers;
  junk bytes under NULL strings; dictionary keys 99, -1, `i32::MAX`, `i32::MIN`, in range, or 0 under NULL keys).
  Before, NULL slots held 0 or an empty string, which hides every "read the slot under a NULL" bug. Junk depends only on the
  position, not on the test's random stream. `seeded(base)` / `scaled(n)`: environment variables `HASH_OPS_SEED`
  (default 0 = exactly the old streams) and `HASH_OPS_SCALE` (case-count multiplier) so repeated runs explore new
  inputs; the original tests ran the same fixed seeds every time.
- `src/main.rs`: `check` writes one file per thread count; `rss` prints the process's own peak RSS (`VmHWM`).
- New scripts: `check.sh`, `mutate.py`, `run-all.sh`, `duckdb-time.sh`.
- `Cargo.toml`: `[profile.release] overflow-checks = true` and the dependency override of section 6.

## 3. Verification, final configuration

Final configuration = `overflow-checks = true` for our code, `false` for dependencies (section 6).

| Check | Result | Command |
|---|---|---|
| Tests | **37 pass**: 10 kernel unit, 13 properties, 5 null-slots, 5 hash-quality, 4 overflow-config | `cargo test --release` |
| Seed sweeps | 0 failures in 25 seeds x 3 scale (before the junk generator), 30 seeds x 3 (after it, before overflow-checks), 20 seeds x 3 (final configuration; 7 test binaries each) | `HASH_OPS_SEED=$s HASH_OPS_SCALE=3 cargo test --release` |
| DuckDB differential | **7 of 7 queries identical at 1, 4 and 8 threads (21 of 21)**, digit for digit, run in three configurations (checks off, all checks on, final) | `spikes/hash-ops/check.sh` |
| NULL injection | **21 of 21** (7 queries x 10/50/100 percent NULLs, engine against the scalar reference at 1 and 4 threads), run in the same configurations | `hash-ops nullcheck data/tpch-sf1` |
| Plain `cargo test` (debug-style `test` profile) | null-slots and overflow-config tests pass (build 7 min 15 s on the loaded machine) | `cargo test --test overflow_config --test null_slots` |

DuckDB diff details (SF1; DuckDB v2.0.0; rows per query: agg_orderkey 1,493,405; agg_partkey 200,000;
agg_modedate 17,677; agg_comment 1,482,071; q3 11,620; q12 2; j3 35). The same text form is produced by both
sides (`src/out.rs` `lines()`: key columns, count, exact decimal sums, bytewise sorted). sha256 prefixes of the
DuckDB text from the first run: agg_orderkey `e3b9514312e1`, agg_partkey `64cb6371ea2f`, agg_modedate `9c46d79a1f3e`,
agg_comment `8ea69d2b4ba3`, q3 `eaf42bb16276`, q12 `01fd331449d5`, j3 `1fc78a4153fc`. The check also asserts that the engine's
1, 4 and 8 thread results and the hand-fused baseline are identical to each other. The data has no NULLs, so
the NULL semantics are covered by `nullcheck` and the property tests, not by the DuckDB diff.

NULL-injection limit: at 100 percent NULLs every column the query reads is NULL, including the filter columns, so
the aggregate queries return 0 groups there; the NULL-group paths are exercised at 10 and 50 percent and by the
property tests.

## 4. The two mutants that survived the first full run, and what killed them

The orchestrator's clean-machine GCP run of the 30 mutants reported 28 caught and 2 SURVIVED. The same two had
survived or been caught locally as follows.

### 4a. "string flag looks up the dictionary before the NULL test" (`Step::FlagIn`)

- Why it survived: no test exercised `FlagIn` with a dictionary whose NULL slots hold an out-of-range key. The
  generated tables always had 0 under a NULL, and the dictionary was never empty at the same time.
- Killed by `tests/null_slots.rs` (new), inputs exactly as the orchestrator described: dictionary arrays whose NULL
  slots hold keys 99, -1, `i32::MAX`, `i32::MIN`, a key out of range for a 2-entry dictionary, and an EMPTY
  dictionary with all-NULL keys, across three batches. Arrow accepts these (`DictionaryArray::try_new` validates only
  the valid slots). The tests run them through every path that reads a dictionary key and compare with hand-computed
  expectations and the scalar reference: the string filter, the CASE flag in both polarities (NULL gives 0 in both),
  an aggregate keyed on the dictionary (reference `rf::agg`), the Q12 pattern (group by an integer, sum two flags), a
  join with dictionary keys on both sides plus a dictionary payload with NULL slots (reference `rf::join_nested`;
  1 and 3 threads; caps 1, 2 and 8192), and a build side that is only an empty all-NULL dictionary.
- Same pattern elsewhere in the engine? Read every `keys[...]` use in `src/`: `KCol::bytes` and the dictionary hash
  are only reached for non-NULL rows (the hash path checks the NULL bit first; the probe skips NULL keys; the build
  drops NULL-key rows; `KStore` tests the stored flag first). `src/fused.rs` indexes `keys[i]` without a NULL test, but it
  is the hand-fused baseline for the NULL-free TPC-H data and is not run on NULL data (a limit, not a bug of the engine).
- Local result: CAUGHT by the test layer (`mutate.py "string flag looks up"`, run before the overflow
  changes of section 6; the test it fails is unchanged since). Re-run in the final configuration: for the GCP run.

### 4b. "join key hash ignores the second key column" (`hash_cols`)

- Semantically equivalent for results, and that is why it survived: equality still compares every key column, so
  every row lands in the right group or join match. Only the collision rate changes (the hash becomes a function of
  the first column, so keys that differ in a later column collide and the table degenerates towards a list). No
  property test against a reference can see that.
- Killed by `tests/hash_quality.rs` (new), which measures instead of comparing results:
  1. Keys that differ ONLY in the second, or only in the third, column (10,000 distinct keys, four type combinations
     including strings, dictionaries, dates and decimals): the fraction of hashes that collide must be under 1 percent.
     Measured: 0.0000 percent in all eight cases (a 64-bit hash). Also 10,000 random composite keys: under 1 percent.
  2. Join build on such keys (5,000 rows, 1 and 4 threads): at most n/4 key comparisons in total and the fullest of the
     64 partitions holds at most n/16 rows. Measured with the engine's own new counters: 189 to 226 comparisons for
     5,000 rows (n/25 or less; hashbrown compares only on a matching 7-bit tag) and 93 to 99 rows in the fullest
     partition (mean 78).
  3. The detectors are shown to fail on weakened hashes: a hash of the first column only collides for 99.99 percent
     of the keys; a hash cut to 8 bits for 97.4 percent; with the test hook `with_hash_mask(0x3)` the build makes
     11,168,406 comparisons for 5,000 rows and puts all 5,000 rows in one partition.
- Local result: CAUGHT by the test layer.

## 5. Mutation check

`mutate.py` applies one breakage at a time to a private copy (`$HASH_OPS_MUT_ROOT`, default
`target/mutate`), runs the tests, then `nullcheck`, then `check.sh` (DuckDB), and stops at the first layer that fails.
It never touches the real sources (it byte-compares them before and after; this passed). 30 mutants, listed by
`python3 mutate.py --list`.

Local results for rows 1-23 (23 of 30 ran before the session that started the run ended; run by `mutate.py` in the configuration
before the overflow-checks work; every mutant that was caught was caught by the **test layer**). Rows 24-30 come from the GCP run, and the GCP run also re-checked all 30 (all CAUGHT, see below):

| # | Breakage | Result |
|---:|---|---|
| 1 | first group has a NULL key: NULL flag lost (the original bug) | CAUGHT (tests) |
| 2 | string filter looks up the dictionary before the NULL test (original bug) | CAUGHT (tests) |
| 3 | string flag looks up the dictionary before the NULL test | SURVIVED, then CAUGHT after `null_slots.rs` (section 4a); CAUGHT (tests) on GCP |
| 4 | join build keeps rows with a NULL key | CAUGHT (tests) |
| 5 | join build keeps NULL-key rows when there is no selection | CAUGHT (tests) |
| 6 | join probe does not skip rows with a NULL key | CAUGHT (tests) |
| 7 | fan-out: only the first match of a chain is emitted | CAUGHT (tests) |
| 8 | fan-out: the build chain drops earlier duplicates | CAUGHT (tests) |
| 9 | fan-out: every match emitted twice | CAUGHT (tests) |
| 10 | probe ignores the selection vector | CAUGHT (tests) |
| 11 | build ignores the selection vector | CAUGHT (tests) |
| 12 | aggregate ignores the selection vector | CAUGHT (tests) |
| 13 | merge loses the last thread's partial table | CAUGHT (tests) |
| 14 | merge drops the COUNT(*) of the partial tables | CAUGHT (tests) |
| 15 | merge drops the NULL-skip counters of the partial tables | CAUGHT (tests) |
| 16 | NULL group dropped from the output | CAUGHT (tests) |
| 17 | hash collision treated as equality (aggregate) | CAUGHT (tests) |
| 18 | hash collision treated as equality (aggregate merge) | CAUGHT (tests) |
| 19 | hash collision treated as equality (join probe) | CAUGHT (tests) |
| 20 | hash collision treated as equality (join build) | CAUGHT (tests) |
| 21 | group-key equality ignores NULL against a value | CAUGHT (tests) |
| 22 | dictionary join key compared by code, not by value | CAUGHT (tests) |
| 23 | dictionary key hashed by code, not by value | CAUGHT (tests) |
| 24 | output batching: a batch may exceed the cap by one row | CAUGHT (tests) on GCP |
| 25 | output batching: a final batch of one row is lost | CAUGHT (tests) on GCP |
| 26 | SUM adds the values of NULL rows | CAUGHT (tests) on GCP |
| 27 | SUM does not count its NULL inputs (non-NULL count wrong) | CAUGHT (tests) on GCP |
| 28 | empty build side not short-circuited in the probe | CAUGHT (tests) on GCP |
| 29 | join key hash ignores the second key column | SURVIVED on the first GCP run; CAUGHT locally after `hash_quality.rs` (section 4b); CAUGHT (tests) on GCP |
| 30 | range filter keeps one extra, rejected row | CAUGHT (tests) on GCP |

Final GCP run (2026-11-04, run `nata-1104-150429`, job `parmut-hashops`, `n4a-highcpu-32` Spot, Neoverse-N3, 32 vCPU, 12 parallel workers, Debian 12 arm64, 19 minutes of VM time, about $0.13): **30 of 30 mutants CAUGHT, all by the test layer; the unmutated baseline passed all three layers (tests, nullcheck, DuckDB diff) in every one of the 30 workers' runs (169-301 s each).** Dependency versions were the locked ones (`Cargo.lock`, arrow 60.1.0, foldhash 0.2.0, hashbrown 0.17.1). The two mutants that survived the first GCP run (rows 3 and 29) are caught with the new tests.

Two earlier attempts of this GCP job were not usable, both because of the mutation script and not the engine: (1) the baseline failed in 18 of 30 items because `mutate.py` restored files with their old modification times, so cargo reused the previous mutant's binary in a worker's reused target directory (reproduced locally with two runs on one root; fixed by copying with fresh mtimes; check: `mutate-reuse-check.sh`); (2) after that fix every item failed the DuckDB layer with `Permission denied` on `./check.sh`, because the first fix dropped the executable bit (fixed by `shutil.copy`; the same script checks it). An ARM64 diagnostic run (`hashtests`, about $0.01) had already shown that the hash-ops tests pass on ARM64 with the locked dependencies (all test binaries, release and debug, and 20 of 20 repeats of the test that had failed).

Limit: the default run stops at the first catching layer, so no mutant has yet reached `nullcheck` or the DuckDB
diff. That those two layers can fail is therefore **not yet demonstrated**; the GCP list below has the command
(`--matrix`, which runs every layer for a mutant). Mutants of the NULL kind cannot be seen by the DuckDB diff at all
(the TPC-H data has no NULLs), so use fan-out and merge mutants for that layer.

## 6. Finding for ADR 0003 decision 6: overflow-checks and dependencies

Decision 6 sets `overflow-checks = true` in the release profile so overflow is an error and not a wrap. The
spike showed that this setting also reaches the **dependency crates**, and that Arrow, built with it, panics on legal
input.

**Symptom.** With `overflow-checks = true` on everything, the unmutated baseline failed 2 property tests
(`join_then_aggregate_matches_the_reference`, `join_output_matches_the_nested_loop_reference`), and, after the null-slot
tests existed, a third (`a_join_on_dictionary_keys_and_payloads_ignores_keys_under_null_slots`):

    arrow-data-60.1.0/src/transform/primitive.rs:46:67: attempt to add with overflow

**Diagnosis (the orchestrator's, checked).** Confirmed, with one refinement.

- Source: `build_extend_with_offset` does `values[start..start+len].iter().map(|x| *x + offset)` with a plain `+`.
  Arrow's `concat` of dictionary arrays merges the dictionaries and re-bases every key of the later arrays by the
  size of the earlier dictionaries, **including the key stored under a NULL slot, which Arrow leaves unspecified**.
  An `i32::MAX` there plus an offset of 1 overflows. Without overflow checks it wraps and nothing reads the value;
  with them it panics inside `concat`.
- Refinement: the keys are `Int32`, so the overflowing value is `i32::MAX`, not `u32::MAX` (`u32::MAX` stored in an
  `Int32` is `-1`, and `-1 + 1` does not overflow). The control test below shows `-1` and `i32::MAX - 1` are fine.
- Arrow-only repro, no engine code (`tests/overflow_config.rs`). A 2-row dictionary array with a valid row and a NULL slot
  whose key is `i32::MAX`, concatenated with another dictionary array that has a different dictionary:

      concat(&[dict_with_null_key(99), dict_with_null_key(i32::MAX)])      // panics: attempt to add with overflow

  Also panics through `MutableArrayData::try_extend` directly. Outcome by build configuration, all measured:

  | Build | arrow concat repro | MutableArrayData repro | control (`i32::MAX - 1`) | our `i64::MAX + 1` guard |
  |---|---|---|---|---|
  | checks on everywhere | **panics** at primitive.rs:46 | **panics** | not run in this configuration | panics (as required) |
  | final: checks on for us, off for dependencies | ok | ok | ok | panics (as required) |
  | checks off everywhere | ok | ok | ok | **wraps: test fails** |

**Decision for the spike, and why.** `[profile.release.package."*"] overflow-checks = false` (plus the same for
`profile.test` and `profile.dev`), keeping `overflow-checks = true` for our own package.

- Arrow computes on the slots under NULLs by design (ADR 0003 "Why": Arrow's kernels compute every slot), and treats
  the contents as unspecified, so wrapping arithmetic there is the contract, not a bug. The panic is a property of the
  *build setting* applied to code that was written and tested without it, and the same class can occur in any other
  Arrow kernel that does arithmetic on a slot under a NULL. One override covers the class; normalising keys covers one call site.
- It does not hide an overflow in our code. Shown by `our_own_arithmetic_still_panics_on_overflow`
  (`hash_ops::guard::plain_add(i64::MAX, 1)` must panic; the function is in our crate): it passes in the final configuration and
  **fails** with checks off (`i64::MAX + 1 wrapped in this crate`). The kernels are in the same crate, so they get the same flag.
  Our checked kernels use `overflowing_*` explicitly, so they do not rely on the flag.
- Alternative tried and removed: normalise the keys under NULL slots to 0 in the join build (a 12-line function applied
  to each gathered build column before `concat`). With it and checks on everywhere all 33 tests of that moment passed,
  so it works for this call site, but it costs a pass over the keys, fixes only this site, and made the engine tests
  pass in both configurations, which would hide the configuration problem. Kept as the fallback if the override
  cannot be used (it was removed from the spike; the code is below).

  ```rust
/// Arrow leaves the key stored under a NULL dictionary slot unspecified: it may be negative, out of range,
/// `i32::MAX`. `arrow::compute::concat` of dictionary arrays adds an offset to EVERY key, NULL slots included,
/// so `i32::MAX` there overflows (a panic with overflow checks on, a silent wrap with them off). Set those keys
/// to 0 before the build chunks are concatenated. Arrays without NULL keys are returned as they are.
fn zero_null_dictionary_keys(a: ArrayRef) -> R<ArrayRef> {
    let Some(d) = a.as_any().downcast_ref::<DictionaryArray<Int32Type>>() else { return Ok(a) };
    let keys = d.keys();
    if keys.null_count() == 0 {
        return Ok(a);
    }
    let fixed: Vec<i32> = keys.values().iter().enumerate().map(|(i, &k)| if keys.is_null(i) { 0 } else { k }).collect();
    let keys = Int32Array::new(fixed.into(), keys.nulls().cloned());
    Ok(Arc::new(DictionaryArray::<Int32Type>::try_new(keys, d.values().clone()).map_err(arrow_err)?))
}
  ```
  (called on every column gathered in `JoinBuild::consume`, before the chunk is stored; it needs `DictionaryArray`,
  `Int32Array` and `Int32Type` imports.)
- Why the explicit test did not see it at first: the junk key `i32::MAX` was never under a NULL slot of a build row
  that survives the NULL-key filter. After the fixture was changed to put it there, the test failed on the unprotected
  engine with checks on, as it should, and passes with the override.

**For ADR 0003 (a human decides; this is a finding, not an edit).**
1. The workspace-root `overflow-checks = true` has the same effect on the real engine: Arrow's `concat` (and any
   other dependency code with plain arithmetic on unspecified slots) panics on legal Arrow input. Decision 6 should
   say that dependencies are excluded (`[profile.release.package."*"] overflow-checks = false`, or an equivalent), and
   the kernels crate needs a guard test like `overflow_config.rs`: our code panics, Arrow's repro does not.
2. Not covered here (reasoned, not measured): generic Arrow code that is *instantiated in our crates* is compiled with
   our setting, so the override does not turn those checks off, and a plain `+` on an unspecified slot inside such a
   generic would still panic. This spike's suite did not hit one; a property test with junk under NULL slots against each
   Arrow wrapper that the kernels crate adopts is the way to find them.
3. `ADR 0003` evidence on "a per-package setting does not protect inlined or generic kernel code called from
   another crate" is consistent with this and is not contradicted: the override here is on dependencies, not on ours.

Results of the original "overflow-checks on" step: with checks on everywhere, the TPC-H results did not change
(`check.sh` 7 of 7 identical, `nullcheck` 21 of 21), and nothing in the engine panicked on those runs. The
only panics were Arrow's, found by tests with junk under NULL slots (above).

## 7. Peak memory guards

`hash-ops rss data/tpch-sf1 <query> <arm> 4` (4 threads, Parquet decoded inside the run, one run each). The in-process
`VmHWM` figure and `/usr/bin/time -v` "Maximum resident set size" agree. The `ms` in the log lines are not speed
results (loaded machine). Final configuration.

| Query | Arm | Groups | Peak RSS (`VmHWM`) | `/usr/bin/time -v` max RSS (KiB) |
|---|---|---|---:|---:|
| agg_orderkey (1.49M groups) | engine | 1,493,405 | 287 MiB | 294,048 |
| agg_orderkey | hand-fused | 1,493,405 | 313 MiB | 320,988 |
| j3 (fan-out join) | engine | 35 | 337 MiB | 345,300 |
| j3 | hand-fused | 35 | 483 MiB | 495,100 |

## 8. Scripts

- `check.sh [data-dir] [keep-dir]`: engine at 1, 4, 8 threads against DuckDB for all 7 plans (`sql/<name>.sql`),
  per query, comparing sorted text. **Disk use fix (orchestrator report):** an earlier version left a ~963 MB `mktemp`
  directory in `/tmp` (a 7.9 GB RAM-backed tmpfs) per run. Now the work directory is created with `mktemp -d` under
  `$HASH_OPS_TMP` (default `$TMPDIR`, else `/tmp`), removed on every exit (trap on EXIT, INT, TERM), and each query's files
  are deleted as soon as it has been compared, so the peak is one query (about 150 MB). A `keep-dir` argument is kept
  for inspection. Checked: the `/tmp` listing before and after a full run is identical; after a SIGTERM mid-run no
  `hash-ops-check.*` directory is left (two unrelated new `/tmp` entries during that test belonged to other jobs).
  `mutate.py` calls `check.sh` and passes no `keep-dir`, so it leaves nothing either; `run-all.sh` writes only its logs
  under `target/run-all/<timestamp>/` (on disk).
- `mutate.py [--list] [--matrix] [--layer L] [--dry] [name ...]`: `--list` prints the 30 exact names; a bare argument
  matches an exact name first and a substring second (an unknown name exits 2); `HASH_OPS_MUT_ROOT` moves the private work and
  target directories so copies and shards do not collide (checked: no other fixed path in the run).
- `run-all.sh [rounds]`: in order, stopping at the first failure: `cargo test --release`, `check.sh`, `nullcheck`,
  `mutate.py`, `rss`, `bench`, `membench` (25 rounds), `duckdb-time.sh` (same queries, 1/2/4/8 threads, in-process
  timer, the result consumed by a hash-sum so nothing is pruned; first run in time order dropped, not the fastest;
  Parquet and in-memory modes). Warns when the load average is above 0.5 per core. Logs and machine description in
  `target/run-all/<timestamp>/`. Not run locally (it contains the benchmarks).

## 9. For the GCP run

Machine: a quiet x86-64 VM; the mutation shards need about 2 GB of disk per copy (private target directory seeded from
the first build) and little RAM (peak RSS in section 7); the benchmarks need a quiet machine and the same VM for all
arms. Durations below are what this loaded machine took, as a guide; **GCP durations to be filled**.

| Command | What it shows | Local duration (loaded; `~` = my estimate, not measured) | GCP |
|---|---|---|---|
| `spikes/hash-ops/run-all.sh` | everything in order, including bench, membench, DuckDB timing | not run (contains the benchmarks) | to be filled |
| `cd spikes/hash-ops && python3 mutate.py --list` then, per name, `HASH_OPS_MUT_ROOT=$D/mut-$i HASH_OPS_TMP=$D/tmp-$i python3 mutate.py "<exact name>"` in side-by-side copies | full 30-mutant table in the final configuration (expected 30 of 30) | baseline 540 to 610 s, then 135 to 720 s per mutant; the full run is over an hour | to be filled (47 min sequential on a quiet VM, per the orchestrator) |
| `python3 mutate.py --matrix --layer duckdb "fan-out: only the first match of a chain is emitted" "merge loses the last thread's partial table"` | the DuckDB diff itself fails on a known-bad engine (not demonstrated yet) | ~12 min | to be filled |
| `python3 mutate.py --matrix --layer nullcheck "SUM adds the values of NULL rows" "NULL group dropped from the output"` | `nullcheck` fails on a known-bad engine (not demonstrated yet) | ~12 min | to be filled |
| `python3 mutate.py "string flag looks up" "hash ignores the second"` | the two survivors are caught in the final configuration | about 18 min (baseline 611 s plus 240 and 261 s) | to be filled |
| `for s in $(seq 1 100); do HASH_OPS_SEED=$s HASH_OPS_SCALE=10 cargo test --release; done` | a larger seed sweep (locally: 75 seeds at scale 3, no failure) | about 4 s per seed at scale 3 | to be filled |

## 10. Limits and not proven

- The property tests compare with a scalar reference written in the same repository by the same agent; it has its own
  tests (`the_indexed_reference_join_equals_the_nested_loop`) and it is independent code, but two readings of NULL
  semantics could agree and both be wrong. The DuckDB diff is the check against a third party, and the TPC-H data has
  no NULLs. NULL behaviour against DuckDB itself (for example `GROUP BY` on a NULL key, `NOT IN` with a NULL) is **not**
  differentially tested here; it should be added to `nata-check`.
- Mutation coverage is 30 hand-picked breakages, not exhaustive. Seed sweeps are samples.
- Key types are Int64, Decimal64, Int32, Date32, Utf8 and `Dictionary<Int32, Utf8>`. A dictionary whose *values* contain a
  NULL (a valid key pointing at a NULL value) is legal Arrow and is not handled: `KCol::from_array` looks only at the key
  nulls. Not tested; the Parquet reader does not produce it. It should become a `not supported yet` error before this
  leaves a spike.
- `src/fused.rs` (the hand-fused baselines) assumes no NULLs and indexes dictionary keys without a NULL test; it is only
  run on the NULL-free TPC-H data.
- Everything ran on x86-64. ARM64 is for the GCP run.
- No speed conclusion is drawn anywhere in this file.


## The DuckDB-diff layer and the nullcheck layer shown to fail on a known-bad engine (2026-11-04)

GCP run `nata-1104-172548-4307` (2026-11-04, `n4a-highcpu-16` Spot, Neoverse-N3, 16 vCPU, Debian 12 arm64; task of the `armcheck-retry` job, run concurrently with four other tasks, so no timing here is evidence).

Closes the limit stated in section 3 (no mutant had reached `nullcheck` or the DuckDB diff). `mutate.py --matrix` with one layer at a time:

- Layer `duckdb` (the 7-query diff against DuckDB at 1, 4 and 8 threads): **2 of 2 caught** - `fan-out: only the first match of a chain is emitted` (caught in 218 s) and `merge loses the last thread's partial table` (145 s). Baseline passed all layers in 404 s.
- Layer `nullcheck` (10/50/100% NULLs against a scalar reference): **2 of 2 caught** - `NULL group dropped from the output` (172 s) and `SUM adds the values of NULL rows` (132 s). Baseline passed all layers in 314 s.

Both runs byte-compared the real sources before and after: unchanged. So both layers can fail, and the earlier 30 of 30 result (all caught by the test layer) is not the only line of defence.

Raw output:

```
data: /mnt/work/src/data/tpch-sf1; 2 mutants; layers: ['duckdb']; matrix=True
baseline passes all layers (404s)
CAUGHT   fan-out: only the first match of a chain is emitted  [duckdb]  (218s)
CAUGHT   merge loses the last thread's partial table  [duckdb]  (145s)

| # | breakage | result | caught by |
|---:|---|---|---|
| 1 | fan-out: only the first match of a chain is emitted | CAUGHT | duckdb |
| 2 | merge loses the last thread's partial table | CAUGHT | duckdb |

caught 2 of 2; survived 0; invalid 0
real sources unchanged (byte-compared before and after)
data: /mnt/work/src/data/tpch-sf1; 2 mutants; layers: ['nullcheck']; matrix=True
baseline passes all layers (314s)
CAUGHT   NULL group dropped from the output  [nullcheck]  (172s)
CAUGHT   SUM adds the values of NULL rows  [nullcheck]  (132s)

| # | breakage | result | caught by |
|---:|---|---|---|
| 1 | NULL group dropped from the output | CAUGHT | nullcheck |
| 2 | SUM adds the values of NULL rows | CAUGHT | nullcheck |

caught 2 of 2; survived 0; invalid 0
real sources unchanged (byte-compared before and after)
```
