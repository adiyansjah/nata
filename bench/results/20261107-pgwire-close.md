# Slice 4b closeout: Postgres wire and real clients

2026-11-07. Measurements and limitations are recorded against card 0003's original
thresholds. The streaming threshold is **NOT MET**; no threshold is relaxed.

Implementation: `fb8ab0b` adds the clean-VM jobs, paired benchmark, exact default
client probes, frontend trace without keys/Bind payloads, and queue/worker-share/
producer-completion instrumentation. No push. Raw evidence lives under
`bench/results/raw/20261107-pgwire-close/`.

## Card target verdicts

| Aspect | Original target / evidence | Verdict |
|---|---|---|
| Correctness | Every successful slice-4 query: 221/221 typed values through four clients match CLI; DuckDB oracle 251/251 on both client VMs. Literal text differs in 76/77 queries; ADBC cannot connect. x86 45 mutants caught, ARM64 sweep not run after baseline failure | NOT MET for literal text/all-client and two-CPU mutation certification; tested values/errors MET  **2026-11-07 reopened amendment:** literal text remains NOT MET; original four clients retain 221/221 typed values. ADBC now connects; default fetch reaches COPY/0A000, configured no-COPY matches 221/221 typed values. final clean ARM64 measured tree catches 51/51 valid mutants (R20 done); [fix evidence](20261107-pgwire-fixes.md). |
| Performance | “adds at most 20%”; raw/psql ratios 61.57–65.41x ARM64, 57.04–87.75x x86 versus ≤1.20x. First row before producer end in every round | NOT MET overhead; MET first row  **2026-11-07 reopened amendment:** bounded batching removes per-row writes; final Rust raw/CLI ARM64 4.697344x / 3.989974x and x86 4.042516x / 3.416084x (one/eight workers), unchanged ≤1.20x NOT MET; earlier raw was Python. Matched psql Nata/PostgreSQL 1.270–1.453x is context only; VM profiles expose per-row allocation/encoding cost. First row MET; NODELAY/coalescing removes the timer floor; [fix evidence](20261107-pgwire-fixes.md). |
| Scalability | “throughput within 2x” for 64 concurrent versus sequential on eight vCPUs: elapsed ratios 1.286027 ARM64 / 1.066360 x86 | MET by fixed median rule, weak under ARM64 twin noise  **2026-11-07 reopened amendment:** final concurrent/sequential ratios 0.468515 ARM64 / 0.597430 x86, original ≤2 threshold MET; paired spread retained; [fix evidence](20261107-pgwire-fixes.md). |
| Reliability | Disconnect/cancel/kill stop work and release counters; 2,000 seeded scenarios on each CPU, healthy sessions unaffected, retained RSS within declared bounds | MET in tested bounds; ARM64 mutant certification NOT MET  **2026-11-07 reopened amendment:** bounded buffer writes keep cancellation/cleanup/stalled-send guarantees; final clean ARM64 release, 2,000 lifecycle, 10,000 hostile cases and 51/51 valid mutants PASS; [fix evidence](20261107-pgwire-fixes.md). |
| Fault tolerance | 10,000 live hostile cases on each CPU; no observed panic/hang/leak, protocol limit tests; regular-frame cap 1,114,112 bytes, startup cap 10,000 | MET in tested corpus, not exhaustive |
| Resource bounds | 64 sessions + eight reserved startup/cancel slots checked; slow-reader produced 2,567/1,048,576 rows on each CPU, peaks 19,260/21,264 KiB within unchanged probe bounds. Original R16 SF10 gap remains | MET bounded slow-reader/socket probes; NOT MET full R16 closure  **2026-11-07 reopened amendment:** two 64-KiB bridge buffers (128 KiB queued data); slow readers produce 2,773 rows ARM64 and 2,773 on x86, both <20,000 with unchanged RSS bounds. Original R16 gap remains; [fix evidence](20261107-pgwire-fixes.md). |
| Portability | Real server, benchmark and all five client installations on clean ARM64/x86-64 VMs | MET ordinary tested surface; ARM64 45-mutant sweep NOT MET  **2026-11-07 reopened amendment:** both clean CPU client/benchmark runs complete; final clean ARM64 measured tree catches 51/51 valid mutants (R20 done); [fix evidence](20261107-pgwire-fixes.md). |
| Compatibility | Versioned psql/JDBC/psycopg/pgcli ordinary native values and supported probes pass; default ADBC bootstrap fails 0A000, dependent operations blocked | NOT MET universal client compatibility; full matrix and known differences recorded  **2026-11-07 reopened amendment:** ADBC 1.13.0 default connect repaired; all 221 default corpus fetches now reach COPY and fail 0A000, documented no-COPY matches 221/221 typed values and ten-type Arrow/NULL fixture on both CPUs. Universal default-client target still NOT MET (R18); [fix evidence](20261107-pgwire-fixes.md). |
| Security/privacy | Loopback-only/default unsafe-listen refusal tests; no TLS/auth claim; staged/tree leak scans plus stricter new-result email/address audit, redacted raw results | MET tested scope |
| Operability | Per-session id/duration/state, queue/admission share logs, positioned SQLSTATE errors and existing EXPLAIN protocol tests; fixed seeds/versions reproduce outcomes | MET tested scope |

