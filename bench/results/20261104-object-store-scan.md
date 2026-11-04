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
