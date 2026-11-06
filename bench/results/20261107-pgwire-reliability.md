# Slice 4b, step 4: named preparation and local reliability

2026-11-07. **Local x86-64 only; no GCP run, push, throughput claim or slice closeout.**
Host: Intel Core i3-1220P, eight available logical CPUs (affinity 0–7), about 16 GiB
RAM; Linux /proc. rustc/cargo 1.99.1, cargo-deny 0.20.2. Concurrent compilation and
existing workspace tests also loaded this shared development machine. Times below
are functional-run durations, not benchmark comparisons. ADR 0008's step-4
amendment supersedes step 3's unnamed/text-only default-client workaround.

## Built and decisions

- Per-session named/unnamed Parse, Bind, Describe, Execute and Close. Statements
  survive Sync/simple Query until Close/DEALLOCATE/disconnect. Auto-commit Sync
  destroys portals; explicit compatibility COMMIT/ROLLBACK also destroys them.
  Named duplicate statements/portals return 42P05/42P03, lookup misses 26000/34000.
  PostgreSQL Close silently accepts absent objects (source linked in ADR 0008).
- Statement cap 256 including unnamed; stored SQL plus name cap 4 MiB. Portal
  cap 256 plus conservative 16 MiB charge (SQL/description/name + 4 MiB parameter
  allowance per portal), permitting at most three small portals. Exhaustion is
  54000, class 54. Unknown names, silent unnamed replacement, duplicate retention,
  Close/reuse, Sync cleanup, count/byte caps and release on disconnect are checked.
- Default-driver binary scalar input for bool/integer/float/text/date; binary
  results for all mapped slice-4 types, including exact numeric and date epochs.
  NULL is still length -1. Malformed binary scalars return 22P03. Numeric binary
  input stays 0A000 (default Decimal dumps are text); float-special parameter
  literals stay outside the dialect. Parameter evaluation uses literal ASTs.
- Nonzero Execute max-rows on row-producing queries remains 0A000. Parking an
  admission/worker share/scan window between arbitrary fetches needs an explicit
  idle-portal expiry/fairness policy. Rowless commands accept max-rows, enabling
  pgjdbc's max-rows=1 ROLLBACK. No fake PortalSuspended, buffering or ignored limit.
- Metrics now expose the live registry, statement/portal objects, admissions,
  workers and up to 64 completed scans' handle/footer/buffer gauges. Production
  CLI leaves optional metrics disabled; gauges retain no SQL or values. An
  injectable execution file opener widens cancel/kill races without changing
  default file behavior. Session cancellation still uses no signal handler.

## Real default clients

Reused the portable JDK, pgjdbc jar and psycopg venv already under the home
scratch directory. No protocol/preparation/format settings changed. The Java
URL sets only a five-second connect timeout. Fixtures come from tests/support:
three all-type rows including all NULL, signed extrema, negative zero, NaN,
infinities, exact scaled decimals, empty/Unicode text and leap-day dates;
large fixture has 1,048,576 rows and a 1,024-byte string column.

| Client | Measured result |
|---|---|
| pgjdbc **42.7.7**, Java **21.0.13**, default prepareThreshold=5 | PASS: same PreparedStatement ten times; all ten mapped types with Java class and value checks on every repeat, NULLs, typed integer/boolean parameters and NULL, 42703 position 8, cancel 57014 |
| psycopg **3.3.8**, libpq **18.0.6** (180006), default prepare_threshold=5 | PASS: same cursor.execute ten times; Python types and values for every output type/NULL; default %s integer widths, bool, text, date, double, exact text Decimal and NULL parameters, ten times per type; 42703 position 8, rollback and cancel 57014 |
| pgjdbc fetchSize=100, autocommit=false | Explicit 0A000 as decided; subsequent rollback and SELECT 1 PASS |
| psycopg SQL server-side cursor | Explicit 0A000 for DECLARE; named wire portals with unlimited Execute are tested separately |

Inspect [Java output](20261107-pgwire-reliability/pgjdbc.txt) and
[Python output](20261107-pgwire-reliability/psycopg.txt). Permanent probes:
`crates/nata-pgwire/tests/Clients4.java` / `clients4.py` (arguments: port,
all-type fixture path, large fixture path). Historical Clients3/clients.py remain
controls expecting the old limitations; do not use them as current pass probes.

One real-client finding changed implementation: psycopg sends DEALLOCATE ALL
after rollback to invalidate its prepared cache. Another: pgjdbc sends Execute
max-rows=1 for ROLLBACK. Requiring unlimited Execute even for a rowless command
broke recovery; the permanent rowless-command regression and its mutant cover it.

