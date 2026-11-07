# 0004: Deep-check closeout of slices 4 and 4b

Status: accepted (by the assistant under the owner's delegation, 2026-11-07)
Date: 2026-11-07
ADRs: 0003 (overflow is an error), 0006 (testing), 0007 (dialect), 0008 (client protocol); amends none unless an item below says so

Rule: `docs/way-of-working.md`, section 2, "Deep checks on the closing commit". Cards 0002 and 0003 were closed before the nightly workflow
had run on their closing commits. The first nightly run (2026-11-07, `ebdc68e`'s parent `7b799b4`, run 42324643615) and the debt-A step
(`bench/results/20261107-debt-a.md`) left the open findings below. This is a short fix card, not a feature slice. Slice 5 starts after it.

## 1. Goal

Every finding of the first nightly run is fixed, or recorded as a deviation with a register item. R15 is closed. Then the nightly passes
on the closing commit of this card, and cards 0002 and 0003 each get a part-6 line that says their deep checks passed (or what remains).

## 2. Findings, rule and check (each rule fixed before the work)

| # | Finding (evidence) | Decision rule | Check that can fail |
|---|---|---|---|
| A | **Hang on a damaged Parquet file.** parquet-rs 60.1.0 `ThriftReadInputProtocol::skip_bytes`/`read_bytes_owned` ignore a short `io::copy` and loop over a list count from the file; about 10,000 empty reads per second. Nightly fuzz seed 20261107, `tiny_and_empty_groups` case 10072 | A read after end of file is an error (`CountingRead::at_eof`). The scan of the pinned file ends in under 1 s with `Corrupt` naming the file and row group. Also find whether `read_bytes_owned`'s `Vec::with_capacity(len)` can be reached with a huge length (an abort no catch stops). If it can, bound it in our wrapper or record it as an upstream-only risk | Pinned fixture test: passes with the fix, hangs past its 20 s deadline without it (shown 2026-11-07). The damaged-file fuzz at nightly size (30,000 per shape) passes |
| B | **Checksummed file read as other rows with `page-crc` on** (local nightly-size run, after A's fix) | Find the case. If the damage is outside the bytes a page CRC covers (a page header), the test's claim is too wide: narrow it to the CRC-covered payload and state R4's limit in `docs/improvements.md`. If CRC-covered bytes changed and the check passed, it is a bug: fix it | Replayed case, before and after. A mutant that skips verification is still caught |
| C | **`- -9223372036854775808` returns 9223372036854775808 (`DECIMAL(19,0)`)** (nightly SQL fuzz batch 1816 query 15; checked by hand). Dialect rule 10 types `-9223372036854775808` as `BIGINT`, so negating it again overflows. PostgreSQL and DuckDB both raise an error | Nata raises the overflow error, as its dialect and ADR 0003 require. A double negation is never folded into a different literal type | A test per spelling (`- -x`, `-(-x)`, `-(-(-x))`, the INTEGER minimum); the replayed fuzz query; a mutant that restores the fold |
| D | **`BETWEEN … AND CAST(b AS INTEGER)` raises a cast error where DuckDB answers** (nightly SQL fuzz batch 2202 query 31) | Dialect §9b: the upper bound is evaluated only for rows that the lower bound did not decide as FALSE. Find the failing row. If its lower bound was FALSE, Nata broke its own rule: fix it. If the lower bound was NULL, Nata followed its rule: add a listed difference class with a test | The row's lower-bound value is recorded; a test that fails on the wrong behaviour |
| E | **Loom m21 ran out of time** (rc=124 after 3,000 s at bound 3, no failure; m09 passed in 25 min) | m21 alone gets 19,800 s in a 350-minute job. If it still does not finish, split the model or run m21 at bound 2 nightly, and record it as a deviation | The next nightly's m21 row: rc=0, or recorded as a deviation |
| F | **R15: TSan reports 7 races, all inside Tokio's I/O driver.** A 10-line Tokio-only program reproduces 5 (`bench/results/20261107-debt-a.md`) | A TSan suppression file that names only Tokio's epoll registration and readiness frames, with a written reason: the kernel orders the `epoll_ctl`/`epoll_wait` handoff, and TSan does not model it. Accepted as a trust point, like R10. No Nata frame may be suppressed | A race planted in Nata code is still reported with the suppressions on (the known-bad version). The full TSan workspace run exits 0 on a clean x86-64 VM |
| G | **Two GCP script tests for Iceberg fail** on stale assertions (step 4b child's report) | Fix the assertion that is wrong against the current behaviour, after checking which side is right | `scripts/gcp/test/*` all pass |
| H | **Upstream reports** (each one needs the owner's yes before it is posted) | parquet-rs: the thrift skip and length-check bug behind A. ADBC: `;` followed by whitespace is not stripped before COPY wrapping (R22; the earlier fix #1147 strips only a final `;`) | Each report has a minimal reproduction run on the latest release and on `main` |

Final check: the nightly workflow runs by hand on this card's closing commit and passes, or each remaining failure is a recorded deviation
with a register item.

## 3. How the work is run

- **Children:** Codex `gpt-6.1-sol`, each in its own worktree, each brief with a "not in scope" list and the stop-and-report rule, verdicts in the
  fixed form (rule, numbers, verdict, command). Up to two children at once, on disjoint paths:
  1. Parquet: A and B (`crates/nata-parquet`, the fuzz), high effort.
  2. SQL: C and D (`crates/nata-sql`, `crates/nata-exec`, `docs/dialect.md`, `spikes/sqlfuzz` classes), xhigh effort.
  3. After 1 and 2: TSan, F (`scripts/gcp`, a suppression file, `docs/improvements.md`), xhigh effort. E and G are small: the orchestrator does them.
- **The orchestrator** re-runs each child's key check, reads the diff, then merges, pushes and triggers the nightly.
- **GCP:** only F needs a clean VM (`eng-checks`, x86-64). First a local trial (the TSan run on one crate, plus `test-job-chains.sh`), then the plan's
  worst case shown. Cap 0.25 USD, one rerun. Everything else runs locally or in GitHub Actions at no cost.

## 4. Not in scope

Slice 5 work (R12 kernels, P11 vectorization). The streaming target against PostgreSQL (P16, for the next protocol card). Any threshold
change. New dependencies. COPY forms other than ADBC's.

## 5. Carried-in improvements

N/A: this card exists to close slices 4 and 4b. Slice 5's card takes in the oldest open on-path items: 4b's closeout added more than three
register items, so the "growth triggers payback" rule applies.

## 6. Closeout (fill in when done)

- Results file:
- Nightly run on the closing commit:
- Cards 0002 and 0003 part-6 lines added:
- Register updated:
- Upstream reports posted (with the owner's yes):
