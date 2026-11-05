# Verification plan for the proposed ADRs

Written 2026-11-04. Every item is a test, proof or measurement (way-of-working.md, section 6); none is a review. **Gate** = wanted before the ADR is accepted. **Later** = goes under "revisit when". A check counts only if it was shown to fail on a known-bad version.

## ADR 0001: pipeline model

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 1 | The scheduler never loses a shutdown or wake, never runs a task twice, never hangs on cancel | loom model, checked to fail on the old bug and on wake-protocol mutants | local | Gate | **done within bounds** (`bench/results/20261104-loom-scheduler.md`); exchange and early finish not modelled |
| 2 | The protocol scheduler runs Arrow batches with selection vectors and returns DuckDB's results | differential test | local | Gate | **done on x86-64** (`bench/results/20261104-arrow-pipeline.md`); found and fixed the stop-depth bug; loom re-run and speed pending |
| 3 | `LIMIT` and early finish stay correct for random plans | property test against a simple reference | local | Gate | **done** (1,500 random cases, 33 of 33 mutants caught) |
| 4 | Exchange backpressure stays bounded | loom or property test | local | Later | **done within bounds** (`bench/results/20261104-exchange-loom.md`): found a cancel leak (fixed); 31 of 31 models and 16 of 16 mutants on ARM64; the `stop_depth` early finish is also modelled (16 of 16 models, 13 of 13 mutants, `bench/results/20261104-stop-depth-loom.md`) |
| 5 | Push cost with much cheaper kernels, static dispatch, branching pipelines | paired benchmark with a control arm | local, ARM64 about $0.01 | Later | **measured on ARM64; x86-64 recorded but SANITY ONLY** (`bench/results/20261105-timing-arm64.md`, `bench/results/20261105-timing-x86.md`): ARM64 (printed start load 0.72): `dyn-direct` equals `static` in every cheap cell; the scheduler and protocol cost about 70-130 ns per batch for a one-stage chain and about 410 ns for three stages at 1,024 rows, and disappear into the noise from 8,192 rows; the real exchange is 77% faster than a hand-written blocking queue at capacity 1 and 1,024-row batches, 4-29% slower in other cells (2-4 microseconds per batch). x86-64: printed start load 0.83 (the settle rule is below 0.5), so the tables are SANITY ONLY and no conclusion is drawn; a re-run with a longer settle is needed before x86-64 counts as evidence |

## ADR 0002: async runtime and threading

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 6 | The tokio-to-CPU-pool waker bridge loses no wakeup and does not deadlock when the pool is saturated | loom or shuttle, plus a contention stress test | local | Gate | **done within bounds** (`bench/results/20261104-async-bridge.md`); latency and stress to re-run on a quiet machine |
| 7 | A busy CPU pool does not stall I/O, and a slow object store does not flood memory | measurement with a mock slow `object_store` | local | Gate | **done within bounds** (`bench/results/20261104-async-bridge.md`) |
| 8 | `object_store`, `crossbeam-deque` and the compression crates build and test on x86-64 and ARM64 | CI on GitHub's ARM runner | free (public repo) | Gate | needs a commit and push to run |
| 9 | Pool sizing respects cgroup CPU limits | test in a CPU-limited container | local | Later | not started |
| 22 | Fairness: a CPU-heavy query must not starve a concurrent small query or a consumer task (time slice or one morsel per run) | measurement with a starvation control | local | **Gate** (new, from item 6) | **done within bounds** (`bench/results/20261104-fairness.md`); overhead on a quiet machine still to measure |
| 10 | Split design against all-on-tokio | slice 11 bake-off | later | Later | not started |

