# Slice 4b second reopened fix

2026-11-07 (Asia/Jakarta). Implementation and evidence; no push. The original
**raw/CLI ≤1.20x target remains NOT MET**. No threshold or executor memory
allowance changes. R18 default ADBC fetch and R21 wire float formatting are the
bounded scope of this step; all other deviations remain explicit below.

## Built and measured source

Final measured code-only tree **`c11a35ef897ea9135f5fc6da668740aa45feb54b`**; manifest in
`crates/nata-bench/pgwire-source-manifest.txt`. Reproduce with
`python3 scripts/pgwire-source-tree.py`. This uses a private Git index and records
627 source paths/modes/blob hashes; docs, results and baseline CSVs are excluded
to avoid a circular hash. Built debug locally on x86 and release on both clean CPUs. Both final item35 VMs record this same tree and pass 221/221 default-ADBC typed queries. The earlier item35 jobs shipped `3f14c8628ebe63a2a8820a52f0d606b44be8de97` before harness-only normalization.

The benchmark and pgwirecheck jobs ship tree
**`b62a977b774efdf593f4cde236e5f82f78fd0719`** (manifest retained under raw/local).
Its production Rust and tests are identical to the final tree.
The first source difference is the PostgreSQL setup log destination in
pgwire-job.sh: `results/bench/postgres-setup.txt` becomes
`results/$mode/postgres-setup.txt`. The clients mode had not created the bench
folder, so both initial item35 executions failed before any client probe. The
bench mode has the same effective path in both trees. Do not mislabel this as a
server failure or silently claim every job used the same full tree.
The second harness difference is item35.py: trim trailing SQL whitespace only
for default ADBC and fail the job if any of the six client paths has a typed
mismatch. This follows the driver's independently reproduced upstream semicolon
bug below. No production Rust changes follow the measured tree. Both earlier
harness versions and the first source manifest are retained under raw/local.

## COPY contract and independent binary oracle

