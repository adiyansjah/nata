# Open verification, and what can run in parallel

Written 2026-11-06 by the assistant from `docs/decisions/verification-plan.md` (re-read on this date) and the run records in `bench/results/`. This is a working list, not a decision record. Cost figures are rough estimates (earlier run costs and `scripts/gcp/prices.env`); the binding number for any GCP run is the worst case that `scripts/gcp/orchestrate.sh plan` prints, and every run needs its cost shown and the owner's yes.

## 1. Is anything needed before development starts?

For the read path: no. Every design choice the read path depends on has evidence (push pipeline, CPU pool, strict kernels, decimals, NULL handling, reuse of `parquet-rs`), and each slice carries its own verification card (`docs/way-of-working.md`, section 2). Slice 3 (Parquet scan) is already under way.

ADRs 0001, 0002, 0003 and 0006 and rules 1 to 7 of ADR 0005 were accepted by the owner on 2026-11-05; ADR 0004 waits for its acceptance condition (section 6).

What must not be skipped later:

| Development | Needs first |
|---|---|
| Write slice (Iceberg) | item 24 code gate (a refinement check or an equivalent test tying the code to the model); the owner's truncate-overflow decision (recommended: refuse the write with a clear error); the operation-token rule (already decided by the owner on 2026-11-05) |
| Sort, window, outer joins | item 31 protocol gaps settled: no operator end-of-input hook; a sink shared by two pipelines is finalized twice (`spikes/operators-beyond/README.md`); check ADR 0001 |
| Distributed execution | item 28 stage B (separate VMs; needs a multi-VM mode in `orchestrate.sh`); item 32 scale |
| Any scale claim | item 32 (SF10 to SF100, 32 to 64 vCPUs, spill, 1,024-file limit) |

## 2. Gate items not fully verified (status on 2026-11-06)

| Item | Unverified |
|---|---|
| 24 Iceberg commit | Model checked within bounds only (3 writers, one operation each); no refinement check between model and code; equality deletes and deletion vectors not modelled; no real S3/GCS/Azure conditional writes; Polaris 1.8.0 ran in a test configuration only |
| 32 Scale | Not started. Needs `recover --run-id` and a large-disk path first |
| 28 Network exchange | Stage B (separate VMs) not started. Stage A caveats: one VM, not a real network; raw rows, not Arrow batches; the 10 ms + 0.05% loss profile varies about 9x |
| 21 Overflow-checks cost | Generic-ops not re-measured on x86-64 |
| 31 Operators beyond | The two protocol gaps above; confirm ADR 0001 fixes them |
| 5, 16 | Push-cost, NULL-path phases after the first, and the Arrow engine-only block missed the 0.5 settle rule on both machines: sanity only |
| 11, 15 | Step-6 "cannot overflow" bound (i128 sums of fewer than 2^64 addends of at most 2^63) argued, not proved; five Kani crate harnesses unproved; hash-join spill unmeasured |
| 1, 2, 6, 22 | Rows still say "loom re-run", "speed pending", "quiet-machine latency", "overhead still to measure"; later runs probably settled them. Reconcile |

Not stale (these were wrongly listed as open in a conversation on 2026-11-06 and are done): CI on x86-64 and ARM64 runners (item 8, the `differential` job is allowed to fail until slice 4); network exchange stage A (item 28); real GCS bucket measurements (item 27); pyiceberg and Polaris interop (item 24, sections 11 to 13 of the Iceberg record).

Other known limits: DuckDB's own row-group skipping returns wrong rows for NaN on pyarrow files (reproduced, not reported upstream); the scan tests cover DuckDB, parquet-rs and pyarrow as writers only; no nested types, page index or bloom filter in the scan.

## 3. Open decisions as first listed (superseded by section 6)

| Decision | Recommendation |
|---|---|
| Truncate-overflow zone: refuse or reroute | Refuse the write with a clear error naming the value and the column |
| Manifest decoder: own or `iceberg-rust` | Own compact decoder for planning and writing, `iceberg-rust` kept as a test oracle; confirm with a short local bake-off first |
| Real object-store bucket for the write path (conditional writes) | A dedicated empty bucket, one-day lifecycle, narrow short-lived credential on one job VM only; after the read slice exists |
| Iceberg v3 | Defer; keep the manifest reader and writer versioned |
| Accept ADRs 0001 to 0004 and 0006 | After reading the diffs |
| Scrub the GCP project number `617096657002` from public history (it is in four earlier commits) | The owner's call; it needs a history rewrite and a force push |

## 4. What can run in parallel

Two limits decide this. GCP runs are separate VMs and can run in parallel (wave 1 did). Local authoring stays one agent at a time on the development VM.

### Group 1: local work, one after another (prepares the runs)

