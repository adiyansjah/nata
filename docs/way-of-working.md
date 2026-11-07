# Way of working

Principle: **small and complete beats broad and lacking.** Every capability that lands is finished: it works, it is tested, it is measured, it is documented, and it fails clearly outside its limits.

## 1. Vertical slices

Build one thin capability end to end before starting the next. A slice goes through the whole stack:

`parse -> plan -> optimize -> execute -> result over the wire`

Example slice: "`SELECT ... WINDOW row_number() OVER (PARTITION BY ... ORDER BY ...)` over Parquet." Not "all window functions". Not "the parser, then later the executor".

Order slices by what the internal data team needs next (window functions, multi-table joins, Iceberg scans), not by what is interesting to build.

**Detours need a reason** (added 2026-11-07). A slice taken out of the order in `feature-map.md` says, in its card, what it unblocks and why that cannot wait for its place in the order. Until the internal team can run aggregates, joins and window functions (slices 5 to 8), new slices outside that path are deferred unless they block it. Slice 4b (the server, slice 10 in the order) was a justified detour for plan item 35; after it, four days in, the team still cannot run a `GROUP BY`.

## 2. Definition of done (every slice)

A slice is not done until all of these hold:

1. **Correct.** Results match DuckDB on a differential test suite (same query, same data, compared output), including NULLs, empty inputs and ties.
2. **Tested at the edges.** Empty tables, one row, all-NULL columns, duplicates, data larger than memory (spill path). Concurrent code (schedulers, channels, wakers, shutdown) is also tested under CPU contention and with injected delays that widen race windows: a scheduler bug passed 10 of 10 runs on an idle machine and hung in most runs under load (`bench/results/20261103-operator-trait-prototype.md`).
3. **Measured.** A benchmark exists and its result is recorded in `bench/results/`. Any speed claim states what limits the number and what was run (warm or cold cache, data size, hardware). Do not report a number you have not explained. How measurements are made: section 6.
4. **Explainable.** `EXPLAIN` and `EXPLAIN ANALYZE` show the new operators.
5. **Documented.** The supported-SQL page lists exactly what works, with an example.
6. **Fails loudly.** Anything not supported returns a clear `not supported yet: <what>` error. Never a wrong result, never a silent fallback, never a panic.
7. **Clean.** `cargo fmt`, `cargo clippy -- -D warnings` and all tests pass in CI.
8. **Verified against its card.** The slice has a verification card (next subsection), every row of it is checked or justified, and the ADR it relies on is current.

### The slice verification card

Every development point has a short card, written **before** the implementation, in `docs/slices/NNNN-name.md` (template: `docs/slices/0000-template.md`). The card is how we decide and how we know we are done. It is accepted by the owner, like an ADR: an agent or a human drafts it, the owner accepts it, and the work starts from the accepted card.

A card has six parts.

1. **Goal and ADR.** What the slice delivers, and the ADR it relies on or creates. If a decision it needs is open, the card says so and names the baseline spike (part 4).
2. **Quality targets.** One row for each aspect below: the target or budget, the check that will show it (with its oracle), and the baseline to beat or match. A row may say "N/A, because ...". It may not be missing.
   **Measure the baseline before setting the target** (added 2026-11-07). A performance or resource target is set from a measured number of the reference (DuckDB, PostgreSQL, the previous slice) on the same workload, recorded in the card, not from a guess. Slice 4b's 1.2x streaming target was below what real PostgreSQL itself reaches (2.3–2.6x), and that was only found at closeout. If the baseline cannot be measured before the card is accepted, the target is marked provisional and fixed after the baseline run, before any Nata number is taken.

   | Aspect | What the row must say |
   |---|---|
   | Correctness | the oracle (DuckDB, a scalar reference, two independent readers) and the mutants that show the check can fail |
   | Performance | a target and a baseline (DuckDB, the previous slice, or a hand-fused loop), measured with a control arm, on both CPU types |
   | Scalability | the sizes and core counts that matter, and the budget |
   | Reliability | no hang, no leak, cancel and early finish at every step; the model, stress or loom check that shows it |
   | Fault tolerance | the stated rule for each failure that can reach this code (ADR 0004), and the injected-fault check |
   | Resource bounds | memory, open files, disk, tested under the default limits of a fresh machine, not the dev machine's |
   | Portability | ARM64 and x86-64 on clean VMs |
   | Compatibility | other engines, clients and file readers, and the known differences, listed |
   | Security and privacy | no secrets, keys or project identifiers in logs, results or errors; a leak scan where relevant |
   | Operability | clear errors, and a seed or trace that replays a failure |