## Reliability, live fuzz and resource bounds

All tests use real loopback sockets and Parquet execution. Seed **322661912839**
(0x4b20261107), xorshift64 (13,7,17). Case and seed knobs print in the log.
Normal workspace defaults: 28 lifecycle scenarios and 100 live fuzz cases; long
sizes are release tests. Panic hooks count background panics, including ones
Tokio would otherwise contain while the listener remained alive. Sockets have
ten-second read/write deadlines; cleanup waits have ten-second deadlines, the
slow-reader test a six-second deadline. A timeout is failure, never a pass.

**Lifecycle:** 2,000 seeded scenarios with eight busy-loop competitors (one per
available logical CPU), 1 ms delays per execution file open and 100 us per file
read. Seven scenario classes: truncated startup, mid-Parse disconnect,
mid-result disconnect, mid-extended sequence, CancelRequest, actual Python child
SIGKILL at randomized startup/Parse/extended/result stages, admission waiting
(cancel/disconnect/SIGKILL while both query slots are occupied). Random delays
0–1,999 us widen boundaries. Class counts: 302, 272, 293, 305, 273, 260, 295.
Four concurrent healthy clients completed **6,692** SELECT 1 queries, with
periodic real Parquet reads checking -32768. They remain connected throughout;
their eight client/server fds explain the baseline/final fd difference.

After every scenario, sessions and registry return to four healthy sessions,
object maps/admissions/workers return to zero, and every retained scan's file,
footer and buffer gauges are zero. After stopping healthy clients/competitors:
**fds 16 → 8, threads 7 → 7, RSS 22,736 → 24,464 KiB**, peak 24,464 KiB;
retained RSS delta **1,728 KiB**. Declared limits before assertion: retained RSS
baseline +64 MiB, sampled peak baseline +256 MiB. This sample does not prove a
bound for every SQL/file/cell-width shape. Lifecycle duration 77.924 s.

**Live fuzz:** **10,000 cases PASS**, with zero observed panics, hangs or leaked
resources. Class counts: 831, 747, 907, 773, 855, 840, 851, 821, 823, 887, 828,
837. Duration **69.841 s**, RSS baseline/final **24,552/24,560 KiB**, below the
predeclared +32 MiB retention limit; exact fd/thread baselines preserved. Each case ends with write-side EOF and must
observe ErrorResponse and/or a closed/reset connection. Cases cover malformed
startup, Execute without Bind, Bind unknown statement, Close absent then Describe,
Sync storms, huge parameter counts/lengths, short P/B/D/E/C bodies, oversized
headers, random Parse truncations, random unknown tags/bodies and duplicate Parse.
Semantic errors explicitly require ErrorResponse. Healthy SELECT 1 probes run
between cases; registry/maps/admissions/workers/scans return to baseline after
each. Reset or NotConnected on a socket already rejected by the server is a valid
close, not a harness failure. Initial harness trials exposed these ordinary TCP
races; the final oracle handles them without accepting deadlines or other errors.

**Slow reader:** 1 byte/ms on the 1,048,576-row × 1,024-byte result, with a healthy
Parquet session completing concurrently. Read **1,700 bytes**, produced **2,678
rows**, RSS baseline **17,840 KiB**, sampled peak **22,636 KiB** (delta 4,796 KiB,
below the declared +128 MiB bound). The two-second unwritable-frame deadline
cancels/releases the query; the client does not need to drain buffered kernel
bytes to free server resources. The row count assertion (<20,000) and peak RSS
reject collect-all behavior. This is backpressure plus stalled-client eviction,
not a promise to finish a gigabyte transfer at this read rate; arbitrary-width
cells and R16's SF10 projection/allocator issue remain unmeasured here.

**Connection bound:** 64 authenticated sessions plus eight partial startups;
the 73rd socket is closed, and a 65th authenticated attempt gets **53300**.
All 64 concurrent sessions then execute the small Parquet query with the correct
-32768 row; all counters return to zero. Existing startup-deadline and reserved
cancel tests remain gating. Throughput is deliberately not measured here.

## Mutation and workspace gates