| Order | Task | Feeds |
|---|---|---|
| now | Slice 3 step 2b (parallel row-group tasks, cancel, bounds, loom, fuzz) | slice 3 benchmarks |
| 1 | Settle inside the NULL-path, push-cost and Arrow scripts | clean timing rerun |
| 2 | Kani harness for the step-6 sum bound, and the five unproved crate harnesses | Kani run |
| 3 | Hash-join spill benchmark in the hash-ops spike | spill run |
| 4 | Scan benchmark job for slice 3 (R2 pruning, 8-thread scalability, twin control) | benchmark run |
| 5 | Plan reconciliation and a "not yet verified" register in `verification-plan.md` | bookkeeping |
| 6 | Item 31 protocol check against ADR 0001 | sort, window, outer joins |
| 7 | Refinement test for the Iceberg commit (code against model) | write slice |
| 8 | Manifest decoder bake-off; item 9 cgroup pool sizing | two decisions |
| later | Orchestrator: `recover --run-id`, large-disk path, multi-VM mode | scale, network exchange stage B |

### Group 2: GCP runs that can go in one parallel wave (each after its prep)

| Run | Machine | Rough cost | Needs |
|---|---|---|---|
| Timing rerun with settling, ARM64 and x86-64 | `n4a-highcpu-8`, `c3d-highcpu-8` | about $0.2 together | Group 1 step 1 |
| Generic-ops with `overflow-checks` on x86-64 | `c3d-highcpu-8` | about $0.1 | nothing |
| Kani step-6 and crate harnesses | high-memory (`n4a-highmem-8`) | an earlier crate run cost $0.44 | Group 1 step 2 |
| Hash-join spill | `n4a-highmem-8` | about $0.2 to $0.3 | Group 1 step 3 |
| Slice 3 benchmarks on both CPU types | `n4a-highcpu-8`, `c3d-highcpu-8` | about $0.1 to $0.3 | step 2b and Group 1 step 4 |

Total for the wave: roughly $1 to $1.5, to be replaced by the planner's worst-case figures.

### Group 3: needs a prerequisite before any GCP run

| Run | Prerequisite |
|---|---|
| Scale, items 32 (`scale-sf10`, `scale-sf100`, 32 and 64 vCPU scaling; plan estimate about $3 in total) | `recover --run-id`; large-disk path (about 300 GB for SF100) |
| Network exchange stage B (4 VMs) | multi-VM mode in `orchestrate.sh` |
| Engineering checks, item 34 (`eng-checks`, about $0.16: ASan, TSan, Miri, UBSan, `cargo-deny`, `cargo-audit`, `cargo-geiger`, regression harness) | planned for the slice 4 card, when real code exists |
| Real client compatibility, item 35 | a server slice |

### Group 4: waits for the owner

ADR acceptance, the truncate-overflow decision, the decoder choice after its bake-off, the real-bucket access decision.

## 5. Suggested order

1. Finish slice 3 step 2b.
2. Group 1 steps 1 to 4, then launch Group 2 as one wave with a cost plan shown first.
3. Use the waiting time for Group 1 steps 5 to 8 and the orchestrator tools.
4. Start the write slice only after the item 24 code gate is designed.

## 6. Decisions (state on 2026-11-06)

Most of section 3 had already been decided by the owner on 2026-11-05 (see the revision notes of each ADR); the assistant's list in section 3 was stale. The owner then delegated the remaining decisions to the assistant on 2026-11-06 ("decide best on your way so it cannot become a blocker").

| Decision | State |
|---|---|
| ADRs 0001, 0002, 0003, 0006 | Accepted by the owner, 2026-11-05 |
| ADR 0005 rules 1 to 7 | Accepted by the owner, 2026-11-05: v2 first, truncate overflow zone REFUSED, own manifest decoder (bake-off done: 18x faster than the generic reader, 41x than `iceberg-rust` 0.11, 2.1 to 4.0x less memory) with `iceberg-rust` as test oracle |
| ADR 0005 rule 8 | Retried commits: the operation token alone (owner, 2026-11-05). Which catalog to certify stays open; it is decided when the write slice is designed |
| ADR 0004 | Proposed. Acceptance condition: stage B of item 28 has run, or the first distributed slice is designed. The owner decided on 2026-11-05 not to build the multi-VM mode yet; on 2026-11-06 the owner asked for Group 3 to be finished, which includes stage B, so the assistant builds the multi-VM mode now (step B10) and accepts ADR 0004 if stage B (E4) passes |
| Iceberg v3 | Deferred (rule 1); reader and writer stay versioned |
| Real bucket for write tests | Created by the assistant under the delegation: a dedicated bucket (us-central1, public access prevented, uniform access, no soft delete, objects deleted after one day) and a dedicated service account with no project roles, object access on that bucket only, no keys. Names kept outside the repository |
| GCP project number in old public commits | No history rewrite. The current tree is redacted and a check (`scripts/check-leaks.sh`, in CI) blocks it from new commits (done, step A3) |
| Push | After each verified milestone, always after the leak check |
| Cost | The owner accepted the costs of phases C and E on 2026-11-06; each run still uses the planner's worst case as its hard cap |

