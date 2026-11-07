# Slice 4b reopened fixes

2026-11-07 (Asia/Jakarta). No push. The unchanged **Rust raw / CLI ≤1.20x target
is NOT MET** on either CPU. Bounded batching and the delayed-ACK defect are fixed;
ADBC connects and configured Arrow fetch works, while default COPY remains open.

## Measured code and method correction

Final measured **code-only Git tree `ad89453261a3b894039ed4280f7d95a56a100b99`**, identical on both final VMs.
`crates/nata-bench/pgwire-source-manifest.txt` records the 623 paths, modes and Git
blob hashes; each final committed source blob matches it. The sparse Git tree
covers crates/scripts/check source (rs/py/sh/java/toml/lock/env/sql), Cargo.lock
and the toolchain pin. Results, documentation and baseline CSVs are excluded to
avoid a circular hash when recording measurements. Both VM source-tree.txt files
and the local final source check retain the hash. The final commit contains the
measured source; nothing was pushed.

**Dated method correction:** the initial 4b6 `raw` arm was Python. Its 0.39–0.42 s
versus psql's 0.22–0.25 s exposed client overhead and could not isolate server
cost. Those samples stay in their original directories; they are not the final
card raw arm. The final `raw` is the compiled Rust `pgwire-raw` binary. The Python
reader remains explicitly `python-raw`. The legacy share1-of8/raw block remains
Python and is informational. Initial fast-client tree
`10f850a4728ef540c2d90c899db670dcb7fc1609` and its manifest/three changed harness
files are retained under local/initial-fast-*: x86 completed, ARM failed at perf.
The corrected final harness adds the profiling fallback and restart-safe private
PostgreSQL reference setup; production Rust is identical between these two trees.

## Method, correctness and limits

Spot **n4a-highcpu-8 ARM64 / c3d-highcpu-8 x86-64**, eight vCPUs, 16 GiB, Debian 12,
us-central1-a, ordinary allocator/default descriptor limits. Warm million-row
BIGINT Parquet, row groups 8,192; one/eight Nata workers, one admission, default
scan window. Five alternating primary/twin rounds of CLI, Rust raw, Python raw,
psql, PostgreSQL Rust raw and PostgreSQL psql, settled guest load <0.5 before each
block. Min/median/max/spread and every sample are retained. CLI/psql include
process startup; Rust times query send through ReadyForQuery on an established
socket, excluding its process/connection startup and JSON reporting. The Rust
reader uses bounded buffers, counts/discards DataRows and bytes, without cell
conversion in timed arms. Before timing it verifies all one million ordered
integer values; both servers return **16,888,943 query-response bytes** in every
Rust sample. DuckDB CSV, Nata CSV and both psql CSV outputs match exactly first.
A deliberately wrong-row server is rejected by Rust --verify; malformed/truncated
and ErrorResponse frames have a permanent helper test.

The reference is the distribution **PostgreSQL 15.19** package on the same VM,
a fresh trust/loopback-only work-disk cluster. COPY loads the same ordered CSV
into `million(i BIGINT)`, ANALYZE, warm queries, no parallel gather/JIT/autovacuum.
The same psql options and Rust reader are used, changing only port and source
query (`SELECT i FROM million` versus the Parquet source). This is a matched
client/protocol reference including PostgreSQL scan/format work, **not an isolated
client CPU measurement or an executor/storage bake-off**. It never replaces the
card's CLI denominator or changes 1.20x.

## Streaming verdict, exactly the original bound

Card: **“Result streaming adds at most 20% to `nata sql --csv` for 1 million rows
over loopback, and the first row arrives before the scan ends.”** Apply the
unchanged **Rust raw / CLI ≤1.20x**, using five-round medians.

| CPU | workers | CLI s | Rust raw s | raw / CLI | verdict |
|---|---:|---:|---:|---:|---|
| arm64 | 1 | 0.036351688 | 0.170756400 | 4.697344x | **NOT MET** |
| arm64 | 8 | 0.038292640 | 0.152786635 | 3.989974x | **NOT MET** |
| x86 | 1 | 0.042072523 | 0.170078864 | 4.042516x | **NOT MET** |
| x86 | 8 | 0.053571713 | 0.183005472 | 3.416084x | **NOT MET** |

