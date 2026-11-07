# 0012: Identity-validated bounded Parquet metadata cache

Status: proposed
Date: 2026-11-07

## Context

[Card 0006](../slices/0006-object-store-io.md) adds remote footers; P12 in [the register](../improvements.md) records duplicate binder/execution footer loading. The local reader already holds metadata through `Arc<FileScan>` and bounds file opening with `nata-sched`'s ahead window. A process cache must not defeat those lifetimes or reuse metadata after an object changes. [ADR 0009](0009-query-memory-manager.md) separates query grants from other retained process memory. This proposal covers Parquet metadata only; Iceberg REST/manifest decoding is slice 12.

## Decision

Propose a process-local cache of validated immutable Parquet metadata, with explicit byte ownership shared from binder through execution, and an off/zero-cap control. All defaults below are **reasoned provisional targets**, not measured optima.

- **Key:** provider + canonical endpoint/region/bucket/object identity + credential/authorization scope identity + object version/generation (preferred), or opaque ETag and object length + reader metadata/options/schema-interpretation version. Do not put credential/token bytes in keys or diagnostics; provider context supplies an opaque scope ID. Never normalize distinct object keys into one. Parsed metadata is projection-independent; any option-dependent view belongs in the key or is built per query. No cache of failures or authorization results.
- **Validate before use:** for an unversioned URI, perform bounded authenticated identity lookup each query, even on a warm cache. Bind and execution share the resulting pinned identity and footer handle, with all data reads conditional/versioned under ADR 0011. Revalidate prepared-plan execution each time; a changed identity forces rebind or a clear replan error, never old schema with new bytes. New version/ETag/length creates a new entry; old entries age out and cannot be found by path alone. Without a trustworthy validator, disable cross-query reuse; fail if the backend cannot pin a consistent scan. TTL alone is not invalidation, and ETag is not a checksum or proof against a malicious endpoint lying consistently.
- **Admission/eviction targets:** cache at most 64 MiB charged capacity and 1,024 entries; maximum single entry 16 MiB charged parsed metadata. Least-recently-used eviction of unpinned entries, deterministic tie order for tests. Charge keys, metadata arrays/strings/capacities and cache bookkeeping, not just footer wire length or `size_of` the root. Borrowed `Arc` memory is charged once, but **pinned entries still count against the cache cap**, including evicted-but-live handles. If pins prevent admission, bypass caching and hold the parsed footer under the query grant instead; never wait for cache space while holding an execution grant/lock. Reject metadata whose construction cannot fit a query grant; an entry above the cache-entry target may be read uncached only within the query/footer limits.
- Reserve the node cache allowance before dividing the remainder into ADR 0009 execution admissions; do not give queries memory already promised to the cache. Cache construction/staging is query-charged before parsing; transfer the charge to the cache reservation atomically on successful admission, without an uncharged interval or double-owned allowance. If accurate bounded parsed allocation cannot be established, keep the cache disabled until the representation is bounded; wire size alone is insufficient (the Parquet Thrift length lesson).
- Concurrent misses for the same identity may share one bounded fill; fill slots/waiters are bounded by admitted queries and the request queue limits. Do not hold cache locks over network/parse/wake callbacks. Cancel a waiter without cancelling another query's fill; when the last waiter leaves, abort/drop the fill, release grants and wake everyone. A failed/cancelled/partial parse is never published. Initially bypassing duplicate misses is preferable to an unverified single-flight protocol.
- Successful warm lookup issues no footer GET; HEAD/identity lookup is counted separately. Report hit/miss/bypass/eviction, retained/pinned bytes and validation requests with no object identifiers. Local files retain existing behavior; any local cache extension needs a trustworthy change identity and its own race tests, not path/mtime assumptions copied from remote ETags.

## Why

**Reasoned:** validated identity avoids silent stale-schema/pruning errors; sharing a pinned handle removes P12's duplicate metadata path without weakening schema checks. Byte and entry limits catch both huge metadata and many tiny keys. Pinned-byte accounting bounds actual retention; an LRU map-size bound alone does not. A simple LRU is proportionate until measurements justify a more complex policy.

## Alternatives considered

- **No process cache, reuse only within a query:** safe control and fallback; removes bind/execution duplication but misses repeated-query metadata reuse. Keep this mode if bounded admission is not established.
- **Path-only cache with TTL:** simple/cheap but permits stale footers after replacement and cross-scope authorization leakage; fails correctness regardless of latency.
- **Cache raw footer bytes only, or a persistent/distributed cache:** raw bytes bound storage more easily but repeat parse work and still require bounded parsing/identity checks; retain as fallback representation if parsed charging fails. Persistent/distributed state and data caching add invalidation/security scope not needed for this slice.

## Evidence

- **Measured:** [slice 4 closeout](../../bench/results/20261106-slice4-close.md) records SQL/scan latency misses; P12 attributes duplicate reads from the code, **not a profile proving the miss's cause**. [Footers ahead](../../bench/results/20261106-slice4-bench.md) supports bounded ahead opening on local files, not process-cache speedup. [Real store](../../bench/results/20261105-object-store-real.md) measures request latency and many-small-file overhead, not any cache policy. [Deep-check Parquet](../../bench/results/20261107-deep-check-parquet.md) shows why parser allocation/EOF guards must survive caching.
- **Reasoned:** key, validator/invalidation rule, LRU, capacities, accounting and fill cancellation. No cache hit-rate/latency claim has been measured.
- **Not covered:** parsed heap expansion accounting, pin pressure, replacement during bind/read, prepared-plan refresh, auth-scope changes and concurrent fill fairness on either CPU. Slice 12 must define its own snapshot/manifest keys and bounded external-format wrappers; this ADR does not certify them.

**Planned acceptance checks:** cache on/off vs local/DuckDB rows; overwrite same-length object with a new ETag, delete/recreate, versioned read, missing validator, credential-scope change, failure during fill, and mixed-version range attempts. Generate more entries than capacity and pin them while admitting others; independent retained-byte/RSS gauges must stay bounded and eviction/bypass must be observed. Repeat-query/footer-count and binder-handoff checks must fail on mutants removing identity from the key, ignoring validation, retaining evicted pins uncharged, admitting before reservation or publishing failed fills. Model real fill/waiter ownership and cancel/wake under the card's loom bounds if single-flight is implemented. These checks are planned, not executed evidence.

## Confidence

Medium in the need for identity validation and shared binder/execution ownership by reasoning; low in proposed capacity/eviction performance and unbuilt allocation accounting. Every cited measurement retains its own workload/architecture limits; none establishes cache speedup. Any stale result or retained-byte breach blocks acceptance; an unaccountable metadata representation favors raw-byte or query-only caching.

## Consequences

Warm queries still pay identity-validation requests; cache hits are not zero-network claims. Pin pressure can lower hit rate without raising memory. Implementers must expose measured parsed ownership, rather than multiplying footer bytes by an unexplained factor. Shared scope/identity interfaces leave slice 12 ready for read integration without touching ADR 0005 rule 8 or any write path.

## Revisit when

The reference run cannot meet the memory envelope, LRU churn dominates real repeat workloads, a backend cannot validate identity, or slice 12 introduces snapshot/manifest identities. Freeze sizes after the reference record and owner/parent acceptance; preserve no-cache control and stale-version mutants.
