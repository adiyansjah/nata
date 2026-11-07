# 0008: Engine performance before sort

Status: draft
Date: 2026-11-08
ADRs: 0001–0003, 0006–0010 (relies on); dated amendments if measured decisions change

Rule: [way of working §2](../way-of-working.md#2-definition-of-done-every-slice). Card numbers are not feature-map slice numbers.

## 1. Goal

Finish the engine performance work after feature-map slice 5 closes in Track A, before slice 6 (sort) starts. This is the owner's
2026-11-07 decision. The detour unblocks sort, join and windows: they must inherit a measured engine and a speed budget, rather
than compound unexplained scan, expression and aggregate costs. Track B/C work is not absorbed here.

Run scan, filter, project and aggregate queries through shared vectorized strict kernels, including the remaining text/date/float
conversion adapters. Prove the production functions within stated bounds. Profile each query's whole path on x86-64 c3d and
ARM64 n4a. Repair scaling and metadata overhead, resolve the historical scan flag, and install a repeatable regression gate.

[Card 0005](0005-hash-aggregate.md) retains steps **2b** (whole-path/fusion, P4), **2c** (proved bounds/check elision, P5) and **2d**
(spill volume). They are prerequisites, not work packages in this card. Import their final evidence and deviations at entry.
This card's per-query profiles cover the broader slice 2/4/5 workload after those steps. It does not repeat them.
The five owner decisions in card 0005 remain binding, including exact integer SUM/AVG, unordered grouped output, DISTINCT scope,
the memory default and the spill-format decision.

| Existing evidence | What it establishes | What it does not establish |
|---|---|---|
| [Performance steps 1 and 2a, including settled timing at the end](../../bench/results/20261107-hash-aggregate-perf.md) | Step 1 removes hashing, cancellation and reservation overhead. Settled step 2a improves Q1 on both CPUs; its frozen targets remain NOT MET. Text/date/float adapters remain open | The shared-machine local sanity is not a regression verdict. SF1 results do not settle SF10 scaling or all-kernel proof coverage |
| [Slice 5 reference](../../bench/results/20261107-hash-aggregate-reference.md) and [frozen card](0005-hash-aggregate.md#2-quality-targets) | Existing aggregate latency, spill and architecture-specific scaling rules | No replacement denominator for a later session |
| [Slice 4 closeout](../../bench/results/20261106-slice4-close.md) | P12 metadata-path miss and P13 historical scan flag | Neither attributes the cause to a particular code change |

**Not in scope:** the write path; new features from feature-map slices 6–13 (including sort, join and windows implementation);
new wire/client features; a scheduler/runtime replacement; JIT, hand-written SIMD or new dependencies unless named in an accepted
brief. This card selects no new dependency. Existing dependencies stay at their locked versions. Future briefs must explicitly name
any promotion from `Cargo.lock`, require `cargo deny` and record its unsafe code. An absent lockfile crate is a stop-and-report.

## 2. Quality targets

New numeric budgets are **proposed, frozen by the reference run** in part 4, before new Nata timing. They are design proposals,
not measured claims. Inherited frozen targets stay fixed. No averaging across queries or CPUs can hide a failed cell.

| Aspect | Target / budget | Check (and oracle) | Baseline to beat or match |
|---|---|---|---|
| Correctness | One production semantic implementation for supported arithmetic, comparison, boolean and casts, across numeric/text/date/float types; exact values, NULLs, selection and positioned errors unchanged | SQL differential against DuckDB for shared semantics; independent scalar/exact rounding oracle for dialect differences. Production-path Kani proofs with covers and explicit domain bounds. Mutants for ignored selection/NULL, overflow, rounding, NaN/±0, invalid date/text and selected-out error must fail. List unproved domains; do not call bounded proof universal | Slice 2 kernels, slice 4 SQL and slice 5 aggregate suites; card 0005 exact decimal contract |
| Performance | T1: scan/filter/project whole-query time ≤1.5x paired DuckDB. T2: full Q1 ≤1.5x; agg_orderkey/agg_partkey and matched-budget aggregate spill ≤2x, inherited from card 0005. T3: P12 example SQL/scan ≤1.2x. T4: no per-cell regression above the gate's 10% tolerance | Correctness before alternating timings; whole CLI processes with equivalent output sinks and separate decoded-batch timings. Per-query profiles identify remaining costs. Each cell must pass its own rule below | Same-session DuckDB, `nata scan` for P12, and slice 5 closing binary; historical results above are context only |
| Scalability | T5: Q1 SF10 8/1 speedup ≥3.5x x86-64 and ≥5x ARM64, inherited frozen rules; measure 1/4/8 threads for all workload families | Same input/output at each thread count; paired DuckDB sweep, engine and output phases separated. Serialized-work mutant must breach a phase-parallelism check; a ratio below either floor fails | [Frozen reference](../../bench/results/20261107-hash-aggregate-reference.md); do not restore the rejected common 5x x86 floor |
| Reliability | No new hang, panic or retained task/grant/file on cancel, LIMIT, error or early output finish | Delay/CPU-contention stress at batch/conversion/metadata boundaries; existing loom models for changed concurrent paths. Lost-cancel/wake mutant fails; deadline expiration is a failure | Slice 4/5 cancel, scheduler and cleanup checks |
| Fault tolerance | Missing, changed, corrupt or truncated input and allocation denial fail clearly, with no successful partial answer; footer reuse cannot bypass schema/fault checks | Inject replacement/schema mismatch between bind and execute, I/O failure and malformed metadata. A stale-footer/schema-check-removal mutant fails; preserve spill error/cleanup checks | ADR 0004 local-query rules, card 0005; no distributed retries introduced |
| Resource bounds | Preserve slice 5's frozen query B, RSS allowance, disk quota, FD and recursion bounds; account for adapter vectors and retained footers. No larger defaults to buy speed | Fresh-VM default limits plus inherited low-FD/forced-spill cases; measured RSS alongside grants. Uncharged-vector and missing-eviction/cap mutants must fail | Slice 5 closeout records actual frozen limits; unresolved limits block entry rather than becoming invented allowances |
| Portability | Every verdict on clean x86-64 c3d and ARM64 n4a; no architecture fallback to a second scalar semantic implementation | Same committed source, queries, proof coverage inventory and semantic vectors; record physical/logical cores, SMT, compiler and CPU features | Existing both-CPU evidence above; neither CPU substitutes for the other |
| Compatibility | SQL types, error positions, grouped ordering and CLI/default-client behavior unchanged | Existing differential/dialect and default-client corpora on changed types; independent PostgreSQL wire oracle where needed. Retain explicit R22 normalization and its original outcome | Card 0005 owner decisions and client matrix; protocol speed is outside T1/T2 |
| Security and privacy | No project IDs, bucket names, IPs, emails, keys or cell values in committed evidence/logs/errors | Redact machine identities; staged/result leak scans and planted-secret rejection; footer caching preserves private query ownership | `scripts/check-leaks.sh --staged`; inherited wrapper/fuzz protections |
| Operability | T6: whole-path profile and reproducible verdict for every timed query on both CPUs; T7: CI/nightly gate catches a known slowdown; T8: successor budgets recorded | Missing profile/cell/command fails completeness. Keep raw timings, perf samples/loss, stage attribution, input/binary hashes and config. Plant a slowdown above tolerance and a noisy twin; require FAIL and UNRESOLVED respectively | Existing bench/profile scripts and W7; EXPLAIN ANALYZE counters are cross-checks, not a substitute for whole-process profiles |

## 3. Decision rules (fixed before measuring)

Let R be median paired Nata/DuckDB elapsed-time ratio, computed from alternating rounds, not unrelated historical medians.
All speed rules require correct results and unchanged resource bounds. A reference failure/OOM has no denominator; mark that cell
UNASSESSED and report it. Noise is UNRESOLVED, never MET. No threshold/default changes after observing candidate timings.

| Target | Decision rule | Check that can fail |
|---|---|---|
| T1 scan/filter/project | Proposed R ≤1.5 at SF1/SF10, 1/4/8 threads, each CPU and each manifest query; equivalent startup/planning/decode/output scope | Any valid cell above 1.5 fails. Record execution-only and output costs separately; do not substitute a reduced kernel result for a CLI miss |
| T2 aggregate | Preserve card 0005: Q1 ≤1.5x, agg_orderkey/agg_partkey ≤2x, SF1/SF10, 1/4/8 threads; spill ≤2x only where matched-budget DuckDB completes | Replay the card's final query/budget matrix, including its high-cardinality HAVING fixture. Any measured miss remains a miss, even if slice 5 closed with a deviation |
| T3 P12 | Preserve SQL/scan ≤1.2 for the slice 4 example, SF1/SF10, 1/8 threads, both CPUs. Retain reuse only with identical validation and bounded ownership | Paired old/new with a disable-reuse control, count footer reads and profile bind/decode. Duplicate-footer mutant exposes the extra read; stale-schema mutant exposes wrong reuse |
| T4 regression and P13 | Proposed candidate/accepted baseline ≤1.10 per cell under the noise rule below. P13 additionally replays the historical ARM64 SF1 one-thread scan case and preserves its original 20% flag | Same-session historical/current source comparison with identical data/bytes read; run the unchanged historical gate. Only a reproducible fix or demonstrated session-noise cause resolves P13; an unexplained flag stays open |
| T5 scaling | Inherit SF10 Q1 floors: x86-64 ≥3.5x, ARM64 ≥5x, 8 versus 1 thread; no CSV-output scaling claim used as engine scaling | A failed floor on either CPU fails. Record CPU utilization, waits, merge/output shares and the paired DuckDB sweep |
| T6 shared kernels and profiles | Consolidation is unconditional. Inventory every production adapter to a shared kernel and its actual proof harness/domain or explicit trust gap. Keep a speed optimization only beyond twin noise on both CPUs, without per-cell regressions beyond noise | SQL-path trace/coverage plus proofs/covers/mutants; no unused proved helper counts. Each timed query has end-to-end stacks and stage costs for bind/metadata, scan/decode, conversion, filter/project, aggregate/merge/spill when present, scheduler/waits, allocation and output |
| T7 gate | Proposed 10% per-query regression tolerance; no rebaseline without an explicit owner decision and retained old verdict | Known >10% slowdown fails on settled controls; output mismatch fails before timing; missing/noisy data cannot pass |
| T8 slices 6–8 speed budget | Proposed whole-query R ≤2 for sort/TopN, hash join and windows, in memory and matched-budget spill where DuckDB completes, both CPUs, SF1/SF10, 1/4/8 threads | Successor cards must copy the frozen ratios and name representative queries before implementation. A missing budget or any valid cell >2 fails that card; operator-only wins cannot excuse a whole-query miss |

### Regression rule for CI and nightly

Use a versioned manifest frozen in part 4. It includes the slice 2 arithmetic/comparison/boolean/cast batch cases (including
text/date/float adapters), slice 4 scan-only, pushed filter, expression projection and example SQL, plus full Q1, agg_orderkey,
agg_partkey and high-cardinality HAVING from slice 5. Freeze SQL hashes, data hashes/layout, budgets, output sinks and baseline SHA.
No cherry-picked geometric mean. Shared-runner CI runs correctness and SF1 trends per commit; its latency flags request a settled
check, not a hardware-independent speed verdict. Nightly runs the full gate on both settled CPU types, including SF10 and spill.
A missing approved runner/run is NOT RUN and blocks performance closeout; this card authorizes no cloud spend.

Proposed sampling rule: one untimed warm-up per arm, then five alternating rounds with baseline, candidate and DuckDB, each with
an identical twin. Reverse arm order each round. Use one settled VM per CPU, no competing heavy jobs, matched threads/cache/data
and process boundaries. Cold-cache diagnostics are separate, never pooled into the warm gate.
For each arm let twin noise be `median(abs(primary_elapsed / twin_elapsed - 1))`. Let N be the largest arm's twin noise.
If N >5%, the cell is UNRESOLVED. Otherwise a regression fails when median paired candidate/baseline >1.10 **and** at least four
of five paired ratios exceed 1.10. It passes only when the median and at least four ratios are ≤1.10. All other outcomes are
UNRESOLVED. The 5% noise ceiling and 10% tolerance are proposals to freeze, not observations. T1/T2/T3/T8 likewise require the
median and at least four paired ratios within their ceiling; T5 requires the median and four speedups above its floor.

One complete retry of an unresolved cell is allowed after re-settling, only if the brief authorizes it and any cloud cost is approved.
Keep both attempts; the retry is decisive only if settled and conclusive. A FAIL is not retried into a pass. Remaining uncertainty
stops and reports. Store min, median, quartiles, all round/twin ratios, loads, exits and limits; never discard an inconvenient round.

## 4. Baseline spike (required)

| Step | Work and control | Exit check |
|---|---|---|
| Entry | Confirm slice 5 closed; import steps 2b/2c/2d, frozen bounds, final SHA and deviations. Reconcile the live register with part 5 | A missing prerequisite stops Track A here; do not start sort |
| Baseline spike | Time box one working day. Prepare the manifest from the existing slice 2 kernel cases, `crates/nata-bench/scan-bench.sh`, `sql-bench.py` and card 0005's final harness. Use DuckDB and generic/independent scalar controls; validate the gate on synthetic timing records | Every target has a query/control, timing boundary and failing negative control; unresolved choices return to the orchestrator |
| Reference run | On clean c3d/n4a, measure DuckDB for all supported cells and planned sort/join/window fixtures; record bounds and timing floors. Successor fixtures cover full sort and TopN, inner/left/semi/anti join, ranking/frames/lag-lead with partitioned spill. They run in DuckDB only here | Record reference numbers and explicit exclusions. Owner freezes proposed targets, tolerance, noise rule and successor fixture hashes in this card before any new Nata timing. Infeasible proposals return for decision before that boundary |
| Walking skeleton | First runnable change exercises one query per workload family, every adapter family, the gate and negative controls, real default clients, forced spill, low limits and a local SF1 smoke against the reference | Every closeout check is wired in thin form, including touched fuzzers at nightly size on approved runners; missing checks stop implementation |
| Implementation | Shared proved/vectorized kernels, metadata reuse, scaling repair and regression integration, with tests/proofs/mutants alongside each change | No skipped semantics or bounds to meet speed. Build-versus-reuse follows ADR 0003; a new choice needs its own bake-off and accepted amendment |
| Clean-VM checks | Full manifest and per-query whole-path profiles on clean c3d x86-64 and n4a ARM64, default fresh-machine limits; use 8-vCPU instances for comparable sweeps | All quality rows have a verdict per CPU. Heavy measurements/checks stay off the development VM; local trial and §8 cost approval precede cloud jobs |
| Closing commit | Trigger nightly by hand on the exact closing implementation commit: full touched fuzz campaigns, SQL subset at full nightly size, loom at bound 3 and engineering checks | Pass, or record each failure as an owner-visible deviation with a register item. Any code fix invalidates this certification until rerun |

Planned result: `bench/results/YYYYMMDD-engine-performance.md`, with redacted raw data and commands. No file or measured result is
created by this draft. Cloud estimate is pending the manifest and machine plan; §8 requires a separately costed owner approval.
Future sort/join/window cards inherit T8 after this reference freeze and add their own correctness/resource/scaling checks. If their
later matching reference shows a problem, stop for an explicit card decision before Nata timing; do not silently relax the budget.

## 5. Carried-in improvements (from [the register](../improvements.md))

**On-path blockers come first.** R12/P11, P12/P13 and the regression gate are this slice's purpose, not optional quarter-cap debt.
**Growth triggers payback.** At entry count slice 5's new register items. If more than three were added, take the oldest remaining
on-path items up to about a quarter of the work; do not defer them for lack of room. Reconcile this table before acceptance and again
at entry. A scope conflict returns to the orchestrator. Register entries can lag later evidence; import results, not stale verdicts.

| Register item | Taken in or deferred | Decision rule / reason | Check |
|---|---|---|---|
| R12 / P11 shared strict kernels and vectorization | **taken in, blocker** | T6, including remaining text/date/float conversion and comparator adapters; keep semantic consolidation even if a faster loop loses | Production call-path inventory, bounded Kani/covers with mutants, independent scalar/SQL differential and paired T1/T2/T4 |
| P12 binder footer handoff | **taken in** | T3, unchanged 1.2x SQL/scan target; no skipped schema/fault checks | Footer read counters, paired disable-reuse control, mismatch/I/O mutants and retained-memory bound |
| P13 historical ARM64 scan flag | **taken in** | T4 paired attribution; preserve the historical 20% flag, no cross-session code claim | Historical/current binaries in one session, same bytes/cache/threads, twins and whole-path profile; unexplained stays open |
| W7 nightly performance trend | **taken in** | T7; shared CI trends, settled both-CPU verdicts | Planted slowdown FAIL, noisy twin UNRESOLVED, saved per-commit samples and baseline identity |
| P4 / P5 / spill-volume finding | **prerequisite in card 0005, not duplicated** | Steps 2b/2c/2d close there; import evidence or explicit deviations. Do not reopen check elision or spill-format tuning as an unnamed child task | Read final result and reproduce only through this card's broader regression matrix; unresolved blocking work returns to slice 5 |
| P9 Q18 serial step | **aggregate-only prerequisite in 0005; full query deferred to slice 7** | Preserve slice 5 aggregate diagnosis; Q18 joins/sort do not exist in this scope | Profile aggregate fixture here; full Q18 control and attribution in slice 7 |
| R1 memory manager | **slice 5 prerequisite; preserve here** | No second memory design; inherited bounds are blockers for performance changes | Real RSS/grants/FD/disk checks and cancellation models on changed paths |
| R10 division proofs | **deferred to next kernel-proof slice with a suitable solver** | Accepted trust point, not a claim of proof; no unbounded solver project here | Retain guards, exact independent division/rounding tests and listed proof domains |
| R6a/b/c/d Parquet dependency risks | **deferred to next Parquet dependency change, track B/card 0006** | No speculative upgrade; keep bounded wrappers and known metadata/header gaps. Any blocker stops for parent | Existing damaged-Parquet nightly and pinned EOF/length/panic regressions; no claim all gaps are fixed |
| P14 / P16 protocol encoding and fair target; R22 ADBC wrapping | **deferred to track C/card 0007; retain harness normalization** | Separate protocol cost in profiles; engine speed does not certify wire throughput. No protocol redesign here | Existing default-client checks and original/normalized R22 outcomes |
| P3 / R2 / R3 join parallelism, repartitioning, FDs | **deferred to slice 7** | Join code outside scope; T8 constrains its future performance | Slice 7 parallel build, skew/spill, low-FD and paired DuckDB checks |
| P6 / P7 / R23 remote I/O, manifests, dependency weight | **deferred to track B/card 0006 and slice 12** | This baseline is local Parquet; no remote planning or dependency project | Their cards' range/manifest and dependency audits |
| P8 exchange batching | **deferred to first distributed slice** | No exchange/runtime redesign | Batch-size control and bounded queues in that slice |

Other open items on write/distributed-only paths remain with their register destinations. If profiling exposes a new on-path blocker,
record evidence and stop for a scoped brief; do not silently grow this card.

**Child stop-and-report rule:** each brief names paths, dependencies, thresholds/defaults, time box and allowed reruns. Stop before
work outside it, a new dependency, a threshold/default change or an extra rerun. No child launches cloud jobs without the human's
cost approval. Report each verdict as the card rule, measured numbers, verdict the rule gives and reproduction command. Out-of-scope
findings are proposals, not completed work.

## 6. Closeout (fill in when done)

- Results file: pending; one verdict per target/query/CPU, raw samples, commands, hashes and profile coverage.
- ADR updated: pending; dated amendments only, measured versus reasoned claims, confidence and remaining proof domains.
- Benchmark added to regression set, baseline numbers: pending; frozen manifest, accepted SHA, both-CPU numbers and failing gate controls.
- Successor budgets: pending; frozen T8 ratios and fixture hashes handed to feature-map slices 6, 7 and 8 before they start.
- Deviations from this card, and why: pending; retain all misses/unresolved cells and owner decisions, without retroactive thresholds.
- Spike code deleted: pending; retain results and reusable regression harness only.
- Improvement register updated: pending; close taken-in items only with evidence; add every new performance/reliability finding and destination. Apply growth-trigger payback to the next on-path card.
- Closing-commit nightly and clean-VM checks: pending; record exact SHA and both-CPU outcomes. This draft runs no build, test or cloud job.
