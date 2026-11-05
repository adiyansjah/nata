# The object-store read path with real latency (verification-plan item 27, a gate)

Date 2026-11-04. Spike: `spikes/object-store-scan/` (its own Cargo workspace). Written by a delegated agent (Sonnet 5.5).

**Status: BUILT AND COMPILE-CHECKED ONLY. NOTHING HAS BEEN RUN.** The owner's rule is that every run happens on GCP, and this
shared box was used only for `cargo check --tests --examples --bins` (and `clippy`). No test, server, benchmark or measurement
has been executed, so there is no result in this file. Everything under "GCP results" is to be filled. Section 9 lists what
could not be checked.

ADR 0002 rests on bridging async I/O (tokio) into the CPU pool; the bridge was tested against a mock store that only sleeps
(`20261104-async-bridge.md`). This spike puts a real S3-API client (`object_store`), real Parquet decoding (`parquet`), real
HTTP connections and a latency distribution with a heavy tail under the same bridge, and measures what the ADR's "Not covered"
lists: a real object store, real network wakeups and the tail.

## 1. What was built

| file | what |
|---|---|
| `src/{api,batch}.rs` (by `#[path]` from `spikes/operator-trait`), `src/{chan,sched,sync}.rs` (by `#[path]` from `spikes/async-bridge`) | the operator protocol, the bridge channel with the waker slot, the scheduler snapshot the bridge was tested with. Nothing copied; `gcp-check.sh` checks with `cmp` that `operator-trait`'s `api.rs`/`batch.rs` are still byte-identical to the snapshot in `async-bridge`. |
| `src/sim.rs` | the SIMULATED store: an HTTP/1.1 server speaking path-style S3 `GET`/`HEAD` with `Range`, serving a directory. First-byte latency from a heavy-tailed distribution (log-normal body, Pareto tail), a per-connection cost per MiB, a global bandwidth cap, random failures (5xx, reset, hang) and SCRIPTED failures ("the first K attempts of every request matching a path fail, or all of them"), counters and a per-request log, a control endpoint. Started by `gcp-check.sh` as a separate process. |
| `src/store.rs` | builds the client for either store through the same `object_store` S3-API client; the real store's endpoint, bucket, prefix and keys come ONLY from environment variables; `Redactor` removes them from every message. |
| `src/fetch.rs` | the one place where every request is made, counted, timed (first byte and total, client side), limited (requests in flight), retried (ours, with backoff; `object_store`'s own retries are switched off), and cancelled; memory gauges; the known-bad variants (`Mutant`). |
| `src/scan.rs` | the plan (footers via the parquet metadata reader), the fetch policies, the producers on tokio, decode on tokio or on the pool, the run of a query through the real bridge, everything measured per run. |
| `src/exec.rs` | the CPU-pool side written to the ADR 0001 operator protocol: `ScanSource` (receives from the bridge, decodes raw units when asked, converts Arrow columns to the protocol's `i64` batches), a filter operator, an aggregate / `LIMIT` sink with a settable per-batch delay (the slow consumer); the three queries. |
| `src/scenarios.rs` | the self-test, correctness, memory-bound, cancellation and retry/error scenarios, each with its known-bad versions. |
| `src/bench.rs`, `src/duck.rs` | the interleaved throughput measurement and the DuckDB reference. |
| `src/testdata.rs` | small deterministic lineitem-like Parquet files and the reference result by plain loops, so the tests need no DuckDB (one test compares them with DuckDB when it is on `PATH`). |
| `src/main.rs` (`objscan`) | the command line used by `gcp-check.sh`. |
| `tests/scan.rs` | nine tests that run the scenarios on tiny generated data against an in-process simulated store. |
| `gcp-check.sh <results-dir> [rounds]` | the whole thing on a clean VM (section 8). |

## 2. The read path under test

```
tokio (2 threads)                                          CPU pool (std threads, the ADR 0001 scheduler)
 plan: footers of every file (suffix reads)                  ScanSource (task i) --> FilterLe --> AggSink
 producer j (4 of them): for each of its units               decodes raw units (DecodeMode::Pool) or takes
   fetch the unit's byte ranges under the policy             decoded batches (DecodeMode::Tokio), converts to i64
   DecodeMode::Tokio: parquet async stream decodes here
   DecodeMode::Pool : the raw bytes are sent as they are
   send into the bounded bridge channel j  ---------------->  recv: Item | Blocked(waker stored) | Closed
 cancel: the source's finish() closes the channel AND sets a CancelToken that the producers race against
```

* The Parquet **async reader** is used (`ParquetRecordBatchStreamBuilder` over an `AsyncFileReader`), but it is handed bytes that
  `Fetcher` has already fetched (`PreloadedReader`), so that one place counts every request and can coalesce and prefetch across
  row groups. In `DecodeMode::Pool` the synchronous reader decodes the same bytes on the pool. This is a deviation from "the async reader
  does the I/O"; it is stated here so nobody reads the numbers as `ParquetObjectReader`'s behaviour. `ParquetObjectReader` itself
  (which uses `object_store`'s `get_ranges`, 1 MiB coalescing, 10 parallel) is **not** measured; policy C copies its defaults, it is
  not the same code.
* The decode placement matters for the ADR: the brief asks for record batches crossing the bridge (`Tokio`), the ADR prescribes decode on
  the pool (`Pool`). Both exist; the main matrix uses `Tokio` (as briefed) and the arm `D-pool` is `D` with `Pool`.
* The scheduler is the snapshot the bridge was tested with, **without** the time slicing of `spikes/fairness`. The pool is not
  saturated by other work here, so fairness is not in question; it is simply not part of this spike.

## 3. Fetch policies

A policy answers four questions (`Policy` in `scan.rs`). Every policy uses the same 4 producers and the same pool, so only the fetch behaviour differs.

| policy | requests in flight (whole query) | merge ranges across a gap | row groups per unit | fetch the next unit while decoding / sending |
|---|---|---|---|---|
| **A** one request per column chunk, sequential | 1 | no | 1 | no |
| **B** N in flight | 16 | no | 1 | no |
| **C** coalesced | 16 | gap <= 1 MiB (the `object_store` default), merged request <= 64 MiB | 1 | no |
| **D** prefetch | 16 | gap <= 1 MiB | 1 | yes: the next unit of each producer is fetched by a spawned task while the current one is decoded and sent (aborted on cancel) |
| **E** (extra) | 16 | gap <= 1 MiB | 8 | no. Lets neighbouring row groups merge into one request, which per-row-group coalescing (C) cannot do when the projected columns are not adjacent. |

Arms measured in every cell (`bench.rs`; the order rotates every cell and round, so each arm takes every position over the rounds):
`A`, `B`, `B-twin` (B again: the **control twin**, whose ratio to B is the noise), `C`, `D`, `E`, `D-pool` (D with decode on the pool),
`local-B` (B reading the same files from local disk: the **CPU-bound control**, no network), and `duckdb` (the reference).

Cells: 4 layouts x 2 queries x 9 arms x `rounds` (default 5).

| layout | files | row groups (asked for) | what it stresses |
|---|---|---|---|
| `big1` | 1 | ~4 (`ROW_GROUP_SIZE 1,500,000`) | few large row groups: bandwidth and large ranges |
| `mid1` | 1 | ~49 (DuckDB's default 122,880) | the realistic middle |
| `small1` | 1 | ~300 (20,000) | many small row groups: request count and the per-request floor |
| `files100` | 100 | 1 per file (~60,000 rows) | many small files: footer reads and per-file overhead |

All four hold the same rows, generated from TPC-H SF1 `lineitem` (`scripts/gen-tpch.sh`) with DuckDB `COPY ... (FORMAT parquet, ROW_GROUP_SIZE n)`;
`objscan layouts` reads the footers and fails the run if a layout does not have the shape it is named for.
Queries: `sum` = `count(*), sum(l_orderkey), sum(l_extendedprice)` (2 non-adjacent columns), `group` = a TPC-H Q1-shaped group-by with a
date filter (5 columns, 2 one-character group keys, decimal sums), `limit1` = `SELECT l_orderkey ... LIMIT 1` (cancellation tests).

Reported per cell and arm: wall time (median, min-max), useful MiB/s (bytes the query needs / wall), requests (server count on the
simulated store; client count on the real one), MiB fetched, waste (fetched / needed - 1; footers separate), time to the first batch at the pool,
per-request client-side latency p50 / p99 (total and first byte, pooled over the rounds), peak and average requests in flight, cores used
(process CPU seconds / wall). The numbers are explained, not just reported: `avg in flight x per-request latency` against `wall` says whether a
policy is latency-bound; `cores used` says whether it is CPU-bound; the `local-B` arm gives the CPU ceiling.

## 4. The two stores

**(a) SIMULATED** (`sim.rs`, labelled SIMULATED in every output). Loopback only: no TLS, no slow start, no cross-zone path, one kernel for client and server.
It exists to make the correctness, cancellation, retry and memory-bound tests deterministic and to show how the policies behave, not what GCS does.
Profile `gcs` for the throughput run (an **assumption**, not a measurement of any store): median first byte 25 ms (log-normal, sigma 0.35), a 1 percent
tail starting at 120 ms (Pareto, shape 1.5, capped at 2 s), 12 ms per MiB per connection (~83 MiB/s), 1200 MiB/s in total. Other profiles: `zero`, `slow`, `const50`, `bw50`.

**(b) REAL** (mode b). Skipped loudly if absent: `REAL STORE NOT MEASURED` on stdout, in `SUMMARY.txt`, in `REAL-STORE-NOT-MEASURED.txt` and in `RESULTS.md`;
`NATA_REQUIRE_REAL=1` turns the skip into a failing exit status (5). Configuration only from environment variables:

| variable | meaning |
|---|---|
| `NATA_OBJSTORE_ENDPOINT` | S3-API endpoint, e.g. GCS's `https://storage.googleapis.com` (GCS speaks the S3 API for HMAC keys and for public objects) |
| `NATA_OBJSTORE_BUCKET`, `NATA_OBJSTORE_PREFIX` | bucket (same region as the VM) and the temporary test prefix; `<prefix>/<layout>/part-NNNN.parquet` |
| `NATA_OBJSTORE_ACCESS_KEY_ID`, `NATA_OBJSTORE_SECRET_ACCESS_KEY` | optional HMAC pair; without it reads are anonymous |
| `NATA_OBJSTORE_REGION` (default `auto`), `NATA_OBJSTORE_LABEL` (non-secret text for the report), `NATA_OBJSTORE_HTTP1=1`, `NATA_OBJSTORE_ALLOW_HTTP=1` | optional |

Where the values can leak and what stops it: nothing prints them; `StoreSpec`'s `Debug` hides them; every error string from the client or DuckDB goes through
`Redactor` (unit-tested, with a known-bad empty redactor that must leak); test and benchmark output is piped through `objscan redact`; DuckDB gets its secret on
**stdin**, never in an argument or a file; at the end `objscan leakscan --dir <results>` searches every file under the results directory for every configured value and
`gcp-check.sh` exits 6 if it finds one (its own self-test plants a value and must find it). Credentials over plain `http://` are refused unless
`NATA_OBJSTORE_ALLOW_HTTP=1`. Not recorded anywhere: endpoint, bucket, prefix, keys; the VM's zone and machine type are recorded with the project number stripped.

The data for the real store must already be under the prefix. `objscan upload --data <layouts dir>` (the same environment, with write credentials) puts the four layouts
there; `objscan real-check` verifies they are readable (exit 4, "REAL STORE NOT MEASURED", if not).

## 5. DuckDB as the reference (how)

Same files, same store, same machine. `INSTALL httpfs` once before the timing (needs network), then per run a NEW `duckdb` process (no cache carries over) whose stdin is
`LOAD httpfs; SET threads=<nproc>; SET s3_endpoint='<host[:port]>'; SET s3_url_style='path'; SET s3_use_ssl=<false for the simulated store, true for the real one>; SET s3_region=...;`
(plus a `CREATE SECRET (TYPE S3 ...)` only when HMAC keys are configured), `.timer on`, and the query over `read_parquet([ 's3://bucket/prefix/layout/part-NNNN.parquet', ... ])`
with the files named explicitly (so DuckDB sends no `LIST`). The time reported is the CLI's own `.timer` for that one statement: process start, extension load and the `SET`s
are excluded; DuckDB's planning (footer reads) and execution are included, as our wall time includes the footers, planning, fetch, decode and aggregation. On the simulated store the
request and byte counts for DuckDB are the SERVER's counters (reset before each run); on the real store they are not available. DuckDB's result is compared with the reference
before and during the timing (a wrong result makes the run fail). Differences that cannot be removed: DuckDB speaks HTTP/1.1 through its own client, ours negotiates HTTP/2 against
a real endpoint unless `NATA_OBJSTORE_HTTP1=1`; DuckDB parallelises over row groups with its own scheduler.

## 6. How the checks are made to be able to fail (way-of-working sections 5, 6 and 8)

Every guarded behaviour has a known-bad version in the same code (`Mutant`), and the run fails if a known-bad version is NOT caught.

| check | known-bad version | what must fail |
|---|---|---|
| harness sees an injected 50 ms first-byte latency | a store with no latency | the same check (45-90 ms total, >= 45 ms first byte) |
| harness sees a 50 MiB/s cap | an uncapped store | the same check (35-60 MiB/s) |
| harness sees a tail (20 percent at 100 ms in p99, not in p50) | no tail | the same check |
| client request count equals the server's; 8 concurrent requests are seen as >= 7 in flight | (the counters are compared with an independent count) | |
| memory: peak bytes queued in the bridge <= channels x (cap + 2) x the largest item (ADR 0002: capacity + producers + 1 per consumer; one producer per channel here); peak fetched bytes alive <= producers x (unit + prefetched unit + queued units) | **`unbounded-channel`**: capacity 2^24 | the unbounded run exceeds the bound that the bounded runs (cap 1 and 4, policies B and D, both decode modes, a consumer sleeping 2 ms per batch) stay within |
| LIMIT / cancel: no request completes more than 50 ms after the cancel, no new request beyond the race window (<= one per producer), all producers ended within 250 ms, nothing in flight, no task alive, the server's in-flight count returns to 0 | **`cancel-notices-only-on-send`** (producers see the cancel only when they next send); **`cancel-ignored`** (they keep fetching) | late completions and the quiesce time (first); requests started after the cancel (second) |
| transient 503 x2, reset mid-body, reset before the response, hang (attempt timeout), 503 on the footer: correct result, exactly K+1 attempts per faulted request, each retry waits >= half the nominal backoff, never more than 4 attempts, no task or request left | **`retry-no-backoff`** | the backoff check |
| permanent 503 on one file: clean error, no result, within 3 s, 4 attempts on the failing request | **`retry-forever`** (stopped by a watchdog after 8 s); **`swallow-error`** (the producer drops the unit and carries on: a silent partial result) | "within 3 s" and "bounded"; "fails with an error and no result" |
| permanent 403: exactly 1 attempt | **`retry-permanent-errors`** | the attempt count |
| a producer that vanishes without its `Done` marker makes the query fail (never a short result) | (unit test: the sender is dropped without `Done`) | |
| redaction and the leak scan | an empty redactor; a planted value | |
| the comparison of results | a perturbed expected result | |

The reference arm `object_store`'s own retry (4 retries) against the same transient 503 x2 is run and reported but guards nothing; the fault tests always use our retry layer with
`object_store`'s switched off (so every retry is counted).

## 7. Stated memory bound

Per consumer task (one channel, one producer): the channel holds at most `cap` items, the producer holds the one it is trying to send, the consumer holds the one it is processing:
`cap + 2` items. With `C` channels and the largest item `M` bytes (a decoded batch in `Tokio` mode, a unit's raw bytes in `Pool` mode): `C x (cap + 2) x M`. Separately, fetched bytes alive:
`C x (1 + prefetch + (Pool ? cap + 1 : 0)) x U` for the largest unit `U`. Footers are not counted (parsed at once). Not bounded by these formulas and not measured: the decoded `Vec<i64>`
batches inside the pool, the aggregation state, and `object_store`'s and hyper's own buffers.

## 8. How to run it, what it costs

Nothing here has been started. Each run needs the owner's cost approval (way-of-working, "Cloud spend").

**Orchestrator hook (not applied: `scripts/gcp/` is outside this agent's edit scope).** `job.sh` needs a job; the whole check is one resumable step (a preemption restarts it; the build cache and the data
stay on the data disk). Add before the final `case` list, and add `objscan` to the `crates|fairness-bench|...` list:

```bash
  objscan)
    # verification-plan item 27. The real store's coordinates, if any, are read from a file the owner placed on the data disk
    # (mode 600, 'export NAME=value' lines); they are never in the repo, the metadata or the results.
    run o-object-store-scan bash -c 'cd "$SRC" && { [ -f "$WORK/objstore.env" ] && set -a && . "$WORK/objstore.env" && set +a; }; spikes/object-store-scan/gcp-check.sh "$RESULTS/object-store-scan" 5 > "$RESULTS/object-store-scan-run.txt" 2>&1' ;;
```

The orchestrator starts the job as soon as the source has been uploaded, so a file placed by hand would arrive too late. For a run WITH the real store, `orchestrate.sh` needs a small
change: an `--env-file PATH` option that copies that file to `/mnt/work/objstore.env` (mode 600) over the existing ssh connection BEFORE it creates `START`, and never prints it. That is a decision for the
owner (section 10); until then a run through the orchestrator is a SIMULATED-only run and says so.

**ARM64** (`n4a-highcpu-8`, 8 vCPU, 16 GiB, Spot, us-central1; `orchestrate.sh plan` prints a worst case of **$0.244** at `MAX_VM_HOURS=2`):

```bash
MAX_VM_HOURS=2 scripts/gcp/orchestrate.sh run --job objscan --machines n4a-highcpu-8 --approved-usd 0.25 --run-id objscan-arm64
```

**x86-64** (`c3d-highcpu-8`, 8 vCPU = 4 cores x 2 threads, 16 GiB, Spot, us-central1; worst case **$0.158** at `MAX_VM_HOURS=2`):

```bash
MAX_VM_HOURS=2 scripts/gcp/orchestrate.sh run --job objscan --machines c3d-highcpu-8 --approved-usd 0.16 --run-id objscan-x86
```

(Both bounds are what `orchestrate.sh plan --job objscan` printed on 2026-11-04 from `prices.env`; recompute before a run. `plan` calls nothing.) Not on an orchestrated VM:
`spikes/object-store-scan/gcp-check.sh <results-dir> [rounds]` from the repo root of any clean Debian 12 VM with `duckdb`, `curl`, `build-essential` and network access.

**Expected duration** (a reasoned estimate, not measured): build 5-8 min (arrow, parquet, `object_store`'s TLS stack, release plus test binaries), unit tests 3-6 min, data generation 2-4 min, simulated-store
tests 4-8 min (the `retry-forever` and `cancel-ignored` known-bad versions each take several seconds by design), simulated benchmark 15-25 min (policy A on `small1` issues ~1,500 sequential requests per
query per round), real benchmark 15-25 min. Total 45-75 min with the real store, 25-45 min without. Expected VM cost 1-1.3 h x $0.0984 = **$0.10-0.13 on ARM64**, 1-1.3 h x $0.0687 = **$0.07-0.09 on x86-64**.
Disk: about 12 GB of the 30 GB data disk (cargo target ~4 GB, TPC-H and four layouts ~2.5 GB, toolchain and registry ~3 GB).

**Real-store cost estimate** (from memory of the list prices, NOT confirmed against the current price page): storage of ~1.1 GiB of Standard storage for a day is about $0.001; reads at ~$0.0004 per 1,000
requests: roughly 90,000 requests for 5 rounds (all arms, both queries, the verification passes) is about $0.04, and even ten times more is under $0.5; reads from a VM in the same region are not charged for network egress.
If the bucket is public, egress to anyone else is charged to the owner ($ per GiB at internet egress rates), which is why the access options below limit what is exposed and for how long.

## 9. What was checked, and what could not be checked without running

Checked here (this box, compile only): `cargo check --lib --bins --tests --examples` passes with no warnings from the compiler; `cargo clippy` gives style warnings only (nested `if`, a long argument list);
`bash -n gcp-check.sh` passes; the worst-case cost lines come from `orchestrate.sh plan`, which calls nothing.

**Not checked, because nothing was run. A first run on GCP should be expected to find bugs in this harness itself** (that is what the self-test, the unit tests and the known-bad versions are there to show before any number is believed):

* that the hand-written HTTP server and `object_store`'s S3 client understand each other (status lines, `Content-Range`, keep-alive, HEAD, `ETag`, `Last-Modified`), and DuckDB's `httpfs` too (it also sends `HEAD`; DuckDB 2.0's exact `SET` names were not available to check, `s3_endpoint`, `s3_url_style`, `s3_use_ssl`, `s3_region`, `CREATE SECRET (TYPE S3 ...)` and the CLI's `.timer` line are from memory);
* every threshold: 45-90 ms, 35-60 MiB/s, 250 ms to quiesce, 50 ms for a late completion, 6 ms of slack on the backoff gaps, the 3 s failure deadline; they have margin on paper and have not met a loaded machine;
* that DuckDB writes the row-group sizes it was asked for (`objscan layouts` fails the run if not), and that `rowid` ranges split `files100` evenly;
* the exact request counts per policy, the behaviour of `object_store` on a mid-body reset (its own body retry is off here), HTTP/2 against a real endpoint, GCS's S3 interoperability with HMAC keys and with anonymous reads, and the real store's errors (they are redacted before they are printed, so a first failure may be hard to read);
* the unit tests (nine in `tests/scan.rs`, plus unit tests inside the modules) have been compiled, not run; the known-bad versions have not been shown to be caught yet.

Not covered by design: writes (no PUT path), `LIST`, `ParquetObjectReader` as an arm, cross-region or cross-zone reads, TLS cost on the simulated store, many concurrent queries (the scheduler snapshot has no time slicing), a CPU-saturated pool while the I/O runs (the earlier bridge spike covers that with a mock), retry budgets shared between requests, adaptive request sizes (hedged requests, splitting very large ranges into parallel 8-16 MiB reads), Iceberg manifests.

## 10. Access to a real bucket: options and trade-offs (the owner decides before any real bucket is used)

The orchestrator creates VMs with **no service account and no scopes**, so a VM cannot read a private bucket on its own, and this spike does not change that. The TPC-H data is synthetic and non-sensitive; nothing below should expose more than a temporary test prefix.

| option | how | security | notes |
|---|---|---|---|
| **1. Anonymous reads of a public-read prefix** (recommended) | a dedicated, empty, throw-away bucket in the VM's region with uniform bucket-level access; grant `allUsers` the object-viewer role **with an IAM condition limiting it to the test prefix**; upload the four layouts; delete the bucket after the run | **no credential exists, so none can leak, be logged or be left on the VM.** What is exposed: ~1.1 GiB of synthetic data under one prefix, for the duration of the run. Cost exposure: public egress is charged to the owner (1,000 downloads would be ~$100), so keep it to hours, use a budget alert, and delete the bucket after | needs no change to the orchestrator beyond passing three non-secret strings. Anonymous reads skip request signing, so they measure a slightly cheaper path than an authenticated engine. |
| 2. HMAC key of a dedicated service account, read-only, with an IAM condition limited to the prefix | create the account and an HMAC key just before the run, delete both right after; the key goes to the VM as `NATA_OBJSTORE_*` through a mode-600 file on the data disk (the `--env-file` change above) | the data stays private; but a long-lived secret (HMAC keys do not expire) exists on a Spot VM's disk and in the owner's shell until deleted; a leak would grant read of the prefix only (the account has no other role). Never put it in instance metadata (readable by anyone with project access, and logged by `gcloud`) or in the repo | needs the `--env-file` change, an account and an IAM binding; the leak scan and redaction here are the safety net, not the design. |
| 3. V4 signed URLs, one per object | the owner signs a URL for each of the 104 objects with a short expiry (hours) | no long-lived secret; each URL is a secret until it expires and exposes exactly one object | **not supported by this spike**: it needs a URL manifest and an `http` mode (and DuckDB would need the URLs too); listing is impossible. |
| 4. Give the VM a read-only service account | create the VM with an account and scope limited to the bucket; use metadata-server tokens, no secret in any file | the best practice in general; but it changes the orchestrator's rule ("no service account, no scopes") that the owner said not to change, and `object_store`'s native GCS client would be needed (this spike speaks the S3 API) | not recommended for this run. |

**Recommendation: option 1**, on a bucket created for the run and deleted after it. If public objects are not acceptable, option 2 with the `--env-file` change. In either case the same region as the VM (us-central1 for the orchestrator's zones).

## 11. Decisions needed from the owner

1. Which access option (section 10) for a real bucket; the bucket and prefix, created and populated by the owner (`objscan upload` does the population when write credentials are supplied in the environment).
2. Whether `scripts/gcp/` may get (a) the `objscan` job block (section 8) and (b) an `--env-file` option for a real-store run. Without (b) an orchestrated run measures the SIMULATED store only.
3. The cost approval for the runs in section 8 (ARM64 ~$0.10-0.13, x86-64 ~$0.07-0.09; worst cases $0.244 and $0.158; plus about $0.05 for a real-store run).
4. Whether decode on the CPU pool (the ADR) or on the I/O runtime (as the brief reads) is the configuration the ADR evidence should be about; both are measured here (`D` and `D-pool`, and the correctness and memory-bound tests run both), but the ADR text should say which.
5. Whether the sim profile `gcs` (section 4, an assumption) is acceptable as the reference shape, or the owner has real first-byte and bandwidth figures for the target store to use instead (`objscan ctl --cmd "set median_ms 40"` etc.).

## 12. GCP results: to be filled

Run id, date, machine type and zone, `uptime` before timing, commit of the tree, per architecture.

### Tests (SIMULATED store)

| suite | ARM64 | x86-64 |
|---|---|---|
| unit tests (`tests/scan.rs` and modules) | to be filled | to be filled |
| harness self-test (latency, bandwidth, tail, counts) | to be filled | to be filled |
| correctness against DuckDB, all policies, both decode modes, 4 layouts | to be filled | to be filled |
| memory bound (peak vs bound per cap / policy / decode mode; unbounded control) | to be filled | to be filled |
| cancellation (LIMIT 1, external cancel; two known-bad versions) | to be filled | to be filled |
| retries and errors (7 cases x 2 decode modes; four known-bad versions) | to be filled | to be filled |

### Throughput and request pattern (SIMULATED, profile `gcs`)

For each of `big1`, `mid1`, `small1`, `files100` and each of `sum`, `group`: the table printed by `objscan bench` (wall, MiB/s, requests, waste, first batch, p50/p99, in flight, cores), the B / B-twin noise ratio, and an explanation of each number (latency-bound, bandwidth-bound or CPU-bound, from `avg in flight`, `cores used` and `local-B`). To be filled.

### Throughput and request pattern (REAL store)

To be filled, or: **REAL STORE NOT MEASURED** (reason).

### Tail latency

Per-request p50 / p99 client-side, simulated and real; to be filled. The earlier bridge spike left the tail beyond p99 unresolved; this run has ~1,000 requests per policy per round on `small1`, so p99 is resolvable and p99.9 only after pooling the rounds.

### Conclusion for ADR 0002

To be written after the runs: whether the bridge holds with real I/O (memory bound, cancellation, errors), which policy wins on which layout and why, and what remains uncovered.


## GCP results (2026-11-05)

Runs `objscan-arm64-r2` and `objscan-x86-r3` (job `objscan`, 5 rounds, TPC-H SF1 in four layouts). **The store is SIMULATED on loopback with the profile the agent assumed (25 ms median first byte, a 1% heavy tail from 120 ms, about 83 MiB/s per connection); no real object store was measured.** The real-store part was left out on purpose (it needs an access decision from the owner): the output says `REAL STORE NOT MEASURED`.

Three defects in the harness were found and fixed before these runs passed: the injected mid-body reset never fired on responses of 64 KiB or less (the retry test failed on both architectures); a preempted run reused the previous incarnation's `sim.addr` and sent every request to a dead port (x86-64); both are in the commit log.

**What passed on both architectures** (exit status 0): the 16 unit and integration tests, the harness self-test (an injected latency, a bandwidth cap and a tail are measured as injected; the same checks FAIL on a store without them), the leak-scan self-test, **132 of 132 correctness checks** against the reference for every policy and decode mode (and against DuckDB over the same store), the **memory bound with real I/O** (34 checks; the unbounded control exceeds it), **cancellation** (14 checks; `LIMIT 1` and an external cancel stop outstanding requests promptly; the two known-bad versions are caught), **retries and errors** (89 checks: bounded retries with backoff, permanent errors fail cleanly with nothing left behind, the four known-bad versions caught), and the benchmark (360 runs, all valid).

**Readings of the benchmark** (all SIMULATED; the twin arm `B-twin` shows that identical configurations differ by up to 1.5x in wall time at these sub-second scales, so differences smaller than that are not results):
1. **One request at a time (`A`) is 4-6x slower than 8 or 16 in flight (`B`)** on every layout with more than a few requests: small1 / sum 19.3 s against 3.3 s (ARM64) and 19.0 s against 3.4 s (x86-64). This is far outside the noise.
2. **Coalescing ranges (`C`, `D`) fetched 116-141% more bytes than needed and was not faster than plain parallel requests** in this profile, because merging reduces the number of requests in flight. Prefetch (`D`) added nothing visible.
3. **Reading several row groups per unit (`E`) was the fastest policy on the many-row-group layouts** (mid1 0.38 s against 0.97 s for `B` on ARM64; small1 1.6 s against 3.3 s) at the price of the most extra bytes (411% waste on small1). Within a factor of the noise on some cells.
4. **Decoding on the CPU pool (`D-pool`) is indistinguishable from decoding on tokio (`D`)** at this scale (mid1 0.86 s against 0.93 s on ARM64), which supports ADR 0002's choice without proving a gain.
5. **DuckDB over the same simulated store was faster than every one of our arms on mid1 and small1** (0.27 s and 0.98 s against our best 0.38 s and 1.6 s on ARM64): it fetched fewer bytes on those layouts (42% and 130% extra) and does its own request scheduling. Our scan is a spike, not tuned; this is a gap to close, not a result about the architecture.
6. The CPU ceiling (`local-B`, same files from local disk) is 0.05 s on ARM64 and about 0.19 s on x86-64 per query: with the simulated latencies the queries are I/O-bound, as intended.

Not shown: any real object store (latency, throughput, HTTP/2, tail), cross-zone reads, many concurrent queries, writes.

### ARM64, `n4a-highcpu-8` (Neoverse-N3); $0.059; the VM was preempted once and the step resumed

Test results (SIMULATED store):

```
unit tests passed: 16 tests
RESULT harness self-test: 9 checks, 0 failed; 0 known-bad versions, 0 not caught
SIMULATED store listening on 127.0.0.1:43505
RESULT correctness against the reference: 132 checks, 0 failed; 0 known-bad versions, 0 not caught
RESULT memory bound (small1): 34 checks, 0 failed; 2 known-bad versions, 0 not caught
RESULT cancellation (mid1): 14 checks, 0 failed; 4 known-bad versions, 0 not caught
RESULT retries and errors (files100): 89 checks, 0 failed; 4 known-bad versions, 0 not caught
RESULT bench SIMULATED: 360 runs, 0 not ok
REAL STORE NOT MEASURED: none of NATA_OBJSTORE_ENDPOINT, NATA_OBJSTORE_BUCKET, NATA_OBJSTORE_PREFIX is set. The throughput and tail-latency numbers of this run are SIMULATED only.
```

Benchmark tables (SIMULATED store; `A` one request at a time, `B` 16 in flight, `C` coalesced ranges, `D` coalesced plus prefetch, `E` eight row groups per unit, `D-pool` decode on the CPU pool, `local-B` the same files from local disk, `duckdb` the reference over the same store):

## big1 / sum (1 file, a few large row groups (~1.5M rows each))

Bytes the query needs (projected column chunks): 38.4 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| A | 1.108 (1.040-1.233) | 35 | 9 | 38.4 | 0 | 0.329 | 88 / 210 | 24 / 57 | 1 | 0.9 | 0.3 |
| B | 0.291 (0.263-0.383) | 132 | 9 | 38.4 | 0 | 0.222 | 105 / 220 | 27 / 60 | 8 | 3.5 | 1.1 |
| B-twin | 0.285 (0.272-0.326) | 135 | 9 | 38.4 | 0 | 0.211 | 103 / 205 | 27 / 60 | 8 | 3.5 | 1.0 |
| C | 0.274 (0.260-0.327) | 140 | 9 | 38.4 | 0 | 0.201 | 87 / 194 | 25 / 41 | 8 | 3.4 | 1.1 |
| D | 0.323 (0.284-0.438) | 119 | 9 | 38.4 | 0 | 0.215 | 82 / 348 | 25 / 208 | 8 | 3.2 | 1.0 |
| E | 0.466 (0.461-0.562) | 82 | 9 | 38.4 | 0 | 0.237 | 93 / 191 | 27 / 50 | 8 | 2.1 | 0.6 |
| D-pool | 0.285 (0.265-0.320) | 135 | 9 | 38.4 | 0 | 0.211 | 98 / 209 | 26 / 56 | 8 | 3.4 | 1.0 |
| local-B | 0.087 (0.085-0.101) | 441 | 9 | 38.4 | 0 | 0.018 | 2 / 22 | 0 / 1 | 8 | 0.5 | 3.4 |
| duckdb | 0.196 (0.164-0.292) | 196 | 26 | 38.7 | 1 | NaN | n/a | n/a | 0 | NaN | NaN |

Noise: median wall of B / B-twin = 1.021 (the same configuration twice; differences between arms smaller than this are not results).

## big1 / group (1 file, a few large row groups (~1.5M rows each))

Bytes the query needs (projected column chunks): 44.6 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| B | 0.342 (0.305-0.527) | 130 | 21 | 44.6 | 0 | 0.210 | 51 / 195 | 26 / 125 | 16 | 4.3 | 1.7 |
| B-twin | 0.357 (0.306-0.401) | 125 | 21 | 44.6 | 0 | 0.216 | 57 / 199 | 26 / 73 | 16 | 4.3 | 1.7 |
| C | 0.369 (0.327-0.532) | 121 | 9 | 44.6 | 0 | 0.223 | 95 / 381 | 25 / 198 | 8 | 3.0 | 1.5 |
| D | 0.351 (0.343-0.487) | 127 | 9 | 44.6 | 0 | 0.232 | 173 / 321 | 28 / 263 | 8 | 3.2 | 1.4 |
| E | 0.660 (0.626-0.832) | 68 | 9 | 44.6 | 0 | 0.260 | 173 / 247 | 29 / 195 | 8 | 1.7 | 0.8 |
| D-pool | 0.358 (0.334-0.451) | 125 | 9 | 44.6 | 0 | 0.232 | 120 / 250 | 29 / 66 | 8 | 3.2 | 1.4 |
| local-B | 0.124 (0.120-0.166) | 359 | 21 | 44.6 | 0 | 0.013 | 1 / 15 | 0 / 1 | 16 | 0.4 | 4.2 |
| duckdb | 0.211 (0.203-0.402) | 211 | 34 | 44.9 | 1 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 1.625 (1.534-2.572) | 27 | 21 | 44.6 | 0 | 0.449 | 50 / 201 | 27 / 53 | 1 | 0.9 | 0.3 |

Noise: median wall of B / B-twin = 0.958 (the same configuration twice; differences between arms smaller than this are not results).

## mid1 / sum (1 file, DuckDB's default row group size (122,880 rows))

Bytes the query needs (projected column chunks): 38.4 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| B-twin | 0.651 (0.635-0.958) | 59 | 100 | 38.4 | 0 | 0.100 | 35 / 68 | 27 / 55 | 8 | 5.3 | 0.4 |
| C | 0.884 (0.881-2.864) | 43 | 51 | 82.9 | 116 | 0.126 | 60 / 88 | 27 / 52 | 4 | 3.2 | 0.4 |
| D | 0.932 (0.856-1.006) | 41 | 51 | 82.9 | 116 | 0.122 | 60 / 106 | 26 / 74 | 4 | 3.2 | 0.4 |
| E | 0.378 (0.301-0.725) | 102 | 51 | 82.9 | 116 | 0.130 | 63 / 103 | 25 / 61 | 16 | 8.7 | 0.9 |
| D-pool | 0.862 (0.841-1.307) | 45 | 51 | 82.9 | 116 | 0.115 | 61 / 121 | 27 / 88 | 4 | 3.4 | 0.3 |
| local-B | 0.054 (0.054-0.065) | 707 | 100 | 38.4 | 0 | 0.002 | 0 / 2 | 0 / 1 | 8 | 0.5 | 5.0 |
| duckdb | 0.269 (0.204-0.516) | 143 | 83 | 54.5 | 42 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 3.908 (3.676-4.344) | 10 | 100 | 38.4 | 0 | 0.179 | 36 / 163 | 27 / 149 | 1 | 1.0 | 0.1 |
| B | 0.966 (0.687-1.541) | 40 | 100 | 38.4 | 0 | 0.095 | 35 / 188 | 26 / 177 | 8 | 4.2 | 0.3 |

Noise: median wall of B / B-twin = 1.483 (the same configuration twice; differences between arms smaller than this are not results).

## mid1 / group (1 file, DuckDB's default row group size (122,880 rows))

Bytes the query needs (projected column chunks): 45.0 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| C | 0.792 (0.757-0.854) | 57 | 51 | 50.8 | 13 | 0.106 | 48 / 86 | 26 / 64 | 4 | 3.1 | 0.7 |
| D | 0.863 (0.723-1.200) | 52 | 51 | 50.8 | 13 | 0.102 | 49 / 180 | 28 / 156 | 4 | 3.0 | 0.7 |
| E | 0.330 (0.312-0.393) | 137 | 51 | 50.8 | 13 | 0.120 | 50 / 78 | 27 / 55 | 16 | 7.5 | 1.7 |
| D-pool | 0.730 (0.663-0.889) | 62 | 51 | 50.8 | 13 | 0.110 | 47 / 91 | 26 / 71 | 4 | 3.3 | 0.7 |
| local-B | 0.106 (0.101-0.131) | 424 | 247 | 45.0 | 0 | 0.002 | 0 / 2 | 0 / 1 | 16 | 0.6 | 5.0 |
| duckdb | 0.239 (0.208-0.322) | 188 | 98 | 47.7 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 8.747 (8.188-8.834) | 5 | 247 | 45.0 | 0 | 0.251 | 32 / 72 | 27 / 65 | 1 | 1.0 | 0.1 |
| B | 0.888 (0.835-1.728) | 51 | 247 | 45.0 | 0 | 0.109 | 31 / 151 | 26 / 148 | 16 | 9.6 | 0.6 |
| B-twin | 0.937 (0.746-1.164) | 48 | 247 | 45.0 | 0 | 0.116 | 32 / 74 | 26 / 68 | 16 | 8.9 | 0.6 |

Noise: median wall of B / B-twin = 0.947 (the same configuration twice; differences between arms smaller than this are not results).

## small1 / sum (1 file, many small row groups (20,000 rows each))

Bytes the query needs (projected column chunks): 38.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| D | 3.329 (2.814-4.758) | 12 | 296 | 92.6 | 141 | 0.101 | 35 / 79 | 27 / 71 | 4 | 3.4 | 0.1 |
| E | 1.613 (1.389-1.752) | 24 | 39 | 196.7 | 411 | 0.194 | 131 / 480 | 25 / 376 | 4 | 3.3 | 0.2 |
| D-pool | 3.477 (2.896-4.839) | 11 | 296 | 92.6 | 141 | 0.103 | 34 / 81 | 27 / 73 | 4 | 3.3 | 0.1 |
| local-B | 0.056 (0.053-0.080) | 683 | 590 | 38.5 | 0 | 0.003 | 0 / 2 | 0 / 1 | 8 | 2.3 | 5.3 |
| duckdb | 0.983 (0.643-2.341) | 39 | 329 | 88.4 | 130 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 19.342 (18.053-22.771) | 2 | 590 | 38.5 | 0 | 0.143 | 30 / 131 | 27 / 129 | 1 | 1.0 | 0.0 |
| B | 3.334 (3.286-4.545) | 12 | 590 | 38.5 | 0 | 0.084 | 29 / 135 | 27 / 131 | 8 | 5.7 | 0.1 |
| B-twin | 3.445 (2.870-3.666) | 11 | 590 | 38.5 | 0 | 0.114 | 29 / 126 | 26 / 123 | 8 | 5.6 | 0.1 |
| C | 3.466 (3.289-4.752) | 11 | 296 | 92.6 | 141 | 0.102 | 34 / 208 | 27 / 199 | 4 | 3.2 | 0.1 |

Noise: median wall of B / B-twin = 0.968 (the same configuration twice; differences between arms smaller than this are not results).

## small1 / group (1 file, many small row groups (20,000 rows each))

Bytes the query needs (projected column chunks): 47.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| E | 1.490 (1.374-1.839) | 32 | 39 | 191.7 | 303 | 0.198 | 129 / 179 | 27 / 65 | 4 | 3.3 | 0.4 |
| D-pool | 2.815 (2.711-3.043) | 17 | 296 | 53.4 | 12 | 0.095 | 31 / 137 | 26 / 133 | 4 | 3.6 | 0.2 |
| local-B | 0.115 (0.108-0.175) | 413 | 1472 | 47.5 | 0 | 0.004 | 0 / 2 | 0 / 1 | 16 | 3.3 | 5.4 |
| duckdb | 0.779 (0.543-1.298) | 61 | 329 | 54.5 | 15 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 48.962 (46.474-53.818) | 1 | 1472 | 47.5 | 0 | 0.245 | 29 / 76 | 27 / 74 | 1 | 1.0 | 0.0 |
| B | 4.569 (4.282-5.574) | 10 | 1472 | 47.5 | 0 | 0.110 | 28 / 78 | 27 / 75 | 16 | 10.1 | 0.2 |
| B-twin | 5.551 (4.233-5.772) | 9 | 1472 | 47.5 | 0 | 0.121 | 28 / 128 | 27 / 126 | 16 | 9.1 | 0.2 |
| C | 3.227 (2.523-4.355) | 15 | 296 | 53.4 | 12 | 0.094 | 31 / 159 | 27 / 155 | 4 | 3.4 | 0.2 |
| D | 3.022 (2.634-4.468) | 16 | 296 | 53.4 | 12 | 0.095 | 31 / 131 | 27 / 126 | 4 | 3.4 | 0.2 |

Noise: median wall of B / B-twin = 0.823 (the same configuration twice; differences between arms smaller than this are not results).

## files100 / sum (100 small files, one row group each (~60,000 rows))

Bytes the query needs (projected column chunks): 38.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| D-pool | 1.710 (1.518-3.582) | 22 | 200 | 85.0 | 121 | 0.473 | 44 / 94 | 26 / 79 | 16 | 2.7 | 0.2 |
| local-B | 0.065 (0.059-0.093) | 594 | 300 | 38.5 | 0 | 0.005 | 0 / 2 | 0 / 2 | 16 | 1.0 | 4.8 |
| duckdb | 0.818 (0.688-1.423) | 47 | 400 | 40.7 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 10.484 (10.002-11.937) | 4 | 300 | 38.5 | 0 | 3.302 | 32 / 130 | 27 / 126 | 1 | 0.7 | 0.0 |
| B | 2.386 (1.636-3.748) | 16 | 300 | 38.5 | 0 | 0.543 | 31 / 148 | 26 / 141 | 16 | 2.9 | 0.2 |
| B-twin | 1.484 (1.426-2.182) | 26 | 300 | 38.5 | 0 | 0.326 | 32 / 134 | 27 / 131 | 16 | 4.6 | 0.2 |
| C | 1.933 (1.403-2.841) | 20 | 200 | 85.0 | 121 | 0.437 | 45 / 201 | 27 / 183 | 16 | 2.6 | 0.2 |
| D | 1.729 (1.514-1.970) | 22 | 200 | 85.0 | 121 | 0.362 | 44 / 82 | 26 / 63 | 16 | 2.8 | 0.2 |
| E | 1.566 (1.409-2.212) | 25 | 200 | 85.0 | 121 | 0.321 | 44 / 99 | 26 / 81 | 16 | 3.1 | 0.2 |

Noise: median wall of B / B-twin = 1.607 (the same configuration twice; differences between arms smaller than this are not results).

## files100 / group (100 small files, one row group each (~60,000 rows))

Bytes the query needs (projected column chunks): 45.6 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| local-B | 0.107 (0.104-0.135) | 427 | 600 | 45.6 | 0 | 0.005 | 0 / 3 | 0 / 2 | 16 | 1.6 | 5.0 |
| duckdb | 0.713 (0.600-0.980) | 64 | 400 | 48.2 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 19.296 (18.869-20.220) | 2 | 600 | 45.6 | 0 | 3.318 | 29 / 63 | 26 / 59 | 1 | 0.8 | 0.0 |
| B | 2.166 (1.774-2.220) | 21 | 600 | 45.6 | 0 | 0.430 | 29 / 85 | 26 / 84 | 16 | 7.6 | 0.3 |
| B-twin | 1.856 (1.806-3.117) | 25 | 600 | 45.6 | 0 | 0.334 | 29 / 75 | 26 / 72 | 16 | 8.7 | 0.3 |
| C | 1.913 (1.494-1.982) | 24 | 200 | 51.4 | 13 | 0.682 | 38 / 175 | 27 / 162 | 16 | 2.2 | 0.3 |
| D | 1.815 (1.360-1.848) | 25 | 200 | 51.4 | 13 | 0.351 | 37 / 249 | 26 / 236 | 16 | 2.6 | 0.3 |
| E | 1.572 (1.400-2.018) | 29 | 200 | 51.4 | 13 | 0.387 | 38 / 74 | 27 / 63 | 16 | 2.5 | 0.4 |
| D-pool | 2.177 (1.340-3.725) | 21 | 200 | 51.4 | 13 | 0.631 | 38 / 150 | 26 / 139 | 16 | 1.8 | 0.2 |

Noise: median wall of B / B-twin = 1.167 (the same configuration twice; differences between arms smaller than this are not results).

Notes: 'requests' for DuckDB and, on the simulated store, for every arm are the SERVER's count (all methods, retries included); on the real store 'requests' of our arms are the client's count and DuckDB's is not available. 'MiB/s useful' = bytes the query needs / wall. 'waste' = bytes fetched over bytes needed minus 1 (footer bytes not included; for DuckDB it is the server's bytes sent). 'avg in flight' = sum of per-request times / wall. 'cores used' = process CPU seconds / wall (ours only; includes the I/O runtime, the CPU pool and the decode).


### x86-64, `c3d-highcpu-8` (AMD EPYC 9B14, 4 cores x 2 threads); $0.030

Test results (SIMULATED store):

```
unit tests passed: 16 tests
RESULT harness self-test: 9 checks, 0 failed; 0 known-bad versions, 0 not caught
SIMULATED store listening on 127.0.0.1:36443
RESULT correctness against the reference: 132 checks, 0 failed; 0 known-bad versions, 0 not caught
RESULT memory bound (small1): 34 checks, 0 failed; 2 known-bad versions, 0 not caught
RESULT cancellation (mid1): 14 checks, 0 failed; 4 known-bad versions, 0 not caught
RESULT retries and errors (files100): 89 checks, 0 failed; 4 known-bad versions, 0 not caught
RESULT bench SIMULATED: 360 runs, 0 not ok
REAL STORE NOT MEASURED: none of NATA_OBJSTORE_ENDPOINT, NATA_OBJSTORE_BUCKET, NATA_OBJSTORE_PREFIX is set. The throughput and tail-latency numbers of this run are SIMULATED only.
```

Benchmark tables (SIMULATED store; `A` one request at a time, `B` 16 in flight, `C` coalesced ranges, `D` coalesced plus prefetch, `E` eight row groups per unit, `D-pool` decode on the CPU pool, `local-B` the same files from local disk, `duckdb` the reference over the same store):

## big1 / sum (1 file, a few large row groups (~1.5M rows each))

Bytes the query needs (projected column chunks): 38.4 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| A | 1.211 (1.186-1.226) | 32 | 9 | 38.4 | 0 | 0.346 | 78 / 181 | 28 / 40 | 1 | 0.8 | 0.6 |
| B | 0.407 (0.393-0.422) | 95 | 9 | 38.4 | 0 | 0.207 | 81 / 190 | 26 / 50 | 8 | 2.4 | 2.0 |
| B-twin | 0.409 (0.407-0.478) | 94 | 9 | 38.4 | 0 | 0.209 | 154 / 250 | 27 / 208 | 8 | 2.4 | 2.0 |
| C | 0.414 (0.409-0.714) | 93 | 9 | 38.4 | 0 | 0.213 | 154 / 484 | 28 / 442 | 8 | 2.4 | 2.0 |
| D | 0.416 (0.401-0.523) | 92 | 9 | 38.4 | 0 | 0.209 | 105 / 195 | 27 / 62 | 8 | 2.4 | 2.0 |
| E | 0.920 (0.914-0.942) | 42 | 9 | 38.4 | 0 | 0.224 | 94 / 194 | 27 / 55 | 8 | 1.0 | 0.8 |
| D-pool | 0.396 (0.392-0.407) | 97 | 9 | 38.4 | 0 | 0.206 | 83 / 181 | 26 / 42 | 8 | 2.4 | 1.9 |
| local-B | 0.221 (0.217-0.232) | 174 | 9 | 38.4 | 0 | 0.026 | 5 / 27 | 0 / 2 | 8 | 0.3 | 3.8 |
| duckdb | 0.224 (0.173-2.106) | 172 | 26 | 38.7 | 1 | NaN | n/a | n/a | 0 | NaN | NaN |

Noise: median wall of B / B-twin = 0.995 (the same configuration twice; differences between arms smaller than this are not results).

## big1 / group (1 file, a few large row groups (~1.5M rows each))

Bytes the query needs (projected column chunks): 44.6 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| B | 0.335 (0.325-0.362) | 133 | 21 | 44.6 | 0 | 0.194 | 55 / 199 | 29 / 61 | 16 | 4.4 | 1.9 |
| B-twin | 0.341 (0.333-0.380) | 131 | 21 | 44.6 | 0 | 0.197 | 48 / 195 | 24 / 50 | 16 | 4.1 | 1.8 |
| C | 0.359 (0.352-0.390) | 124 | 9 | 44.6 | 0 | 0.221 | 98 / 212 | 25 / 55 | 8 | 3.0 | 1.8 |
| D | 0.384 (0.350-0.398) | 116 | 9 | 44.6 | 0 | 0.226 | 88 / 208 | 25 / 48 | 8 | 2.8 | 1.7 |
| E | 0.652 (0.610-0.661) | 68 | 9 | 44.6 | 0 | 0.261 | 91 / 238 | 27 / 83 | 8 | 1.7 | 0.8 |
| D-pool | 0.356 (0.347-0.380) | 125 | 9 | 44.6 | 0 | 0.219 | 104 / 208 | 29 / 52 | 8 | 3.0 | 1.4 |
| local-B | 0.158 (0.151-0.166) | 282 | 21 | 44.6 | 0 | 0.019 | 3 / 23 | 0 / 2 | 16 | 0.6 | 4.2 |
| duckdb | 0.219 (0.194-0.369) | 204 | 34 | 44.9 | 1 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 1.706 (1.548-1.755) | 26 | 21 | 44.6 | 0 | 0.459 | 51 / 266 | 28 / 195 | 1 | 0.9 | 0.3 |

Noise: median wall of B / B-twin = 0.983 (the same configuration twice; differences between arms smaller than this are not results).

## mid1 / sum (1 file, DuckDB's default row group size (122,880 rows))

Bytes the query needs (projected column chunks): 38.4 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| B-twin | 0.933 (0.766-1.033) | 41 | 100 | 38.4 | 0 | 0.108 | 35 / 190 | 27 / 177 | 8 | 4.2 | 0.8 |
| C | 0.966 (0.914-1.732) | 40 | 51 | 83.0 | 116 | 0.116 | 59 / 87 | 27 / 55 | 4 | 3.1 | 0.8 |
| D | 0.878 (0.839-2.874) | 44 | 51 | 83.0 | 116 | 0.110 | 59 / 86 | 27 / 53 | 4 | 3.4 | 0.9 |
| E | 0.470 (0.460-0.548) | 82 | 51 | 83.0 | 116 | 0.142 | 63 / 108 | 26 / 74 | 16 | 6.5 | 1.7 |
| D-pool | 0.879 (0.840-1.033) | 44 | 51 | 83.0 | 116 | 0.116 | 59 / 92 | 26 / 60 | 4 | 3.4 | 0.9 |
| local-B | 0.191 (0.182-0.200) | 201 | 100 | 38.4 | 0 | 0.002 | 0 / 2 | 0 / 0 | 8 | 0.1 | 4.3 |
| duckdb | 0.498 (0.214-0.645) | 77 | 83 | 54.4 | 41 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 3.884 (3.761-4.441) | 10 | 100 | 38.4 | 0 | 0.191 | 35 / 164 | 26 / 153 | 1 | 1.0 | 0.2 |
| B | 0.708 (0.688-1.630) | 54 | 100 | 38.4 | 0 | 0.087 | 35 / 78 | 26 / 69 | 8 | 5.0 | 1.1 |

Noise: median wall of B / B-twin = 0.758 (the same configuration twice; differences between arms smaller than this are not results).

## mid1 / group (1 file, DuckDB's default row group size (122,880 rows))

Bytes the query needs (projected column chunks): 45.0 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| C | 0.733 (0.719-0.920) | 61 | 51 | 50.8 | 13 | 0.104 | 45 / 83 | 25 / 61 | 4 | 3.2 | 0.8 |
| D | 0.794 (0.680-1.333) | 57 | 51 | 50.8 | 13 | 0.085 | 47 / 93 | 27 / 75 | 4 | 3.1 | 0.7 |
| E | 0.395 (0.345-0.540) | 114 | 51 | 50.8 | 13 | 0.117 | 48 / 146 | 26 / 126 | 16 | 6.5 | 1.4 |
| D-pool | 0.722 (0.668-0.791) | 62 | 51 | 50.8 | 13 | 0.104 | 46 / 90 | 26 / 70 | 4 | 3.3 | 0.7 |
| local-B | 0.143 (0.135-0.153) | 315 | 247 | 45.0 | 0 | 0.002 | 0 / 4 | 0 / 1 | 16 | 0.6 | 4.8 |
| duckdb | 0.235 (0.213-0.259) | 192 | 98 | 47.6 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 8.373 (8.135-9.004) | 5 | 247 | 45.0 | 0 | 0.288 | 31 / 71 | 26 / 64 | 1 | 1.0 | 0.1 |
| B | 0.905 (0.788-1.674) | 50 | 247 | 45.0 | 0 | 0.097 | 31 / 151 | 26 / 142 | 16 | 9.4 | 0.6 |
| B-twin | 0.769 (0.726-1.024) | 59 | 247 | 45.0 | 0 | 0.094 | 31 / 71 | 27 / 67 | 16 | 10.5 | 0.7 |

Noise: median wall of B / B-twin = 1.177 (the same configuration twice; differences between arms smaller than this are not results).

## small1 / sum (1 file, many small row groups (20,000 rows each))

Bytes the query needs (projected column chunks): 38.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| D | 3.054 (2.720-4.821) | 13 | 296 | 92.8 | 141 | 0.105 | 34 / 78 | 26 / 70 | 4 | 3.6 | 0.3 |
| E | 1.444 (1.401-2.151) | 27 | 39 | 196.9 | 412 | 0.185 | 126 / 265 | 26 / 165 | 4 | 3.3 | 0.6 |
| D-pool | 3.111 (2.712-5.275) | 12 | 296 | 92.8 | 141 | 0.097 | 34 / 137 | 26 / 129 | 4 | 3.5 | 0.3 |
| local-B | 0.189 (0.185-0.198) | 203 | 590 | 38.5 | 0 | 0.003 | 0 / 0 | 0 / 0 | 8 | 0.3 | 4.5 |
| duckdb | 1.033 (0.668-2.415) | 37 | 329 | 88.7 | 130 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 19.030 (18.021-21.625) | 2 | 590 | 38.5 | 0 | 0.143 | 29 / 129 | 26 / 127 | 1 | 1.0 | 0.0 |
| B | 3.378 (3.327-5.003) | 11 | 590 | 38.5 | 0 | 0.112 | 29 / 78 | 26 / 76 | 8 | 5.5 | 0.3 |
| B-twin | 3.176 (2.957-4.038) | 12 | 590 | 38.5 | 0 | 0.087 | 28 / 124 | 26 / 123 | 8 | 5.7 | 0.3 |
| C | 3.269 (2.906-4.718) | 12 | 296 | 92.8 | 141 | 0.099 | 34 / 190 | 26 / 181 | 4 | 3.4 | 0.3 |

Noise: median wall of B / B-twin = 1.063 (the same configuration twice; differences between arms smaller than this are not results).

## small1 / group (1 file, many small row groups (20,000 rows each))

Bytes the query needs (projected column chunks): 47.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| E | 1.398 (1.318-1.535) | 34 | 39 | 191.9 | 304 | 0.190 | 123 / 282 | 26 / 184 | 4 | 3.4 | 0.4 |
| D-pool | 2.985 (2.680-4.229) | 16 | 296 | 53.4 | 12 | 0.094 | 31 / 155 | 27 / 152 | 4 | 3.5 | 0.2 |
| local-B | 0.148 (0.142-0.157) | 321 | 1472 | 47.5 | 0 | 0.004 | 0 / 2 | 0 / 1 | 16 | 2.7 | 5.5 |
| duckdb | 0.660 (0.619-1.362) | 72 | 329 | 54.5 | 15 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 48.560 (46.097-51.762) | 1 | 1472 | 47.5 | 0 | 0.245 | 29 / 78 | 26 / 76 | 1 | 1.0 | 0.0 |
| B | 5.044 (3.946-5.662) | 9 | 1472 | 47.5 | 0 | 0.105 | 28 / 74 | 26 / 73 | 16 | 9.2 | 0.1 |
| B-twin | 5.089 (4.236-5.958) | 9 | 1472 | 47.5 | 0 | 0.098 | 28 / 130 | 26 / 128 | 16 | 9.6 | 0.1 |
| C | 3.222 (2.565-4.559) | 15 | 296 | 53.4 | 12 | 0.111 | 30 / 159 | 26 / 155 | 4 | 3.2 | 0.2 |
| D | 2.834 (2.677-4.403) | 17 | 296 | 53.4 | 12 | 0.096 | 31 / 131 | 27 / 128 | 4 | 3.5 | 0.2 |

Noise: median wall of B / B-twin = 0.991 (the same configuration twice; differences between arms smaller than this are not results).

## files100 / sum (100 small files, one row group each (~60,000 rows))

Bytes the query needs (projected column chunks): 38.5 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| D-pool | 1.809 (1.429-2.036) | 21 | 200 | 85.0 | 121 | 0.508 | 43 / 77 | 26 / 59 | 16 | 2.6 | 0.5 |
| local-B | 0.193 (0.186-0.195) | 199 | 300 | 38.5 | 0 | 0.005 | 0 / 2 | 0 / 1 | 16 | 0.2 | 4.3 |
| duckdb | 1.198 (0.591-2.275) | 32 | 400 | 40.6 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 10.156 (9.759-11.930) | 4 | 300 | 38.5 | 0 | 3.135 | 31 / 71 | 26 / 64 | 1 | 0.7 | 0.1 |
| B | 2.251 (1.605-3.439) | 17 | 300 | 38.5 | 0 | 0.482 | 31 / 196 | 25 / 193 | 16 | 3.1 | 0.4 |
| B-twin | 1.516 (1.356-1.982) | 25 | 300 | 38.5 | 0 | 0.325 | 31 / 137 | 26 / 132 | 16 | 4.4 | 0.5 |
| C | 1.724 (1.497-2.068) | 22 | 200 | 85.0 | 121 | 0.456 | 43 / 174 | 26 / 158 | 16 | 2.8 | 0.5 |
| D | 1.677 (1.473-2.774) | 23 | 200 | 85.0 | 121 | 0.388 | 43 / 85 | 26 / 69 | 16 | 2.8 | 0.5 |
| E | 1.517 (1.428-2.026) | 25 | 200 | 85.0 | 121 | 0.298 | 43 / 84 | 26 / 67 | 16 | 3.0 | 0.5 |

Noise: median wall of B / B-twin = 1.485 (the same configuration twice; differences between arms smaller than this are not results).

## files100 / group (100 small files, one row group each (~60,000 rows))

Bytes the query needs (projected column chunks): 45.6 MiB.

| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |
|---|---|---|---|---|---|---|---|---|---|---|---|
| local-B | 0.140 (0.135-0.149) | 326 | 600 | 45.6 | 0 | 0.005 | 0 / 3 | 0 / 2 | 16 | 1.8 | 5.2 |
| duckdb | 0.706 (0.659-1.669) | 65 | 400 | 48.2 | 6 | NaN | n/a | n/a | 0 | NaN | NaN |
| A | 18.802 (18.690-19.421) | 2 | 600 | 45.6 | 0 | 3.400 | 29 / 64 | 26 / 58 | 1 | 0.8 | 0.0 |
| B | 2.163 (1.801-3.151) | 21 | 600 | 45.6 | 0 | 0.492 | 29 / 78 | 26 / 73 | 16 | 7.5 | 0.3 |
| B-twin | 2.172 (1.927-3.235) | 21 | 600 | 45.6 | 0 | 0.433 | 29 / 76 | 26 / 73 | 16 | 7.3 | 0.3 |
| C | 1.723 (1.556-1.963) | 26 | 200 | 51.4 | 13 | 0.499 | 36 / 137 | 26 / 126 | 16 | 2.4 | 0.3 |
| D | 1.733 (1.363-1.841) | 26 | 200 | 51.4 | 13 | 0.583 | 38 / 95 | 26 / 83 | 16 | 2.4 | 0.3 |
| E | 1.866 (1.317-2.204) | 24 | 200 | 51.4 | 13 | 0.615 | 37 / 67 | 26 / 57 | 16 | 2.3 | 0.3 |
| D-pool | 1.695 (1.397-2.212) | 27 | 200 | 51.4 | 13 | 0.323 | 37 / 200 | 26 / 190 | 16 | 2.5 | 0.3 |

Noise: median wall of B / B-twin = 0.996 (the same configuration twice; differences between arms smaller than this are not results).

Notes: 'requests' for DuckDB and, on the simulated store, for every arm are the SERVER's count (all methods, retries included); on the real store 'requests' of our arms are the client's count and DuckDB's is not available. 'MiB/s useful' = bytes the query needs / wall. 'waste' = bytes fetched over bytes needed minus 1 (footer bytes not included; for DuckDB it is the server's bytes sent). 'avg in flight' = sum of per-request times / wall. 'cores used' = process CPU seconds / wall (ours only; includes the I/O runtime, the CPU pool and the decode).
