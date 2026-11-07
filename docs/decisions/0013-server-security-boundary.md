# 0013: Server security boundary for read-only client compatibility

Status: accepted (by the assistant under the owner's delegation, 2026-11-07, with card 0007)
Date: 2026-11-07
Related: [ADR 0008](0008-client-protocol.md), [card 0007](../slices/0007-wire-protocol-and-catalog-views.md)

## Context

The [feature map](../feature-map.md) puts simple/extended wire and discovery in T0, TLS and password authentication in T1. [serve.md](../serve.md) currently offers loopback trust, explicit unsafe-listen opt-in and SSL/GSS refusal. Slice 10 must decide whether completing default read-only clients means building a secure network service. The owner's read-only-first parallel-track plan favors finishing T0 without introducing a new security subsystem.

## Decision (proposed; reasoned)

- For card 0007, retain loopback-default trust and the explicit unsafe-listen warning/opt-in. User/database are routing labels, not authenticated identities. Never describe this mode as secure remote access. Keep bounded startup/cancel slots and deadlines from ADR 0008; never log SQL, authentication material, cancel secrets or connector configuration.
- Refuse SSLRequest and GSSENCRequest correctly, before ordinary startup. A client requiring encryption or certificate verification must fail, not fall back silently. Default prefer-mode success and explicit require/verify failure are separate mandatory tests. No ad hoc cleartext password or MD5 authentication is added to make a driver connect.
- Defer TLS and authentication together to a separately accepted T1 secure-server card. Its proposed direction is a TLS-required server mode with certificate/key configuration and SCRAM-SHA-256 verifiers, using maintained TLS/SASL implementations behind bounded wrappers, not homemade cryptography. No dependency, crypto provider, verifier store or certificate provisioning design is selected here.
- That future mode must forbid trust on an ordinary remote listener, reject insecure startup rather than requesting a password over plaintext, and test downgrade resistance, wrong user/password, expired/untrusted/name-mismatched certificates, malformed/slow TLS/SASL exchanges, cancel registry isolation and bounded pre-auth CPU/memory/slots. Unknown-user and wrong-password paths must avoid distinguishable diagnostics; timing behavior needs a stated threat model. Authentication identifies a principal; RBAC remains a separate feature and must not be implied.
- A request for secure remote use during slice 10 is a stop for owner scope resolution, not permission to widen T0. This proposal supplements the current 0008 boundary if accepted; it does not edit that accepted ADR or promote feature tiers.

## Why

**Reasoned:** default PostgreSQL clients can negotiate refusal in their normal prefer mode; encryption/authentication need an explicit deployment and credential lifecycle contract. Building only a password prompt would create a misleading protection boundary. Read-only access still exposes data, so read-only is no substitute for transport security or authentication.

## Alternatives considered

- **Implement TLS and SCRAM now:** desirable for remote deployment, but expands T0 into feature-map T1 and adds unselected dependencies and credential/certificate handling. Return to the owner if remote deployment is a blocker.
- **Password authentication without TLS, or legacy MD5:** smaller superficially, but does not establish confidential transport; not selected for a new secure mode.
- **Require external TLS termination permanently:** could be a documented deployment option later, but proxy trust and identity forwarding are not designed/tested here; cannot claim native server authentication from it.
- **Remove unsafe-listen in this card:** tighter default boundary, but changes an accepted CLI contract. Recommend owner consideration separately rather than silently editing ADR 0008.

## Evidence

- **Measured:** [baseline](../../bench/results/20261107-pgwire-baseline.md), [server](../../bench/results/20261107-pgwire-server.md) and [reliability](../../bench/results/20261107-pgwire-reliability.md) cover the existing codec, refusal/trust startup, bounded sessions and cancellation. They are not TLS/SCRAM measurements.
- **Reasoned:** tier/scope boundary and future TLS-required SCRAM direction; all future acceptance conditions above.
- **Not covered:** encrypted handshake cost, TLS/SASL parser safety, credential rotation/storage, channel binding, proxy mode, remote threat model, authorization or secure deployment certification. These need the T1 card and independent empirical checks before acceptance of an implementation.

## Confidence

Medium for retaining T0 scope, high that the current mode cannot claim secure remote access. No measured TLS/authentication claim. Revisit immediately if the internal team's adoption requires a remote listener with authenticated access.

## Consequences and revisit when

Card 0007 can ship usable local clients without a crypto dependency. Operators keep an explicit unsafe boundary. The owner must accept the deferral; the T1 card must choose dependencies and deployment defaults with fail-capable conformance/fault/fuzz checks before implementing secure mode.