## Method

Spot n4a-highcpu-8 (ARM64) and c3d-highcpu-8 (x86-64), eight vCPUs, Debian 12,
us-central1-a; ordinary allocator and VM descriptor limits. Warm OS cache, five
alternating rounds with identical twin arms, load below 0.5 before every timed
block. One million ordered BIGINT rows in 8,192-row groups; query `SELECT i FROM
'million.parquet'`, exact results checked against DuckDB, CLI CSV, raw and psql
before timing. One/eight workers, one admission, window defaults twice workers.

Raw uses persistent sockets, includes framing/decoding without Python cell
conversion; CLI and psql include process startup. All discard rendered output
for timing. This is the card's end-to-end interface comparison, not an isolated
executor comparison. The server sends text results, and does not collect rows.
First DataRow is timed at the client against server producer completion; the
latter includes bounded-bridge backpressure. Client time starts earlier, so this
comparison is conservative. Kernel buffering alone is not the first-row oracle.

Concurrency uses 64 established connections, 64 fixed identical LIMIT-1 Parquet
queries, eight workers/eight admissions (one worker per admission); sequential
and barrier-released concurrent runs alternate with twins. Elapsed ratio is the
inverse throughput ratio for the same 64 queries. Queue wait covers semaphore
acquisition only. Configured share is not measured CPU utilization. Preparation
costs are 100 roundtrips per sample, repeated five times, comparing Parse each
time, named reuse with text input, and named reuse with int4 binary input.

Compatibility uses all 221 queries in check/queries/sql that CLI answers (30
rejections excluded), every SQL fixture plus TPC-H SF0.01. Each client receives
the same query and data. nata-check's DuckDB oracle runs first, including its
existing declared tolerance rules. Exact text differences are retained per query;
a second comparison uses OIDs for booleans, float32 roundtrip, float specials and
exact decimals. It never converts integers/decimals through float. Native-type,
NULL, error/position, cancel and ten-execution probes use separate all-type and
1,048,576-row × 1,024-byte fixtures. Generated files stay on the work disk.

## Initial-run corrections

- ARM64 reliability's source tar raced an instrumentation edit and failed compilation.
  No tests from that run count. Fixed before the committed retry; no production
  correctness claim relies on it.
- First benchmark runs returned normal exit status but had incomplete coverage:
  `slow` matched zero tests, first row was compared only with transfer completion,
  and preparation/fixed-share blocks were absent. Their streaming/concurrency
  samples are retained, but not used as final certification. Retried once after
  correcting the job, with the same thresholds and caps.
- First item35 ran native pgjdbc/psycopg successfully, but the pgcli probe accessed
  tuple results as attributes, JDBC comparison was absent, and full-SF1 JDBC output
  was placed in results. This was a job defect. Corrected tuple handling and
  comparison, moved JDBC output to WORK and used the bounded SF0.01 compatibility
  dataset for the retry. First-run raw JDBC rows are intentionally not copied.

The ARM64 replay retry passes release tests and the exact long reliability/fuzz
sizes, then fails mutation setup: the private workspace omitted
`rust-toolchain.toml`. Clean rustup has no default, unlike the development host.
The runner now copies the pin. An isolated local RUSTUP_HOME with installed
1.99.1 but no default reproduces the old runner's exit 2; the fixed runner's
baseline passes and wrong-type-OID mutant is CAUGHT. This is one local mutant,
not 45 ARM64 catches. ARM64 mutation certification is **NOT MET: 0/45 attempted**.
No third VM was launched after the permitted retry. R20 records the remaining
replay; the existing x86-64 45/45 result is not relabelled as ARM64 evidence.

The original card's unnamed/text-only scope was extended in step 4 with bounded
named preparation and mapped scalar binary input/results so default pgjdbc and
psycopg preparation would work. ADR 0008's dated amendment records that decision
and its tests/limits. This closeout makes no further protocol-scope expansion:
COPY, extra SQL types and suspended cursors remain excluded.

## Final benchmark targets and regression baselines

Card: **“Result streaming adds at most 20% to `nata sql --csv` for 1 million rows over loopback, and the first row arrives before the scan ends”** Same warm query/cache/workers; ratios use the five-round median.

| CPU | workers | CLI median s | raw median s | psql median s | raw / CLI | psql / CLI | ≤1.20 verdict |
|---|---:|---:|---:|---:|---:|---:|---|
| arm64 | 1 | 0.037576844 | 2.409377091 | 2.457725391 | 64.118666x | 65.405317x | NOT MET |
| arm64 | 8 | 0.039099403 | 2.489231828 | 2.407296157 | 63.664190x | 61.568617x | NOT MET |
| x86 | 1 | 0.040791929 | 3.579397869 | 2.779211121 | 87.747698x | 68.131397x | NOT MET |
| x86 | 8 | 0.050884172 | 3.628683744 | 2.902415501 | 71.312622x | 57.039653x | NOT MET |

| CPU | workers | first DataRow median ms | producer end median s | first < end in every round |
|---|---:|---:|---:|---|
| arm64 | 1 | 1.303680 | 2.409008 | MET |
| arm64 | 8 | 1.246769 | 2.488767 | MET |
| x86 | 1 | 0.996540 | 3.578963 | MET |
| x86 | 8 | 1.485459 | 3.628225 | MET |

Card: **“64 concurrent connections each running a small query; throughput within 2x of 64 sequential runs on 8 vCPUs.”**

| CPU | sequential median s | concurrent median s | elapsed ratio (≤2) | sequential / concurrent queries/s | concurrent queue median / max ms | verdict |
|---|---:|---:|---:|---:|---:|---|
| arm64 | 0.051448819 | 0.066164555 | 1.286027 | 1243.95 / 967.29 | 9.258000 / 23.051000 | MET |
| x86 | 0.076406591 | 0.081476924 | 1.066360 | 837.62 / 785.50 | 15.077500 / 33.981000 | MET |

Eight total workers / eight admissions give **one scan worker per admitted query**; queued connections share those eight admissions. The single-admission streaming arm gives eight workers to one query.

| CPU | one export at share 1/8, median s | same export at share 8/8, median s | Parse each time ms/call | named text ms/call | named binary int4 ms/call |
|---|---:|---:|---:|---:|---:|
| arm64 | 2.251717 | 2.489232 | 43.996838 | 43.996790 | 44.037855 |
| x86 | 2.867883 | 3.628684 | 44.037897 | 43.997804 | 43.998464 |

Preparation/binary costs are **informational**: 100 calls per sample; elapsed seconds ×10 = milliseconds/call. The ~44 ms network/timer floor prevents attributing tiny deltas to parse or binary conversion. Share measurements are separate settled blocks and output-bound, not a causal CPU-utilization comparison.

Card resource threshold: **“a slow client exerts backpressure (the server does not buffer a whole result)”**. Permanent one-byte/ms probe; **peak ≤ baseline +131,072 KiB** and **produced <20,000 of 1,048,576 rows**, fixed before assertion.

| CPU | baseline / peak RSS KiB | bound KiB | rows produced | bytes read | verdict |
|---|---:|---:|---:|---:|---|
| arm64 | 13,652 / 19,260 | 144,724 | 2,567 | 1,893 | MET |
| x86 | 14,480 / 21,264 | 145,552 | 2,567 | 1,886 | MET |