Separate context only; **no new threshold**:

| CPU | workers | Nata psql s | PostgreSQL psql s | Nata/PG psql | PostgreSQL Rust s | Nata/PG Rust | Nata Python s |
|---|---:|---:|---:|---:|---:|---:|---:|
| arm64 | 1 | 0.231952948 | 0.159601636 | 1.453324x | 0.101621086 | 1.680324x | 0.423099540 |
| arm64 | 8 | 0.214058255 | 0.158964985 | 1.346575x | 0.100839398 | 1.515148x | 0.419958907 |
| x86 | 1 | 0.233018398 | 0.183470328 | 1.270060x | 0.118001348 | 1.441330x | 0.401329272 |
| x86 | 8 | 0.247195179 | 0.189056037 | 1.307523x | 0.123896348 | 1.477085x | 0.421832804 |

Historical before/after with the **same psql client**, eight workers: ARM64
**2.407296157 → 0.214058255 s**, x86 **2.902415501 → 0.247195179 s**. Original
one-worker psql **2.457725391 / 2.779211121 s**, now **0.231952948 / 0.233018398 s**
(ARM64/x86). Original Python raw eight-worker **2.489231828 / 3.628683744 s**,
now **0.419958907 / 0.421832804 s**. These are historical clean-VM comparisons,
not paired old/new causal arms. Original numbers and all failed targets remain
visible in the closeout. Do not report the old Python-to-new-Rust speedup as a
server-only speedup.

Primary Rust sample noise:

| CPU / workers | CLI min / median / max s | Rust min / median / max s | Rust spread |
|---|---|---|---|
| arm64 / 1 | 0.036226035 / 0.036351688 / 0.039065558 | 0.164015948 / 0.170756400 / 0.172202288 | 4.79% |
| arm64 / 8 | 0.038198281 / 0.038292640 / 0.038473308 | 0.150704115 / 0.152786635 / 0.156979896 | 4.11% |
| x86 / 1 | 0.041101781 / 0.042072523 / 0.042334313 | 0.168872925 / 0.170078864 / 0.172124911 | 1.91% |
| x86 / 8 | 0.052334740 / 0.053571713 / 0.054934274 | 0.181137727 / 0.183005472 / 0.183909473 | 1.51% |

All other arms' min/median/max, spreads, queue waits and twins remain in each
summary.txt/samples.json. The unchanged 20% primary-versus-twin regression gate
**passes on both CPUs**; the deliberately 30%-lower baselines **fail, exit 1**.
Eighteen primary medians per CPU refresh the pgwire baseline CSVs. Streaming/raw
now explicitly means Rust; the method change is recorded, not silently compared
to the old Python series. Earlier regression CSVs/verdicts are also retained.

## Measured cause and bounded transport fix

Local x86-64 `strace -f -c`, original server versus the initial batching repair,
one million rows with the same raw reader: **sendto 1,000,014 → 368**, **futex
1,972,069 → 2,367**, epoll_wait **29,635 → 352**. This measures the removed per-row
transport/synchronization cost (2,717x fewer send calls), not individual CPU
percentages for allocation/conversion/timers. Traced 77.229 / 0.769 s and shared-
build-host untraced 10.469 / 0.660 s are diagnostic only, never card timings.
This early syscall profile predates the final constant-memory framing guard;
final native profiles below include it. Commands/counts/raw samples are retained.

DataRows, including requested scalar binary conversion, now cross the bridge in
at-most **64 KiB** byte buffers, flushed promptly at executor batch boundaries.
Two channel slots hold **128 KiB queued payload**; producer/writer can each own
one more (256 KiB bridge payload), plus the 64 KiB socket response writer.
Current-row encoding scratch and the existing scan window are separate; this
is not arbitrary-cell-width or original SF10 RSS certification. Metadata/EXPLAIN
packets over 64 KiB return 54000. Blocking send retains backpressure; dropping
the receiver wakes production. Each complete buffer write **and flush** has the
existing two-second deadline; cancelled writes keep the same future for up to
one second. No whole-result collection or increased executor memory allowance.

