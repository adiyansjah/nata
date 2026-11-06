# NNNN: Slice title

Status: draft | accepted (by <owner>, <date>) | done (<date>) | dropped
Date: YYYY-MM-DD
ADRs: 000X (relies on), 000Y (creates or amends)

Rule: `docs/way-of-working.md`, section 2 ("The slice verification card"). Keep this short; link detail instead of copying it.
A small slice gets a short card; a trivial slice may be one line per row. A row may say "N/A, because ...", never be missing.

## 1. Goal

What the slice delivers, in one or two sentences, and the example query or behaviour that shows it works.
If a decision it needs is still open, say which, and link the baseline spike (part 4).

## 2. Quality targets

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | | | |
| Performance | | | |
| Scalability | | | |
| Reliability | | | |
| Fault tolerance | | | |
| Resource bounds | | | |
| Portability | ARM64 and x86-64, clean VM | | |
| Compatibility | | | |
| Security and privacy | | | |
| Operability | | | |

## 3. Decision rule (fixed before measuring)

The threshold that settles the open question, written down BEFORE the numbers exist.

## 4. Baseline spike (only if needed)

Required for: a build-versus-reuse choice, a performance number that drives a design, a concurrency or protocol change, a new
external format, or a failure behaviour not yet stated. Otherwise write "none needed: baseline is <link>".

- Question:
- Smallest experiment, with its control arm:
- Time box and cloud-spend estimate (needs the owner's yes, `way-of-working.md` section 8):
- Result file: `bench/results/...`

## 5. Carried-in improvements (from `docs/improvements.md`)

Every open register item on this slice's code path: **taken in** (decision rule fixed before measuring, and its check) or **deferred**
(reason, and the slice that takes it). Taken-in items: at most about a quarter of the slice's work.

| Register item | Taken in or deferred | Decision rule / reason | Check |
|---|---|---|---|

## 6. Closeout (fill in when done)

- Results file:
- ADR updated (amendment or revision note), confidence and what changed:
- Benchmark added to the regression set, baseline numbers:
- Deviations from this card, and why:
- Spike code deleted:
- Improvement register updated (items done, new findings added):
