# 0001: Parquet scan with projection pushdown and row-group pruning (local files)

Status: accepted (by the owner, 2026-11-05, as proposed: the minimal `nata scan` command, the three decision rules with their numbers, and the baseline spike within the stated cost)
Date: 2026-11-05
ADRs: 0001 (relies on; this slice decides its open "morsel size"), 0002 (relies on: CPU pool, `crossbeam-deque`), 0003 (relies on: Arrow batches, dictionary columns kept, strict errors)

Rule: `docs/way-of-working.md`, section 2 ("The slice verification card"). Slice 3 of the order in `docs/feature-map.md`. Drafted by the assistant, accepted by the owner on 2026-11-05 without changes.

## 1. Goal

A Parquet source on local files for the push pipeline of ADR 0001: it reads only the projected columns, skips every row group whose statistics PROVE that no row can match a pushed-down simple predicate (`column <op> literal`, `AND` of those), keeps
dictionary columns as Arrow dictionary arrays, hands out one row group (or a part of one, see the decision rule) per morsel, and returns the same rows as DuckDB. Example: `l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01'` over TPC-H SF1 `lineitem` written sorted by `l_shipdate`
reads about a fifth of the row groups and prints exactly DuckDB's rows.

Not in this slice: SQL (slice 4), object stores (slice 11), Iceberg (slice 12), nested types and `MAP`/`LIST` columns, Parquet encryption, writing Parquet.

**Decided by the owner, 2026-11-05: the minimal developer command.** There is no SQL yet, but the definition of done needs something runnable and the differential tool `nata-check` drives the engine through `nata sql`. This card therefore adds a deliberately minimal developer command,
`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv`, which `nata-check` can call as a second candidate form (an `-- scan:` directive). The alternative is to merge this slice into slice 4 and verify the scan through SQL only. The first costs a throwaway command; the second delays
verification of pruning until the parser and binder exist.

Decisions it needs: (a) build or reuse the Parquet decoder, (b) the morsel size. Both are settled by the baseline spike in part 4.

## 2. Quality targets

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | Same rows as DuckDB for every projection and filter in the test set (all `lineitem` columns, dates, decimals, strings, NULLs, NaN, -0.0, empty files, one-row files, files with many row groups). Pruning is SOUND: it never skips a row group that holds a matching row. | Differential against DuckDB (`nata-check`, existing harness) on files written by DuckDB, pyarrow and parquet-rs; a property test of pruning against a brute-force scan on random files and predicates; **mutants** of the pruning rule (each must be caught): `<` taken as `<=`, NaN min/max trusted, -0.0 narrowed, truncated string bounds treated as exact, a missing statistic treated as zero, an off-by-one in the row-group range, the null count ignored (a row group of only NULLs kept or dropped wrongly for `<>`). | `spikes/iceberg-plan` planner soundness (22 of 22 mutants) and `spikes/parquet-write` statistics rules (ADR 0005 rules 3 and 5) |
| Performance | Single-thread decode of the projected columns of `lineitem` at least 0.8x of DuckDB's `read_parquet` on the same columns (decision rule R1); pruning on the sorted-`l_shipdate` file reads at most 40% of the bytes of the unpruned scan (R2). | Paired benchmark with a control arm (twin) on settled VMs of both CPU types (ADR 0006 rule 7); `nata-bench` | DuckDB; `spikes/pipeline-model` (parquet-rs, plain threads); ADR 0003: dictionary string columns decode in 13-14 ms against 80-85 ms expanded |
| Scalability | 1 to 8 threads on SF1 and SF10: scan throughput at 8 threads at least 5x the single-thread figure (parallel efficiency 0.625). 32 and more cores, SF100 and larger than memory: deferred to item 32, said here, not forgotten. | Same benchmark, thread sweep; SF10 generated with the existing `scripts/gen-tpch.sh` | the thread sweep of `spikes/generic-ops` (1.06-1.12x of fused on Q1 end to end) |
| Reliability | No hang, leak or panic on cancel at any point (a `LIMIT`, a closed consumer, an error in another morsel); every file handle and buffer is released. | Cancel injected at random points under the deterministic simulation harness and the loom models of ADR 0001; a leak counter in the tests (the `Tracked` pattern of the sim harness) | `spikes/exchange-loom`, `spikes/sim-harness` |
| Fault tolerance | A corrupt, truncated or unreadable file is an error that names the file and the row group, never a wrong or partial result, never a panic. | Byte-flip and truncation fuzz of valid files (3,000 cases per file shape, no panic, no hang, timeout per case), injected I/O errors | `spikes/manifest-decode` (the same fuzz found a UTF-8 hole); note that plain Parquet pages have no checksum by default: a flipped value byte can decode to a different valid value, which the card states and does not hide |
| Resource bounds | Peak memory at most (threads x largest row group of the projected columns) plus a fixed slice; open files at most the thread count (not the file count). | A scan of a directory of 10,000 small files under `ulimit -n 256` and a memory cap; peak RSS measured | `object-store-scan` memory bound (the unbounded control exceeds it) |
| Portability | ARM64 and x86-64, clean VMs, every number on both | `orchestrate.sh` job on `n4a-highcpu-8` and `c3d-highcpu-8` | `bench/results/20261105-timing-*.md` |
| Compatibility | Reads Parquet v1 and v2 data pages, dictionary, plain, delta and byte-stream-split encodings, codecs snappy, zstd, gzip, lz4 and none, the types of `docs/supported-sql.md` and `decimal-rules.md`. Anything else answers `not supported yet: <what>`, never a guess. | A matrix of writers (DuckDB, pyarrow, parquet-rs; a Java-written file is NOT available, said here) x encodings x codecs read by Nata and by DuckDB, identical | `spikes/parquet-write` and `spikes/iceberg-plan` interop (80 of 80 tables we write read identically by DuckDB) |
| Security and privacy | Untrusted files cannot cause memory unsafety, an unbounded allocation or a hang (lengths in the footer and pages are bounded before allocating); error messages carry file names and positions, never data values; `unsafe_code` stays forbidden in the engine crates. | The fuzz above; a test that error messages contain no cell value; `cargo geiger` count of the `parquet` dependency recorded | `spikes/manifest-decode` (length checks before allocating) |
| Operability | A scan reports files, row groups total and skipped, bytes read and the morsel size (printed by `nata scan`, later by `EXPLAIN`); fuzz and property failures print a seed that replays. | Tests of the report against known files | `docs/way-of-working.md` section 3 |