A first prototype missed cancellation across buffers inside a wide DataRow.
The final five-byte-header/body-length cursor tracks frame boundaries without
retaining rows. If cancellation/producer failure leaves a partial row, close the
socket rather than inserting ErrorResponse into it; normal boundary cancellation
still returns 57014. A 4 MiB-row cancel test and a boundary-removal mutant show
this can fail. Exact 200 KiB text/binary rows across multiple buffers are checked.

### Final on-VM profile: remaining server work

Rust remains above 1.20x, so final code is profiled on each VM after timed samples.
x86 perf CPU-clock sampling has **zero lost samples**. Top flat self samples:
per-row iterator/Result collection **13.89% + 7.66%**, Vec<Option<String>>
construction **5.14%**, `_int_free` **6.85%**, `realloc` **5.59%**, `malloc`
**3.52%**, memmove **6.13%**, codec::row **3.79%**, binary::packet **3.34%**,
csv::cell **2.61%**, framing cursor **2.89%**, Snappy decode **2.25%**. These are
flat self samples, not summed inclusive call stacks or a client CPU profile.

ARM perf cannot sample even CPU-clock: **“PMU Hardware doesn't support
sampling/overflow-interrupts.”** The corrected harness falls back to a separate
Callgrind server, outside all native timing arms. It verifies/counts one million
rows; **2,145,370,110 guest instructions** including startup. Flat instruction
shares: `_int_free` **12.34%**, malloc **10.07%**, codec::row **6.69%**, free
**5.23%**, `_int_realloc` **5.22%**, realloc **4.76%**, csv::cell **4.43%**,
memcpy **3.74%**, Snappy **2.79%**, integer formatting **2.49%**, framing cursor
**1.73%**. Instruction shares are **not native CPU-time percentages**, nor
cross-ISA performance comparisons. Full reports remain inspectable.

Both profiles expose remaining per-row value/packet allocation, copying and
formatting. The Python reader was an additional cost, but the remaining Rust
miss cannot be called wholly client-side. P14 stays open for direct/reused
encoding buffers and further measured work; the original 1.20x is not relaxed.

## Small-query floor: strict one-change control

Two full `4373a63` archived workspaces differ in exactly one line:
`socket.set_nodelay(true)` on acceptance. All other file hashes match; source
patch/check retained. Five alternating 100-call SELECT-1 samples give
**44.087184 ms off / 0.058383 ms on** per roundtrip. This isolates Nagle/delayed
ACK behavior. **Correction:** the earlier 48.040 / 0.425 ms diagnostic reused a
newer session module in one executable; it is retained but excluded as the strict
one-change control. The repeated full-archive control replaces that claim.

Accepted sockets now set TCP_NODELAY. The bounded response writer coalesces
frames and flushes at ReadyForQuery, frontend Flush, before blocking for input,
and streamed buffer boundaries. Final coalesced SELECT-1 socket controls:

| CPU | NODELAY on ms | off ms |
|---|---:|---:|
| arm64 | 0.036741 | 0.034370 |
| x86 | 0.057362 | 0.049988 |

Neither coalesced arm has the old timer floor; tiny differences are not
attributed to NODELAY. Original prepared roundtrips were about 44 ms; final
100-call Parquet preparation samples are distinct from the minimal SELECT-1
control and include planning/metadata work:

| CPU | Parse each / named text / named binary ms per call |
|---|---|
| arm64 | 1.169105 / 0.818345 / 0.806738 |
| x86 | 1.712796 / 1.169278 / 1.163832 |

## First row, concurrency and slow-reader bounds

First DataRow is compared conservatively to producer completion (including bridge
backpressure), not merely last-byte receipt. Every round passes on both CPUs.

| CPU / workers | first-row median ms | producer-end median s | verdict |
|---|---:|---:|---|
| arm64 / 1 | 1.563553 | 0.170565 | MET |
| arm64 / 8 | 1.768354 | 0.152461 | MET |
| x86 / 1 | 1.628061 | 0.169787 | MET |
| x86 / 8 | 2.039611 | 0.182493 | MET |

Card: **“64 concurrent connections each running a small query; throughput within
2x of 64 sequential runs on 8 vCPUs.”** Existing Python clients, eight admissions,
one scan worker per admission. Configured share is not measured CPU utilization.