RSS is the embedded-server integration-test process (including its small client threads), a conservative server upper bound, not an isolated CLI process measurement. This probe has two total workers/two admissions, one worker per query, default scan window; streaming/concurrency configurations above differ. No result buffering: the unchanged collect-all mutant was caught locally in step 4 (911,204 produced versus the <20,000 bound). See the earlier record; it is not an ARM64 mutant catch.

Every arm’s min/median/max, spread, configured share and queue waits are in each CPU’s `summary.txt`; all five samples are in JSON. Twelve primary medians per CPU are stored in `crates/nata-bench/baselines/pgwire-{arm64,x86}.csv`. Twin medians are separate samples from the same alternating session, not an independent later run. The regression threshold stays **20%**.

arm64: primary-versus-twin gate **NOT MET**; planted 30%-lower baseline correctly **FAILS** (exit 1).

- pgwire/connections64/sequential: 1.826x (limit 1.200x): FAIL

x86: primary-versus-twin gate **MET**; planted 30%-lower baseline correctly **FAILS** (exit 1).

A noisy regression failure remains a failure, not a raised tolerance or an excluded metric. P15 records packetization/timer/noise isolation. The raw directory retains every verdict and the planted negative control.

## Item 35 matrix (both clean CPU types)

Both CPU types produce identical structured corpus results: **221/221 typed
row sets match** for psql, pgjdbc, psycopg and pgcli. Every row is checked; exact
text equality is deliberately reported separately. No extra preparation/binary/
fetch-size driver settings are supplied. Psql CSV/null/verbose options control
presentation only. Pgcli is driven through its actual PGExecute API, including
its normal connect probe and type loaders; interactive completion is not claimed.

| Client and recorded version | Corpus: exact text / typed values | Native types and NULL | Error SQLSTATE / position | Cancel | Preparation, ten executions |
|---|---|---|---|---|---|
| psql 15.19, Debian 15.19-0+deb12u1 | 145/221 exact; 221/221 values | display text/NULL PASS; no native object API | 42703 / caret at 8 PASS | 57014 PASS | SQL PREPARE FAIL 0A000; repeated simple query 10 PASS; no wire-preparation API |
| pgjdbc 42.7.14, OpenJDK 17.0.21 | 145/221 exact; 221/221 values | all ten mapped types/classes, extrema/specials/NULL PASS | 42703 / 8 PASS | 57014 PASS | default threshold 5, named/binary transition, 10 PASS; integer/bool/NULL parameters PASS |
| psycopg 3.3.8, binary libpq 18.0.6 | 145/221 exact; 221/221 values | all ten mapped types/classes, extrema/specials/NULL PASS | 42703 / 8 PASS | 57014 PASS | default threshold 5, 10 PASS; default scalar/Decimal/date/NULL parameters PASS |
| pgcli 4.7.1, psycopg 3.3.8 | 144/221 exact; 221/221 values | all mapped native values/NULL PASS; DATE is intentionally str | 42703 / 8 PASS | 57014 PASS | PGExecute default connection preparation, 10 PASS |
| ADBC PostgreSQL 1.13.0, pyarrow 26.0.1 | 0 queries reached; default connection FAIL | BLOCKED | bootstrap 0A000 at ORDER BY; ordinary 42703/8 probe BLOCKED | BLOCKED | BLOCKED |

**Card correctness literal text threshold: NOT MET** for 76 queries in each of
psql/pgjdbc/psycopg, 77 in pgcli; these are enumerated in `differences-CLIENT.json`
and in each VM's corpus JSON, not removed from the denominator. Canonical typed
values pass all 221. The independent `nata-check` SQL suite is **251/251 PASS**
on each client VM (including its 30 expected rejection cases); that oracle's
existing float-tolerance directives remain unchanged. Flight SQL is not tested.

Known differences, cause, and bug status:

- **psql, pgjdbc and psycopg:** Postgres text bool is `t`/`f` rather than CLI
  `true`/`false`; float specials use `NaN`/`Infinity`/`-Infinity` rather than
  `nan`/`inf`/`-inf`. The SQL NULL sentinel is selected explicitly for CSV display.
  Cause: ADR 0008 type-specific wire formatting and libpq/JDBC readback. Not a
  wrong-value bug; a literal-text deviation retained in every affected query.