Only `COPY (<slice-4 SELECT>) TO STDOUT [WITH] (FORMAT binary)` is accepted,
including single-quoted `binary`. The exact 1.13.0
[ExecuteCopy source](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/result_helper.cc)
uses `PQexecParams` and `(FORMAT binary)`; its
[reader](https://github.com/apache/arrow-adbc/blob/apache-arrow-adbc-25/c/driver/postgresql/statement.cc)
requires the header and first tuple together. COPY FROM, files, PROGRAM,
text/CSV, table shorthand and extra options remain clear 0A000 errors. No SQL
or general catalog extension. The existing encoders supply all mapped scalar
binary values. Numeric now removes redundant zero groups and normalizes zero
weight/sign while retaining dscale.

CopyOutResponse has overall and every column format 1. CopyData contains the
11-byte PGCOPY signature, two zero int32 header fields, tuple int16 column counts,
int32 lengths (-1 for NULL), binary scalar values, and int16 -1 trailer. The
header accompanies the first tuple, or the trailer when empty. CopyDone and
`COPY n` completion follow. Extended Describe returns NoData; Execute ignores
max-rows like PostgreSQL's COPY utility command, and ReadyForQuery waits for
Sync. Failed extended commands still recover only through Sync.

COPY reuses the DataRow bridge and writer: two 64 KiB queued buffers, blocking
producer backpressure, shared admission, two-second write/flush deadline,
constant-memory frame-boundary tracking, cancel/disconnect and worker join.
Wide tuples may cross transport buffers without enlarging the bridge. Current
row scratch remains separate; this is not arbitrary-cell-width RSS certification.
The new protocol tests exercise refusal, extended sequence, NULLs, empty results,
backpressure, cancellation, disconnect, timeout and healthy-session isolation.

Independent oracle: `crates/nata-pgwire/tests/postgres_oracle.py` loads the same
three-row ten-type fixture into a real PostgreSQL table with explicit matching
Postgres types. It compares concatenated COPY bytes, including the whole header
and trailer, **without decoding or tolerances**. Local PostgreSQL **17.11**:
**244 bytes, 3 rows, 0 differing bytes**. Types include bool, text (TINYINT's map),
int2/4/8 extrema, float4 signed zero/infinity, float8 NaN/negative infinity,
fixed-scale numeric (123.450 and -0.001), UTF8/empty text, date epoch/leap day,
and a row NULL in every column. Local default ADBC 1.13.0 also fetches this
fixture ten times and an empty result with all ten typed columns.

An initial separate-header CopyData implementation passed the byte-stream
oracle but failed default ADBC with an empty first-record input. This exposed
the driver's per-message assumption; PostgreSQL's header-plus-first-tuple
framing fixes it. The permanent framing test and default-client probes retain
that distinction. Cloud attempts canceled during startup before workloads are
recorded separately from actual failed item35 executions.

## Float text oracle and known-bad controls

Only wire text changes. CLI CSV still uses its DuckDB-oriented formatter.
PostgreSQL positional exponents are -4..14 for float8 and -4..5 for float4;
scientific exponents have an explicit sign and at least two digits. No `.0` on
integral values, with `-0`, `NaN`, `Infinity`, `-Infinity` preserved.
`extra_float_digits` accepts 1..3 and refuses older rounding values below 1 with
clear 0A000, both in SET and startup. ADR 0008 records that choice.

Rust's shortest digits alone are insufficient: PostgreSQL's
[d2s](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/common/d2s.c)
and [f2s](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/common/f2s.c)
disable Ryu's acceptBounds. The implementation rounds ties to even at Rust's
shortest precision and excludes exact binary midpoint endpoints, increasing
precision only there. Endpoint equality cancels factors of five and powers of
two exactly with checked integers, not approximate float comparisons.

Seed **322661912839**, 100,000 random IEEE bit patterns per width plus decimal
powers and adjacent representations of both signs, zero/negative zero,
subnormals, min-normal and max-finite edges. The oracle loads the original IEEE
bytes into PostgreSQL with COPY FROM binary, invokes float4out/float8out through
`::text`, and compares every Nata text value exactly. Local PostgreSQL 17.11:
**103,805 float8 and 100,513 float4 values; zero differences in either width**.
This is seeded empirical evidence, not exhaustive float-domain certification.

The initial layout-only implementation differed on 45 float8 values. Explicit
ties reduced that to the two signed 1e23 endpoint cases; exact endpoint handling
reduced it to zero. Permanent tests assert 1e23, exponent boundaries, signed zero,
smallest float4 subnormal/max-finite and unsupported legacy settings. The
threshold, retained-.0 and closed-midpoint mutants demonstrate failure.

## Item 35: both clean CPUs, default settings

Both CPUs have identical results and versions: psql/PostgreSQL reference 15.19,
pgjdbc 42.7.14 / OpenJDK 17.0.21, psycopg 3.3.8 / libpq 18.0.6, pgcli 4.7.1,
ADBC PostgreSQL 1.13.0 / pyarrow 26.0.1. Every actual row and column name is
compared by OID-aware value against Nata CLI; independent DuckDB nata-check is
**251/251 PASS on each client VM**. No query is excluded. Arrow numeric remains
an opaque extension backed by exact strings, not native Arrow decimal.

| Client/path (both CPUs) | Corpus exact / typed | Native types and NULL | Error / position | Cancel | Repeat/preparation |
|---|---|---|---|---|---|
| psql | 141/221 exact; 221/221 values | display text/NULL PASS; no native-object API | 42703 / caret 8 PASS | 57014 PASS | ten simple repeats PASS; SQL PREPARE 0A000 |
| pgjdbc defaults | 141/221 exact; 221/221 values | all mapped classes/extrema/specials/NULL PASS | 42703 / 8 PASS | 57014 PASS | ten default executions, named/binary transition PASS |
| psycopg defaults | 141/221 exact; 221/221 values | mapped native values/NULL and scalar parameters PASS | 42703 / 8 PASS | 57014 PASS | ten default executions/preparation PASS |
| pgcli PGExecute defaults | 144/221 exact; 221/221 values | mapped values/NULL PASS; DATE intentionally str | 42703 / 8 PASS | 57014 PASS | ten default repeats PASS |
| ADBC defaults, trailing whitespace trimmed (final) | 146/221 exact; **221/221 values** | all-type Arrow/NULL PASS | ordinary error/position not certified | not certified | ten default all-type fetches PASS |
| ADBC defaults, original file text | 145/221 exact; **220/221 values, 1 refusal** | all-type Arrow/NULL PASS | ordinary error/position not certified | not certified | ten default all-type fetches PASS |
| ADBC use_copy=false | 146/221 exact; 221/221 values | all-type Arrow/NULL PASS | not certified | not certified | fixture fetch PASS; ten identical repeats not separately certified |

The earlier default ADBC run's sole cloud failure is `s1_trailing_semicolon.sql`:
the input ends in `;\n`, so its exact source strips no semicolon and generates
`COPY (SELECT …;\n) …`. Nata refuses this PostgreSQL-invalid form with 0A000.
A real PostgreSQL 17.11 default ADBC probe also fails that wrapper, with 42601;
`query.rstrip()` lets the driver strip the terminator and succeeds. This is
upstream query wrapping, not evidence for widening COPY or excluding a query.
R22 tracks it. The prepared harness trims only trailing whitespace for default
ADBC, uses no driver options, and passes **221/221 typed Arrow row sets locally**.
The original 220/221 cloud result remains explicit. Its old job returned success
despite that per-query failure; the new all-client typed gate demonstrably fails
on the retained 220/221 JSON. The owner explicitly approved two additional cloud normalization checks beyond
the used failed-run retry limit. **Both pass 221/221 typed Arrow row sets**,
146/221 exact text, zero failed queries, with the strict all-client value gate.

Literal CLI text parity stays **NOT MET**. The PG-correct finite formatter
increases psql/JDBC/psycopg differences from 76 to 80 queries; typed values stay
221/221. Pgcli has 77 display differences; ADBC successful original-default queries
have 75 and one refusal; no-COPY has 75. These include bool/special floats, PG
finite layout, numeric leading zeros, NULL/display policy, and pgcli's own
float/Decimal/date loaders. Full per-query results remain in corpus.json.
TINYINT still returns text by the unchanged OID map. Unsupported SQL PREPARE,
SQL/suspended cursors and general catalogs remain separate limitations.

All completed cloud PostgreSQL **15.19** oracles independently match the same **244 COPY
bytes** and **103,805 float8 / 100,513 float4** text values with **zero differences**.
Local PostgreSQL 17.11 provides the additional version control described above.

## Both-CPU streaming measurements

Spot n4a-highcpu-8 ARM64 / c3d-highcpu-8 x86, eight vCPUs / 16 GiB, Debian 12, us-central1-a, warm million BIGINT rows, five settled alternating primary/twin rounds, one/eight workers. Same CLI/Rust/psql/PostgreSQL methods as the first fix record; Rust startup excluded, CLI/psql process startup included. Verify every ordered integer before timing. COPY is a new Rust count/discard arm, checked by value and response byte count against PostgreSQL first. Native timing excludes profiling and reliability tests. Full min/median/max/spreads/queue waits are retained.

| CPU | workers | CLI s | raw s | raw/CLI ≤1.20 | COPY s | PG COPY s | COPY/PG |
|---|---:|---:|---:|---|---:|---:|---:|
| arm64 | 1 | 0.037146498 | 0.160298966 | 4.315318x **NOT MET** | 0.322906238 | 0.135317677 | 2.386283x |
| arm64 | 8 | 0.039210219 | 0.152654519 | 3.893233x **NOT MET** | 0.335732183 | 0.133776974 | 2.509641x |
| x86 | 1 | 0.041180346 | 0.181469851 | 4.406710x **NOT MET** | 0.321802913 | 0.129522108 | 2.484540x |
| x86 | 8 | 0.050689921 | 0.185579836 | 3.661080x **NOT MET** | 0.333259121 | 0.129110847 | 2.581186x |

Both CPUs: first DataRows precede producer completion. Concurrent/sequential medians ARM64 0.415540 / x86 0.613801 ≤2 **MET**. Both CPUs: Primary/twin 20% regression gate **PASS**; planted 30%-lower baseline **FAIL**, exit 1. All 22 primary medians refresh each CPU’s regression baseline (four added COPY/reference medians).

COPY is materially slower than PostgreSQL (2.39–2.58x here), even though it is bounded and correct. The implementation still allocates/converts a text DataRow before binary conversion and CopyData reframing; this is a reasoned source-level contributor, not a profile isolating COPY cost. P14 carries this new measured gap. The existing per-row text allocation profiles remain in the first fix record. Current text psql Nata/PG ratios are 1.305–1.358x, context only. No target is changed or failed sample discarded.


## Local verification and limitations

Final workspace: **410 passed, zero failed/ignored**. Full private-copy mutation
sweep: **60/60 CAUGHT, zero INVALID/survivors**, exit 0, unchanged Rust-source
fingerprint. All 51 prior mutants remain caught; nine new mutants cover wrong
PGCOPY signature, missing trailer, NULL length zero, numeric weight +1, date epoch
1970, COPY without backpressure, float exponent threshold +1, retained `.0`, and
closed midpoint intervals. Compilation failure and outer timeout never count. A separate targeted legacy-setting control restores silent acceptance of extra_float_digits=0 and is also CAUGHT locally (not added to the 60-entry cloud sweep).
Tests, mutations and full output are retained under raw/local (workspace log uses
deterministic gzip).

Final gates: `cargo fmt --check`; workspace/all-target clippy with `-D warnings`;
`cargo test --workspace`; `cargo deny check` (advisories/bans/licenses/sources OK,
existing duplicate/unmatched-allowance warnings); job-chain tests; Python and
shell syntax. Nata-check with AI_AGENT and CODEX_CI unset: **scan 5/5, SQL 251/251,
differences 37/37 PASS**. TMPDIR is on disk under the home directory, never /tmp.
Staged leak and strict new-evidence audits run before commit.

One early protocol-only trial failed the existing reserved-startup socket timing
assertion during simultaneous fresh optimized dependency builds; an uncontended
repeat and both unmutated mutation suites pass, as does the final workspace.
Early mutation starts were stopped before useful mutant evidence while sources
were still changing; only the full final unchanged-source sweep counts. These
failures/stopped probes are not reported as caught mutants.

R15 full sanitizer certification, R16 original SF10 RSS, R19 exceptional Date32,
SQL PREPARE/suspended and SQL cursors, general catalogs/COPY, Flight SQL and
TLS/auth remain outside this step. Literal CLI text equality remains NOT MET
because PG type/client presentation differs. The original streaming threshold
remains NOT MET. Real PostgreSQL itself measured about 2.3–2.6x CLI CSV in the
first fix record (eight-worker matched Rust reference); that puts the original 1.20x below the observed protocol floor,
without changing it. New P16 asks the next client-protocol card to choose a fair
fixed same-client PostgreSQL reference target and close the remaining roughly
1.3–1.5x matched-client gap; P14 retains the measured allocation/encoding work.


## Clean ARM64 reliability and mutation certification

`pgwirecheck-arm64-4b7-r2`: release protocol/reliability suite PASS; seed
322661912839, eight CPU competitors, injected 1 ms opens and 100 us reads:
**2,000 lifecycle / 10,000 live hostile scenarios PASS**. Full mutation replay:
**60/60 CAUGHT, zero INVALID/survivors**, job exit 0. Every old mutant remains
caught; new COPY backpressure and float-format controls are included.

Recorded counters and bounds:

- Lifecycle baseline fd/threads/RSS `(16, 5, 21464 KiB)`, final
  `(8, 5, 24180 KiB)`, sampled peak 30,144 KiB, elapsed 143.987 s. Closing four
  healthy sockets explains the fd reduction. Retained RSS is below +64 MiB.
- Live hostile corpus kind counts `[831,747,907,773,855,840,851,821,823,887,828,837]`,
  elapsed 91.928 s, baseline/final RSS 21,436/21,436 KiB, below +32 MiB retention.
- One-byte/ms DataRow reader: 2,773 rows produced of 1,048,576, 1,883 bytes read,
  RSS 15,768→21,516 KiB, below the unchanged +131,072 KiB budget. This is the
  original bounded slow-reader probe, separate from COPY-specific protocol tests.

Finite seeded contention/lifecycle certification is not exhaustive proof. The
COPY tests share the exact production bridge/writer path, but no new SF10 or
arbitrary-width COPY RSS claim is made. Full routine session logs are redacted
then compressed with deterministic gzip; summaries remain readable beside them.


The ARM benchmark job independently repeats **60/60 valid mutant catches**, the
long lifecycle/hostile suite, and bounded-reader/protocol tests. The x86 benchmark
job passes its release protocol/reliability suite. Both complete and clean up.
No preemptions occurred in any completed run; every sample stays retained.

## GCP accounting, stopped attempts, retries and cleanup

Plan precedes every run; no --approved-usd exceeds the stated cap. All Spot,
eight vCPUs, 16 GiB, us-central1-a, 20 GB boot disk; 30 GB data for item35/x86
bench and 50 GB for ARM bench/check. Max VM time 1.3 h for final item35,
1.5 h for initial item35/x86 bench, 1.8 h for ARM bench/check. No service
accounts/scopes. Only run-labelled resources were touched; unrelated buckets
and accounts were not changed. Post-run status proofs show no instances/disks.

Cached prices.env rates from Nov 3/4, not billing invoices. Compute estimates
exclude actual disk/IP charges; the printed worst cases include allowances.
The first attempts were stopped during startup when the local ADBC per-message
requirement was found, before workloads/source measurements. They are not
successful checks. The first actual item35 executions failed before probes
because of the setup log path; their permitted retries retained the 220/221
ADBC driver-wrapping outcome. The owner explicitly approved two additional
normalization/gating runs after seeing that remaining query and printed costs.
No failed or partial row set is hidden as a PASS.

| Run | machine | VM s | compute USD | worst / cap USD | outcome | cleanup |
|---|---|---:|---:|---|---|---|
| item35-arm64-4b7 | n4a-highcpu-8 | 90 upper | 0.002460 upper* | 0.191 / .20 | stopped before workload | none left |
| item35-arm64-4b7-r2 | n4a-highcpu-8 | 214 | 0.006 | 0.191 / .20 | harness path failure | none left |
| item35-arm64-4b7-r3 | n4a-highcpu-8 | 343 | 0.009 | 0.170 / .20 | matrix: 220/221 default ADBC | none left |
| item35-arm64-4b7-r4 | n4a-highcpu-8 | 332 | 0.009 | 0.170 / .20 | PASS | none left |
| item35-x86-4b7 | c3d-highcpu-8 | 0 | no VM; disk only | 0.119 / .20 | stopped before workload | none left |
| item35-x86-4b7-r2 | c3d-highcpu-8 | 213 | 0.004 | 0.119 / .20 | harness path failure | none left |
| item35-x86-4b7-r3 | c3d-highcpu-8 | 342 | 0.007 | 0.104 / .20 | matrix: 220/221 default ADBC | none left |
| item35-x86-4b7-r4 | c3d-highcpu-8 | 347 | 0.007 | 0.104 / .20 | PASS | none left |
| pgwirebench-arm64-4b7 | n4a-highcpu-8 | 57 upper | 0.001558 upper* | 0.228 / .30 | stopped before workload | none left |
| pgwirebench-arm64-4b7-r2 | n4a-highcpu-8 | 1438 | 0.039 | 0.228 / .30 | PASS checks; throughput NOT MET | none left |
| pgwirebench-x86-4b7 | c3d-highcpu-8 | 91 upper | 0.001737 upper* | 0.119 / .20 | stopped before workload | none left |
| pgwirebench-x86-4b7-r2 | c3d-highcpu-8 | 474 | 0.009 | 0.119 / .20 | PASS checks; throughput NOT MET | none left |
| pgwirecheck-arm64-4b7 | n4a-highcpu-8 | 0 | no VM; disk only | 0.228 / .25 | stopped before workload | none left |
| pgwirecheck-arm64-4b7-r2 | n4a-highcpu-8 | 1034 | 0.028 | 0.228 / .25 | PASS | none left |

Sum of rounded ordinary compute estimates plus conservative stopped-VM upper
bounds: **$0.123755**, plus actual disk/IP charges within plan allowances.

*R14 accounting correction:* three stopped startup runs report zero counted VM
seconds / $0.000 despite an incarnation. Those are not free. Conservative
creation-log-to-completed-cleanup intervals are 90 s (item35 ARM), 57 s (bench
ARM), 91 s (bench x86), giving the upper estimates in the table at cached rates.
Actual instance deletion can precede completed cleanup, so these are labelled
upper bounds, not exact billing durations. The other two stopped runs created
no VM, only a data disk; no compute cost is invented, and disk charges remain.
Raw original reports and explicit correction stay together.
