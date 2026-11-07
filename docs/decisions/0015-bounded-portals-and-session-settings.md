# 0015: Bounded fetch portals and truthful session settings

Status: accepted (by the assistant under the owner's delegation, 2026-11-07, with card 0007)
Date: 2026-11-07
Related: [ADR 0008](0008-client-protocol.md), [ADR 0009](0009-query-memory-manager.md), [ADR 0014](0014-read-only-catalog-views.md), [card 0007](../slices/0007-wire-protocol-and-catalog-views.md)

## Context

[serve.md](../serve.md) and ADR 0008 explicitly reject positive Execute max-rows and nonzero statement_timeout. Merely retaining a running producer between fetches can occupy admissions, scan windows and worker shares indefinitely. Named wire portals already work with unlimited Execute; SQL DECLARE/FETCH does not. Search_path is currently stored without changing file-path resolution. Finishing read-only clients needs bounded incremental fetch and honest settings, not transaction isolation or writes.

## Decision (proposed; reasoned)

- Extend a bound portal with a single execution position and lifecycle: bound → executing → suspended → executing → completed, with failed/closed/expired terminal states. Positive Execute sends at most the requested rows, PortalSuspended when more execution remains, CommandComplete only at exhaustion with the PostgreSQL completion count semantics captured by card 0007. An exact boundary/end and Execute zero are oracle cases; do not guess completion by re-executing or collecting the result. Preserve RowDescription/Describe and discard-until-Sync behavior.
- Retain the existing admitted-query share, bounded bridge/window and charged reservations while suspended; no result spool, rerun or unbounded parked worker allocation. A paused producer must stop at the bounded bridge; metadata generation is pinned. Parked work still counts against max-queries and the global CPU/memory budget; it is not a new pool. No promise that an expired portal resumes transparently.
- Add an explicit idle suspended-portal lifetime, **proposed target 5 seconds**, started after PortalSuspended is flushed and reset only by a valid resume. Expiry cancels and joins the query, releases all charges/admission, and leaves a bounded terminal marker until close/transaction end or object-cap eviction; next lookup reports 34000 with a safe expiry explanation. No unsolicited asynchronous ErrorResponse corrupting the next exchange. The setting/default and marker accounting must be frozen before implementation. All parked admissions may delay other queries only within this lifetime and the existing admission deadline; resource/starvation tests must demonstrate this, otherwise revise the policy before acceptance.
- Sync outside a compatibility transaction releases portals; inside BEGIN, suspended portals survive Sync until exhaustion/Close/COMMIT/ROLLBACK/error cleanup/expiry. Close, unnamed replacement, disconnect, timeout, cancel and shutdown wake and join retained work exactly once. Registry active token includes a suspended query; wrong/stale cancel cannot cancel a replacement. Cancel of a suspended query becomes a recorded terminal error delivered at the next legal response, with PostgreSQL-oracle behavior frozen before implementation. Never append an error into a partial DataRow/COPY frame; keep ADR 0008's finish-or-close rule.
- Implement statement_timeout as a per-statement wall deadline starting at receipt of Query or each Execute (including admission/planning/output); SET values use checked bounded integer milliseconds and documented unit suffixes, zero disables. Capture PostgreSQL semantics for multi-message/pipelined exchanges before freezing exact reset points. A new Execute gets a fresh statement deadline; time awaiting the client's next Execute is governed by portal-idle expiry. Timeout invokes the same cancellation token and cleanup path, SQLSTATE 57014. It cannot be bypassed by blocked admission or socket writes; partial frames may force close. No process-wide signal handler. Cancel/expiry/timeout racing completion has one terminal owner and no success after a selected failure.
- Share one validated registry for startup settings, SET, SHOW, RESET, SHOW ALL, current_setting and pg_settings. search_path now controls registered-table resolution through ADR 0014, while quoted file sources keep their existing semantics. Unknown settings/semantic changes remain explicit errors; missing_ok returns NULL only as documented. Transaction-local settings and arbitrary startup options remain unsupported. Existing DateStyle/encoding/float/TimeZone restrictions stay visible. Report read-only execution capability truthfully in metadata and document compatibility transaction markers; do not silently change accepted default_transaction_* settings or claim isolation. A read-only-setting change requires a dated parent amendment to ADR 0008 after the default-client traces prove it safe.
- Keep SQL DECLARE/FETCH, scrollable/holdable cursors, transactional snapshots and writes outside card 0007. Explicitly test refusal. Extended COPY remains a utility stream, not a row-limited suspended portal; add only frozen read-only STDOUT forms in the card, with PostgreSQL framing/format oracles.

## Why and alternatives considered

**Reasoned:** bounded retention is a small extension to today's admission/bridge, and an explicit short lease prevents arbitrary clients from keeping all resources forever. This trades long client think time for predictable local service capacity; the owner must accept that limitation.

- **Ignore max-rows or collect everything:** wrong fetch semantics or unbounded results; rejected.
- **Release execution and rerun per fetch:** duplicate/missing rows and changing data/error behavior; rejected.
- **Spool all results and release admission:** could support long pauses, but adds disk quota/cleanup/pagination design (feature-map T1); deferred.
- **Continue refusing fetch:** lowest scope, valid fallback only if the owner accepts a narrower client matrix after the model/capture cannot settle a bounded design. It is not a passing implementation of this proposal.

## Evidence

- **Measured:** [extended](../../bench/results/20261107-pgwire-extended.md), [reliability](../../bench/results/20261107-pgwire-reliability.md) and [second fixes](../../bench/results/20261107-pgwire-fixes2.md) check existing refusal, admission/backpressure, default drivers, frame cancellation and binary COPY, not suspension or statement deadlines.
- **Reasoned:** all new lifecycle, lease and timeout policies above; the lease is a target, not an empirically selected optimum.
- **Not covered:** paused-producer model, fair progress when all admissions park, PostgreSQL exact boundary/count/timeout/Sync traces, new scratch/RSS bounds or track-A grant integration. Card 0007 requires capture, a real-code bounded model with lost-wake/double-release mutants, loaded live stress and both-CPU measurements; no written review substitutes for those checks.

## Confidence, consequences and revisit when

Low until the captured sequences and cancellation model pass; medium that explicit retention accounting is preferable to invisible parked work. The server can serve JDBC fetch sizes with bounded retention, but slow thinkers may lose a portal. Revisit if normal default-client use exceeds the lease, parked producers cannot release resources on deadline, or a real requirement for SQL cursors/spooling arises. If the safe bounded policy cannot satisfy the mandatory matrix, stop for the owner rather than extending transaction or spool scope implicitly.