**45 CAUGHT, 0 SURVIVED, 0 INVALID** in the completed final sweep (exit 0),
including all 30 previous mutants and 15 additions. Inspect
[mutant results](20261107-pgwire-reliability/mutants.txt). The 30 existing mutants remain in mutate.py, with the binary
format refusal mutant adapted to an invalid format code (2) now that code 1 is
supported. New mutants cover named storage/overwrite, Sync portal destruction,
statement count/SQL-byte and portal byte budgets, DEALLOCATE, binary input
endianness/date epoch, rowless-command recovery, cancellation, unbounded slow
reader buffering, leaked admission/registry and live-decoder panics. INVALID
never counts as CAUGHT. The private-copy runner checks an unmutated baseline
first and fingerprints real Rust sources; compiler failures/timeouts are INVALID.
An interrupted trial of disabled cancellation exposed a harness unwind bug:
healthy client threads lacked a stop guard when assertions panicked. Added a
drop guard, reran the entire sweep, and counted only completed final results.
The unbounded-bridge negative control produced **911,204 rows** before the
slow-reader assertion failed, versus 2,678 for the bounded implementation.

**403 workspace tests PASS, zero failed/ignored, across 61 test binaries/doc-test
summaries.** fmt, workspace clippy with -D warnings, workspace tests (328.45 s)
and cargo-deny PASS; deny has only pre-existing unmatched-license warnings.
The added Rust surface has 17 protocol tests and four live-resource tests.
The corrected healthy-thread shutdown path also passes the mutation runner's
unmutated baseline and fresh clippy. Staged leak scan PASS before the implementation commit.
A second full gate run PASS on the final Rust tree: **403 tests**, 288.49 s; fmt,
clippy and deny also PASS. [Both gate summaries](20261107-pgwire-reliability/checks.txt)
record exits/times; staged leak scans pass for both commits. Optional psql runs
only when present on PATH; this step certifies the two clients above. Required commands, with disk TMPDIR and AI_AGENT absent:

```sh
mkdir -p "$HOME/tmp-4b4"
export TMPDIR="$HOME/tmp-4b4"
unset AI_AGENT
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
scripts/check-leaks.sh --staged
python3 crates/nata-pgwire/mutate.py --work "$HOME/tmp-4b4/mutations"
NATA_PGWIRE_SEED=322661912839 NATA_PGWIRE_SCENARIOS=2000 \
NATA_PGWIRE_LIVE_CASES=10000 NATA_PGWIRE_CONTENTION=$(nproc) \
cargo test --release -p nata-pgwire --test reliability -- --nocapture
```

The GCP job-chain check passes locally; deleting its live-check step in a private
copy makes it fail with `FAIL pgwirecheck steps` (exit 1). No VM or cloud request
was made by these stubbed script checks.

## Step 5 and exact GCP replay

Step 5 must build a paired, correctness-first benchmark on both x86-64 and ARM64:
one million ordered rows via CLI CSV versus raw wire/psql, alternating repeats,
min/median/noise, same cache/threads/window; ≤20% streaming overhead and first row
before scan end. Measure 64 concurrent small queries versus 64 sequential on eight
vCPUs (within 2×), fixed worker-share utilization (one query versus many), queue
wait, cancellations under load and default named preparation/binary conversion
cost. Record hardware and separate warm/cold data. The functional timings above
are not baselines for those thresholds. R15 full sanitizer and R16 SF10 RSS
closure, ARM64 checks, pgcli/ADBC and item 35's full matrix remain later work.

The prepared `pgwirecheck` job replays **this step's checks**, not step-5
performance. It runs release pgwire tests, the exact long sizes/seed/contention,
and mutations; generated files/TMPDIR/build caches stay on the work disk. It does
not install or certify external clients. After refreshing prices and separate
human cost approval, the parent can run (project configuration stays outside git):

```sh
# Plan only; perform neither plan nor run as part of this local step.
scripts/gcp/orchestrate.sh plan --job pgwirecheck --machines n4a-highcpu-8 \
  --max-vm-hours 1 --data-gb 50
scripts/gcp/orchestrate.sh plan --job pgwirecheck --machines c3d-highcpu-8 \
  --max-vm-hours 1 --data-gb 50
# Set APPROVED_USD to the separately approved bound for each architecture.
scripts/gcp/orchestrate.sh run --job pgwirecheck --machines n4a-highcpu-8 \
  --max-vm-hours 1 --data-gb 50 --approved-usd "$APPROVED_USD"
scripts/gcp/orchestrate.sh run --job pgwirecheck --machines c3d-highcpu-8 \
  --max-vm-hours 1 --data-gb 50 --approved-usd "$APPROVED_USD"
```

Larger nightly sizes: 4,000 lifecycle and 20,000 live cases on Linux x86-64 and
ARM64, plus the existing million-case decoder corpus. No GCP was run locally.
