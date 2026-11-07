# Card 0006 part 4, spike 1: the request-policy wrapper, the metadata cache and the runtime arm (2026-11-07)

Spike: `spikes/object-store-baseline/` (throwaway, outside the workspace; `object_store` 0.14.2 authorized for it by card 0006 rule 1, features `aws`, `gcp`, `fs`; `parquet`/`arrow` 60 as in the sibling spikes). Card: [0006](../../docs/slices/0006-object-store-io.md), ADRs [0011](../../docs/decisions/0011-object-store-io.md), [0012](../../docs/decisions/0012-metadata-cache.md), [0002](../../docs/decisions/0002-async-runtime-and-threading.md). The reference timings (DuckDB, controls, noise, scaling, proposed frozen thresholds) are in [20261107-object-store-reference.md](20261107-object-store-reference.md). Raw, redacted: `bench/results/raw/object-store-baseline/{arm64,x86-64,local-x86-dev}/`.

What was built: `remote.rs` is a wrapper around `object_store` with ADR 0011's policy (retries owned by one layer, 4 attempts, equal-jitter backoff 100 ms to 5 s, 30 s per attempt over headers and body, 120 s per operation, Retry-After honored only inside the deadline, 16 active attempts per query and 32 per node, a byte grant reserved before each request, If-Match and version pinned to the identity, response checked chunk by chunk against the requested range, cancellation at every wait). `plan.rs` merges ranges (gap 1 MiB, response 64 MiB, 25 % extra). `cache.rs` is ADR 0012's cache (key with identity and scope, charged parsed bytes, LRU among unpinned entries, pinned bytes stay charged, bypass over cap). `fake.rs` is a hostile S3-style endpoint over raw sockets, scheduled by request ordinal or by seed.

Where it ran. Fault suites: the fake endpoint on `n4a-highcpu-8` (ARM64, Neoverse-N3), `c3d-highcpu-8` (x86-64, AMD EPYC 9B14) and the x86-64 development VM; same results on all three. Real store: a real GCS bucket (us-central1, same region as the VMs), native API, the VM's service account; the S3 path was exercised only against the fake endpoint. **Real S3 and MinIO were not available**: MinIO's download URL returns 410 Gone, and a container pull was not done because the development VM's root disk had 1.1 GB free (shared with other sessions' work; builds and scratch data for this spike lived on `/dev/shm`).

## Verdicts

**1. Decision rule 1 (reuse first), as written:** "propose `object_store`, not a new HTTP/signing client (vision). It passes only if bounded streaming, conditional/versioned range reads, both auth chains, cancel and dependency checks work behind our wrapper. If not, stop and propose an alternative with a narrow reuse/custom bake-off; no automatic custom client." Part 4 adds: "If the library allocates from hostile lengths before our bound, rule 1 fails; `catch_unwind` is insufficient."

| condition | result | evidence |
|---|---|---|
| bounded streaming | met | a 206 whose Content-Length claims 1 TiB and streams zeros: error `BadLength` after the requested length plus one chunk, 1 attempt, process alive under `ulimit -v` 1536 MiB; mutant without the length check dies under the same limit |
| conditional / versioned range reads | met on the fake endpoint; success path only on real GCS | 412 on a changed ETag, response ETag or length mismatch caught when the endpoint ignores If-Match; on real GCS every ranged GET of the 2,296 timed runs per CPU carried the identity the store returned (ETag and/or generation; which of the two GCS returned was not logged) and was accepted. A real replace-during-scan on GCS was not run |
| both auth chains | GCS metadata-server chain met; S3 chain **NOT MEASURED** | all real-store runs authenticated through the VM's metadata server; the fake S3 endpoint is anonymous and does not verify SigV4; no MinIO |
| cancel | met | cancel during a stalled body, a backoff sleep and a permit wait: `Cancelled` with quiescence under 1 ms measured, in-flight gauge back to 0, another query unaffected |
| dependency checks (`cargo deny`, unsafe inventory) | **NOT RUN** | the spike is outside the workspace; they belong to the walking skeleton |
| no allocation from hostile lengths before our bound | **met only with our transport bound** | see the finding below |

**Finding (relevant to rule 1).** `object_store` 0.14.2's default transport buffers the whole body of an error response (client/retry.rs reads it with `text()` to put it in the error). A 503 with an endless body killed the process under `ulimit -v 1536 MiB` (mutant `no_transport_bound`, exit without result). The wrapper installs an `HttpConnector` (a supported extension point, no fork) that keeps at most 4 KiB of a non-success body, drops the rest unread, and leaves only a status marker and a valid Retry-After; with it the same case ends in `Server5xx` after 4 attempts in about 0.1 s. Success bodies are still streamed and are bounded by our chunk checks, but bodies the library itself reads from success responses (credential JSON, LIST and multipart XML) pass through this hook unbounded and were **not probed**. Headers: a 200 MiB single header line ends in a bounded error in 40 to 270 ms (the HTTP stack rejects it).

**Verdict by the rule: not passed yet, and not failed.** Four of six conditions are met within what was probed; the S3 auth chain and the dependency checks are open; the allocation condition holds only because the wrapper bounds the transport. No condition failed, so the rule's stop trigger (propose an alternative) is not pulled. Proposed gate for the walking skeleton: S3 chain against MinIO or real S3, `cargo deny` plus unsafe inventory, and a hostile-length probe of every library path that reads a success body.

Reproduce: `cd spikes/object-store-baseline && cargo build --release --locked && target/release/osb hostile-all` and `target/release/osb hostile-all --mutant no_transport_bound`.

**2. Fault tolerance row (card part 2), as written:** "ADR 0011: bounded retries for throttling/5xx/transport timeout/truncation; permanent auth/missing/invalid-range errors fail directly; changed identity fails, never partial success. Check: scripted MinIO proxy/hostile HTTP endpoint: 429, 503, other retryable 5xx, reset, stalled headers/body, truncated/oversized body, false Content-Length/Content-Range, ignored Range, identity changes during retries. Assert attempts, backoff, elapsed deadline, exact rows or nonzero failure. Mutants remove attempt/deadline/length bounds or swallow error."

Result: 29 in-process cases (28 on the ARM64 VM, see "Code and trees") and 3 allocation cases (under `ulimit -v`) all pass on the real wrapper on ARM64, x86-64 and the development VM; 15 mutants of the wrapper and cache (14 on the ARM64 VM), **15 CAUGHT, 0 SURVIVED, 0 INVALID** (the unmutated control passes before the sweep). The hostile endpoint is our own fake (MinIO was unavailable), so "scripted MinIO proxy" is replaced by it. Table below. Reproduce: `target/release/osb faults`, `osb cache`, `osb hostile-all`, `osb mutants` (about 4 minutes), or the `tests` step of `run-reference.sh`.

**3. Decision rule 4 (cache), as written:** "No stale/mixed-version result is acceptable. Unchanged objects avoid repeated footer GETs, including binder→execution; bypass over-cap entries, never relax admission. Admit a cache/default only after key/invalidation, retained-byte and eviction checks pass. No TTL substitute for identity validation."

Result: stale and mixed versions: none in 3 mid-scan swap points and in the replace test; unchanged objects: 0 footer GETs on every warm lookup (real GCS, 100 files and 1 file, both CPUs) with the identity lookup counted separately; over-cap entries bypassed; key, retained-byte (pinned bytes charged), eviction and failed-fill checks pass and each is failed by its mutant. Binder→execution reuse is by construction in this spike (one `Opened` handle feeds planning and decode) and is **NOT MEASURED** as a separate binder path; credential scope is in the key (tested), refresh is not. Verdict by the rule: **the cache design passes the checks that exist; admission as a default waits for the binder handoff and the parsed-size accounting on the production representation** (below). Reproduce: `target/release/osb cache` and `osb mutants`.

**4. Decision rule 3 (runtime), as written:** "ADR 0002's split runtime remains the production choice. Its required separate-Tokio-CPU-runtime comparison is still open: same CPU/I/O thread budgets, policy and bytes, both idle and saturated. A ≥1.2x latency/throughput gain outside noise on both CPUs with no fault/resource regression triggers an owner proposal, never an automatic runtime rewrite."

Result (tables below): across 10 cells, idle and saturated, 5 rounds with twins, on both CPUs, the ratio separate-tokio-runtime / split is **0.90 to 1.08** (ARM64 0.94 to 1.08, x86-64 0.90 to 1.07), CPU seconds equal within 8 %; the largest apparent gain (0.90) lies inside that cell's twin spread (0.94 to 1.03). No cell reaches a 1.2x gain. Verdict by the rule: **threshold not met; the split runtime stays**, no owner proposal. Limits: the tokio-CPU arm runs a non-yielding decode as one async task per row-group unit, not operators as poll-based state machines; most cells are network-bound at 8 workers (the saturated condition keeps all 8 CPU workers busy with 1 ms morsels yet does not move the wall time of network-bound cells), the CPU-bound cells are `big1`, `files100` and SF10 `files10`; bridge channel, wake protocol and fairness of the production scheduler were not used (a plain thread pool with a bounded queue stands in for the split design), so this does not re-test ADR 0002's loom-checked bridge. Reproduce: step `bench-rt` of `run-reference.sh`.

**5. Resource-bounds row (card part 2), the caps as written:** "≤16 active data/metadata attempts per query and ≤32 per node; merged response ≤64 MiB, gap ≤1 MiB with ≤25% extra data bytes per scan; footer wire bytes ≤16 MiB; cache ≤64 MiB charged bytes and ≤1,024 entries ... Mutants bypass grant/cap/eviction and must breach gates."

Result: peak active attempts 16 for one query (60 concurrent reads) and 32 for three queries (90 reads); `no_inflight_cap` is CAUGHT. Range planning: property test over 300 seeds (every needed byte covered, no response over the cap, gaps within the gap size, cumulative extra within the percentage). Extra bytes on the real store: 0 % in every cell except `small1` projected with 8-row-group planning, which reached exactly 25 %. Footer over the cap is rejected before allocation (checked by the length test; no 16 MiB footer was fetched). Cache cap and pinned bytes: tests above. Verdict: **caps hold in the spike**; the RSS envelope is in the reference record. The inflight-cap case was added after the ARM64 run started and ran on x86-64 (VM) and the development VM, not on the ARM64 VM (its tree is `ae46acb…`, see below).

## The fault cases (same outcome in all three environments)

Time constants are scaled (`Config::fast_tests`: backoff 20 ms to 200 ms, attempt timeout 0.8 s, deadline 6 s); attempt counts and caps are ADR 0011's. "Class" is the wrapper's error class; the client library's text is never exposed. Elapsed times are from the ARM64 VM.

| case | endpoint does | outcome |
|---|---|---|
| throttle 503 SlowDown twice, then ok | two 503, then data | exact bytes, 3 GETs seen, 44 ms |
| 429 with Retry-After 1 | one 429 then data | exact bytes after 1,002 ms (header honored) |
| Retry-After 3600 | sticky 429 | `Throttled`, 1 attempt, no second GET (header beyond the deadline) |
| persistent 500 | sticky 500 | `Server5xx`, exactly 4 attempts, 111 ms |
| backoff gaps | sticky 500 | gaps 19, 25, 67 ms against nominal 20, 40, 80 ms (equal jitter, half to full) |
| stalled headers, then ok | no answer, then data | timeout at 0.8 s, retry, exact bytes, 821 ms |
| stalled body, persistent | headers and 1,000 bytes, then silence | `Timeout`, 4 attempts, 3,315 ms (bounded by the attempt timeout) |
| truncated body, then ok | Content-Length correct, 5,000 bytes, close | retried, exact bytes, no duplicate bytes |
| truncated persistent | close after 100 bytes, every time | `Transport`, 4 attempts |
| connection reset twice, then ok | closes without answering | exact bytes |
| chunked range response | no Content-Length, cut off | `BadResponse`, 1 attempt (the client crate requires Content-Length: fail closed, not retried) |
| Content-Range claims more than asked | range + 1000 bytes, consistent framing | `BadResponse` (the crate rejects), 1 attempt, no retry |
| Content-Range shifted | range − 100 | `BadResponse`, 1 attempt |
| Range ignored | 200 with the whole object | `BadResponse`, 1 attempt |
| Content-Length longer than range | Content-Range right, 10 extra bytes | `BadLength`, 1 attempt |
| Content-Length shorter than range | 10 bytes missing, framing consistent | `BadLength`, 1 attempt |
| object replaced between two range reads | second read sees another ETag | first read exact, second `IdentityChanged` (412), 1 attempt, no bytes returned |
| endpoint ignores If-Match | serves the new object with its ETag | `IdentityChanged` from the response check |
| size changes, If-Match ignored | longer object | `IdentityChanged` |
| 403, 404 | | `Auth`, `NotFound`, 1 attempt each |
| range beyond the pinned length | | `BadRange`, 0 requests |
| grant smaller than the response | 64 KiB grant, 200 KB range | `GrantTooSmall`, 0 requests |
| cancel in a stalled body / in backoff | | `Cancelled`, quiescence under 1 ms, gauge 0 |
| cancel in a permit wait | | the cancelled query ends, the other query's read is exact |
| deadline | 503 forever, 1,000 allowed attempts | `DeadlineExceeded` at 1,134 ms for a 1,200 ms deadline (17 attempts) |
| active-attempt caps | 200 ms endpoint | peaks 16 (one query) and 32 (three queries) |
| seeded chaos | 35 % of GETs faulted, mixed faults, seed 20261107 | 72 reads exact, 8 ended in a bounded error (at most 4 attempts); the fault schedule by request ordinal is identical across two runs of the seed |
| endless error body (`ulimit -v`) | 503, body never ends | `Server5xx`, 4 attempts |
| endless range body (`ulimit -v`) | 206, Content-Length 1 TiB | `BadLength`, 1 attempt |
| 200 MiB header line (`ulimit -v`) | | bounded error |
| cache and scan cases | see rule 4 | pass; the scan equals an independent Arrow oracle for 4 queries × 3 policies × 2 executors |

Mid-scan version change: the second file is swapped before its 3rd, 4th and 6th GET (its footer was read and its identity pinned before): the scan ends `IdentityChanged` each time and returns no rows; a scan that mixed versions would have printed a number matching neither version. **A defect of the first version of this test:** it scheduled the swap by global GET ordinal, so under concurrency the swap could precede the file's identity lookup and the scan legitimately ran on the new version (the first full run of the suites on the development VM failed once because of it); the fix swaps by the target key's own GET ordinal.

## Mutants (known-bad wrappers)

| mutant | CAUGHT by |
|---|---|
| `no_attempt_cap` (retry until the deadline) | persistent-500 attempts, stalled-body attempts, truncated attempts; endless error body |
| `no_deadline` | deadline case (harness hang detector fires) |
| `no_attempt_timeout` (client timeout also off) | stalled headers, stalled body (hang detector) |
| `no_version_check` (no If-Match, no response check) | the three identity cases |
| `no_length_check` | both Content-Length cases; endless range body (process dies under `ulimit -v`) |
| `retry_permanent` | 11 cases (every permanent class is retried) |
| `swallow_error` (failure reported as zeros) | 22 in-process cases and the three allocation cases |
| `no_transport_bound` (plain transport) | Retry-After cases (marker missing); endless error body (process dies) |
| `ignore_retry_after` | 429 honored, Retry-After beyond deadline |
| `append_on_retry` (duplicate bytes) | truncated-then-ok, seeded chaos |
| `no_inflight_cap` | active-attempt caps |
| cache `key_without_identity` | replaced object served stale |
| cache `no_validation` (no HEAD on a warm lookup) | footer/HEAD counts, replaced object |
| cache `uncharged_pins` | pinned bytes not charged |
| cache `no_cap` | LRU bound, pinned bound, over-entry cap |

Raw: `tests-mutants.txt` in each raw directory (the ARM64 VM ran the 14 mutants that existed in its tree; the x86-64 VM and the development VM ran 15).

## Metadata cache: GETs and bytes, no cache against the bounded cache

Fake endpoint, 10 objects × 5 queries: no cache 50 HEADs, 50 footer GETs, 3,276,800 footer bytes; bounded cache 50 HEADs, **10** footer GETs, 655,360 bytes (identity lookups stay at one per open). Real GCS (SF1, 5 statements per run, 5 rounds, twins; cold = first statement):

| CPU | cell | no cache: HEADs / footer GETs / footer bytes per statement | cache: statement 1 | cache: statements 2 to 5 | wall, no cache (all statements) | wall, cache, statements 2 to 5 |
|---|---|---|---|---|---|---|
| ARM64 | `files100` (100 files) | 100 / 100 / 6,553,600 | 100 / 100 | **100 / 0** | 0.53 s | **0.21 s** |
| ARM64 | `small1` (1 file, 293 row groups) | 1 / 2 / 533,013 | 1 / 2 | **1 / 0** | 0.10 s | **0.026 s** |
| x86-64 | `files100` | 100 / 100 / 6,553,600 | 100 / 100 | **100 / 0** | 0.66 to 0.77 s | **0.28 s** |
| x86-64 | `small1` | 1 / 2 / 532,846 | 1 / 2 | **1 / 0** | 0.10 s | **0.029 s** |

Footer reads fetch the last 64 KiB first: for 2 KB footers of `files100` that is 29 times the footer size (6.5 MB for 100 files); for the 531 KB `small1` footer it needs a second GET. Parsed size (cache charge basis, `ParquetMetaData::memory_size`) measured on the SF1 layouts: `small1` footer 531,268 B wire → 2,048,071 B parsed (3.9×, 32 such entries fill 64 MiB), `mid1` 91,032 → 346,921 B (3.8×), `big1` 8,008 → 33,178 B (4.1×), a `files100` file 2,235 → 12,285 B (5.5×). So the entry cap (1,024) binds for small footers and the byte cap for large ones.

## Runtime arm: split against a separate tokio CPU runtime (median seconds, 5 rounds, twins)

Same 8 CPU workers and 2 I/O threads, same policy, bytes and store; the saturated condition keeps all 8 workers busy with 1 ms morsels submitted through the same executor.

ARM64:

| condition | cell | wrap med s | wrapT med s | wrapT / wrap | wrap twin ratio | wrapT twin ratio | wrap CPU s | wrapT CPU s |
|---|---|---|---|---|---|---|---|---|
| idle | sf1/big1/full | 0.496 | 0.518 | 1.04 | 0.92 | 1.07 | 0.84 | 0.83 |
| idle | sf1/big1/proj | 0.221 | 0.239 | 1.08 | 0.85 | 1.09 | 0.16 | 0.16 |
| idle | sf1/files100/full | 0.601 | 0.601 | 1.00 | 1.00 | 0.95 | 1.08 | 1.03 |
| idle | sf1/files100/proj | 0.587 | 0.595 | 1.01 | 1.02 | 1.07 | 0.30 | 0.31 |
| idle | sf1/mid1/full | 0.384 | 0.377 | 0.98 | 0.99 | 0.96 | 0.95 | 0.89 |
| idle | sf1/mid1/proj | 0.296 | 0.289 | 0.98 | 1.02 | 0.98 | 0.24 | 0.24 |
| idle | sf1/small1/full | 1.036 | 1.053 | 1.02 | 0.98 | 1.00 | 1.21 | 1.20 |
| idle | sf1/small1/proj | 0.950 | 0.962 | 1.01 | 0.99 | 1.02 | 0.36 | 0.35 |
| idle | sf10/files10/full | 1.615 | 1.601 | 0.99 | 1.03 | 1.01 | 8.13 | 8.11 |
| idle | sf10/files10/proj | 0.621 | 0.651 | 1.05 | 0.95 | 1.02 | 1.85 | 1.84 |
| saturated | sf1/big1/full | 0.510 | 0.526 | 1.03 | 0.93 | 1.01 | 3.87 | 3.89 |
| saturated | sf1/big1/proj | 0.235 | 0.223 | 0.95 | 1.00 | 0.99 | 1.83 | 1.71 |
| saturated | sf1/files100/full | 0.634 | 0.626 | 0.99 | 1.00 | 1.05 | 4.86 | 4.74 |
| saturated | sf1/files100/proj | 0.570 | 0.568 | 1.00 | 0.97 | 0.95 | 4.47 | 4.46 |
| saturated | sf1/mid1/full | 0.412 | 0.421 | 1.02 | 1.00 | 0.97 | 3.09 | 3.09 |
| saturated | sf1/mid1/proj | 0.319 | 0.325 | 1.02 | 0.98 | 1.04 | 2.47 | 2.54 |
| saturated | sf1/small1/full | 1.069 | 1.077 | 1.01 | 1.00 | 1.00 | 8.41 | 8.48 |
| saturated | sf1/small1/proj | 0.979 | 0.979 | 1.00 | 1.03 | 1.01 | 7.72 | 7.66 |
| saturated | sf10/files10/full | 1.653 | 1.638 | 0.99 | 1.00 | 1.00 | 12.09 | 12.19 |
| saturated | sf10/files10/proj | 0.687 | 0.648 | 0.94 | 1.05 | 0.99 | 5.07 | 4.69 |

x86-64:

| condition | cell | wrap med s | wrapT med s | wrapT / wrap | wrap twin ratio | wrapT twin ratio | wrap CPU s | wrapT CPU s |
|---|---|---|---|---|---|---|---|---|
| idle | sf1/big1/full | 0.628 | 0.629 | 1.00 | 0.90 | 1.10 | 1.24 | 1.21 |
| idle | sf1/big1/proj | 0.275 | 0.249 | 0.91 | 1.17 | 0.90 | 0.25 | 0.24 |
| idle | sf1/files100/full | 0.931 | 0.903 | 0.97 | 1.02 | 0.98 | 1.74 | 1.66 |
| idle | sf1/files100/proj | 0.869 | 0.895 | 1.03 | 1.01 | 1.06 | 0.43 | 0.42 |
| idle | sf1/mid1/full | 0.445 | 0.435 | 0.98 | 0.99 | 1.00 | 1.56 | 1.50 |
| idle | sf1/mid1/proj | 0.324 | 0.333 | 1.03 | 0.92 | 1.05 | 0.33 | 0.34 |
| idle | sf1/small1/full | 1.129 | 1.163 | 1.03 | 0.94 | 0.97 | 1.70 | 1.77 |
| idle | sf1/small1/proj | 1.002 | 1.018 | 1.02 | 0.99 | 1.00 | 0.45 | 0.44 |
| idle | sf10/files10/full | 2.359 | 2.345 | 0.99 | 1.03 | 1.00 | 15.32 | 15.37 |
| idle | sf10/files10/proj | 0.772 | 0.718 | 0.93 | 0.99 | 0.94 | 2.62 | 2.64 |
| saturated | sf1/big1/full | 0.693 | 0.680 | 0.98 | 1.10 | 1.00 | 5.11 | 5.00 |
| saturated | sf1/big1/proj | 0.254 | 0.268 | 1.05 | 0.69 | 0.95 | 1.89 | 1.96 |
| saturated | sf1/files100/full | 0.929 | 0.925 | 1.00 | 1.09 | 0.94 | 6.90 | 6.84 |
| saturated | sf1/files100/proj | 0.915 | 0.948 | 1.04 | 1.01 | 1.06 | 6.99 | 7.33 |
| saturated | sf1/mid1/full | 0.514 | 0.506 | 0.98 | 1.02 | 0.93 | 3.82 | 3.72 |
| saturated | sf1/mid1/proj | 0.378 | 0.339 | 0.90 | 1.03 | 0.94 | 2.65 | 2.46 |
| saturated | sf1/small1/full | 1.398 | 1.325 | 0.95 | 1.07 | 0.97 | 10.73 | 10.14 |
| saturated | sf1/small1/proj | 1.161 | 1.191 | 1.03 | 0.98 | 1.05 | 8.91 | 9.24 |
| saturated | sf10/files10/full | 2.553 | 2.454 | 0.96 | 1.00 | 0.98 | 18.91 | 18.55 |
| saturated | sf10/files10/proj | 0.792 | 0.847 | 1.07 | 1.00 | 1.00 | 5.79 | 6.13 |

## What this does not show

- Real S3, MinIO, the S3 credential chain and SigV4 (not available); GCS auth refresh beyond the first token (runs are under one hour); a real object replacement on GCS during a scan.
- Hostile lengths in library paths other than GET/HEAD bodies, headers and error bodies (credential JSON, LIST/multipart XML).
- `cargo deny`, unsafe inventory for `object_store` 0.14, `catch_unwind` containment of Parquet parsing (not the subject here; R6a-d are unchanged).
- Binder→execution handoff in production code, prepared-plan revalidation, single-flight fills (the spike bypasses duplicate misses, as ADR 0012 prefers), ADR 0009 grants (a byte semaphore stands in).
- The production scheduler, its bridge and fairness; Nata's own query timings (this is a reference run, not a Nata number).
- Cross-region reads, other storage classes, concurrent queries sharing connections beyond the fault fixtures.

## Code and trees

The cloud runs ship the working tree. Root code tree (`scripts/code-tree.sh`, which does not cover `spikes/`): `f9215f68fae5f1d5eff495bc0c8a18ab5328ab0445961839a9a54dbe899edc78` for every run. Spike tree (`git rev-parse <commit>:spikes/object-store-baseline`): ARM64 final run `ae46acb04fb43a5aef81a620d2978f4db3209719` (commit a1dff20); x86-64 final run `82dc67cc61a3a1ae0a09c8fa942e57971d05c6b0` (commit f76e71c, which adds only the active-attempt-cap case and its mutant to the tests); later commits (footer-charge probe, fake-endpoint delay and `NATA_UNITS_AHEAD` knob, records) were not shipped. The ARM64 tests were therefore run with 28 cases and 14 mutants, x86-64 with 29 and 15.