| CPU | sequential / concurrent median s | elapsed ratio ≤2 | concurrent spread | verdict |
|---|---|---:|---:|---|
| arm64 | 0.045735158 / 0.021427611 | 0.468515 | 40.03% | MET |
| x86 | 0.067883918 / 0.040555899 | 0.597430 | 23.84% | MET |

No noisy samples are discarded; MET by the fixed median rule is not a universal
throughput guarantee. Twins/spreads and queue waits are retained.

Card: **“a slow client exerts backpressure (the server does not buffer a whole
result).”** Unchanged probe: one byte/ms, produced **<20,000 of 1,048,576 rows**,
peak RSS **≤ baseline +131,072 KiB**, cutoff at a two-second stalled buffer write.

| CPU | produced | RSS baseline / peak KiB | bytes read | verdict |
|---|---:|---|---:|---|
| arm64 | 2,773 | 13,784 / 19,600 | 1,892 | MET |
| x86 | 2,773 | 14,440 / 21,000 | 1,879 | MET |

## ADBC: exact source and observed boundary

The version is pinned to **ADBC PostgreSQL 1.13.0** (Apache tag
[apache-arrow-adbc-25](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/python/adbc_driver_postgresql/adbc_driver_postgresql/_static_version.py)).
[RebuildTypeResolver](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/type_resolver_init.cc)
first projects attrelid, attname, atttypid from pg_catalog.pg_attribute ordered
by attrelid, attnum, then queries oid, typname, typreceive, typbasetype, typrelid,
typarray from pg_type with its exact receive/send/range/array predicates (three
attempts). The session allowlist matches these exact token sequences, preserving
string literals. It returns zero attributes (no exposed composites) and ten
fixed built-in type rows, including accepted varchar, correct receive names and
zero base/class/array references. No mutable catalogs, array types or SQL dialect
extensions. Unknown catalog queries return 0A000. Strings/file names containing
pg_catalog remain ordinary SQL.

[ExecuteQuery / UseCopy](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/statement.cc)
default to COPY for PostgreSQL; [ExecuteCopy](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/result_helper.cc)
wraps the statement in COPY (...) TO STDOUT (FORMAT binary). The documented
`adbc.postgresql.use_copy=false` selects PqResultArrayReader, whose
[result reader](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/result_reader.cc)
requests ordinary **binary** Bind results. It does not require text support;
existing scalar binary output is sufficient. Both clean CPUs connect and fetch
the ten-type/NULL fixture as Arrow with that option. Numeric is an opaque Arrow
extension backed by exact strings (123.450), not a native Arrow decimal.

Default fetch now **reaches COPY and fails 0A000**, instead of failing connect.
R18 stays open for default streaming COPY fetch. COPY is deliberately not added:
it needs additional CopyOut framing/trailer/lifecycle/mutation coverage and is
unnecessary for this small, read-only bootstrap repair. ADR 0008 records that
choice. Default-client coverage and configured no-COPY coverage are separate.

The first item35 run on each CPU exposed an inherited harness defect: the ADBC
cursor was left open after the first COPY refusal, so later probes returned
client state errors. Those failures are retained, not labelled COPY outcomes.
The single retry closes cursors, rolls back after errors, and also probes the
configured no-COPY path through every successful SQL query.

## Text review and remaining differences

Bool t/f, NaN/Infinity/-Infinity and ordinary ISO date wire forms were already
PostgreSQL forms. NULL presentation, pgcli bool/date/float/Decimal displays and
TINYINT's text fallback remain declared client/type differences. A real server
formatting defect is fixed: DECIMAL(p,p) now emits **0.500 / -0.500**, retaining
scale and the leading zero PostgreSQL expects, while CLI's DuckDB .500 spelling
is unchanged. The test checks both signs; a leading-zero-loss mutant catches it.

The optional full finite-float spelling audit is not completed: the existing
DuckDB-oriented finite formatter can retain .0 and uses different scientific
layout thresholds from PostgreSQL (see its
[f2s](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/common/f2s.c) and
[d2s](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/common/d2s.c)
formatters). This is a server text-compatibility gap,
not dismissed as client display or wrong typed values; R21 records it. Matching
all shortest-roundtrip tie/layout rules needs its own PostgreSQL oracle. The
original literal-text target remains NOT MET, alongside default COPY, exceptional
Date32 (R19), sanitizer certification (R15), and original SF10 RSS (R16).