- **pgcli:** bool display uses Python `True`/`False`; native float32 formatting
  can expand its exact promoted float value; Decimal adds a leading zero
  (`.500` → `0.500`) or renders scaled zero in exponent form
  (`0.0000000000` → `0E-10`), and DATE is intentionally loaded as
  text by pgcli's own adapters. Cause: client formatting/type-loader policy. The
  OID-aware float32 roundtrip comparison passes; not a server value/type bug.
  Per-query examples include all differences, not only these summaries.
- **TINYINT in all clients:** text/str rather than a signed eight-bit native
  Postgres type. Cause: Postgres has no signed eight-bit integer OID; R2's exact
  type-identity fallback is text. Intentional known difference, not a bug.
- **psql SQL PREPARE:** 0A000 (`not supported yet: Prepare statements`). Cause:
  SQL PREPARE is outside the SQL dialect; supported wire Parse/Bind is a separate
  interface. Known limitation, not a broken named wire statement. Ten simple
  queries are not presented as successful SQL preparation.
- **pgjdbc fetchSize=100 with autocommit=false:** 0A000; rollback and SELECT 1
  recover. **psycopg named SQL cursor:** DECLARE returns 0A000. Cause: suspended
  portals/SQL cursors require a bounded idle-portal policy. Intentional scope
  limits; no fake PortalSuspended or collected result.
- **ADBC:** distribution/PyPI default driver first queries
  `pg_catalog.pg_attribute ORDER BY attrelid, attnum`; server returns **0A000,
  not supported yet: ORDER BY**, with the position at ORDER BY (line 8).
  **Connection creation fails. No default operation works beyond the handshake
  and attempted bootstrap.** Execute, Arrow fetch, binary-result readback,
  preparation, cancel and COPY are blocked by that failed connection; no claim
  is made that COPY itself was reached or returned an error. Cause: required
  catalog introspection is out of scope, even though step 4 supports binary
  results. This is a compatibility miss (R18), not a reason to invent catalogs.
- **All clients:** ordinary DATE values are certified; exceptional Date32 domains
  remain unmeasured (R19). Catalog/backslash/completion queries, COPY, TLS/auth,
  SQL/suspended cursors and transactional isolation remain outside this slice.

Recorded frontend chatter (full sequences per VM in `frontend-trace.txt`, no
BackendKeyData or Bind payloads logged): psql sends SSL probe, startup user/
database/application_name=psql and user SQL, no mandatory SQL bootstrap; pgjdbc
sends SSL probe, UTF8/ISO/Etc/UTC startup settings, SET application_name, extended
Parse/Bind/Describe/Execute/Sync, BEGIN/ROLLBACK where its tests require them;
psycopg sends startup routing labels, BEGIN before operations, COMMIT/ROLLBACK and
DEALLOCATE ALL on rollback cache invalidation; pgcli uses its psycopg connection
with autocommit, SELECT 1 probe and normal user SQL (no interactive completion);
ADBC sends SELECT version() then its failing catalog query. The trace also
records cancel requests with their key omitted. All expected harmless chatter
on successful clients works without protocol/preparation workarounds.

## Reliability, fault tolerance and scope of certification

ARM64 retry seed **322661912839**, eight CPU competitors, 1 ms injected file-open
and 100 us file-read delays: **2,000/2,000 scenarios PASS**, **10,000/10,000 live
fuzz cases PASS**. Lifecycle duration 91.781 s; fd/thread/RSS baseline
(16, 8, 19,688 KiB), final (8, 8, 22,112 KiB), sampled peak 28,004 KiB. The fd
reduction includes the four healthy clients' sockets closing; retained RSS delta
2,424 KiB is below the unchanged +64 MiB bound. Fuzz takes 91.331 s, baseline/
final RSS 20,612/22,524 KiB, below +32 MiB retention; healthy probes and resource
counters return to baseline after each case. No observed panic/hang. The reserved
startup/cancel socket and 64 concurrent correct-query checks pass. Inspect
`pgwire-live.txt` for class counts, exact counters and timeouts.

The existing local x86-64 result is 2,000 lifecycle and 10,000 fuzz PASS, **45/45
mutants CAUGHT** (step-4 evidence retained); ARM64 mutation setup fails before
running any mutant, as described above. Reliability/fuzz behavior is empirically
MET in these bounds on both CPUs; **two-CPU mutation certification is NOT MET**.
A failed baseline is neither a caught mutant nor 45 individual INVALID runs.

Resource certification is limited to these fixtures/default VM limits. The
stalled reader is evicted after its two-second unwritable-frame deadline; this
is bounded backpressure/cleanup, not successful completion of a gigabyte at one
byte/ms. R16's original SF10 SQL peak **114,964 KiB versus 93,048 KiB budget**
remains NOT MET on x86-64; its original ARM64 SF10 probe passed (67,352 KiB).
This work does not rerun or repair that original allocator gap. R15's local
maximum-depth ASan repair is retained, but full sanitizer certification is NOT
MET. Both are explicit carried-in deviations; no changed K/fixed allowance or
reduced depth limit. R17, R6b and broader executor work remain deferred.

No new server value/protocol correctness bug was exposed by item 35. Fixed job/
harness defects are distinguished from server changes: compilation/upload race,
empty slow-reader selection, incomplete first-row/preparation/admission probes,
pgcli tuple handling/JDBC comparison, large results placement, and toolchain pin
omitted from private mutation copies. The latter has a failing old-runner control
and a caught fixed-runner mutant. The register carries P14/P15/R18/R19/R20 and retains
R15/R16; R13's bounded cancellation/signal audit is done.

## Interpretation and remaining work

The large export miss is real at this fixture width; it cannot be explained by
process startup (CLI startup is included), and identical twins remain far above
the 1.20 ceiling. ARM64 raw and psql share approximately the same server-limited
cost. The code allocates/transfers/writes a packet and sets up async timeout logic
for each row. That is a **reasoned** cause candidate, not a profile isolating
packet allocation versus channels versus syscalls. P14 requests bounded batching
with the existing correctness/cancel/backpressure mutants retained.

Small prepared-query samples cluster near a ~44 ms roundtrip floor. Their tiny
text/binary/named differences are below that floor's observed variation and do
not isolate conversion or parse cost. The server does not set TCP_NODELAY and
writes small protocol frames separately: a delayed-ACK/Nagle interaction is a
**reasoned** explanation to test with a one-change paired control, not an
established causal measurement. P15 also records the large primary/twin
sequential-query spread. The concurrency threshold uses the fixed median rule;
a MET result with that noise is weak evidence, not a universal throughput bound.

The 1-versus-8-worker export-share comparison is dominated by output; it records
configured admission shares and elapsed time, not CPU utilization or scheduler
borrowing efficiency. A later CPU-heavy multi-query benchmark is needed before
changing the fixed-share policy. No performance optimization or protocol-scope
expansion is smuggled into this measurement change.

## Cloud accounting and cleanup

Every run was preceded by plan; all printed worst cases fit the owner’s caps. All used Spot, eight vCPUs, 20 GB boot disks, no service account/scopes, and only resources bearing that run’s label. Data disks: 30 GB except pgwirecheck 50 GB; max VM time 1.5 hours per run. Orchestrators ran detached with logs under the home directory and were waited through cleanup. No unrelated bucket, account or resource was changed.

ARM = **n4a-highcpu-8**; x86 = **c3d-highcpu-8**. Rates are the cached prices.env estimates (November 3/4 source), not billing invoices. VM-compute estimates exclude billed disks/IP; the approved worst-case plans include their allowances. No run reports $0.000, so no R14 manual correction is needed here. Dates use Asia/Jakarta; raw UTC timestamps fall on November 6.