## 7. Execution order until Group 3 is finished

One agent at a time on the development VM; GCP runs in parallel. After each local step: the assistant re-runs the tests, a leak scan, and one hand-broken rule that must make a test fail, then commits locally and pushes at milestones (marked PUSH). "ASK" marks a cost approval.

### Phase A: finish slice 3 and record the decisions
| # | Step | Kind |
|---|---|---|
| A1 | Slice 3 step 2b finishes (scheduler, cancel, bounds, loom, leak scenarios, fuzz, geiger); assistant verifies | local, running |
| A2 | Correct this file to the decisions already taken (section 6) | done |
| A3 | Leak check in CI and locally, with its test; source of the project number fixed | done (`5e60591`) |
| A4 | Commit this file. PUSH | - |

### Phase B: Group 1, local preparation (in this order)
| # | Step | Feeds |
|---|---|---|
| B1 | Settle inside the NULL-path, push-cost and Arrow scripts | C1 |
| B2 | Kani: step-6 sum bound; the five unproved crate harnesses | C3 |
| B3 | Hash-join spill benchmark in the hash-ops spike | C4 |
| B4 | Slice 3 scan benchmark job (R2 pruning on sorted `lineitem`, 1 to 8 threads, SF1 and SF10, twin control, settle) | C5 |
| B5 | Orchestrator: write-scope option for the dedicated service account, and a test that it cannot write outside its bucket | C6 |
| | **ASK: cost plan for wave C** | |
| B6 | Plan reconciliation (rows 1, 2, 6, 22) and a "not yet verified" register | - |
| B7 | Item 31 protocol check against ADR 0001 (end-of-input hook, shared sink finalized twice) | slices 6 to 8 |
| B8 | Item 9 cgroup pool sizing (the decoder bake-off was done on 2026-11-05) | ADR 0002 |
| B9 | Iceberg commit refinement test (code against the model) | item 24 code gate |
| B10 | Orchestrator: `recover --run-id`; large-disk path; multi-VM mode (all tested with the fake `gcloud`) | phase E |

B6 to B10 run while wave C runs on GCP.

### Phase C: Group 2, one parallel GCP wave (after B1 to B5 and one approval)
| # | Run | Machine | Rough cost |
|---|---|---|---|
| C1 | Timing rerun with settling | `n4a-highcpu-8`, `c3d-highcpu-8` | about $0.2 |
| C2 | Generic-ops with `overflow-checks` | `c3d-highcpu-8` | about $0.1 |
| C3 | Kani step-6 and crate harnesses | `n4a-highmem-8` | about $0.45 |
| C4 | Hash-join spill | `n4a-highmem-8` | about $0.2 to $0.3 |
| C5 | Slice 3 benchmarks | `n4a-highcpu-8`, `c3d-highcpu-8` | about $0.1 to $0.3 |
| C6 | Iceberg conditional writes on the real bucket (the item 24 real-store gap) | `n4a-highcpu-8` | about $0.05 |

Total about $1.1 to $1.4, replaced by the planner's worst case before the yes.

### Phase D: record wave C and close slice 3
| # | Step |
|---|---|
| D1 | Record each run (results file, raw copy, plan rows, ADR evidence); failures of script or environment get up to two retries, real findings are never retried blindly |
| D2 | Slice 3 closeout: card section 5, ADR 0001 and 0003 amendments, benchmark added to the regression set, baseline spike deleted. PUSH |
| D3 | Draft the slice 4 card (parser and binder); decided by the assistant under the delegation; slice 4 starts |

### Phase E: Group 3, after B10 (each run its own approval)
| # | Run or step | Rough cost |
|---|---|---|
| E1 | `scale-sf10` on `n4a-highcpu-32` with spill and a 1,024-descriptor limit; ASK | about $0.43 |
| E2 | `scale-sf100` on `n4a-highcpu-32`, 300 GB disk, only if E1 passes; ASK | about $2.1 |
| E3 | Q1/Q6 core-count scaling at 32 and 64 vCPUs; ASK | about $0.45 |
| E4 | Network exchange stage B on 4 separate VMs (multi-VM mode); ASK | about $0.2 to $0.3 |
| E5 | Engineering checks (`eng-checks`: ASan, TSan, Miri, UBSan, `cargo-deny`, `cargo-audit`, `cargo-geiger`, regression harness) once slice 4 has real code; ASK | about $0.16 |
| E6 | Record E1 to E5; update plan items 28, 32, 34. PUSH |

Not finishable in Group 3: item 35 (real clients) needs a server slice; it is listed so that it is not forgotten.

Rough total GCP for phases C and E: about $4.5 to $5.
