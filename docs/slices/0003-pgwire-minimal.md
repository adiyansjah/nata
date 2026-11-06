# 0003: A minimal Postgres-wire server for slice 4's queries ("4b"), and real-client compatibility (plan item 35)

Status: draft (by the assistant, 2026-11-06; accepted when slice 4 closes: it depends on slice 4's SQL path)
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

Decision it needs: reuse the `pgwire` crate or write the protocol (rule R1 below, a bake-off).

## 2. Quality targets

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | Every query of slice 4's test set returns, through each client, the same rows (as text) as `nata sql --csv` and as DuckDB; errors arrive as `ErrorResponse` with the right SQLSTATE class and position. Mutants (each caught): a wrong type OID, a NULL sent as an empty string, a lost `CommandComplete`, rows of one query sent to another session, an extended-protocol `Sync` that does not end the error state. | The item 35 job: real clients against a real server on a clean VM; a protocol-level test with recorded message sequences | `nata sql` (slice 4) and DuckDB |
| Performance | Result streaming adds at most 20% to `nata sql --csv` for 1 million rows over loopback, and the first row arrives before the scan ends | Paired benchmark, `psql` and a raw-protocol client, both CPU types | `nata sql --csv` |
| Scalability | 64 concurrent connections each running a small query; throughput within 2x of 64 sequential runs on 8 vCPUs | Load test with a fixed seed | - |
| Reliability | A client that disconnects mid-result, a cancel request, a killed client: the query stops, its tasks end, memory and handles are released, other sessions are not affected | The slice 3 leak counters across sessions; scenarios with disconnect and cancel at random points; under CPU contention | slice 3 cancel |
| Fault tolerance | A malformed or truncated protocol message is an `ErrorResponse` and a closed connection, never a panic or a hang; a message larger than a stated limit is refused before allocating | A fuzz of the protocol decoder (10,000 cases) and a test per limit | - |
| Resource bounds | A stated maximum of connections; memory per session bounded by the scan's window; a slow client exerts backpressure (the server does not buffer a whole result) | Tests with a client that reads slowly; peak RSS | slice 3 bounds |
| Portability | ARM64 and x86-64, clean VMs | `orchestrate.sh` jobs on both | - |
| Compatibility | Real clients, each with its version recorded: `psql`, pgjdbc (JDBC), psycopg 3, the ADBC PostgreSQL driver, and one BI-style client (DBeaver's driver is pgjdbc; `pgcli`). Known differences listed. | The item 35 job (one VM, about 1 hour, about $0.12) | - |
| Security and privacy | Loopback only in this slice (refuses other addresses unless an explicit flag, documented as unsafe until TLS and auth exist); errors carry no cell values; no secrets in logs | Tests; leak scan | - |
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
with `psql` and pgjdbc against both; local; result file `bench/results/<date>-pgwire-baseline.md`.

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

## 6. Closeout (fill in when done)

- Results file:
- ADR updated: ADR 0008 written
- Benchmark added to the regression set, baseline numbers:
- Deviations from this card, and why:
- Spike code deleted:
- Improvement register updated:
- Plan item 35 status:
