# Slice 11 continuation: parked scans and shared footers (2026-11-08)

## Verdicts and limits

- **Local correctness/reliability: PASS within the checks below.** Workspace: 565 reported passes, zero failures, two intentional ignores. Four differential sets: 377 queries per mode, local and fake, with zero mismatches/errors. This is the loopback fake, not MinIO or real S3/GCS. No cloud calls were made.
- **Parked bridge: PASS at local bound 2** for nine short production-code models; dropped-wake and undelivered-cancel mutants **CAUGHT**. The combined five-thread model remains **NOT COMPLETED** locally and a GCP closeout gate. **Object-store mutants: 18 CAUGHT, zero survivors**, with clean controls first.
- **Frozen timing ceilings:** **NOT MEASURED**. Neither loopback delays nor historical reference numbers are timing denominators. The real-bucket commands below keep the frozen 1.5x DuckDB ceilings and four exceptions, 1.2x spike ceiling, and twin-noise rule.
- **Scope limit:** parallel scan opens/ranges park without holding a worker. Planning and the aggregate cursor API still have a synchronous compatibility path. Moving aggregate continuations to this interface requires Track A coordination and is outside this brief's ban on aggregate internals. The frozen `full` cell needs `strlen` and `date_diff` reductions unsupported by Nata's current SQL subset; that cell must fail rather than measure a substituted query. Slice 11 is not closed.

## Implementation and prior-attempt review

Reused the previous worktree's `bridge.rs` completion cell, reservation adapter and scheduler continuation design after reading its diff. Repaired its unfinished test adapter (duplicate `Work` implementation, missing `open_step`), connected real remote opens/fetches, and added native/loom checks. The previous worktree was read only. No dependencies or locked versions changed.

The scheduler retains parked tasks with their original sequence/window slots. A wake while running is sticky; a later park immediately queues the task again. A parked opening retains its ahead slot. Completion stores the result before invoking its registered wake. Cancel/lowest-error teardown settles parked jobs under the scheduler lock and drops their requests outside it; request drop aborts the Tokio job. Resumes remain possible below a later error. Worker count remains the requested/available CPU count; remote CLI and server windows default to the request-attempt cap, 16.

`reserve_step` is the production memory handshake, shared with loom. Repeated pressure on an already-revoked operator no longer wakes itself indefinitely; a real capacity release still notifies the FIFO head. The no-self-wake model checks that retrying with unchanged capacity registers a wait instead of spinning. Grant waits outside a retry operation also have the operation/query deadline.

Models (`crates/nata-sched/tests/bridge.rs` and `src/parked_core.rs`): completion before/after park; parked footer; cancel of a parked task without any I/O completion; two workers/two preclaimed waiters at the real parking core, both wake orders and cancel, with window cap; bounded shared-fill registrations and cancellation of one subscriber; grant/release racing registration and budget cap; no repeated self-wake. All use production code through the std/loom facade. Bounds are scheduling bounds, not unbounded proofs; Tokio/network internals are covered by native fault/cancel tests, not loom.

The fully combined two-CPU/two-I/O (five-thread) bound-2 probe exceeded the 180-second local time box: **NOT COMPLETED**, not PASS. The short suite separates axes: a full one-CPU scheduler with two independent I/O threads; and the real parking core with two CPU threads, two already-claimed jobs and one I/O thread delivering both wake orders (plus cancel). The latter omits file opening and sink/shutdown setup to keep the state space bounded. This does not prove the combined five-thread case. Its real-code reproducer remains available with `BRIDGE_COMBINED_IO=1`; the GCP heavy file runs it at bound 3 with a 7,200-second model cap. CI runs the split-axis suite; the combined case is explicitly GCP-heavy and remains a closeout gate.

## Cache allowance, ownership and counters

The object-store node reserves its cache allowance (default 64 MiB) from an ADR 0009 ledger before allocating the remaining response/staging allowance. Cached handles retain the reservation even if the node/cache is dropped while a handle is pinned. Each query keeps its own ADR 0009 service; every response/staging grant also owns a parent grant from the shared remaining node service. Both caps must fit before allocation, and dropping the child releases both exactly once. The node service serializes its grant queue. Fixed equal query caps divide its remaining allowance using the caller's configured admission bound; queries do not borrow another query's cap. The server supplies its existing max-query bound. Cancelling one query signals only its token, never another query's shared memory manager. The CLI/server withhold the entire object-service allowance, including the cache, from the execution pool before dividing admissions. These ledgers cover object responses/staging and metadata; they do not certify all decoder/library/allocator allocations, so the independent RSS gate remains open. The CLI's object-memory option now describes a node allowance; this is conservative for multi-query servers and needs real concurrent-load sizing evidence.

Cold identity-keyed misses share a fill (at most 32 fills, 32 subscribers per fill). The table holds weak references; the last subscriber drops/aborts the fill. A cancelled subscriber cannot cancel another query's fill. Success alone is published, with parsed-byte staging still granted before parsing. Broadcast registration/completion uses one lock and invokes wakes outside it. A post-lookup publication race rechecks the cache before creating another fill. Full retained-byte/RSS and prepared-plan closeout checks remain due.

Binder and execution share a query-local `RemoteIo` scope and the same query service. The scope retains only the first binder footer for handoff; subsequent bind-all schema checks return a pinned `RemoteFile` and release their own pins after inspection. There is no node-wide handoff table, so concurrent binders cannot steal each other's identity or cancellation context. Execution validates identity with HEAD before consuming the first handle; replacement returns an error. This removes duplicate footer GETs with the cache off as well as on, without trusting a path/TTL or dropping an uncached construction grant before the binder finishes reading it. Cache lookups across scopes remain identity-validated. Full prepared-plan/registry-refresh and credential-scope stress remain open.

Construction grants shrink to retained parsed metadata/key capacity plus bookkeeping after parsing, before admission/bypass. Query and parent node ledgers return that unused capacity together; a bypass does not pin the 128x parser workspace for the rest of the scan. Key strings are charged by capacity, not length.

EXPLAIN ANALYZE appends sanitized cache hits/misses/evictions/retained bytes/reservation, identity checks, footer/data GET attempts, fetched/successful bytes, retries, peak inflight and grant bytes. These are explicitly **node-cumulative**, including binder activity; they are not attributed per query when queries overlap. Per-query attribution, latency distributions and stable error aliases remain closeout work.

## Local worker/window experiment

Rule: indicative evidence that I/O concurrency comes from the window rather than oversubscribed workers, not a real-store performance verdict. Run:

```sh
TMPDIR="$HOME/tmp-s11" cargo test -p nata-objstore --test scan window_not_workers_carries_io_concurrency -- --ignored --nocapture
```

32,000 rows / 32 row groups, one projected integer column, 50 ms delay per GET, metadata planning excluded from the timed execution, three interleaved rounds: `(workers,window) = (1,1), (1,16), (4,16), (1,1)`. Every arm returned exactly 32,000 rows and reported the requested workers/window. x86-64 development VM only; debug build, no real-store ceiling applies.

| CPU workers | Unit window | Samples | Minimum ms | Median ms | Maximum ms |
|---|---|---|---|---|---|
| 1 | 1 | 6 | 1662.38 | 1664.59 | 1668.58 |
| 1 | 16 | 3 | 105.87 | 106.29 | 107.14 |
| 4 | 16 | 3 | 105.18 | 105.38 | 105.66 |

The window change at one worker gives 15.66x lower median elapsed time. Adding workers at window 16 changes the median by less than 1%, within the observed range of the one-worker arm. This isolates the artificial network delay as the bottleneck; it does not establish CPU scaling or real-store throughput. [Raw samples](raw/object-store-s11/window.txt). The CLI integration test separately verifies `--threads 1` remains one worker with window 16 in both `sql` and `scan`, and checks the EXPLAIN counters.

## Nightly and parent commands

`.github/workflows/nightly.yml` now has separate ARM64/x86-64 fault/cache/hostile-length/mutant jobs (90 min), transport and damaged-object fuzz jobs (200,000 cases each, 330 min per target/architecture), and bridge/fill/grant loom plus bridge mutants at bound 3 (90 min). These fit one GitHub runner per matrix entry; actual nightly runtime is not yet measured. Existing long scheduler models keep their own matrix. Heavy VM runs below also provide both-architecture coverage without requiring CI to finish first.