3. **Decision rule, fixed before measuring.** The threshold that decides the question, written down first: for example, "build our own kernel only if it is at least 1.5x faster on both CPU types, otherwise reuse the crate". Do not choose the threshold after seeing the numbers.
4. **Baseline spike, only when needed.** A spike is required when the slice contains a build-versus-reuse choice, a performance number that drives a design, a concurrency or protocol change, a new external format, or a failure behaviour that is not yet stated. Otherwise the card links the existing baseline. A spike is time-boxed, throwaway (section 6), and any cloud spend follows section 8.
5. **Carried-in improvements** (added 2026-11-06 at the owner's request, to pay down technical debt as we go). The card lists every open item
   of the improvement register (`docs/improvements.md`) whose code path this slice touches, and for each says either **taken in** (with its
   own decision rule fixed before measuring, and its check, exactly like the slice's own targets) or **deferred** (with the reason and the
   slice that will take it). Taken-in items are capped at about a quarter of the slice's work: the slice's own goal comes first, and what
   does not fit stays in the register for the next slice on that path. An improvement that changes a decision is still an ADR amendment.
   **Two limits on the register** (added 2026-11-07; it grew from 19 to 22 open items over slices 4 and 4b):
   - **On-path blockers come first.** An open item that the slice's own goal depends on (for example the query memory manager and the grace join's limits, R1 to R3, for the spilling slices) is taken in even past the quarter cap. The card says so and why.
   - **Growth triggers payback.** If a slice's closeout adds more than three items to the register, the next slice takes in the oldest open items on its path up to the quarter cap, and its card may not defer them for lack of room.
6. **Closeout.** When the slice is done: the results file in `bench/results/`; the ADR updated (a dated amendment if it is accepted, with its confidence and what changed); the benchmark added to the regression set with its baseline numbers; deviations from the card recorded; spike code deleted; **the improvement register updated**: taken-in items marked done with their result, and every new performance or reliability finding of the slice added (with its evidence, the code path and the slice it fits).

**Proportion.** A small slice gets a short card, and a trivial one may be a single line that says what is checked and against what. The card must never be longer than the work it protects. The rule is not "fill the table"; it is "no slice starts without saying how we will know it is right, fast enough, safe to fail and finished". The same holds for results files and closeout records (added 2026-11-07): lead with the verdicts and the deviations, link the raw data and the commands instead of retelling them, and keep the history of how the work went out of it unless it changes what a reader should trust.

**Order of work inside a slice.** Card accepted (with its carried-in improvements chosen), then baseline spike if required, then the walking skeleton below, then implementation with its tests and mutants in the same change (carried-in items alongside, each with its own check), then the checks on a clean VM on both architectures (the measurement rules of section 6), then the deep checks on the closing commit, then closeout (including the register).

**Walking skeleton first** (added 2026-11-07). The first runnable change of a slice runs, end to end and in a thin form, every check that closeout will run: the real clients with their *default* settings, a local smoke benchmark at SF1 against the measured baseline, the fuzzers at nightly size on the code paths the slice touches, and the resource limits of a fresh machine (for a spilling operator: spill on, at a small budget, from the first day). Closeout then confirms results; it should not discover them. In slice 4b, default-client failures, 60–90x slower streaming and the ADBC failure were all found at or near closeout and each reopened finished work.

