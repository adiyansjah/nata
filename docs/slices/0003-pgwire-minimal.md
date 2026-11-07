# 0003: A minimal Postgres-wire server for slice 4's queries ("4b"), and real-client compatibility (plan item 35)

Status: re-closed with deviations, 2026-11-07 (Rust raw streaming budget, default ADBC COPY, literal text and carried-in certification gaps recorded below)
Date: 2026-11-06
ADRs: 0001, 0002, 0007 (relies on); creates ADR 0008 "client protocol" (which protocol first, and how types map)

Rule: `docs/way-of-working.md`, section 2. Taken out of the feature map's slice 10 so that real clients can be tested right after slice 4
(the owner asked what item 35 needs; this is the answer). Slice 10 keeps the rest of the protocol and `information_schema`.

## 1. Goal

`nata serve --listen 127.0.0.1:5432` accepts Postgres-wire connections and runs slice 4's queries: `psql -h 127.0.0.1 -c "SELECT ...
FROM 'lineitem.parquet' WHERE ... LIMIT 10"` prints DuckDB's rows. Supported: the startup handshake (protocol 3.0, no TLS, trust auth on
loopback only), the SIMPLE query protocol, the EXTENDED protocol for unnamed statements without parameters or with text-format parameters
that are literals of the dialect (Parse, Bind, Describe, Execute, Sync, Close), text-format results, `RowDescription` with the type OIDs of
ADR 0008, `CommandComplete`, `ErrorResponse` with SQLSTATE codes and the error position, the cancel request, and the session chatter
real drivers send on connect (`SET` of the parameters drivers send, `SHOW`, `SELECT version()`, `current_setting`): answered when harmless,
otherwise a clear error. Not in this slice: TLS, passwords, binary format, COPY, transactions beyond auto-commit no-ops, `information_schema`.

Decision (ADR 0008, local baseline 2026-11-07): write the supported message codec; unchanged pgwire 0.42.1 fails R1(iii).