No cloud run is authorized or performed here. Runtime env file paths/identifiers must be supplied privately by the parent; no write-test env filename belongs in committed commands. Run the local trial and `scripts/gcp/test/test-job-chains.sh` before seeking cost approval. Inspect every `.exit` file; a timeout/build failure is not a caught mutant. Cleanup steps still run after a failed timing step.

```sh
# Same heavy.steps for either CPU; choose the architecture when planning.
cp scripts/object-store/heavy.steps.example heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-16 --max-vm-hours 8
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-16 --max-vm-hours 8
# Only after parent's cost approval, run each plan with its approved cap:
# scripts/gcp/orchestrate.sh run --job heavy --machines <type> --max-vm-hours 8 --approved-usd <approved-cap>

# Bound 3 commands inside that file (also directly reproducible on the VM):
LOOM_MAX_PREEMPTIONS=3 BRIDGE_MODEL_TIMEOUT=1200 bash crates/nata-sched/check_bridge_mutants.sh
BRIDGE_COMBINED_IO=1 BRIDGE_ONLY=^two_workers_two_waiters_keep_the_window_bound$ LOOM_MAX_PREEMPTIONS=3 BRIDGE_MODEL_TIMEOUT=7200 bash crates/nata-sched/run_bridge_loom.sh
LOOM_MAX_PREEMPTIONS=3 MEMORY_MODEL_TIMEOUT=1200 bash crates/nata-sched/run_memory_loom.sh
NATA_OBJ_FUZZ_CASES=200000 NATA_OBJ_FUZZ_SEED=20261107 cargo test --release -p nata-objstore --test fuzz -- --nocapture --test-threads=1
cargo run --release -p nata-objstore --features mutants --bin objstore-sweep -- mutants

# Real bucket, comparable eight-vCPU machines. Set NATA_OBJ_ENV_FILE to the
# private absolute runtime path in the shipped environment, never in this file.
# The private file must be ignored, inside this checkout and outside excluded
# data/target directories, so the shipped tree carries it without committing it.
python3 - <<'PY_STEPS'
import os, shlex
from pathlib import Path
root = Path.cwd().resolve()
relative = Path(os.environ['NATA_OBJ_ENV_FILE']).resolve(strict=True).relative_to(root)
lines = []
for line in Path('scripts/object-store/real-bucket.steps.example').read_text().splitlines():
    if not line or line.startswith('#'):
        lines.append(line)
    else:
        name, command = line.split(' ', 1)
        lines.append(name + ' env NATA_OBJ_ENV_FILE="$SRC"/' + shlex.quote(str(relative)) + ' ' + command)
Path('heavy.steps').write_text('\n'.join(lines) + '\n')
PY_STEPS
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --max-vm-hours 4
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --max-vm-hours 4
# run after approval with --max-vm-hours 4 and that plan's approved cap.
# The generated, ignored heavy.steps sets the shipped runtime path in every step.
```

Both step files read machine type inside a step with:

```sh
curl -s -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/instance/machine-type | sed 's|.*/||'
```

The real steps build the spike control, generate/verify layouts and local DuckDB answers, preflight/upload with the write-test identity, run read-only correctness and paired cells, default clients, cleanup and leak scan. `cells.py` now interleaves Nata/spike/DuckDB and independent twins **per cell and round**, preserves raw paired samples, checks every timed answer, and applies both frozen ceilings with noise. The control uses the frozen merged `wrap` policy. It includes its own verification and startup probes; only its cold measured sample is used. Nata CLI wall includes process startup; DuckDB/spike timings are in process, so this conservative comparison must be labelled and startup isolated before a fair final timing verdict. The two all-layout benchmark blocks formerly run minutes apart are removed.

Corrections to the skeleton's cell driver: the frozen full workload reads all 16 columns (with string/date reductions); selective bounds are 40–41% of `6,000,000 x SF`; object paths include the scale-factor directory; requested workers match across arms; DuckDB dot commands are on separate lines, secret setup output is hidden, and timed answers are checked rather than trusting only the first answer. Unsupported full SQL is reported NOT MEASURED, not silently changed. The matched `full` performance verdict remains BLOCKED on that unsupported SQL / Track A seam.

## Verification and remaining work