## Item 35 amendment, both clean CPUs

Versions: psql 15.19, pgjdbc 42.7.14/OpenJDK 17.0.21, psycopg 3.3.8/libpq 18.0.6,
pgcli 4.7.1, ADBC PostgreSQL 1.13.0/pyarrow 26.0.1. Full client matrix measured
on the corrected item35 VMs; final framing changes preserve ordinary row bytes,
with text/binary and wide-cancel tests on both final CPU VMs.

| Client/path | exact text / typed corpus | native and edge outcomes |
|---|---|---|
| psql | 145/221 exact; 221/221 typed | text/NULL; 42703/caret 8, cancel 57014, simple repeat 10 PASS; SQL PREPARE 0A000 |
| pgjdbc defaults | 145/221 exact; 221/221 typed | all mapped native/NULL; 42703/8, 57014, prepare/binary transition 10 PASS |
| psycopg defaults | 145/221 exact; 221/221 typed | mapped native/NULL; 42703/8, 57014, preparation 10 PASS |
| pgcli PGExecute defaults | 144/221 exact; 221/221 typed | mapped values/NULL, DATE str; 42703/8, 57014, preparation 10 PASS |
| ADBC defaults | connect PASS; **221/221 COPY attempts fail 0A000** | default Arrow fetch unavailable; ordinary error-position/cancel/repeat fetch not certified |
| ADBC use_copy=false | 146/221 exact; **221/221 typed** | ten-type Arrow/NULL PASS; numeric opaque exact-string extension; error-position/cancel/ten identical repeats not separately certified |

DuckDB oracle **251/251 PASS on both client VMs**. Every actual row is checked;
76/77 original-client text differences and 75 configured-ADBC differences remain
in corpus.json. No query is excluded to improve a verdict. Literal-text and
universal default-client targets remain **NOT MET**; no Flight SQL claim.

## Verification, failures and remaining limits

Final local workspace **407 PASS, zero failed/ignored**; all **51 mutants CAUGHT,
zero INVALID/survivors, exit 0**. Final clean ARM64 benchmark VM also catches
**51/51**, and passes release tests plus **2,000 lifecycle / 10,000 live hostile
cases**, fixed seed and eight CPU competitors. Final clean x86 release protocol
and reliability suites pass. New controls cover per-row flush restoration,
bridge-byte-cap removal, NODELAY omission, wrong catalog OID, fractional numeric
zero loss, and partial-wide-row error insertion; the existing stalled-send
mutant still targets the complete buffer write/flush deadline. Existing mutants
remain caught. A failed compilation/outer timeout is INVALID, never a catch.

The private mutation copy retains rust-toolchain.toml, resolves work paths
absolutely and saves full baseline failures. Fresh no-default rustup fails
without the pin and passes with it. One early local baseline failure had its
assertion truncated by the old runner; it is not mutation evidence. A later
50-catch run correctly failed its source fingerprint after a concurrent source
edit; only unchanged-source final runs count. The first ARM replay used an older
per-row mutant that also broke metadata bounds; that new control is excluded,
then corrected in the 50-mutant retry and final 51-mutant sweep. R20 is done.

Initial item35 runs exposed cursor cleanup/recovery errors after the first COPY
failure; both CPUs were retried once with context-managed cursors and rollback,
yielding independent 0A000 for all 221 default attempts plus configured coverage.
The first fast-reference ARM benchmark failed at unsupported perf sampling;
its one corrected retry uses Callgrind. x86 was repeated with the identical final
harness/tree, retaining its successful earlier perf/reference samples. The
PostgreSQL private cluster directory is reset on a preempted job's restart;
no preemption actually occurred in these runs.

R15 full sanitizer coverage, R16's original SF10 allocator miss, R19 exceptional
Date32 domains and new finite-float spelling R21 remain open. No SQL/suspended
cursors, general catalogs, extra SQL types, TLS/auth or COPY is claimed. P14's
remaining encoding cost is explicitly NOT MET; P15 and R20 are done, R18 remains
open for default COPY. Optional full finite-float parity was not completed;
fractional DECIMAL leading-zero parity was fixed.