## ADR 0003: batch format and kernels

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 11 | Kernels never overflow silently, mask errors by selection and validity, and sum exactly | Kani bounded proofs, mutation-checked | local | Gate | **done**: 62 of 62 harnesses, and the proofs are mutation-checked 15 of 15 (`bench/results/20261104-kani-kernels.md`) |
| 12 | The lints catch every banned lenient arrow call; `arithmetic_side_effects` noise on a real crate | negative-test file that must fail clippy; noise count | local | Gate | **done** (`bench/results/20261104-kernel-lints-and-guards.md`) |
| 13 | `WHERE x <> 0 AND 1/x > 1` and `CASE`-style guards work, including through wrapped arrow kernels | differential test against DuckDB | local | Gate | **done** (`bench/results/20261104-kernel-lints-and-guards.md`) |
| 14 | The decimal rules are right in type and value | exhaustive small-precision enumeration against an independent big-integer reference, plus DuckDB | local | Gate | **done, full run on GCP** (`bench/results/20261104-decimal-verification.md`: 56 of 56 mutants, 0 failures) |
| 15 | Joins and a general hash aggregate (many distinct keys, plain strings) cost about what the experiment showed | build both, differential test, benchmark | local, then ARM64 | Biggest evidence gap | **correctness done**: 30 of 30 mutants caught on a clean ARM64 VM, baseline green in every worker (`bench/results/20261104-hash-ops-correctness.md`); the `nullcheck` and DuckDB layers are shown to fail on bad engines (2 of 2 each); **timings and memory measured on settled ARM64 and x86-64 VMs, `overflow-checks` on and off** (`bench/results/20261105-timing-arm64.md`, `bench/results/20261105-timing-x86.md`): the generic engine is 0.76-1.97x (ARM64) and 0.66-1.71x (x86-64) of its own hand-fused loop end to end; DuckDB on the same Parquet files is faster than our engine in 23 and 24 of 28 cells; peak resident memory 296 MiB (`agg_orderkey`, 4 threads) and 338 MiB (`j3`) on x86-64 |
| 16 | The NULL path can drop below 1.3x | word-wise bitmap experiment | local | Later | **correctness and mutation done** (`bench/results/20261104-null-path.md`): tests, check, nullcheck, DuckDB diff and 62 of 62 mutants on a clean ARM64 VM; ARM64 timing on a settled VM (`bench/results/20261105-timing-arm64.md`): penalty at 1% / 10% / 50% NULLs 1.06x / 1.13x / 1.24x for step 6 (step 5: 1.06x / 1.15x / 1.28x; old engine 2.30x / 2.15x / 1.54x), so the 1.3x target is met on that machine; x86-64 (`bench/results/20261105-timing-x86.md`) is **SANITY ONLY** (printed loads 1.48-2.30 before the timed phases, from the script's own earlier steps): step 5 prints 1.30x at 50% NULLs with one thread and 1.33x with four, step 6 1.23x, and the target is not assessed on x86-64 until a re-run with a settle before each phase |
| 17 | Real clients accept `Decimal64` (Arrow IPC / C data interface; Postgres `NUMERIC` later) | test with pyarrow, DuckDB, polars | local | Later | **done** (`bench/results/20261104-decimal-clients.md`): Arrow-native clients accept it, JDBC rejects it so Flight SQL widens to Decimal128; Postgres `numeric` encoder byte-identical to PostgreSQL 16.2 |
| 18 | Compaction before arrow wrappers: what the copy costs | micro-benchmark | local | Later | **done on ARM64 and x86-64, settled** (`bench/results/20261105-timing-arm64.md`, `bench/results/20261105-timing-x86.md`; 270 cells at 10% NULLs): with an index selection, compacting to run one function is never cheaper (30 of 30 cells on both; best 1.24-2.02x ARM64, 1.03-2.10x x86-64); the other break-evens differ by machine: with a bitmap at 1,024 and 8,192 rows compaction for one function pays from about 92-98% selected on ARM64 and never for `Int64` and `Decimal64` on x86-64, and compacting once for three functions with an index selection at 8,192 rows pays from about 18-22% (ARM64) or 98-99% (x86-64) for those types |
| 21 | Re-measure generic-ops and hash-ops with `overflow-checks` on (workspace-wide) | rebuild with the profile, paired benchmark | local, then GCP x86-64 and ARM64 | **Gate** (new, from item 12) | generic-ops done on ARM64 (small effect, `bench/results/20261104-arm64-verify-run.md`) and not re-measured on x86-64; hash-ops measured on settled ARM64 and x86-64 VMs (`bench/results/20261105-timing-arm64.md`, `bench/results/20261105-timing-x86.md`): joins -3.8% to +5.1% on both, grouped aggregates +2.2% to +19.1% (ARM64) and +0.9% to +24.7% (x86-64), and which aggregate pays most differs by machine (ARM64 `agg_partkey` and `agg_modedate`, x86-64 `agg_orderkey`); hand-fused loop -7.9% to +9.1% (ARM64), -3.0% to +11.3% (x86-64) |
| 19 | Why Q1 is further from the fused loop on ARM64; does the V2 core differ | `perf` profile, C4A run | GCP about $0.01, needs a yes | Later | **done** (`bench/results/20261105-perf-v2.md`): the V2 core does not differ from N3 (Q1 generic/fused 1.79x, Q6 0.72x); 62% of generic Q1 time is the dense grouped-sum kernel and 22% the projection pass, none in dispatch; the reason the fused loop is cheaper per row is not shown (no hardware counters) |
| 20 | Results hold on other data: skewed keys, string-heavy, many NULLs, TPC-DS | extra data sets through the same checks | local | Later | **done on ARM64 for skew, strings and NULLs** (`bench/results/20261104-data-shapes.md`; 100 of 100 queries identical to DuckDB); x86-64 and TPC-DS not done |

## How the work is grouped

Agents run at the same time only when their edit scopes are disjoint and none depends on another's unfinished code.

| Agent | Items | Edit scope |
|---|---|---|
| K1 (running) | 11 | `spikes/generic-ops/kani/` |
| L1 (done) | 1 | `spikes/operator-trait/` |
| A (done) | 12, 13 | new `crates/nata-kernels/` (copy of the spike kernels, lints on) |
| B (done) | 6, 7 | new `spikes/async-bridge/` |
| C (done, full run on GCP) | 14, 17 | new `spikes/decimal-verify/` |
| D | 15 | new `spikes/hash-ops/` |

Deferred until K1 and L1 finish, because they would edit the same files: 2, 3, 4, 16, 18. Item 8 needs a push. Items 5, 19 need a cost approval if they use GCP.

Benchmarks on the shared 8-vCPU VM are disturbed by the other agents. Correctness first; timings use interleaved arms with a control and are run when `uptime` shows a low load, and the report states the load.

## Added 2026-11-04 after the scope decision: writes and reads, single-node and distributed, Iceberg first

The owner decided that the engine writes as well as reads, runs single-node and distributed, and targets Apache Iceberg first.
Nothing below has been started. Everything in the table runs on GCP (the owner's rule); each run needs its cost shown and a yes.
Facts about Iceberg and about object-store conditional writes are from memory and must be checked against the current Iceberg
specification and the `object_store` crate before they are relied on.

| # | What to verify | How | Priority | Status |
|---|---|---|---|---|
| 23 | Distributed bugs are reproducible: a deterministic simulation harness (injected clock, network, file I/O, scheduler seed) exists before the first distributed code | spike: run the existing scheduler and exchange under a simulated network with seeded fault injection; a failing seed must replay exactly | **Gate** (architectural: it shapes every later interface) | **done on ARM64 and x86-64** (`bench/results/20261104-sim-harness.md`): 20,000 of 20,000 control seeds, 13 of 13 mutants caught with identical replays, trace hashes identical across the two architectures; the abstraction itself costs about nothing (+0.6% / +1.3%), the fault-point hooks about 100-130 ns per very small batch; built on our own executor (madsim and turmoil not adopted) |
| 24 | The Iceberg commit is atomic and safe under concurrent writers, retries and crashes | TLA+/Quint model of the commit (optimistic concurrency, retry, conflict detection, orphan files); then a linearizability and invariant check (never a snapshot that references a missing or partial file; no lost update) against a real REST catalog and against conditional object-store writes, with injected crashes | **Gate** | not started |
| 25 | What we write, others read, and the reverse | interop matrix: tables written by Nata read by DuckDB, pyiceberg and a reference engine, and the reverse; schema and partition evolution; snapshot isolation; sorted by the Iceberg spec's own test vectors for the partition transforms (bucket uses murmur3, truncate, year/month/day/hour) | **Gate** | built (`spikes/iceberg-plan/`): partition transforms match the spec's vectors; tables we write are read identically by DuckDB (40 of 40) and pyiceberg; tables pyiceberg writes are read identically by us (35 of 35); our transforms match iceberg-rust's partition values (4,192 checks); findings: truncate overflow loses rows in DuckDB and pyiceberg, -0.0/NaN semantics differ across engines; GCP runs `icebergplan` on ARM64 and x86-64: interop both ways passes, planning at 1M files measured; mutants rerun pending (harness defect) **GCP, 2026-11-04, clean ARM64 and x86-64 VMs (`bench/results/20261104-iceberg-plan.md`): tests and both interop directions pass identically on both (80 of 80 tables read identically by DuckDB, 71 of 71 pyiceberg-built tables read identically by us, 1,065 of 1,065 filter counts equal DuckDB); the truncate-overflow hazard reproduces; the rest of the matrix (the REST catalog and the commit protocol) still depends on the `iceberg` run (item 24).** |
| 26 | Planning an Iceberg scan is correct and fast: manifests, manifest lists, partition and column-stat pruning, position and equality deletes, deletion vectors (v3, if targeted) | property test with a reference: pruning must never drop a file that holds a matching row (soundness); planning time and memory on tables with 10k, 100k and 1M data files | **Gate** | built (`spikes/iceberg-plan/`): sound on 60 random tables and 2,400 filters, 22 of 22 mutants caught; planning at 1M files locally: 0.2 s when partitions prune, 4-6 s when they cannot; GCP runs `icebergplan` on ARM64 and x86-64: interop both ways passes, planning at 1M files measured; mutants rerun pending (harness defect) |
| 27 | The object-store read path works with real latency: request coalescing, prefetch, retries, in-flight cancellation, memory bound, tail latency | scan spike over Parquet in a bucket in the same region, against DuckDB on the same files | **Gate** | **simulated store done on ARM64 and x86-64** (`bench/results/20261104-object-store-scan.md`): 132 of 132 correctness checks, memory bound holds (unbounded control exceeds it), cancellation stops outstanding requests (known-bad versions caught), bounded retries with clean failure (known-bad versions caught); parallel requests are 4-6x faster than one at a time; DuckDB over the same store is faster than our best policy. **A real bucket was NOT measured** (needs the owner's access decision) |
| 28 | The network exchange: framing, backpressure, flow control, cancellation, ordering, throughput, behaviour when a worker dies mid-query | spike on 2-8 VMs, with fault injection; compare with the in-process exchange; deterministic simulation (item 23) for the fault cases | **Gate** | not started |
| 29 | The failure model of a distributed query is stated and holds: task retry is idempotent, results are not duplicated, cancellation reaches every node, no orphaned spill or shuffle data | TLA+/Quint model of the query lifecycle; fault-injection tests; a stated rule for what a failed task, a failed worker and a failed coordinator do | **Gate** | model written; small bounds PASS and 12 of 12 mutants caught on the dev box; ADR 0004 drafted; GCP runs `failmodel` (ARM64, 2026-11-04, three attempts): every configuration passes (safety and liveness, up to 2 tasks with 3 workers, 61.5 M states), all 6 canaries trigger, all 12 mutants are caught; ADR 0004 drafted (`spikes/failure-model/README.md`) |
| 30 | Writes produce correct files: Parquet round trip with other readers, column statistics are SOUND (min/max truncation for strings), field ids, name mapping, file sizing, sort order | differential against a reference writer and reader; property tests on statistics | Gate | built (`spikes/parquet-write/`): all gating checks pass on the dev box (100,000 rows, 10 writer configurations, DuckDB round trip and pruning suite, 11 of 11 bad-statistics files flagged); found that parquet-rs writes NaN min/max for all-NaN float chunks (mitigated); GCP runs `parquetwrite` on ARM64 and x86-64 pass (500,000 rows, 163 rolled files) |
| 31 | The operator protocol handles sort and top-N with spill, outer/semi/anti joins, window functions, TPC-H Q18 and Q21 | extend the existing spike; differential against DuckDB; mutants | Gate (was found missing in the first review) | built and checked on the dev box (tests, SF1 vs DuckDB incl. spill, 21 of 21 mutants caught); protocol findings in `spikes/operators-beyond/README.md`: no operator end-of-input hook, a sink shared by two pipelines is finalized twice; GCP runs `opsbeyond` on clean ARM64 and x86-64 VMs (2026-11-04, SF1): build, unit tests, the full DuckDB differential check (31 and 30 comparisons, spill, 1/2/8 tasks) and 21 of 21 mutants all pass on both; no timings yet |
| 32 | Scale: 32-64 vCPUs, SF10-SF100, larger than memory, spilling join and aggregate under a 1,024-file limit | GCP large Spot VM | Gate | not started |
| 33 | A differential SQL fuzzer with DuckDB as the oracle (NULL, NaN and -0.0, dates and time zones, Unicode, collation, ordering), shown to catch known-bad engines | build once, use on every later slice | Gate | built (`spikes/sqlfuzz/`): control clean, 28 of 28 known-bad engines caught on 3 seeds on the dev box, 14 tests; GCP runs `sqlfuzz` on ARM64 and x86-64 pass (tests, 28 of 28 faults on 3 seeds, 2 x 60,000 clean queries) |
| 34 | Engineering checks: sanitizers on the real code, `cargo-deny`/`audit`/`geiger`, benchmark regression harness, our verification tools in CI | CI plus a scheduled GCP job | Later (in parallel with the first slices) | not started |
| 35 | Compatibility with real clients (Flight SQL, ADBC, JDBC, Postgres wire) end to end | test with the real clients | Later | not started |

New ADRs the scope decision calls for (not yet written): table format and catalog (Iceberg first); distributed execution and its
failure model; the testing strategy (deterministic simulation, fault injection, differential fuzzing).

## GCP run plan for the items that had none (written 2026-11-04)

**Working rule from the owner, for now: one agent at a time in the shared VM, no parallel subagents.** The "Agents run at the same
time" table above is suspended; the items below are written and checked one after another, and a spike's GCP run is started only
after its code has passed its local checks. Nothing here has been run or approved. The dollar figures are ROUGH estimates
(`prices.env` rate x hours, plus about $0.01-0.04 for disk and IP); the binding number is the worst case that
`orchestrate.sh plan` prints, and every run still needs its cost shown and a yes.

### Order (each row needs the earlier ones it names)

| Step | Item | Written locally first | GCP run (job name, machine, time) | Rough cost | Depends on |
|---|---|---|---|---|---|
| 1 | 29 Failure model | TLA+ model of the query lifecycle (task retry, worker death, coordinator death, cancellation, orphaned spill) plus its mutants; one short ADR draft | `failmodel`: `n4a-highcpu-16`, TLC with a 1 h budget and a mutant sweep | $0.25 | none (no code) |
| 2 | 31 Operators beyond scan/filter/aggregate/inner join | `spikes/operators-beyond/`: sort and top-N with spill, left/right/full, semi and anti joins, window functions, Q18 and Q21; differential vs DuckDB; mutants | `opsbeyond`: ARM64 `n4a-highmem-8` (spill needs memory control) 2 h, then x86-64 `c3d-highcpu-8` 1.5 h | $0.35 + $0.11 | none |
| 3 | 33 Differential SQL fuzzer | `crates/sqlfuzz/` or a spike: grammar-based generator (NULL, NaN, -0.0, dates, time zones, Unicode, ordering), DuckDB oracle, shrinker | `fuzz`: `n4a-highcpu-8`, 3 h campaign, then 1 h proving it catches the known-bad engines (the hash-ops and null-path mutants) | $0.35 | 2 (so it can fuzz the new operators) |
| 4 | 30 Write correctness | `spikes/parquet-write/`: writer with field ids, sorted output, statistics; round trip against pyarrow, DuckDB and parquet-rs; property tests on min/max truncation | `parquetwrite`: `n4a-highcpu-8` 1.5 h, then x86-64 `c3d-highcpu-8` 1 h | $0.17 + $0.07 | none |
| 5 | 25 Iceberg interop (rest of the matrix) | extend `spikes/iceberg-commit/`: partition-transform test vectors, schema and partition evolution, files read by DuckDB and pyiceberg and the reverse | `iceberg-interop`: `n4a-highcpu-8` 1.5 h (after the `iceberg` run passes) | $0.16 | `iceberg` run (group A) |
| 6 | 26 Iceberg scan planning | `spikes/iceberg-plan/`: generator of metadata-only tables with 10k, 100k, 1M files; planner with partition and column-stat pruning and position/equality deletes; soundness property test against a brute-force reference | `iceberg-plan`: shake-out at 10k on `n4a-highcpu-8` 0.5 h; then full on `n4a-highmem-16` 2 h (memory at 1M files is a measurement) | $0.05 + $0.62 | 5 |
| 7 | 28 Network exchange | `spikes/net-exchange/`: framing, credit-based backpressure, cancellation, worker kill, on the simulation harness | stage A `netexch-sim`: one VM, 2-8 nodes in network namespaces with `tc netem` loss and delay, `n4a-highcpu-8` 1.5 h; stage B `netexch-real`: 4 x `n4a-highcpu-8` in one zone, 1 h (needs a multi-VM mode in `orchestrate.sh`, which does not exist yet, see "Prerequisites") | $0.16 + $0.45 | 1, `simharness` run |
| 8 | 34 Engineering checks | CI workflow files, `cargo-deny`, `cargo-audit`, `cargo-geiger`, benchmark regression harness (a baseline file plus a threshold), sanitizer and Miri jobs for the real crates | `eng-checks`: `n4a-highcpu-8` 1.5 h (ASan, TSan, Miri, UBSan on the Arrow pipeline and kernels; nightly toolchain) | $0.16 | none |
| 9 | 32 Scale | no new code beyond a harness: run the existing pipelines at SF10 then SF100 with spill forced by a memory cap and a 1,024-descriptor `ulimit` | stage A `scale-sf10`: `n4a-highcpu-32` 1 h (400 GiB-equivalent disk not needed, 60 GB); stage B `scale-sf100`: `n4a-highcpu-32` up to 5 h with a 300 GB disk, only if A passes; also the cheap Q1/Q6 scaling check at 32 and 64 vCPUs (1 h) | $0.43 + $2.1 + $0.45 | 2 (spill code), 3 (to check results) |
| 10 | 35 Real client compatibility | nothing yet: needs a server, which does not exist | none planned until a Flight SQL or Postgres-wire slice exists; then `clients` on `n4a-highcpu-8` 1 h with real ADBC, JDBC and psql clients | $0.12 | a server slice |

Rough total if every run passes the first time: about $6.2, of which scale is about $3. Expect a retry on the first run of most
spikes (the earlier runs needed one), so budget about $8 and still approve each run separately.

### Prerequisites to write before the runs

- **Multi-VM mode in `orchestrate.sh`** (only for step 7B): N VMs with one label, a shared private network, one data disk each, and a
  hard cap on the total VM-hours. Until it exists, step 7B stays unplanned and 7A answers most of the design questions.
- **A large-disk path for step 9**: the data disk is 30-100 GB today; SF100 needs about 300 GB, Hyperdisk Balanced or pd-ssd. The cost
  bound must include it (about $0.04 per hour at that size).
- **A `recover --run-id` subcommand** (see `scripts/gcp/README.md`): worth doing before the long runs (steps 7B and 9), because a
  five-hour run is the case where the orchestrator's machine going down hurts most.
- **Pulling only small files**: all new jobs must keep large data under `/mnt/work/data` and never under `$RESULTS`.

### Per-item decisions that are not GCP work

- ADRs for table format and catalog, distributed execution and its failure model, and the testing strategy are written locally. The
  failure-model ADR comes with step 1, the table-format ADR after steps 5 and 6, the testing-strategy ADR after step 3.
- Item 35 is deliberately not given a GCP run now. Planning one before a server exists would only produce a stub.
- Steps 1 and 2 are the ones that can still change the protocol; steps 4 to 8 are expected to confirm or tune.