Checks use on-disk `TMPDIR="$HOME/tmp-s11"`, with heavy local commands run sequentially. `Cargo.lock` is unchanged. Source identity and sanitized outcomes are kept in [checks.txt](raw/object-store-s11/checks.txt), [object-store mutants](raw/object-store-s11/mutants.txt), [bridge models and mutants](raw/object-store-s11/bridge.txt), and [differential summaries](raw/object-store-s11/differential.txt).

```sh
mkdir -p "$HOME/tmp-s11"
export TMPDIR="$HOME/tmp-s11"
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p nata-objstore --all-targets --features mutants -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p nata-objstore --features mutants --bin objstore-sweep -- mutants
LOOM_MAX_PREEMPTIONS=2 bash crates/nata-sched/check_bridge_mutants.sh
bash scripts/gen-tpch.sh 0.01 "$TMPDIR/data"
bash scripts/gen-sql-check-data.sh "$TMPDIR/data"
bash scripts/object-store/check.sh "$TMPDIR/data" fake
bash scripts/gcp/test/test-job-chains.sh
scripts/check-leaks.sh --staged
```

**PASS:** formatting; Clippy across the workspace/all targets and object-store mutant features; workspace tests; cargo-deny; object-store controls and all 18 mutants; all nine short loom models at bound 2 and both bridge mutants (test exit 101); the four differential sets in both modes; shell/Python/YAML syntax; GCP job-chain tests. The two ignored workspace tests are the manually run window experiment and the explicitly GCP-heavy combined loom model. The mutant-checker parser also rejects timeout (124) and build-error (1) canaries even when they print assertion text.

Nightly-size fuzz, bound 3 and real-store commands were prepared, not run locally. The complete spike-paired driver needs a small parent trial before any cloud cost approval; syntax/job-chain checks alone do not certify that path. On-disk scratch is removed after the commit; the parent owns worktree build-directory cleanup after merge/abandonment.

The revised `cells.py` has a local orchestration smoke: SF0.01 lineitem copied under each of the four SF1 layout names, one round/one worker, `--queries proj,sel,meta`, against the fake endpoint and DuckDB httpfs. All 12 cells returned matching answers and `INDICATIVE` (exit 0). These are deliberately identical small files, not the frozen layout shapes; their timings are not retained as performance evidence. A second run with `--queries full` gave four `NOT-MEASURED` results and exit 1; standalone CLI probes also refused `strlen` and `date_diff`. The spike subprocess arm was not included in this smoke.

Remaining slice 11: real S3/MinIO/GCS auth/refresh/TLS/version-race checks; both CPU frozen performance and 0.9x spike-scaling gates; fresh-VM RSS/FD/concurrent-query admission checks; runtime comparison; adaptive footer prefetch; aliases/replay and per-query/useful-byte telemetry; complete shared-fill/prepared-plan/credential-scope stress; closing-commit deep checks, dependency audit/unsafe inventory and closeout/register reconciliation. No timing target is changed and no real-store result is inferred from the fake.

## Real-bucket failure investigation and rerun preparation (2026-11-08)

Read every saved `*.txt` recursively from the parent's ARM64 run, including the
spike's nested results, with runtime identifiers redacted before inspection.
`smoke-example` failed on `gs://<bucket>/<prefix>/sf1/mid1/part-0000.parquet`.
The GCS differential results were scan 0/5, SQL 30/252, SQL-differences 3/55,
and aggregate 3/65; **65/65 was the local aggregate arm**, not a passing GCS
object set. Constant/no-file SQL explains why some queries can pass without
establishing any working Nata object reads. Both production cell sets reported
NOT-MEASURED, and the client server exited before its checks. The spike passed
64 preflight runs, 24 scaling runs at each worker setting, and its RSS cells.
Cleanup deleted all 131 uploaded objects. There is no saved HTTP trace and no
per-object byte-size inventory; no exact failing-object size or encoding is
inferred from these logs.

Both layout upload and check upload call the same `osb upload` implementation:
`put_multipart`, 32 MiB upload parts, 8 MiB local read chunks, up to four parts
in flight and four files at once; neither sets Content-Encoding/gzip. The layout
upload was 114 files / 5,119,746,129 bytes, the check upload 17 files / 5,221,509
bytes, and the spike's passing tiny preflight file 98,798 bytes (later deleted).
The failing mid1 has 49 row groups; the spike also reads the much larger SF10
layouts successfully. These facts do not establish an object-size threshold.