**Deep checks on the closing commit** (added 2026-11-07). Before a slice is closed, the nightly workflow (full fuzz, loom at bound 3, the SQL subset fuzzer at full size) is triggered by hand on the closing commit and passes, or each failure is fixed or recorded as a deviation with a register item. A slice is not closed on a commit the deep checks have not seen. Slice 4 was closed before the nightly found a hang on a damaged Parquet file and two unexplained SQL mismatches.

## 3. No stubs on main

- Nothing half-built merges. Unfinished work stays on a branch or behind a clear unsupported error.
- No `todo!()` or `unimplemented!()` reachable from user SQL.
- The supported-SQL page is the contract. If it is not listed, it is not supported.

## 4. Small, complete tools

Each binary or crate does one job and does it fully:

| Tool | Job | Complete means |
|---|---|---|
| `nata` | run a node / CLI client | start, query, stop, with sensible defaults and no config file |
| `nata-bench` | reproducible benchmarks | one command, prints results and the hardware/config used |
| `nata-check` | differential correctness | runs a query set against Nata and DuckDB and reports mismatches |

Add a new tool only when an existing one cannot do the job. Prefer deleting over adding.

## 5. Correctness before speed

Order within a slice: make it right, prove it right, then make it fast. A fast wrong result is worse than a slow right one. Optimizations land with a benchmark showing the gain and a test showing no change in results. Performance work must hold on both x86-64 and ARM64: see [portability.md](portability.md).

## 6. Decisions rest on evidence and record why

**Preferred evidence, strongest first:** a formal proof or exhaustive model check within stated bounds; a differential or property test against an independent reference; a measurement with a control arm; a reasoned argument (always labelled as reasoned). Use the strongest that is affordable, and combine them: empirical checks find what we did not think of, formal checks cover every input in their bounds. A check is only evidence if it is shown to fail on a known-bad version (a past bug or a deliberate mutation).

A decision that cannot be explained cannot be revisited. Every architecture or build-versus-reuse choice is written in `docs/decisions/NNNN-title.md` (template: `decisions/0000-template.md`). About one page; link the detail instead of copying it.

### What an ADR contains

1. **Context.** The forces and requirements, linked to the docs they come from.
2. **Decision.** What we will do.
3. **Why.** The reasons for this choice in plain sentences, not only the choice.
4. **Alternatives considered.** At least two real ones (including "reuse the existing crate" or "do nothing" where they apply), and why each lost.
5. **Evidence**, in three parts:
   - *Measured*: link to `bench/results/`, with what limits the number.
   - *Reasoned*: arguments that no measurement supports.
   - *Not covered*: what the evidence cannot show.
6. **Confidence.** High, medium or low for the decision, and separately for each measured claim; and what finding would change it.
7. **Consequences** and **Revisit when.**

### Rules

- **Label every claim as measured or reasoned.** Reasoned is allowed. Presenting it as measured is not.
- **Weak evidence is called weak.** "We found no cost" is not "there is no cost".
- **Build versus reuse needs a bake-off**: a small benchmark of the existing crate against the custom attempt.
- **Status.** An agent or a human proposes; a human accepts. An accepted ADR is not edited in place: append a dated amendment, or supersede it with a new ADR. A proposed ADR is updated when new evidence arrives, and says what changed.
- **Independent check before acceptance is empirical and formal, not critique.** Before an ADR is accepted, its claims are re-checked by something that can fail: the measurement re-run by someone who did not take it, a differential test against DuckDB, a property test against a scalar reference, a mutation check that the tests catch known-bad versions, a model checker (loom, shuttle, TLA+) for concurrency, a bounded proof (Kani) for kernels. A reviewer, human or agent, may point at what to check, but every finding becomes a test, proof or measurement, or it is dropped. A written critique alone does not count as the check. Findings are checked against the code before they are acted on (section 8).
- **Outside code is untrusted input** (added 2026-11-07). Every place where bytes from outside reach a third-party crate (Parquet files, the wire protocol, later S3 responses and Iceberg metadata) has a fuzz target at nightly size and a thin wrapper of ours that bounds what the crate can do: lengths checked before allocation, no read after end of file, a deadline where a loop is possible. The `pgwire` crate panicked on short messages (upstream #481) and parquet-rs looped forever on a damaged file; both were found by our fuzzing, not by the crates' tests.
- **Spikes are throwaway.** Spike code lives in `spikes/`, results in `bench/results/`. Delete the code once the ADR is accepted; keep the results.
- **Both architectures.** Evidence for a performance-sensitive decision is recorded on x86-64 and ARM64, or the ADR says which one is unmeasured (see [portability.md](portability.md), section 7).

### How measurements are made

A measurement is evidence only if it follows these.

1. **Check the result before timing it.** Compare the output with the reference engine. A fast wrong answer is not a data point.
2. **Compare variants in the same session, alternating.** Run A, B, A, B. Never time all of A and then all of B, and never compare a number from another day or session: shared machines drift by 10-30%.
3. **Repeat, and report the noise.** Give min and median, and the run-to-run noise you saw. A difference smaller than the noise is not a result.
4. **Time competitors the same way as our own code.** In-process against in-process, same threads, same cache state. Say what startup, planning and metadata loading each side includes.
5. **Explain a number before reporting it.** Profile (`perf`) or change one thing at a time to find the cause. A guess is labelled a guess.
6. **Keep the limits next to the result.** Data size, warm or cold cache, hardware, and what is excluded.
7. **Correct mistakes in the open.** When a claim turns out wrong, add a dated *Correction* to the results file or ADR that says what was wrong and what is right. Do not silently overwrite it.

## 7. Rhythm

- Work in slices of days, not months. Each ends with something runnable.
- At the end of each slice: run the benchmark, update the supported-SQL page, note what is still open.
- Weekly: run the internal data team's real queries and record which pass, fail, or are slower than their current engine.

## 8. Roles (humans and agents)

- Claude Code plans and implements multi-file changes and runs the tooling. It may draft ADRs, run spikes and write the tests, proofs and measurements that check evidence; a human accepts an ADR (section 6).
- **Adversarial review is not the default.** Do not start a second agent to critique ADRs, specs or results as a routine step: the tokens are better spent on tests, proofs and measurements (section 6). A review is allowed only when the human asks for one, or as a single short pass before a spec is frozen that asks for properties and invariants to check, and each of its findings must become a test, proof or measurement, or be dropped. Codex, where installed, may be used the same way for a large slice (`/codex:review`), on request.
- No agent commits, pushes or opens PRs unless asked.
- Any review finding is checked against the code, and turned into a test, proof or measurement, before it is acted on.

### Orchestrator and child agents

Since 2026-11-04 the main Claude Code session works as an **orchestrator**: it plans, briefs, checks and reports, and delegates the implementation and verification work to child agents. The human decides; the orchestrator does not accept ADRs, commit, push or spend cloud money without being asked.

- **Default child:** Claude Sonnet 5.5 at high effort. Other models or settings only when the human asks.
- **The brief is the whole context.** A child sees nothing of the conversation. Each brief states the goal, what is already ruled out, the files to read, the exact scope (which paths it may edit, which results file it writes), the standing rules (no GCP, no commit or push, no private data in files) and the form of the final report.
- **Verification over critique.** Briefs ask for proofs, property tests, mutation checks, model checking (Kani, loom) and measurements. Every claim in a child's report must be backed by a command it ran. A child is not asked to review for the sake of reviewing; a suspected bug becomes a failing test first.
- **Checks must be able to fail.** A proof or model counts only if it fails on a known-bad version (the old bug, or a mutation). The brief requires that evidence.
- **The orchestrator re-runs what matters before relying on it.** A child's summary is a claim. The orchestrator runs the reproduce command and reads the diff before results go into an ADR.
- **Parallel children get disjoint scope.** Where scopes could overlap, a child works in its own worktree. Nothing is committed while a child is mid-edit in the shared checkout.
- **Brief rules, added 2026-11-07** (children went beyond their brief in slice 4b: unrequested binary format support, a third GCP attempt where one rerun was allowed, a verdict that did not follow the card's rule):
  - **Not in scope.** Every brief has an explicit "not in scope" list next to the scope. Work outside the brief stops and is reported as a proposal; it is not done.
  - **Dependencies in the brief** (added 2026-11-07; the slice 5 skeleton stopped twice over crates its own ADRs had chosen). A brief that
    builds a slice names the new direct dependencies the card's ADRs select, and says that a crate already in `Cargo.lock` may become a
    direct dependency at its locked version, with `cargo deny` passing and its unsafe code recorded. Only a crate that is not in the lockfile
    yet is a stop-and-report.
  - **Clean up disk** (added 2026-11-07; finished children's scratch and build directories filled the 252 GB disk). A child deletes its own
    scratch directories (`~/tmp-*`) when it finishes. The orchestrator deletes a child's worktree `target/` once its work is merged or
    abandoned, and checks free disk space before it starts a child.
  - **Stop and report.** A child stops and reports before anything the brief does not name: a rerun beyond the count allowed, a new dependency, a change to a threshold or a default.
  - **Fixed report form.** Each verdict is reported as: the card's rule as written, the measured numbers, the verdict the rule gives, and the command that reproduces it. A verdict that does not follow from the rule is a finding, not a result.
  - **Own worktree by default.** A child that edits code works in its own worktree (`isolation: worktree`), not in the shared checkout, so the orchestrator can test and fix in the main tree at any time.
- **Cloud runs stay with the human's cost approval** (next section). A child never launches one on its own.

### Cloud spend (GCP)

Cloud resources cost real money, so an agent never creates, starts or resizes one on its own. Before any GCP run (for example an ARM64 or x86-64 test or benchmark on a Spot VM):

1. **Ask first.** Say what needs to run on GCP and why it cannot run locally or in CI.
2. **Show the cost before running.** Give an estimate with: machine type and architecture, region, Spot or on-demand, vCPUs and memory, expected run time, disk size, and any egress or static IP. State the total in dollars and say it is an estimate. Prices come from the current Google Cloud pricing page or calculator, with the source and date. If a price could not be confirmed, say so.
3. **Wait for a clear yes.** Do not start anything until the human confirms. A yes covers that run only, not later ones.
   **Local trial first** (added 2026-11-07). Before the cost is shown, the same job has passed locally: the job's script on small data (SF1 or smaller, one round) and `scripts/gcp/test/test-job-chains.sh`. If the trial fails, nothing is launched. Failed or aborted runs were 26% of all GCP spend up to slice 4b ($1.08 of $4.16), mostly script mistakes a local trial catches.
   **Heavy runs go to Spot VMs sized to the work** (owner, 2026-11-07). Fuzz at nightly size, mutant sweeps, loom at bound 3, sanitizer
   suites and benchmarks do not run on the development VM (8 vCPU; two children's local fuzz and mutants drove it to load 30). They run as
   `orchestrate.sh` jobs, the generic one being `heavy` (`scripts/gcp/README.md`), on a machine chosen for the job: 16 or 32 vCPU for
   parallel work, 8 vCPU where numbers must compare with earlier baselines. Children build and run short tests locally.
   Mutant sweeps count as heavy (each mutant rebuilds and re-runs a crate's tests: one sweep held the VM for half an hour). What a child
   does run locally, it runs one job at a time, never several heavy jobs in parallel (one child running a mutant sweep, the fuzzer and the
   workspace tests at once kept the load at 15-17 on 8 vCPU).
4. **Keep it small and stop it.** Use the smallest machine that answers the question. Delete or stop the VM when the run ends, and report what was created and what was removed. Never leave a VM or disk running unnoticed.
5. **Record it.** Note the machine type, region and date next to the result in `bench/results/`, so the cost and the hardware are known (see section 6, measurements).

## 9. Non-goals (to stay small)

Until the internal team is running real queries on Nata:

- No UI.
- No write path (INSERT/UPDATE/MERGE).
- No Trino compatibility.
- No second table format.
- No managed cloud service.