## 3. Decision rules (fixed before measuring; the owner may change them before the spike runs)

- **R1, decoder.** Reuse `parquet-rs` (the Arrow reader) unless the baseline shows that (i) it cannot hand back dictionary arrays for the string columns of `lineitem` without expanding them, or (ii) its single-thread decode of the projected columns is under 0.8x of DuckDB's on
  either architecture on a settled VM. If so, write our own page decoding for the failing encodings or columns only, behind the same source interface, and keep `parquet-rs` as a test oracle (as ADR 0005 did for manifests).
- **R2, pruning.** A row group is skipped only when its statistics prove that no row can match; a missing, NaN, truncated or unsound statistic means "keep". On the SF1 `lineitem` written sorted by `l_shipdate` with 122,880-row groups, the predicate above must skip at least 60% of the row groups (the year is about 14% of the dates, so this is a floor, not a target).
- **R3, morsel size (ADR 0001's open item).** Compare morsel = a whole row group with a quarter of a row group on the `big1` layout (4 row groups of about 1.5 million rows) at 1 to 8 threads. Choose the quarter only if it raises 8-thread throughput by at least 15% without raising peak memory by more than 25%; otherwise the whole row group.

## 4. Baseline spike (required: a build-versus-reuse choice and a design number)

- Question: does `parquet-rs` meet R1 on both architectures, and which morsel size does R3 choose?
- Smallest experiment, with its control arm: `spikes/pipeline-model` and `spikes/generic-ops` already decode `lineitem` through `parquet-rs`; add (1) an arm that reads the same projected columns with dictionaries preserved, (2) DuckDB `read_parquet` of the same columns as the reference,
  (3) the twin of arm (1) as the noise control, (4) the three morsel sizes on `big1`. One thread and 1 to 8 threads, 25 rounds, interleaved, settle before each block.
- Time box and cloud spend: one day of work; two settled VMs for about 30 minutes each, roughly $0.05 in total (worst case about $0.22 at one VM-hour each). **Approved by the owner on 2026-11-05.**
- Result file: `bench/results/<date>-parquet-scan-baseline.md`.

## 5. Closeout (fill in when done)

- Results file:
- ADR updated (amendment or revision note), confidence and what changed: ADR 0001 (morsel size decided), ADR 0003 (dictionary decode: reused or owned)
- Benchmark added to the regression set, baseline numbers:
- Deviations from this card, and why:
- Spike code deleted:
