# 0011: Bounded object-store reads through the async I/O bridge

Status: accepted (by the assistant under the owner's delegation, 2026-11-07, with card 0006)
Date: 2026-11-07

## Context

[Slice 11, card 0006](../slices/0006-object-store-io.md) adds S3/MinIO and native GCS reads independently of Track A's operators. [Vision](../vision.md) starts with `object_store`; [ADR 0002](0002-async-runtime-and-threading.md) keeps I/O on Tokio and decode on CPU workers. The current `nata-parquet/src/reader.rs` uses local `SharedFile(Arc<CountingFile>)`; `CountingRead::at_eof` stops a dependency loop after EOF. `nata-sched/src/lib.rs` preserves ordered claims/errors and bounded footers ahead, not an async network adapter. The spike is not production infrastructure.

## Decision

Propose reuse of `object_store` behind a small Nata-owned read-only boundary, with native S3 and GCS providers and explicit local MinIO configuration. This is a conditional reuse choice, not a measured client bake-off. `object_store` 0.14 was used by the spike; it is absent from the root lockfile, so an implementation brief must explicitly authorize adding it and run dependency/unsafe checks. Use root-locked supporting crates at their locked versions where suitable; no unrelated dependency/runtime replacement.

**Reasoned policy, all numeric defaults are provisional targets:**

- One admission path covers HEAD, footer and data requests, including retries: at most 16 active attempts per query and 32 per node; fair waiting across admitted queries, cancel-aware acquisition. Pending request descriptors are capped at 32 per query and 64 per node (targets); excess producers park before enqueueing, not in an unbounded task list. Retries do not hold request permits while sleeping. Reserve response/bridge capacity under ADR 0009 **before** issuing a request, without holding scheduler/cache locks; cap queued work as well as active work. An unavailable grant parks work, never blocks a CPU worker or spins. Reduce concurrency to the memory minimum; fail clearly if a required unit cannot fit.
- Plan checked half-open byte ranges within the pinned object length. Merge gaps up to 1 MiB only with response size at most 64 MiB and cumulative extra data bytes at most 25% of needed data bytes. Planning metadata and retry bytes are reported separately, never hidden as useful throughput. Split large fetches, do not split CPU row-group morsels. Prefetch/cross-row-group merging is off until card rule 2 passes; buffers already fetched/queued/decoding retain their charge until last use. Footer wire cap is 16 MiB; reject an oversized footer before allocation. Parsed expansion must also fit a grant, not just the compressed/wire cap.
- Disable client-internal retries so exactly one layer owns attempt counts. Retry throttling (429), transient server 5xx, transport resets, timeout and truncated bodies, at most **4 total attempts**. Equal-jitter exponential backoff: initial 100 ms, nominal doubling up to 5 s, delay in half-to-full nominal interval. Honor a valid Retry-After only within the remaining operation deadline; otherwise fail without another attempt. An attempt's headers and body share a 30 s timeout, and the complete logical read (permit/grant wait, attempts and backoff) has a 120 s deadline; query deadline wins when earlier. These are target policies, not spike-measured production optima. Permanent authentication/permission/missing-object/invalid-range or unsupported errors are immediate; credential provider refresh is bounded by the same deadline and cannot restart the attempt budget. Never retry malformed lengths, ignored Range or identity mismatch as though they were transient truncation.
- Pin object identity (version/generation preferred, otherwise ETag plus length) and condition every range/retry on it. Treat ETag as an opaque validator, never a content checksum. A failed precondition or inconsistent response fails the scan, rather than fetching a new footer halfway through. A body retry replaces an uncommitted range in full; never append duplicate bytes or retry a row already delivered. A failed stream may already have emitted rows, but exits nonzero/with a query error and never reports successful completion; no distributed task rerun is introduced (ADR 0004).
- Treat **all response bytes as hostile**, including those reaching the client crate itself. Check overflow, requested range, reported object size/Content-Range, declared length and cumulative streamed length against the expected range and grant; refuse full-object responses to range requests. Do not use an unconstrained `bytes()` collection like the spike. Bound headers/error bodies at the transport configuration boundary before the client can allocate from them. Enforce EOF behavior equivalent to `CountingRead::at_eof` on the byte-backed Parquet reader; validate footer/page ranges and bound parser allocations/loops, retaining existing panic containment. A deadline outside non-yielding decoder code cannot stop it: guards must prevent the loop, with watchdog fuzz tests proving it. If the reused crate cannot be bounded before allocation, stop for a changed proposal.
- Tokio owns fetch/refresh/deadlines; CPU workers decode/decompress through a bounded byte bridge and sticky park/wake notification. Preserve scan ordering, lowest-error and footers-ahead bounds; no `block_on` on CPU workers and no decode on the I/O runtime. Cancel, LIMIT and output close interrupt grant/permit waits, requests, backoff and sends; drain/join tasks and release charges. Target fault-fixture quiescence is at most 1 s. ADR 0002's separate Tokio CPU-runtime alternative is still due a narrow comparison under the card, not approved for production here.
- Credentials come from provider environment chains or instance metadata with refresh; no SQL credentials or secret command-line arguments. Use read-only identities, TLS for real stores, explicit insecure local-test endpoint only. Do not forward auth across redirect origins. Sanitize before logging/formatting dependency errors: stable object aliases and error classes, no private endpoint/bucket/key, token, signed URL or response-body text. Metrics contain no identifying labels.

## Why

**Reasoned:** a single request/retry boundary makes admission, cancellation and failures inspectable. Query and node caps prevent concurrent queries multiplying requests; grants prevent the request cap becoming a memory promise. Reusing signing/auth/TLS avoids owning commodity protocols. Conservative merging preserves a bandwidth/cost bound; the fastest spike policy fetched much more than the query needed.

## Alternatives considered

- **Use `ParquetObjectReader` and client defaults unchanged:** least glue, but the spike did not measure it; implicit coalescing/retry/buffering does not establish our end-to-end bounds. Revisit if it can expose the same controlled admission/streaming/identity contract.
- **Own the HTTP/signing client:** possible escape if reuse fails safety requirements; not justified by existing profiles or a client bake-off. Card rule 1 requires an explicit new decision and experiment first.
- **Sequential reads/no merging or unlimited prefetch:** sequential is a measured slow control; unlimited buffering conflicts with ADR 0009. Bounded unmerged parallel reads remain the policy control and fallback if merging fails its rule.

## Evidence

- **Measured:** [real GCS spike](../../bench/results/20261105-object-store-real.md), SF1, same-region native API, both CPU types: 16 in flight versus sequential was 2.3–13.5x faster; request first byte p50 25–36 ms. Cross-row-group merging won on many groups with up to 303–411% extra bytes. One query at a time, no DuckDB real-store reference, not S3. Per-cell B/twin noise reached ratios 1.14 ARM64 and 1.23 x86-64; small policy differences are unresolved.
- **Measured:** [simulated spike](../../bench/results/20261104-object-store-scan.md), dated GCP results after the initial compile-only plan, has independent fault/memory/cancel counters and known-bad variants. Its body collection and bounds exclude footer/library allocations; its `D-pool` is not the separate Tokio CPU-runtime alternative. [Damaged Parquet](../../bench/results/20261107-deep-check-parquet.md) supports retaining our EOF guard. These are not measurements of this proposed wrapper.
- **Reasoned:** all production defaults, byte-waste ceiling, node admission, response validation, redaction and identity rules above; references motivate them but do not establish optima.
- **Not covered:** real S3/MinIO product integration, both production auth refresh chains, hostile lengths before library allocation, cross-query fairness, runtime alternative, production RSS/deadlines, cross-region or tail beyond p99. Card 0006 requires empirical/model/mutation checks before acceptance; no benchmark or cloud run was made for this proposal.

## Confidence

Medium for bounded parallelism's benefit **within the measured GCS workload**; low for any client-superiority claim (none measured), proposed default optimality and production integration. A response allocation outside the cap, lost wake/cancel, stale-version result or failure of native conditional reads blocks acceptance. A profile-supported client bottleneck prompts a bake-off, not an unmeasured replacement.

## Consequences

The adapter needs a real asynchronous blocked/wake seam, not a URL mapped into local `PathBuf` or a synchronous network `ChunkReader`. Coordinate its grants with Track A's manager without changing accepted ADRs in this draft. Slice 12 can supply explicit identities and scoped credentials to the same range service; REST, manifests and write operations remain outside it.

## Revisit when

The card's bounded-wrapper probe fails, matched reference targets miss, runtime comparison meets its proposal threshold, or concurrent workloads require different admission. Defaults freeze only after the reference run and owner/parent decision; accepted-policy changes need a dated amendment.
