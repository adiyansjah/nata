# 0010: Parallel exact hash aggregation and partitioned spill

Status: accepted (by the owner, 2026-11-07, with card 0005; amended as noted)
Date: 2026-11-07

## Context

[Slice 5](../slices/0005-hash-aggregate.md) adds the first blocking SQL operator. [ADR 0001](0001-pipeline-model.md) specifies morsel push pipelines and dependent breaker sources; [0003](0003-batch-format-and-expression-kernels.md) requires selected-valid strict kernels; [decimal rules](../decimal-rules.md) require exact order-independent sums and AVG. Integer aggregate types and grouped output order are not yet specified. `nata-sql/src/plan.rs` currently has Scan/Filter/Project/Limit only; `nata-exec` uses independent scalar loops (R12). The spikes are evidence to port and test, not production components to wire in unchanged.

## Decision

Propose the following extension; existing accepted ADRs/dialect pages are unchanged by this proposal.

- **Plan:** scan → WHERE/aggregate arguments → task-local partial aggregate; partitioned final-merge tasks → aggregate result source → HAVING → projection → LIMIT/OFFSET. A merge task owns one partition with all contributing partials, whether resident or spilled. No serial all-groups table in `combine`/`finalize`; the coordinator only publishes bounded batches. Bind only group expressions, aggregates and expressions over them after the breaker; no arbitrary ungrouped columns or functional-dependency inference. HAVING accepts BOOLEAN, including aggregates not selected, evaluated only after all partial states for a group have merged. Do not push HAVING below the breaker in this slice.
- **Pipeline integration:** use row-group morsels and the existing CPU budget, with reusable local state per execution task, not one table retained for every row group. Introduce the minimum dependent-pipeline/blocked-spill adapter needed by ADR 0009, bringing the accepted ADR 0002 deque behind the unordered `claim()` seam with its concurrency checks. The product's ordered FIFO scan stays intact. Measure the actual pool with affinity on/off as ADR 0001 requires; the spike's result is not a pool measurement. Aggregate output has no promised order (including LIMIT); verify multisets. ORDER BY belongs to slice 6. A single-source sink/source does not need A1.1/2's `Operator::finish` or multi-producer finalization changes.
- **Table/keys:** reuse `hashbrown::HashTable` with group IDs and owned typed key/state arenas, as in `spikes/hash-ops/src/agg.rs`, with std map as the baseline control. Own canonical key equality/hashing, not table probing. Full key equality resolves collisions. Hash all key columns by value: NULL markers separate from values, dictionary strings by UTF-8 bytes rather than codes, decimals normalized across scales/widths, floats with canonical NaNs and zero signs. Same seed/version gives identical hashes on both CPUs. Fixed vectors and forced collisions check this; do not assume the spike already covers Decimal128/floats. No dense dictionary-product fast path unless it has a cap and a general hash fallback.
- **Shared strict kernels:** one production implementation used by scalar SQL, batch arguments and aggregate updates/merges (R12/P11). Update the whole aggregate set together per selected row; validity is per argument and count, not one shared flag. NULL slots' junk must never be read. P4 fusion and P5 elision remain independently gated optimizations, not permission to change error semantics.

### Result types and evaluation