## 2. Quality targets

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | Every query of slice 4's test set returns, through each client, the same rows (as text) as `nata sql --csv` and as DuckDB; errors arrive as `ErrorResponse` with the right SQLSTATE class and position. Mutants (each caught): a wrong type OID, a NULL sent as an empty string, a lost `CommandComplete`, rows of one query sent to another session, an extended-protocol `Sync` that does not end the error state. | The item 35 job: real clients against a real server on a clean VM; a protocol-level test with recorded message sequences | `nata sql` (slice 4) and DuckDB |
| Performance | Result streaming adds at most 20% to `nata sql --csv` for 1 million rows over loopback, and the first row arrives before the scan ends | Paired benchmark, `psql` and a raw-protocol client, both CPU types | `nata sql --csv` |
| Scalability | 64 concurrent connections each running a small query; throughput within 2x of 64 sequential runs on 8 vCPUs | Load test with a fixed seed | - |
| Reliability | A client that disconnects mid-result, a cancel request, a killed client: the query stops, its tasks end, memory and handles are released, other sessions are not affected | The slice 3 leak counters across sessions; scenarios with disconnect and cancel at random points; under CPU contention | slice 3 cancel |
| Fault tolerance | A malformed or truncated protocol message is an `ErrorResponse` and a closed connection, never a panic or a hang; a message above 1,114,112 bytes (length included, tag excluded; startup 10,000 bytes) is refused before allocating | A fuzz of the protocol decoder (10,000 cases) and a test per limit | - |
| Resource bounds | 64 session slots plus 8 bounded pre-startup/cancel slots (72 sockets total, five-second startup deadline); memory per session bounded by the scan's window; a slow client exerts backpressure (the server does not buffer a whole result) | Tests with a client that reads slowly; peak RSS | slice 3 bounds |
| Portability | ARM64 and x86-64, clean VMs | `orchestrate.sh` jobs on both | - |
| Compatibility | Real clients, each with its version recorded: `psql`, pgjdbc (JDBC), psycopg 3, the ADBC PostgreSQL driver, and one BI-style client (DBeaver's driver is pgjdbc; `pgcli`). Known differences listed. | The item 35 job (one VM, about 1 hour, about $0.12) | - |
| Security and privacy | Loopback only in this slice (refuses other addresses unless `--unsafe-listen`, documented as unsafe until TLS and auth exist); errors carry no cell values; no secrets in logs | Tests; leak scan | - |
| Operability | Each session's queries are visible in a log line with session id and duration; `EXPLAIN` works through the protocol | Tests | slice 4 `EXPLAIN` |

## 3. Decision rules (fixed before measuring)

- **R1, protocol implementation:** reuse the `pgwire` crate unless the bake-off shows it cannot (i) run under our scheduler without a second
  thread pool beyond ADR 0002's tokio I/O runtime, (ii) stream results with backpressure, (iii) be fuzzed without panics, or (iv) pass
  `cargo-deny` and the unsafe inventory. Otherwise write the message codec for the supported messages ourselves (a few hundred lines).
- **R2, type mapping (ADR 0008):** each Nata type maps to the Postgres type OID that pgjdbc and psycopg read back as the same value
  (decimals as `numeric` with their text, dates as `date`, booleans as `bool`); a type with no exact Postgres equivalent is sent as text
  and listed.

## 4. Baseline spike

Required (a build-versus-reuse choice): the `pgwire` crate against a hand-written codec for the supported messages, on R1's four criteria,
with `psql` and pgjdbc against both; local; result file [20261107-pgwire-baseline.md](../../bench/results/20261107-pgwire-baseline.md).

Step 1 measured pgwire 0.42.1 with `default-features = false, features = ["server-api"]`. R1(i), (ii), (iv) pass within the local
probe; R1(iii) fails (2,015 panics in 10,000 regular-frame cases). The checked hand decoder has no panic in 10,000 regular and
10,000 startup cases, and its guard-removal mutant is caught. Select the hand codec exactly by R1; see ADR 0008 for the R2 type map,
threading, security, limits and cancel contract. Resource probes are not R16 certification; the production limits are reasoned defaults.

## 5. Carried-in improvements (from `docs/improvements.md`)

Chosen at slice 4 closeout (2026-11-07), limited to the existing query cancellation, parser stack and
executor resource boundaries; budget at most about a quarter of this slice. No change to parts 1–4 or 6.

| Register item | Taken in or deferred | Reason and bounded work | Check |
|---|---|---|---|
| R13 signal handling dependency | taken in: audit reuse at the session cancellation boundary | The CLI uses process-wide Ctrl-C handling; the server must cancel only the named session. Reuse the explicit query cancel token, inventory the protocol dependency tree, and document which signal hooks remain CLI-only. Keep the standard signal crate unless the inventory supplies evidence for a change. | Cancel/disconnect under contention leaves the other sessions running; unsafe inventory and the card's reliability mutants. |
| R15 sanitizer depth safety | taken in: bounded stack-budget diagnosis and fix | Untrusted protocol SQL traverses the same parser/executor. Resolve the instrumented depth failure before exposing that path to sessions; keep the documented depth limit and defer any broad evaluator rewrite to a separate decision. If it exceeds the improvement budget, record that limit explicitly rather than expanding this slice silently. | Maximum-depth tests under ASan/TSan, ordinary tests on both CPUs, and the existing depth mutant. |
| R16 SQL RSS bound | taken in: diagnose and repair the narrow-projection RSS gap within the existing bound | Per-session memory and slow-client backpressure use this exact executor/output path; multiplying an unexplained per-query RSS gap across 64 sessions increases risk. Profile the gap and keep an allocation/accounting fix only when it restores the existing bound; no larger K/allowance. R15 and R16 together own the carried-in budget; a broader rewrite needs a new decision. | Reproduce the SF10 RSS failure, show the fixed code passes both CPUs with default limits, and retain the unbounded-window mutant. |
| R6b next arrow-rs release | deferred, except a dependency upgrade needed by the selected fixes | R15/R16 take priority within the improvement budget. Keep the current panic catch; upgrade only if needed by those fixes and after checking the upstream release contains the fix. `cargo search parquet --limit 1` returned 60.1.0 at closeout (2026-11-06 19:28 UTC); re-check during this slice. | Workspace, damaged-file fuzz and differential suites on both architectures; record release and upstream fix. |
| R12 shared strict kernels; R17 nearest-double conversion; P11 vectorization | deferred to slice 5 | R17 is a Miri-exposed portability failure in the existing conversion fast path; native differential tests pass on both CPUs. Reworking arithmetic/conversions and proving kernels stays with slice 5, while this slice takes the session/stack/RSS risks within its quarter budget. Preserve the R17 failure and nearest-double contract. | Existing differential suite remains gating. |

## 6. Closeout (2026-11-07)

- Results: `bench/results/20261107-pgwire-close.md` is the consolidated per-target
  verdict, CPU measurements, client/version matrix, raw-evidence index and cloud
  accounting/cleanup record. Earlier stages remain in `20261107-pgwire-baseline.md`,
  `20261107-pgwire-server.md`, `20261107-pgwire-extended.md` and
  `20261107-pgwire-reliability.md`; their local-only limits remain explicit.
- ADR written/amended: ADR 0008 accepted 2026-11-07; dated step-2/3/4 amendments
  and final step-5/item-35 amendment record implementation, format/preparation
  extensions, fixed shares, known differences, confidence and revisit triggers.
- Regression benchmark: `crates/nata-bench/pgwire-bench.py`, clean-VM
  `pgwirebench` job; five settled alternating primary/twin rounds. Baselines:
  `crates/nata-bench/baselines/pgwire-arm64.csv` and `pgwire-x86.csv`, exact
  `metric,median_secs` values in those files and the closeout table. Existing
  `regress.sh` retains its 20% tolerance; paired and planted-gate outputs retained.
  Eight-worker CLI/raw/psql baseline seconds: ARM64 0.039099403 / 2.489231828 /
  2.407296157; x86-64 0.050884172 / 3.628683744 / 2.902415501.
- Deviations: step 4 extended the original unnamed/text-only goal with bounded
  named preparation and mapped scalar binary input/results, under ADR 0008
  amendments, because default driver preparation otherwise failed. COPY, extra
  SQL types and suspended cursors were not added. Streaming overhead exceeds
  the fixed 20% ceiling (NOT MET, P14).
  PostgreSQL/client text spellings differ from CLI CSV; exact differences retained,
  typed values match. Default ADBC is blocked by its catalog bootstrap (R18),
  without adding catalog/COPY support. SQL PREPARE and suspended/SQL cursors are
  explicitly unsupported; default driver wire preparation works. Ordinary type
  probes do not certify exceptional Date32 domains (R19). ARM64 replay passes
  release/2,000 lifecycle/10,000 live-fuzz checks but its mutation baseline cannot
  choose cargo because the private copy lacked the toolchain pin; fixed and
  mutation-checked locally, full ARM64 sweep NOT MET (R20), no third VM after the
  permitted retry. R15's local stack repair is retained; full sanitizer
  certification and R16's original SF10 RSS repair exceed the bounded carried-in
  work and remain open. No budget, SQL limit or memory allowance is relaxed.
- Spike code: **kept**, `spikes/pgwire-bakeoff` outside workspace members, because
  permanent production tests replace the hand-codec checks but do not reproduce
  pgwire's reuse-candidate panic/resource behavior. ADR 0008's released-fix
  revisit rule still needs that independent candidate/control. Historical results
  stay even when a future reuse recheck permits deleting this code.
- Improvement register: R13 bounded session/signal audit done (standard CLI
  dependency retained); R15/R16 remain explicit certification/resource deviations.
  P14/P15 and R18/R19/R20 record new findings and their next work. R6b/R12/R17,
  P11/P12/P13 and R14 stay open without scope expansion.
- Plan item 35: **measured on clean ARM64 and x86-64**, all five clients installed
  and versioned; psql/pgjdbc/psycopg/pgcli ordinary values and supported default
  behavior checked, exact text differences listed; ADBC default connection and
  dependent operations fail/block explicitly. This is completed compatibility
  characterization with a NOT MET ADBC target, not universal client support.
- Local gates: fmt, workspace clippy with warnings denied, workspace tests on
  disk TMPDIR, cargo-deny, scan/SQL/differences nata-check with AI_AGENT unset,
  job-chain canary, toolchain-control mutant and leak scans; exact final outcomes
  and limitations are in the closeout. No push.


### Re-close amendment, 2026-11-07 (reopened fix step)

[Fix evidence](../../bench/results/20261107-pgwire-fixes.md) supersedes the affected
step-5 verdicts without removing the old measurements. P14's per-row bridge is
replaced by two at-most-64-KiB buffers (128 KiB queued); preserve backpressure,
first-row-before-producer-end, cancellation/disconnect, exact text/binary rows,
and two-second stalled-buffer writes. NODELAY plus bounded response coalescing
repairs P15's delayed-ACK floor, with paired one-change controls and passing
20% twin regression gates / failing planted controls. Regression baselines are
updated on both CPUs. ADR 0008 and serve notes record byte bounds, metadata cap,
flush points and exact read-only ADBC bootstrap, with COPY explicitly deferred.

Default ADBC now connects; every default corpus fetch reaches COPY and fails
0A000. Its documented use_copy=false path matches 221/221 typed corpus values
and fetches the ten-type/NULL Arrow fixture on both CPUs; numeric is an opaque
exact-string Arrow extension. Ordinary client versions/default probes remain
recorded. Fractional DECIMAL leading zeros are fixed. Final clean ARM64 measured tree catches 51/51 mutants with zero INVALID/survivors, and passes 2,000 lifecycle/10,000 hostile cases (R20 done).

**Remaining deviations, unchanged thresholds:** final **Rust raw/CLI 3.416–4.697x** still exceeds ≤1.20x (P14); matched
PostgreSQL Rust/psql reference arms and on-VM perf/Callgrind profiles isolate
client context from remaining per-row encoding cost. Psql Nata/PostgreSQL
1.270–1.453x is context only, with no re-threshold. First-row and ≤2x concurrency
pass. Literal text and universal default-client targets remain NOT MET: known
client/CLI spellings, finite-float server spelling gap (R21), default ADBC COPY
(R18), SQL PREPARE/suspended/SQL cursors, exceptional dates (R19), R15 full
sanitizers and R16's original SF10 RSS remain. No threshold, SQL limit or executor
memory allowance is relaxed. Full details, failures/retries, source evidence,
cloud cleanup and local gates are in the fix record. No push.

Final measured code-only Git tree: `ad89453261a3b894039ed4280f7d95a56a100b99`,
with source-blob manifest committed under crates/nata-bench. Final results use a
Rust raw reader; earlier Python measurements remain historical evidence. The
partial-wide-row cancellation guard closes unfinished frames instead of appending
an error; its known-bad mutant is caught on both tested architectures. Both final
20% regression twins pass and planted controls fail. PostgreSQL is the local
reference only, never a replacement for the fixed CLI denominator.


### Re-close amendment, 2026-11-07 (second reopened fix)

**Re-closed with the measured deviations below, 2026-11-07.**

[Second fix evidence](../../bench/results/20261107-pgwire-fixes2.md) records the
narrow read-only COPY binary wrapper, default ADBC Arrow fetch and the independent
PostgreSQL byte oracle (R18), plus PostgreSQL float text layout, rounding endpoints
and ties with a seeded float4/float8 oracle (R21). The DataRow byte bridge,
admission, deadline, cancellation and framing guard carry COPY too. Extra float
digits below 1 now return an explicit unsupported error. CLI CSV stays unchanged.
ADR 0008's dated amendment records the scope, alternatives and limits. Both-CPU
item35, COPY benchmark arms and ARM64 reliability/mutations (60/60 valid mutants
CAUGHT, 2,000 lifecycle / 10,000 hostile cases PASS) are in that record,
along with aborted attempts, costs and explicit cleanup.

**Remaining deviations:** literal text equality to CLI remains NOT MET for
intentional PG type and client display differences; the fixed ≤1.20x streaming
threshold remains NOT MET. Real PostgreSQL itself measured approximately
2.3–2.6x CLI CSV with eight workers in the first fix record, so this target was below the observed
protocol floor. This is context, not a threshold change. P16 requests a fair,
fixed next-client-protocol-card target against PostgreSQL with the same client,
and closing the remaining roughly 1.3–1.5x matched-client gap. P14 retains the
encoding work. SQL PREPARE/SQL and suspended cursors, exceptional Date32 (R19),
R15 full sanitizers and R16 original SF10 RSS remain open. No Flight SQL,
general catalogs, TLS/auth or general COPY is claimed. No push.

Final second-fix source tree: `c11a35ef897ea9135f5fc6da668740aa45feb54b`,
measured on both final item35 VMs; production Rust is identical to the benchmark/
pgwirecheck tree recorded in the results. Default ADBC now matches **221/221 typed
Arrow row sets on both CPUs**, with no driver options. The harness explicitly
trims trailing SQL whitespace so ADBC 1.13.0's own semicolon stripping generates
valid PostgreSQL COPY. The earlier **220/221** cloud results and real PostgreSQL
42601 reproduction are retained (R22 upstream behavior), not hidden as a pass.
Original four clients retain 221/221 typed values; PG-correct float layout means
psql/JDBC/psycopg exact CLI text is now 141/221, still NOT MET. Final raw/CLI
3.661–4.407x remains NOT MET; COPY/PG COPY 2.386–2.581x is a new P14 encoding gap.