| Run ID | machine | counted duration s | orchestrator VM USD | plan worst USD | outcome | cleanup status |
|---|---|---:|---:|---:|---|---|
| pgwirecheck-arm64-4b5 | ARM | 214 | 0.006 | .196 | compile failure | no instances/disks |
| pgwirebench-arm64-4b5 | ARM | 611 | 0.017 | .191 | finished; incomplete probes | no instances/disks |
| pgwirebench-x86-4b5 | x86 | 755 | 0.014 | .119 | finished; incomplete probes | no instances/disks |
| item35-x86-4b5 | x86 | 485 | 0.009 | .119 | finished; incomplete client harness | no instances/disks |
| pgwirecheck-arm64-4b5-r2 | ARM | 338 | 0.009 | .196 | release/live PASS; mutants not run | no instances/disks |
| item35-x86-4b5-r2 | x86 | 351 | 0.007 | .119 | finished; full matrix | no instances/disks |
| item35-arm64-4b5 | ARM | 210 | 0.006 | .191 | finished; full matrix | no instances/disks |
| pgwirebench-arm64-4b5-r2 | ARM | 742 | 0.020 | .191 | finished; complete benchmark | no instances/disks |
| pgwirebench-x86-4b5-r2 | x86 | 1514 | 0.029 | .119 | finished; preempted once, resumed | no instances/disks |

Sum of rounded orchestrator compute estimates: **$0.117**. Approved ceilings were $0.30 ARM benchmark, $0.20 x86 benchmark/item35, and $0.25 ARM pgwirecheck; none was exceeded or increased. Each run directory has its redacted report and an explicit post-run status proof.

The final x86 benchmark suffered one Spot preemption during its first incarnation. Completed setup/build steps resumed from the labelled data disk, and the whole timed benchmark restarted. Final medians use only the replacement VM’s complete five rounds, never a mixture of partial attempts. Settle logs retain both incarnations; the report counts both VM durations. No separate third benchmark run was created.

## Local checks and retained experiment

Before implementation commit `fb8ab0b`: fmt, workspace/all-target clippy with
warnings denied, 403 workspace tests, cargo-deny and staged leak scan pass. Final
closing gate repeats those commands: **403 PASS**, no failed/ignored tests;
scan/SQL/differences nata-check **5/5, 251/251, 37/37 PASS**. TMPDIR stays on disk
under `$HOME/tmp-4b5`; AI_AGENT is unset for DuckDB (CODEX_CI also unset for the
final oracle). Deny reports advisories/bans/licenses/sources OK, with only the
existing unmatched-license-allowance warnings.

The job-chain suite passes; a syntactically valid missing-benchmark-step mutant
fails it with exit 1. Regression pass/slowdown/missing/invalid-input controls
pass as a suite. The isolated no-default toolchain control fails old code and
fixed wrong-type-OID is CAUGHT. The full local benchmark smoke is explicitly
SANITY ONLY, not cloud timing evidence. A closing trial shared build artifacts
with that private mutation probe and failed rustdoc metadata lookup (E0463);
a subsequent complete, uncontended gate rebuilds original crates and passes.
The failed trial is not counted as successful verification.

`spikes/pgwire-bakeoff` is kept outside workspace members: permanent production
checks replace its hand-codec probes but not the independent pgwire reuse-
candidate panic/resource comparison. ADR 0008's released-fix revisit rule still
needs that candidate/control. The slice card, ADR amendment, verification-plan
rows, supported-client notes, regression baselines and improvement register are
updated together. Raw line endings/trailing whitespace are normalized; CSV
fields are checked unchanged. No push; final tracked tree/staged leak scans and a stricter
new-evidence email/address audit precede the closing commit.


## Dated reopened-method correction, 2026-11-07

The historical `raw` tables above used Python and include client parsing cost.
The final card verdict in [fix evidence](20261107-pgwire-fixes.md) uses the Rust
count/discard reader against the unchanged **raw/CLI ≤1.20x** rule. Both CPUs also
use matched psql and Rust arms against an installed PostgreSQL table; those are
context, never a replacement denominator. The literal/card/default-client misses
remain visible. Final measured source tree
`ad89453261a3b894039ed4280f7d95a56a100b99` and source-blob manifest are recorded.
Strict repeated full-old-tree NODELAY off/on control is **44.087 / 0.0584 ms**;
the earlier mixed-session diagnostic is retained but excluded as a one-change
control. Final clean ARM64 51/51 valid mutants closes R20. P14's remaining
encoding cost and R18 default COPY, R19 dates, R21 finite float text and R15/R16
remain open. Full cloud accounting, every corrected/failed run and explicit
cleanup proofs are in the fix record. No push.