| Aggregate input | Result (proposed where integer/count rules were open) |
|---|---|
| SUM of TINYINT/SMALLINT/INTEGER | `BIGINT` (PostgreSQL's rule; overflow is an error) |
| SUM of BIGINT | `DECIMAL(38,0)`, exact integer value |
| SUM of DECIMAL(p,s) | `DECIMAL(38,s)`, [decimal-rules §8](../decimal-rules.md#8-functions-and-aggregates) |
| AVG of DECIMAL(p,s) | `S=min(s+6,38-(p-s))`, `P=(p-s)+S`; `DECIMAL(P,S)`, exact sum/count, half away from zero |
| AVG of signed integers | Apply the same formula to decimal precisions 3/5/10/19, scale 0; INTEGER → `DECIMAL(16,6)`, BIGINT → `DECIMAL(25,6)` |
| COUNT(*) / COUNT(expr) | Non-NULL `BIGINT`, checked at result conversion; too many rows is overflow |
| MIN/MAX of supported scalar input | Same input type; binary UTF-8 strings, dialect NaN/zero comparison for floats, no collation |

**Reasoned:** integer SUM uses an existing exact SQL/wire decimal type rather than introducing HUGEINT; DuckDB integer sum value is an oracle, not its type metadata. Integer AVG stays exact, extending Nata's decimal division rule. These choices need acceptance and later extensions to `decimal-rules.md`/`dialect.md`; no existing rule is silently overwritten.

Global empty input returns one row, COUNT zero and other aggregates NULL; grouped empty input returns no rows. NULL group keys compare equal to NULL; COUNT(expr), SUM/AVG/MIN/MAX skip NULL arguments. All-NULL groups have count(expr) zero and NULL other results. Float SUM/AVG, DISTINCT aggregates, FILTER/aggregate ORDER BY, nested aggregates and grouping extensions are explicitly unsupported (card §1).

Use `i128` for sums of signed integer/Decimal64 values under a checked per-group row-count bound below `u64::MAX`; wider decimal inputs need an exact wider state. Propose Arrow's existing signed 256-bit integer behind checked helpers, with an independent arbitrary-precision test oracle; reuse before a new arithmetic dependency. Widen before AVG rescaling; verify the input-count and rescaled-numerator bounds, including rounding. Only final SUM is checked against 38 digits; local states and spilled partials are not prematurely narrowed. AVG divides the exact sum, not a possibly overflowing SUM result. Thus large positive/negative intermediates that cancel give the same value at every thread count/partitioning. Guard count/merge bounds even when only SUM is requested. If wide helpers cannot meet this contract, stop for an ADR revision; do not fall back to DOUBLE or per-row overflow. P5's existing i64-addend proof is not a proof for wide states/partial merges.

Phase order is part of strictness: WHERE filters input before group/argument evaluation; all aggregate arguments for retained rows contribute before HAVING or output LIMIT. HAVING cannot hide an argument overflow or a final aggregate overflow in its group. Output projection runs only on surviving groups admitted by OFFSET+LIMIT, retaining existing selected-row behavior. LIMIT can stop final output after its groups; it never short-circuits input accumulation. HAVING/group expressions refer to input group expressions, not SELECT aliases or ordinals. Wire types/precision/scale are fixed at bind/Describe time; extend the existing narrow binary COPY wrapper to this SELECT subset, with no other COPY expansion. Empty output cannot change metadata.

### Spill and termination

Use [ADR 0009](0009-query-memory-manager.md) grants throughout. At pressure, flush partial **states**, with keys and non-NULL counts, into hash partitions; later merge resident and spilled states exactly once. Start with uncompressed Arrow IPC streams inside a private versioned envelope: schema/aggregate-state version, partition/hash depth, bounded record lengths/counts and checksum over header and payload. Use fixed byte order and explicit wide-state encoding, never native Rust layout or dictionary IDs. Wide state is internal storage, not a new SQL Decimal256 type. The reader validates envelope/schema, lengths versus file remainder and grant BEFORE invoking Arrow; no wholesale read or unchecked IPC allocation. Fuzz this wrapper as untrusted input, including truncation/bit flips. The baseline must demonstrate the bounds; failure requires a revised format decision.

An oversized partition is recursively repartitioned using successive hash bits; reserve both parent/child buffers and disk overlap, reduce partitions in flight to fit. Proposed target limits: initial fanout 16, fanout 16 per recursion and maximum depth 8; equal keys always stay together. No-progress/collision-heavy partitions, exhausted depth or a single key/state larger than the minimum grant fail with a bounded resource error; never recurse forever. Repeated identical keys coalesce to one state, so a hot key alone must not trigger repeated repartitioning. Stream final partitions in bounded batches; never collect the output. Query FD/disk quota D includes live parent and child files; delete a parent only after replacement children are complete and registered. Cleanup follows ADR 0009 on every exit.

Local spill jobs run on a bounded disk-I/O service, with CPU tasks parked and cancellation propagated. This resolves ADR 0002's open blocking-spill edge by proposal; a new general runtime or unbounded worker pool is not authorized. Slices 6–8 reuse grants, file registry, bounded framing/I/O and fault hooks; their records and algorithms need their own tests, not automatic certification from aggregate spill.

## Why

**Reasoned:** partition ownership makes the final merge parallel without concurrent mutation of a global table. State spill avoids re-evaluating SQL arguments and preserves exact AVG counts. A common resident/spilled merge path limits semantic duplication. Arrow IPC reuses existing batch tooling; the envelope must protect metadata as well as payload (the Parquet CRC lesson). Bounded recursion and disk/FD accounting make small budgets a supported path rather than a late error at the first large partition.

## Alternatives considered

- **One global locked table / serial combine:** simplest, but serializes high-cardinality state and couples workers to its allocation. The join spike's serial finalize is a warning, not a measured aggregate diagnosis.
- **Raw input spill and re-execution:** simpler states, but writes every argument row and risks repeating expression evaluation. Keep as an alternative if partial-state format/cost fails; no measured bake-off yet.
- **Sort aggregation:** can bound memory, but external sort is slice 6 and a different first blocking operator. Revisit for ordered inputs or persistent skew.
- **Custom probing table/codec from day one, or embed another SQL engine:** the former lacks a reuse bake-off; the latter replaces the strict kernels/pipeline contract. Reuse table/IPC unless the card's fixed rules reject them.

## Evidence

- **Measured:** [hash correctness](../../bench/results/20261104-hash-ops-correctness.md) checks task-local tables/parallel partition merges, dictionary/NULL/collision cases and mutants, chiefly ARM64 certification; its key types/accumulators are narrower than proposed here. [Both](../../bench/results/20261105-timing-arm64.md) [CPU timings](../../bench/results/20261105-timing-x86.md) show general aggregates competitive but often slower than DuckDB; they are not SQL/spill timings. [Generic](../../bench/results/20261104-generic-ops-x86.md) aggregation improved with a single pass per aggregate set. [NULL path](../../bench/results/20261105-timing-settled-rerun.md) meets its 1.3x penalty target on both CPUs; [Kani](../../bench/results/20261106-kani-wrap-bound.md) proves only the stated narrow accumulation. [Push-cost](../../bench/results/20261105-timing-settled-rerun.md) separates unfused work from dispatch; it does not justify changing runtimes. [SF10](../../bench/results/20261106-scale-sf10.md)/[SF100](../../bench/results/20261106-scale-sf100.md) supports ARM pipeline scale only; operators-beyond's aggregate is **not spillable**.
- **Reasoned:** proposed table layout/format/recursion, integer types, wide accumulator sufficiency, pipeline integration and I/O policy. None is a measured production result.
- **Not covered:** full Q1 SQL, aggregate spill performance/corruption handling, Decimal128 exact AVG/merge, float key canonicalization, fair-share memory or default aggregate clients on either CPU. Join/sort spill evidence is not certification of these paths.

## Confidence

**Medium** for task-local tables/partition merges and narrow NULL kernels within the cited tests; **low** for wide aggregate arithmetic, bounded IPC and performance of the combined design. **High** in preserving the accepted exact decimal/NULL contract as a requirement, not in its unbuilt implementation. A bound failure or independent-oracle mismatch blocks acceptance; the baseline/card supplies the empirical checks and mutants.

## Consequences

Aggregation adds a real breaker/dependent-source path and memory service, not just a binder node. Later sort/join/window share that infrastructure. Exact wide state costs memory; IPC overhead and general keys may miss spike timings. Integer types/grouped order must be documented after acceptance, including precise DuckDB differences, while accepted ADRs remain intact in this planning change.

## Revisit when

The baseline rejects bounded IPC or reuse tables, exact-wide helpers fail their oracle, skew exhausts the depth budget on a supported workload, profiles meet the custom-replacement rule, or slices 6–8 need a different spill record or scheduling interface.

## Amendments

- 2026-11-07, at acceptance (owner): SUM of TINYINT/SMALLINT/INTEGER is `BIGINT` and SUM of BIGINT is `DECIMAL(38,0)`, as in PostgreSQL, instead of the proposed `DECIMAL(38,0)` for every integer. AVG of integers stays an exact DECIMAL. Grouped output is unordered without ORDER BY. DISTINCT aggregates wait for slice 9. Card 0005 part 3 records the decisions.
- 2026-11-07, after the card 0005 baseline spike: spill stays **Arrow IPC** (136 exact round trips; 352 damaged files rejected with at most 18,448 bytes allocated). A spilled file is read back as untrusted bytes, so the reader validates its **inside**, not only an envelope checksum: a fixed trusted schema, caps on metadata, body, rows and messages, node and buffer layout, range and size checks, dictionary index and string offset checks, and no compression or deltas. Byte caps alone do not justify handing arbitrary IPC to Arrow.
- 2026-11-08, decided by the parent under the owner's delegation, after the clean-VM spill profile (`bench/results/20261107-hash-aggregate-perf.md`,
  "Spill path profile on clean VMs") and step 2d's stop: **the initial spill fan-out is sized from the budget.** A power-of-two
  logical fan-out from 16 through 256: estimate resident state bytes from expected partial rows and the charged key, state and table
  capacity (serialized spill bytes are supporting evidence only, since they underestimate hash-table memory); choose the smallest
  fan-out whose estimated resident partition fits half the available merge-table budget; clamp at 256 and reduce simultaneous merge
  partitions when the estimate exceeds their grants. Recursive fan-out stays 16, depth at most 8, for estimation error and skew;
  exhausted depth, no progress and oversize records still fail cleanly. FD and disk quotas, parent/child overlap accounting, the
  64 KiB scratch floor, canonical XXH3 hashing and validated Arrow IPC stay mandatory; reader partition-membership checks use the
  fan-out the query chose, never an unchecked value from disk. No grant is bypassed. Why: at 64 MiB the high-cardinality query made
  272 partitions at one thread and 4,368 with recursion depth 2 at eight, writing 6.7-10.7x its ~90 MB result. Checks that can fail:
  estimator overflow, the 16/256 clamps, partition membership under hostile input, skew/collision/no-progress, FD and quota bounds,
  merge-concurrency reduction, and the spill counters (bytes, partitions, depth) in settled timing.