Final gates: fmt --check; workspace/all-target clippy -D warnings; workspace tests;
cargo deny (advisories/bans/licenses/sources OK; existing unmatched-license
warnings only); job-chain checks; Python/shell syntax; staged leak scan and strict
new-file audit including decompressed logs. TMPDIR stays on disk. Final
nata-check with AI_AGENT/CODEX_CI unset: **scan 5/5, SQL 251/251, differences 37/37**.
Final source hashes are checked against the measured manifest before commit.

ARM live outcomes (full redacted logs retained compressed; summaries adjacent):

- `RELIABILITY PASS baseline_fd_threads_rss=(16, 8, 19796) final_fd_threads_rss=(8, 8, 23332) peak_rss_kib=27272 elapsed=143.954392241s`
- `LIVE_FUZZ PASS kinds=[831, 747, 907, 773, 855, 840, 851, 821, 823, 887, 828, 837] elapsed=93.0105107s rss_baseline_final_kib=(26468, 26468)`
- `SLOW_READER PASS result_rows=1048576 width=1024 bytes_read=1892 produced=2773 rss_baseline_peak_kib=23580,25724 bound_delta_kib=131072`

## GCP accounting and cleanup

Plan before every run. All Spot, eight vCPUs, 20 GB boot, no service account/scopes,
us-central1-a. Bench/item35 max 1.5 VM-hours; pgwirecheck 1.9. Data disks 30 GB
except check and final ARM fast-reference runs 50 GB. Every printed bound fits
the owner's cap; no --approved-usd exceeds it. Only run-labelled resources were
touched; unrelated buckets/accounts were untouched. Explicit post-run status
proofs show **no instances/disks left for every run**.

Compute estimates below use cached prices.env (Nov 3/4 source), rounded by the
orchestrator, **not billing invoices**. They exclude actual disk/IP charges;
plan bounds include those allowances. All runs have one incarnation, no preemptions.

| Run | machine | VM seconds | compute USD | printed worst / cap USD | outcome | cleanup |
|---|---|---:|---:|---|---|---|
| pgwirebench-arm64-4b6 | n4a-highcpu-8 | 314 | 0.009 | .191 / .30 | Python-client characterization | none left |
| pgwirebench-x86-4b6 | c3d-highcpu-8 | 507 | 0.010 | .119 / .20 | Python-client characterization | none left |
| item35-arm64-4b6 | n4a-highcpu-8 | 218 | 0.006 | .191 / .20 | cursor harness defect; retried | none left |
| item35-x86-4b6 | c3d-highcpu-8 | 336 | 0.006 | .119 / .20 | cursor harness defect; retried | none left |
| item35-arm64-4b6-r2 | n4a-highcpu-8 | 327 | 0.009 | .191 / .20 | corrected matrix | none left |
| item35-x86-4b6-r2 | c3d-highcpu-8 | 323 | 0.006 | .119 / .20 | corrected matrix | none left |
| pgwirecheck-arm64-4b6 | n4a-highcpu-8 | 1010 | 0.028 | .239 / .25 | 50 catches; stale control | none left |
| pgwirecheck-arm64-4b6-r2 | n4a-highcpu-8 | 908 | 0.025 | .239 / .25 | corrected 50 catches | none left |
| pgwirebench-arm64-4b6-r2 | n4a-highcpu-8 | 332 | 0.009 | .196 / .30 | perf unsupported; retried | none left |
| pgwirebench-x86-4b6-r2 | c3d-highcpu-8 | 611 | 0.012 | .119 / .20 | earlier reference/perf complete | none left |
| pgwirebench-arm64-4b6-r3 | n4a-highcpu-8 | 1280 | 0.035 | .196 / .30 | final reference/profile + checks | none left |
| pgwirebench-x86-4b6-r3 | c3d-highcpu-8 | 485 | 0.009 | .119 / .20 | final reference/profile + checks | none left |

Sum of rounded VM compute estimates **$0.164**, plus billed disk/IP within plan allowances.

Raw live logs use deterministic gzip to retain every routine session/scan line without tens of megabytes of repetition; redact/audit before compression. Text line endings and trailing blank lines are normalized; CSV cells are unchanged. Native timing and profiler data are never mixed. No push.