The source exposes an earlier failure: `BoundedConnector::connect` ignores the
per-client `ClientOptions` and copies the store's HTTPS-only setting into its
metadata clients. `object_store` 0.14.2 deliberately passes `metadata_options()`
with HTTP enabled for the GCE token service; the bounded connector rejects that
HTTP request before a HEAD or range response exists. The spike's connector uses
the supplied options. Static/anonymous fake GCS credentials bypass this path.
The focused reproduction and fix are recorded below when verified. Object
header hypotheses remain subject to the diagnostic trace, not assumptions used
to relax range validation.

`gcs-diagnose.sh` uses the VM's link-local metadata token in memory, never keys
or token files. It prints JSON lines for HEAD, full GET, first-eight-byte GET,
suffix GET, full-object bounded range, past-EOF range, and a restricted JSON
metadata request for five fixture/layout objects. It records statuses, header
**names**, allowlisted length/range/encoding/Accept-Ranges values, body counts,
and validator equality flags (never validator values). Bodies are streamed and
discarded with a 2 GiB/180 s per-object-response bound; metadata has an 8 MiB
bound. Errors print only their class. Bucket/prefix are redacted on every output
line. Initial 404s after the prior cleanup are evidence, not a diagnostic failure;
a second diagnostic runs after layout and check uploads and before any Nata
smoke. The selected layout objects passed spike reads, not necessarily Nata reads.

Every real-bucket step now runs through `real-step.sh`. Console stdout/stderr is
filtered before the heavy runner writes it. Programs' own text/CSV outputs go
to a private mode-0700 working directory under `$WORK`; only redacted copies
are exported to `$RESULTS`. This includes nested spike artifacts and preserves
the actual command exit code and error wording. Private copies remain for
cross-step inputs such as `expected.txt`; do not publish that working directory.
`rr-leakscan` scans the exported results, not those private originals. This is an
evidence-boundary repair; the engine's URL-bearing error messages remain a
separate stable-alias closeout item.

Local checks for this preparation: shell/Python syntax, three offline canary
tests (`python3 -I scripts/object-store/test-real-step.py`) and
`scripts/gcp/test/test-job-chains.sh` PASS. Canaries preserve exit 37 and the
malformed-response wording, redact stdout/stderr/nested files and URL-encoded
identifiers, check the diagnostic header allowlist, and prove an injected public
leak fails the actual reference leak scan while sanitized results pass it.
No GCP command was run. No dependency or ADR amendment is needed for these scripts.

Parent rerun commands, separately approved/costed for each CPU (the variables
below must be supplied privately; no literal service-account/project identifiers):

```sh
# Ship the ignored env file at spikes/object-store-baseline/gcs.env.
# The example already prefixes EVERY step with:
# NATA_OBJ_ENV_FILE="$SRC/spikes/object-store-baseline/gcs.env"
cp scripts/object-store/real-bucket.steps.example heavy.steps
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --max-vm-hours 4 --vm-service-account "$VM_SERVICE_ACCOUNT" --vm-storage-scope read-write
scripts/gcp/orchestrate.sh plan --job heavy --machines c3d-highcpu-8 --max-vm-hours 4 --vm-service-account "$VM_SERVICE_ACCOUNT" --vm-storage-scope read-write
# After the parent approves each printed cost bound:
scripts/gcp/orchestrate.sh run --job heavy --machines n4a-highcpu-8 --max-vm-hours 4 --vm-service-account "$VM_SERVICE_ACCOUNT" --vm-storage-scope read-write --approved-usd "$APPROVED_ARM_USD" --out bench/gcp-runs/s11real-arm-rerun
scripts/gcp/orchestrate.sh run --job heavy --machines c3d-highcpu-8 --max-vm-hours 4 --vm-service-account "$VM_SERVICE_ACCOUNT" --vm-storage-scope read-write --approved-usd "$APPROVED_X86_USD" --out bench/gcp-runs/s11real-x86-rerun
```

Use fresh output directories and fresh independent prefixes for the two jobs;
run sequentially if reusing the one env file. Inspect every `.exit`, both
diagnostic traces, cleanup and leak scan even when cells miss. The real-bucket
example intentionally keeps running later steps after a failure to retain
cleanup and evidence. No real-store performance verdict is changed here.
