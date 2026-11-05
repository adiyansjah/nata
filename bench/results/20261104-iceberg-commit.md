# The Iceberg commit under concurrent writers, retries and crashes (verification-plan item 24, gate)

Status: **first GCP run done on 2026-11-05 (section 8): the verdict is FAIL, and not because the model checker found a violation.** The phase `model-full` used up its 7,200 s budget: one configuration timed out (`three-store-ser`, no result) and 14 were never started (`three-rest-snap-i4`, `live-2`, `live-3` and the 11 canaries), so **15 of the 24 model runs have no result**. What did run passed: the seven smaller model configurations, `three-rest-ser` and `three-rest-snap` (no invariant violated), 9 of 9 mutants caught, and the commit spike against the Apache REST reference fixture 1.12.0 and against the object-store mode (every check as expected). The pyiceberg interop phase was skipped. Sections 1-7 below were written before the run; where they say "to be filled" or "nothing has been run", section 8 is the current state. The facts about Iceberg, the REST API and object stores are from primary sources read on 2026-11-04 (section 1).

Code: `spikes/iceberg-commit/` (throwaway, own Cargo workspace; delete once the table-format ADR is accepted, keep this file).
Entry point on a clean Debian VM: `spikes/iceberg-commit/gcp-check.sh <results-dir>`.

## 1. Facts established from primary sources (all accessed 2026-11-04)

| # | Fact | Source |
|---|---|---|
| F1 | Format versions 1, 2 and 3 are "complete and adopted"; **v4 is under development and not adopted**. v2 adds row-level deletes; v3 adds new types, default values, multi-argument transforms, row lineage, **binary deletion vectors**, encryption keys. Latest Java release 1.12.0 (2026-09-30). | Table spec, "Format Versioning": <https://iceberg.apache.org/spec/> (raw: <https://raw.githubusercontent.com/apache/iceberg/main/format/spec.md>, sha256 `3d27f65a…9f47`); GitHub releases API <https://api.github.com/repos/apache/iceberg/releases/latest> |
| F2 | Optimistic concurrency: writers build new metadata assuming the base is current and commit by an atomic swap of the metadata pointer; if the base is no longer current the writer retries on the new version; "the conditions required by a write to successfully commit determine the isolation level". | Spec, "Optimistic Concurrency" |
| F3 | A sequence number is assigned to every successful commit; "if the commit fails and must be retried, the sequence number is reassigned and written into new snapshot metadata". Manifests and files inherit it (entries carry `null`); a manifest can be reused across attempts, only the manifest list is rewritten. A new manifest list is written for each attempt. | Spec, "Sequence Numbers", "Manifest Lists", "Sequence Number Inheritance" |
| F4 | Conflict rules in the spec are four bullets: appends can always be applied; replace (compaction, format change) must verify the files it removes are still in the table; deletes must verify the specific files to delete are still in the table (expression deletes can always be applied); schema/spec changes must validate no schema change. **The spec does not define serializable vs snapshot isolation or the checks behind them.** | Spec, "Commit Conflict Resolution and Retry" |
| F5 | Isolation levels live in the Java implementation: enum `IsolationLevel {SERIALIZABLE, SNAPSHOT}`; "the serializable level guarantees that an ongoing UPDATE/DELETE/MERGE fails if a concurrent transaction commits a new file that might contain rows matching the condition"; table properties `write.delete.isolation-level`, `write.update.isolation-level`, `write.merge.isolation-level`, default `serializable`; `commit.retry.num-retries` 4, min/max wait 100 ms / 60 s, total timeout 30 min. The checks are the `validate*` methods of `MergingSnapshotProducer`. | Tag `apache-iceberg-1.12.0`: `api/.../IsolationLevel.java`, `docs/docs/configuration.md`, `core/.../MergingSnapshotProducer.java` (<https://github.com/apache/iceberg/tree/apache-iceberg-1.12.0>) |
| F6 | Table metadata has `last-sequence-number` ("monotonically increasing long"), `snapshots`, `snapshot-log`, `metadata-log`, `refs` (`main` always points to `current-snapshot-id`). Snapshot `operation` ∈ append, replace, overwrite, delete. Writers should avoid snapshot-id collisions; Java uses a random positive id. | Spec, "Table Metadata", "Snapshots", "Assignment of Snapshot IDs" |
| F7 | The reference builder rejects `add-snapshot` when the sequence number is not greater than `last-sequence-number` unless the snapshot has no parent (`RetryableValidationException`), and rejects a snapshot id that already exists. **This is implementation behaviour, not in the REST spec.** | `core/.../TableMetadata.java` (`Builder.addSnapshot`), tag 1.12.0 |
| F8 | Deletes: position delete files (v2) are **prohibited in v3 tables** (deprecated; existing ones stay valid), deletion vectors (Puffin `deletion-vector-v1`, at most one per data file per snapshot) are v3, equality delete files are prohibited in v4. Scan planning: a position delete applies to a data file in the same partition when the data file's sequence number is <= the delete's; an equality delete when strictly <. | Spec, "Delete Formats", "Scan Planning"; Puffin spec <https://raw.githubusercontent.com/apache/iceberg/main/format/puffin-spec.md> |
| F9 | The file-system commit scheme (rename of `v<N>.metadata.json`) is **deprecated, "unsafe in object stores and local file systems", and will be removed in v4**. The sanctioned scheme is a metastore that does check-and-put on the metadata pointer. A catalog-less commit by conditional create is therefore *our* design (it makes the file-system scheme safe by using put-if-absent instead of rename), not an Iceberg-defined one. | Spec, "File System Tables", "Metastore Tables" |
| F10 | REST commit: `POST /v1/{prefix}/namespaces/{namespace}/tables/{table}` (`updateTable`). Body `{requirements:[…], updates:[…]}`. Requirement types: `assert-create`, `assert-table-uuid`, `assert-ref-snapshot-id` (ref + snapshot-id, `null` = the ref must not exist), `assert-last-assigned-field-id`, `assert-current-schema-id`, `assert-last-assigned-partition-id`, `assert-default-spec-id`, `assert-default-sort-order-id`. Updates include `add-snapshot`, `set-snapshot-ref`, `remove-snapshots`. Servers must answer 400 to unknown ones. | REST spec <https://raw.githubusercontent.com/apache/iceberg/main/open-api/rest-catalog-open-api.yaml> (`info.version 0.0.1`, sha256 `7dc22426…997d6f`) |
| F11 | Commit responses: **409** "CommitFailedException, one or more requirements failed. The client may retry"; **500, 502 and 504** all "the commit state is unknown" (`CommitStateUnknownException`); 503 is *not* listed as unknown. Multi-table: `POST /v1/{prefix}/transactions/commit`. | same file |
| F12 | The REST spec now has an optional **`Idempotency-Key` header** (UUIDv7): a repeated request with the same key returns the stored final response for 200/204 and deterministic 4xx (including 409), 5xx are not stored; the server advertises support with `idempotency-key-lifetime` in `/v1/config` (example `PT30M`). The 1.12.0 reference adapter implements it (in-memory store with TTL and in-flight coalescing). | same file; `core/.../RESTCatalogAdapter.java`, `CatalogHandlers.java` (tag 1.12.0) |
| F13 | The Java client does NOT retry blindly after `CommitStateUnknown`: `SnapshotProducer` rethrows it without cleaning up files, and `RESTTableOperations` does a "lightweight reconciliation": for a snapshot-add-only commit it reloads and treats the commit as done if the expected snapshot id is present. | `SnapshotProducer.java`, `RESTTableOperations.java` (tag 1.12.0) |
| F14 | The reference server's commit is a compare-and-swap in the catalog: `CatalogHandlers.updateTable` validates the requirements against the loaded metadata, applies the updates, and `JdbcTableOperations` runs an UPDATE of the metadata location conditioned on the expected old location; "updated rows != 1" throws `CommitFailedException` (HTTP 409). | `CatalogHandlers.java`, `JdbcTableOperations.java` (tag 1.12.0) |
| F15 | **S3**: `PutObject`, `CompleteMultipartUpload` and `CopyObject` accept `If-None-Match: *` (create only) and `If-Match: <etag>`; failure is `412 Precondition Failed`; a concurrent request can give `409 Conflict`; `If-Match` on a key deleted meanwhile gives `404`; the first of several concurrent conditional writes wins. Conditional writes ignore in-progress multipart uploads. S3 has strong read-after-write consistency for PUT/DELETE **and LIST**; updates to a single key are atomic. Launch dates (secondary source, AWS "What's New" posts found by search): If-None-Match August 2024, If-Match November 2024, copy October 2025. | AWS user guide <https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html>, <https://docs.aws.amazon.com/AmazonS3/latest/userguide/Welcome.html#ConsistencyModel>; <https://aws.amazon.com/about-aws/whats-new/2024/11/amazon-s3-functionality-conditional-writes> |
| F16 | **GCS**: `ifGenerationMatch`; `0` = write only if no live object exists; mismatch is `412`. **Azure Blob**: `If-Match: <etag>` / `If-None-Match: *`; mismatch is `412`. | <https://docs.cloud.google.com/storage/docs/request-preconditions>, <https://learn.microsoft.com/en-us/azure/storage/blobs/concurrency-manage> |
| F17 | **`object_store` 0.14.2** (crates.io, 2026-09-15, file sha256 `f1796bc9…eb94`): `PutMode::{Overwrite, Create, Update(UpdateVersion{e_tag, version})}`; errors `AlreadyExists` / `Precondition`. S3: Create = `If-None-Match: *`, Update = `If-Match` on the ETag (retries 409); GCS: `x-goog-if-generation-match` (Create = `0`, Update = generation in `version`); Azure: `If-None-Match: *` / `If-Match`. **`LocalFileSystem` supports Create (atomic hard link) and returns `NotImplemented` for Update**; `InMemory` supports both. The crate docs show the read-modify-write loop for building a catalog on object storage. | crate source (`src/lib.rs`, `src/aws/mod.rs`, `src/gcp/client.rs`, `src/azure/client.rs`, `src/local.rs`) |

### Where the brief was wrong or incomplete (the spec wins)

1. "500/504 = CommitStateUnknown": **502 too**; 503 is not (F11).
2. "Isolation levels serializable vs snapshot" are **not in the spec**; they are Java behaviour and table properties (F4, F5). The model follows the Java rules.
3. v3 is **ratified** (F1), and it **removes position delete files for v3 tables** in favour of deletion vectors (F8). The spike writes v2 tables with position delete files; v3 deletion vectors are not modelled or written.
4. The REST spec has an **Idempotency-Key** mechanism the brief did not mention (F12); the reference fixture implements it. The model and spike use an operation token in the snapshot summary instead (the Flink-style approach); the header is not exercised.
5. A writer that omits `assert-ref-snapshot-id` is **not unprotected in the reference server**: its sequence-number check rejects a stale `add-snapshot` (F7). The known-bad "no CAS" writer therefore refreshes the sequence number at commit time, which is what a real bug of that shape would do.
6. The catalog-less mode rests on a scheme the spec calls deprecated and unsafe (F9); what makes it safe here is put-if-absent, not rename.

## 2. Part 1: the formal model

**Tool: TLA+ with TLC** (tla2tools v1.7.4, JDK 17), not Quint with Apalache. Why: TLC is one jar and needs only Java, which the catalog build needs anyway; it checks liveness under fairness exactly (Apalache does not do liveness the same way); it reports exact state counts and the shortest counterexample; and the model's value is in exhaustive interleavings of small bounded systems, which is what explicit-state checking does. Quint's nicer syntax does not outweigh needing Node and Apalache's separate pinning here. (Reasoned.) SANY parses the spec and all mutants (checked locally, section 6).

### What is modelled (`model/IcebergCommit.tla`, 464 lines)

One table. The catalog holds numbered metadata versions; the current one is the highest. **Style `REST`**: the server applies the writer's AddSnapshot to the *current* metadata if the requirement holds (assert-ref-snapshot-id, abstracted to "the table is still at the version I read"). **Style `STORE`**: no server; the writer builds the whole next metadata from *its* base and creates it with put-if-absent. Both are one atomic compare-and-swap on the version; they differ in who builds the new metadata, which matters when the CAS is removed.

Writers (2-3, one operation each): **append** (rows), **delete-where(key)** (position deletes on the rows that match at the base), **compact** (rewrite every live file into one, applying deletes). Steps: plan (read the table) → start upload of the data or delete file → wait until it is durable → write the manifest list → send the commit → wait. On 409 or after an unknown outcome: re-read; **look for my own snapshot (the operation token)**; **validate** against the snapshots committed since my base (append: nothing; delete: the files it targets are still live, and under `serializable` no concurrently added row matches its key; compact: its inputs are still live and no new delete touches them); rebuild on the new base re-applying the same plan; retry up to `MaxAttempts`.

Faults: a writer crashes at *any* step (budget); a request is lost or delivered twice (budgets); a writer times out at any moment while waiting, so the request may be applied, still in flight, or gone (CommitStateUnknown); uploads complete atomically and independently of the writer; orphan files are collected by a GC that may only delete a file that no snapshot references, whose writer is finished, and that no request in flight refers to (assumption A-GC: the real age threshold exceeds the lifetime of any request). Readers pin a snapshot and must keep seeing exactly it.

Not modelled: equality deletes, deletion vectors (so also none of v3's conflict rules), partitions, schema and partition-spec evolution, branches and tags, snapshot expiry (so "replaced files are deleted at expiry" is not exercised), multi-table transactions, `Idempotency-Key`, clocks, torn object writes (PUT is atomic, F15), stale LIST (S3 LIST is strongly consistent, F15), non-snapshot metadata changes (so `assert-ref-snapshot-id` and "version unchanged" coincide), 503 handling, and any difference between S3, GCS and Azure beyond "conditional create is atomic".

### Invariants (each a TLA+ state predicate, evaluated in every reachable state)

| Id | TLA+ name | Statement |
|---|---|---|
| I1 | `I1_FilesExist` | The catalog never points at metadata that references a missing file: every snapshot of the current metadata (all are "valid snapshots") has all its data files, delete files and manifest list durable. |
| I2 | `I2_NoLostUpdate` | The rows of the current snapshot equal the rows added by every successful commit minus the rows deleted by every successful commit, taken from a GHOST history of catalog applies (not from the metadata, so a lost snapshot cannot hide). |
| I3 | `I3_ReaderSeesSnapshot` | A reader that pinned snapshot S still finds S in the table and sees exactly the rows it saw when it pinned (so the snapshot record cannot change and no file it needs can go). |
| I4 | `I4_NoPhantom` | Under `serializable`: a delete-where commits a snapshot in which no row matching its predicate is live. Under snapshot isolation this is allowed (it is the write-skew/phantom the level permits); the configuration `three-rest-snap-i4` checks I4 anyway and **must fail**. |
| I5 | `I5_AtMostOnce` | An operation commits at most once, however many times it retries (a retry after CommitStateUnknown must not commit its changes twice). |
| I6 | `I6_SeqIncreases` | Each version's `last-sequence-number` is strictly greater than the previous version's, and no two snapshots share a sequence number. |
| I7 | `I7_NoSnapshotLost` | Each version keeps every snapshot of the previous one and its new snapshot is a child of the previous current snapshot: history is linear and a conflicting rewrite cannot drop a committed snapshot. |
| L1 | `Termination` | With bounded retries and weak fairness on every writer step, delivery and upload completion, all writers end (done, aborted, failed or crashed). |
| L2 | `Progress` | Some writer makes progress: a commit lands, or every writer crashed. |

### Configurations and bounds (`model/cfg/*.cfg`)

`MaxAttempts` 2 (one retry), `MaxVer` writers+2 (never binds without a mutant), `MaxPins` 1, rows {1(a), 2(b)} initial, row 3 (a) and 4 (b) appended. Budgets crash/duplicate/loss in brackets.

| Config | Writers | Style | Isolation | Budgets | Must |
|---|---|---|---|---|---|
| smoke-rest | append | REST | serializable | 1/1/1 | pass |
| appends2-rest, appends2-store | 2 appenders | REST, STORE | serializable | 1/1/1 | pass |
| ad-rest-ser, ad-store-ser | append(a) + delete-where(a) | REST, STORE | serializable | 1/1/1 | pass |
| dc-rest, dc-store | delete-where(a) + compact | REST, STORE | serializable | 1/1/1 | pass |
| three-rest-ser, three-store-ser | append + delete-where(a) + compact | REST, STORE | serializable | 1/1/1 | pass |
| three-rest-snap | same | REST | snapshot | 1/1/1 | pass (all but I4) |
| three-rest-snap-i4 | same | REST | snapshot | 1/1/1 | **I4 violated** |
| live-2 | append + delete-where(a) | REST | serializable | 1/1/1 | L1, L2 hold |
| live-3 | append + delete + compact | REST | serializable | 0/0/1 | L1, L2 hold |
| canary-* (11 runs on three-rest-ser) | | | | | **each violated** |

The canaries are state predicates that are *meant* to fail, each proving a situation is reachable (no vacuous pass): an unknown outcome, a conflict, a committed retry, a committed compaction and delete, a validation abort, a writer that crashed after its commit landed, an old attempt's request landing after a newer attempt started, a duplicate delivery, an orphan that was collected, a pinned reader.

**State counts and diameters:** section 8 (first run: 755 to 129,194,672 distinct states; `three-store-ser` hit the 45-minute limit before it finished).

### Mutants (`model/mutate_model.py`)

Each mutant is a one-line text edit of a definition marked `MUTATION POINT`, applied to a *copy* made with `shutil.copy` (fresh mtimes); the script checks the edit applies exactly once, hashes the real spec before and after (byte-identical), SANY-parses the copy, and runs TLC on the mutant's configurations in order until one violates an invariant. The unmutated copy is staged, parsed and (with `--baseline`) model-checked first with the same code. Result vocabulary: CAUGHT (an invariant is named), SURVIVED (passed everywhere it was tried: a failure), INVALID (edit did not apply, did not parse, or TLC failed for another reason: a failure).

| Mutant | Edit | Configs tried | I expect (hand-traced) |
|---|---|---|---|
| `no-cas` (required) | `CasOk(q) == TRUE` | appends2-rest, appends2-store, three-rest-ser | I2 (also I6, I7) |
| `skip-validation` (required) | `Validated(w) == TRUE` | dc-rest, ad-rest-ser, three-rest-ser | I2 (compaction resurrects the deleted row), I4 (delete misses a concurrent insert) |
| `blind-retry-after-unknown` (required) | `Mine(w)` is false when the writer's state is `unknown` | smoke-rest, appends2-rest, appends2-store | I5 |
| `commit-before-durable` (required) | `UploadDone(w)` also accepts `uploading` | smoke-rest, appends2-rest | I1 (and I3) |
| `reuse-sequence-number` (required) | `NextSeq(bm) == bm.lastSeq` | smoke-rest, appends2-rest | I6 |
| `compaction-deletes-live-files` (required) | on ack, compaction purges its input files | dc-rest, three-rest-ser | I1 (and I3) |
| `accept-stale-base-on-retry` (required) | `NewBaseOnRetry(w) == base[w]` | appends2-rest, appends2-store, three-rest-ser | I2 (also I6, I7) |
| `gc-ignores-active-writers` (extra) | GC also takes files of running writers | smoke-rest, appends2-rest | I1 |
| `gc-ignores-inflight-requests` (extra) | GC also takes files an in-flight request names | smoke-rest, appends2-rest | I1 |

The "reuse a sequence number" mutant is caught because the model's catalog is a pure compare-and-swap (F10); the reference server's additional sequence-number check (F7) would also reject it, which the model deliberately does not include.

**Results:** section 8 (9 of 9 caught).

### Checks of the harness itself (written; only the dry run was executed locally)

`model/test_model_tools.py` (not run locally: it is a test): the staging copy has fresh mtimes even when the source is old; two stagings in a row both pass their baseline (byte-identical to the real spec) and the second sees no leftovers; an edit that does not apply (0 times) or applies twice makes the mutant INVALID; every mutant's edits apply exactly once, change the spec, and leave the real spec untouched; `mutate_model.py --dry` run twice in a row passes; the TLC output classifier distinguishes PASS, VIOLATED, ERROR (including "completed" with a non-zero exit status). `model/lint_spec.py` (run locally: OK): every leaf action assigns every state variable exactly once, every `MUTATION POINT` edit target occurs once, every invariant named in a cfg exists. The dry run was executed twice in a row locally, both passed (section 6).

## 3. Part 2: the concurrent-commit spike (`spikes/iceberg-commit/src`)

### The catalog under test

Chosen: **the Apache Iceberg REST fixture 1.12.0** (`org.apache.iceberg.rest.RESTCatalogServer` over a SQLite JDBC catalog, warehouse on local disk), **built from the verified source release** (`apache-iceberg-1.12.0.tar.gz`, sha512 `505b4d92…1a28` from dist.apache.org) with `./gradlew :iceberg-open-api:shadowJar` (the task the project's own `docker/iceberg-rest-fixture/README.md` names), JDK 17.0.21+9 (Temurin; aarch64 sha256 `3e4eca7e…7084`, x64 `8b9b611d…8711`). No Docker. Why: it is the reference implementation of the REST spec, its commit path is exactly the Java code that real writers meet (F14), it is a single process, and the spike speaks plain REST so another server can be swapped in by URL. Why not a prebuilt artifact: the `iceberg-open-api-1.12.0-test-fixtures.jar` on Maven Central (14,038 bytes, sha256 `79963280…e5e4`) has no dependency metadata (empty `.pom` and `.module`), so it cannot be run without assembling the classpath by hand; the project publishes the runnable fixture only as a Docker image.

Others evaluated from release metadata only (nothing was run): **Apache Polaris 1.8.0** (`polaris-bin-1.8.0.tgz`, 424 MB, sha256 `2d295caf…76d0`; runs on a JVM without Docker; needs bootstrap credentials and a catalog created through its management API: more moving parts); **Lakekeeper v0.14.2** (Rust binary, aarch64 sha256 `501b946d…c6889`, x86-64 `e1eb2de7…1b8`; its README lists S3, Azure and GCS storage profiles and "no local state", i.e. an external database; I did not find a local-disk warehouse in what I read); **Nessie 0.109.0** (`nessie-quarkus-0.109.0-runner.jar`, 276 MB, sha256 `7ec0ed8c…b35f8`; not examined). Re-running the suite against Polaris or Lakekeeper is a follow-up once the project chooses a catalog; the suite needs only a URL, a namespace and a table.

Honest limit: the fixture is a *test fixture*, not a production catalog. It shows that the Java reference commit path is atomic under our load; it says nothing about a particular production catalog's implementation.

### iceberg-rust

Not used, not evaluated. `iceberg` 0.11.0 exists on crates.io (updated 2026-11-03, 4.6 M downloads; <https://github.com/apache/iceberg-rust>). The spike needs control of every request (drop a response after the server applied it, send a request twice, write the manifest and list files itself, build a commit without the CAS requirement) which a catalog client library hides. Whether `iceberg-rust` is mature enough for Nata's write path is a separate bake-off and is not answered here.

### Writers, faults, checker

`ic-spike` (Rust, plain HTTP via `ureq`, no async). Files are real Iceberg v2 files written by hand (`src/avro.rs`, `src/ice.rs`): Avro data files (Iceberg allows Avro; schema `id long [1]`, `k string [2]`), position-delete files, v2 manifests and manifest lists with the spec's field ids and header keys. Operations: **append**, **delete-where(k)** (position deletes), **compaction** (replace; applies deletes; drops the manifests it replaced). The correct writer implements the model's protocol (new snapshot id per attempt, operation token `nata.op` in the snapshot summary, token lookup after 409/unknown, validation, bounded retries, manifests reused and only the manifest list rewritten per attempt, F3).

Faults (`src/catalog.rs`, `FaultyCatalog`): *drop-response@n* (the catalog applies the commit, the writer sees a timeout: CommitStateUnknown), *drop-request@n*, *dup@n* (the request reaches the catalog twice), *delay@n*; a writer **dies at a chosen step** (`after-plan`, `after-files`, `after-list`, `after-commit`; the process exits with status 137 as after SIGKILL, no cleanup), and the parent **SIGKILLs** writer processes at random moments. A fsynced per-writer **journal** records intent, attempts, answers and results before each step.

**Known-bad writers** (each removes one protection; the checker must flag each):

| Mode | Bug | Forced scenario that exposes it | Checker must report |
|---|---|---|---|
| `bad-no-cas` | no `assert-ref-snapshot-id` (REST) / overwrite instead of create-if-absent (store); refreshes seq and parent right before the commit so the server accepts a manifest list built from a stale table | `lost-update`: 3 appenders read the same base, then commit in rank order | I2 |
| `bad-blind-retry` | after CommitStateUnknown reloads and re-commits without looking for its own snapshot | `unknown-outcome`: the commit is applied, the response is dropped | I5 |
| `bad-skip-validation` | after a conflict rebuilds on the new table without validating | `delete-then-compact`, `compact-then-delete` (rows resurrected or a delete lost: I2); `phantom-serializable` (I4) | I2, I4 |

The suite also runs each scenario with the correct writer (must pass), and the bad writers in scenarios where they must *not* misbehave (`lost-request`, `phantom-snapshot-isolation`, `append-then-compact`: a checker that fires there is over-sensitive). Ordering is forced with a file barrier plus commit tickets, so detection does not depend on a race. The `stress` runs (4, 8 and 16 writer processes in turn, 3 operations each, 25% chance of a fault per commit call, 20% of a crash point, one SIGKILL) are the evidence for the correct writer; the same stress with the bad writers is reported but not gating (a race may not occur).

**Checker** (`src/checker.rs`): replays *every* snapshot of main's ancestry and every metadata version in the metadata log, and reconciles with the journals. I1 every named file exists (manifest list, manifests, data, delete files, metadata files); I2 rows(n) = rows(n-1) + what the operation added - what it deleted, an acknowledged commit is in the table, an aborted/rejected one is not; I3 a snapshot record is identical in every metadata version that has it; I4 a serializable delete leaves no matching row in its own snapshot; I5 each operation token at most once, no duplicate live row; I6 strictly increasing sequence numbers, `last-sequence-number` is the maximum, never decreases; I7 one line of history, snapshot log equals the ancestry, no version drops a snapshot. It also lists **orphan files** (not an error: crashes and aborts leave them). `tests/scenarios.rs::checker_sees_tampering` damages a healthy table five ways (delete a data file, duplicate a sequence number, drop a snapshot from the latest metadata, change a snapshot record, alter a journal) and requires the matching invariant to fire.

### Object-store mode

Included, and faithful for exactly one operation. `StoreCatalog` commits by creating `metadata/v<N+1>.metadata.json` with `PutMode::Create`; the current version is the highest `N` (LIST). On `LocalFileSystem` Create is an atomic hard link: the same contract as S3 `If-None-Match: *`, GCS `ifGenerationMatch=0`, Azure `If-None-Match: *` (F15-F17). **Not covered:** network failures and timeouts (the fault injector adds them at the client), `PutMode::Update` (`LocalFileSystem` does not implement it; it would need `InMemory` or MinIO), the 409/404 races S3 documents, S3 listing behaviour, and the cost and latency of real buckets. A run against real S3/GCS needs the owner's cost approval and is not part of this check.

## 4. How to run on GCP (not done; needs the owner's yes)

`gcp-check.sh <results-dir>` runs, in order, and writes one file per phase plus `summary.txt`: `tools` (pinned JDK + tla2tools, sha256-verified) → `model-lint` (lint + harness tests) → `model-parse-and-edits` (every mutant parses, edits apply once; twice in a row) → `model-smoke` (smoke-rest and appends2-rest: does TLC evaluate the spec?) → `rust-build`, `rust-tests` (unit tests, the scenarios against the mock REST catalog and the object-store mode, stress with writer processes) → `catalog-build` (Gradle) → `catalog-start` → `spike-rest-catalog` (the suite against the reference fixture) → `interop-pyiceberg` (not gating: an independent reader must list the same files from our manifests; skipped if `python3-venv` is missing) → `spike-object-store` → `model-full` (all configurations, liveness, canaries) → `model-mutants`. Exit status 0 only if every gating phase passed; any SURVIVED/INVALID mutant, unreached canary, missed known-bad writer, TIMEOUT or mismatch fails the run.

The orchestrator whitelists job names, so a job `iceberg` must be added (outside this task's edit scope; the same two edits as for the other spikes):

```bash
# scripts/gcp/orchestrate.sh, the case list in do_run: add |iceberg to  ci|bench|all|genops|verify|...
# scripts/gcp/job.sh: add this block before the second `case` and add |iceberg to that case's list
  iceberg)
    # verification-plan item 24 (spikes/iceberg-commit): TLA+ model (TLC, every mutant) + commit spike against the Apache REST fixture
    export IC_WORK="$WORK/iceberg-commit-work" IC_RESUME=1
    run ic-all bash -c 'cd "$SRC" && spikes/iceberg-commit/gcp-check.sh "$RESULTS/iceberg-commit" > "$RESULTS/iceberg-commit-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-commit-exit.txt"; true' ;;
```

Then (ARM64 N4A, Spot, us-central1, as for the other ARM runs):

```bash
scripts/gcp/orchestrate.sh plan --job iceberg --machines n4a-highcpu-8 --max-vm-hours 4     # prints the plan and the worst case; calls nothing
# only after the owner's explicit yes to the cost:
scripts/gcp/orchestrate.sh run  --job iceberg --machines n4a-highcpu-8 --max-vm-hours 4 --approved-usd 0.46
```

`plan` on 2026-11-04 (prices from `scripts/gcp/prices.env`, fetched 2026-11-03) printed a **worst case of $0.457** for 4 VM-hours (n4a-highcpu-8 Spot $0.0984/h, 20 GB + 30 GB Hyperdisk, external IP). The expected cost is lower than the cap (an estimate: 2-3.5 h x $0.0984 plus disks, about $0.25-0.40); the cap, not the estimate, is what `run` enforces.

**Expected duration (an ESTIMATE; nothing has been run):** tools 2-3 min; lint/tests/parse 1 min; Rust build 4-8 min cold on ARM64 and tests 2-5 min; Gradle build of the fixture 8-20 min cold; both suites 3-10 min; **the full unmutated model is the unknown: 20-120 min** (state counts unknown); mutants 5-30 min (counterexamples appear early); interop 2-4 min. **About 1.5-3.5 hours.** If the 4-hour cap is hit, the orchestrator ends the run; `IC_RESUME=1` lets a restart skip phases that already passed.

If the unmutated model hits the per-run time limit, reduce `MaxAttempts` or the budgets in `model/cfg/*.cfg` and say so in the results.

## 5. GCP results: to be filled (superseded by section 8, which holds the first run)

| Item | Result |
|---|---|
| Machine, region, date, run id, git commit | |
| Tool versions (`tool-versions.txt`), fixture jar sha256 (`catalog-version.txt`) | |
| `summary.txt` verdict | |
| Unmutated model: per configuration generated / distinct states, depth, seconds | |
| I4 under snapshot isolation (`three-rest-snap-i4`) | |
| Liveness (`live-2`, `live-3`) | |
| Canaries reached (11 of 11?) | |
| Mutants: CAUGHT / SURVIVED / INVALID, invariant named, configuration, seconds | |
| `rust-tests` | |
| Spike vs REST fixture: forced / crash matrix / stress, known-bad writers caught n of n, orphans left | |
| Spike vs object-store mode: same | |
| pyiceberg interop | |
| Bugs found in the model, the spike or the fixture setup, and what they changed | |

## 6. What was checked locally, and what could not be checked without running

Run on the dev box (all within the allowed set):

- `cargo check --tests --bins` through `scripts/local-heavy.sh` in `spikes/iceberg-commit`: **clean, no warnings**.
- SANY (tla2tools v1.7.4, Temurin 17.0.21 JRE) parse of `IcebergCommit.tla`, `MC.tla`: **parses**. `model/mutate_model.py --dry` (stages the model, applies every edit, SANY-parses every mutant, byte-compares the real spec) run **twice in a row**: both "BASELINE staged and parsed", 9 of 9 mutants "edit applies once; parses"; `IcebergCommit.tla` sha256 `365cab76…8763`, `MC.tla` `a15593ad…e827f9`.
- `model/lint_spec.py`: **OK** (every leaf action assigns each of the 17 state variables once; the 9 mutation points and all cfg names resolve).
- `bash -n` on every shell script; `ast.parse` on every Python file.

**Not checked (needs a run):** that TLC evaluates the spec without an evaluation error (SANY does not catch `CHOOSE` on an empty set, a bad record field, a type mismatch in a comparison); that the unmutated model passes (a flaw in my protocol model would show as a baseline violation; I walked the main paths by hand but that is not a check); the state counts and run times; that each mutant is caught, and by the invariant I expect; liveness; that the canaries are reachable. All of the Rust beyond type-checking: the Avro codec, the writers, the checker, the mock, the scenarios, the unit tests. The Gradle build of the fixture (that `:iceberg-open-api:shadowJar` works from the source tarball on JDK 17 on ARM64, that its output has the expected name), that the fixture starts with the `CATALOG_*` environment variables I derived from its Dockerfile and from the strings in `RCKUtils.class` and `RESTCatalogServer.class`, that its JDBC/SQLite catalog copes with the concurrent load without answering 500 (the correct writer treats a 500 as CommitStateUnknown and recovers, so this costs retries, not correctness), that it accepts the snapshot JSON and the requirement `snapshot-id: null` for the first commit, and that pyiceberg reads our manifests. Behaviour of `ureq` 3.5.1 and `object_store` 0.14.2 at run time.

Scope decisions the owner may want to revisit: equality deletes and v3 deletion vectors are not modelled or written; the model has one operation per writer; the suite's catalog is a test fixture; real S3/GCS/Azure conditional writes are not exercised.

## 7. Decisions needed from the owner

1. **A yes (or no) to the GCP run** at the cost above, and permission for someone to add the `iceberg` job to `scripts/gcp/job.sh` and `orchestrate.sh` (two small edits outside this task's scope; other agents are editing `job.sh`).
2. **Dedupe of a retried commit:** the spike and model use an operation token in the snapshot summary (as Flink does). The REST spec now offers `Idempotency-Key` (F12). Which should Nata's writer use, or both? The spike does not exercise the header.
3. **Which catalog to certify against.** The Apache fixture is the reference, not a product. Once the project picks a catalog (Polaris, Lakekeeper, Nessie, a cloud one), the suite should be re-run against it; that needs its setup (credentials, warehouse) and possibly cost.
4. **How far to go on the object-store mode.** Local `Create` is faithful for one operation; a run against real S3/GCS/Azure (and `PutMode::Update` for a pointer file) costs money and needs credentials.
5. **v3 scope.** If Nata targets v3 tables, position delete files are not allowed; deletion vectors and their conflict rule (at most one DV per data file, merge on conflict) need a model extension and writer support. Say whether v3 is in scope for the first slice.

## 8. GCP run `iceberg-full-arm64`, 2026-11-05: verdict FAIL (the model phase ran out of time); nothing here is a pass

**Run.** Job `iceberg` (the whole `spikes/iceberg-commit/gcp-check.sh`), one `n4a-highcpu-8` Spot VM (Neoverse-N3, 8 vCPU, 16 GB, Debian 12 ARM64, kernel 6.1.0-54-cloud-arm64, `rustc` 1.99.1), 2026-11-05 (the job ran 00:49:45 to 03:05:17 UTC; the orchestrator finished at 03:05:53). The run's own report: finished, 1 incarnation, 0 preemptions, 4 steps completed, **8,197 s of VM time counted, no resources left after cleanup, estimated cost $0.224** (last machine rate $0.0984/h x counted seconds, so VM time only; the 20 GB boot disk, 30 GB data disk and external IP are not in that figure). Tools: Temurin JDK 17.0.21+9, TLC 2.19 (tla2tools v1.7.4), Apache Iceberg REST fixture 1.12.0 built from the source release (jar sha256 `e92cfdba…0277e6`, Gradle `BUILD SUCCESSFUL in 2m 51s`). The model files on the VM are the ones in the repository today (`IcebergCommit.tla` sha256 `365cab76…8763`, `MC.tla` `a15593ad…e827f9`, `Cargo.lock` `639035e5…ac4bc`); the VM recorded `git: unknown` because the source is uploaded as a tarball. Raw outputs: `bench/results/raw/20261105-iceberg-full-arm64/` (about 280 KB). **The logs there are truncated**: `model-lint.txt`, `rust-build.txt` and every `mutants/*.tlc.txt` keep only their last 50 lines (a first line says so), the TLC progress logs of the model runs are whole (they are small), and the 7.3 MB REST-fixture request log and the journal archives are not copied (`README.txt` in that directory lists what was left out).

**Verdict line:** `VERDICT FAIL (1 gating phase(s) failed ...)`; `iceberg-commit-exit.txt` holds `exit status: 1`.

### Phases

| Phase | Result | Seconds | Gating |
|---|---|---|---|
| tools | PASS | 4 | yes |
| model-lint | PASS | 0 | yes |
| model-parse-and-edits | PASS | 5 | yes |
| model-smoke | PASS | 6 | yes |
| rust-build | PASS | 14 | yes |
| rust-tests | PASS | 10 | yes |
| catalog-build | PASS | 175 | yes |
| catalog-start | PASS | 2 | yes |
| spike-rest-catalog | PASS | 20 | yes |
| interop-pyiceberg | **SKIPPED** (exit 77) | 13 | no |
| spike-object-store | PASS | 6 | yes |
| model-full | **FAIL** (exit 1) | 7,864 | yes |
| model-mutants | PASS | 12 | yes |

### The unmutated model (`model-full`, TLC with 8 workers, 10,000 MB heap, 2,700 s per run, 7,200 s for the whole phase)

`model-summary.txt` ends `TOTAL mismatches=15`: 1 TIMEOUT and 14 NOT-RUN. In the table below "result" is what happened; for the rows marked "not run" nothing was checked.

| Run | Expected | Result | Generated | Distinct | Depth | Seconds |
|---|---|---|---|---|---|---|
| smoke-rest | PASS | PASS | 1,782 | 755 | 18 | 1 |
| appends2-rest | PASS | PASS | 1,225,228 | 290,636 | 35 | 5 |
| appends2-store | PASS | PASS | 1,225,228 | 290,636 | 35 | 5 |
| ad-rest-ser | PASS | PASS | 1,165,651 | 280,106 | 35 | 5 |
| ad-store-ser | PASS | PASS | 1,165,651 | 280,106 | 35 | 6 |
| dc-rest | PASS | PASS | 1,106,074 | 269,576 | 35 | 5 |
| dc-store | PASS | PASS | 1,106,074 | 269,576 | 35 | 5 |
| three-rest-ser | PASS | **PASS** | 683,202,106 | 116,840,733 | 52 | 2,481 |
| three-store-ser | PASS | **TIMEOUT, no result after 2,700 s** | - | - | - | 2,700 |
| three-rest-snap | PASS (all but I4) | **PASS** | 756,039,330 | 129,194,672 | 52 | 2,650 |
| three-rest-snap-i4 | VIOLATED:I4_NoPhantom | not run (budget spent) | - | - | - | - |
| live-2 | PASS | not run (budget spent) | - | - | - | - |
| live-3 | PASS | not run (budget spent) | - | - | - | - |
| 11 canaries (`canary-Unknown`, `-Conflict`, `-RetryCommitted`, `-CompactCommitted`, `-DeleteCommitted`, `-ValidationAbort`, `-CrashedButCommitted`, `-LateOldAttempt`, `-DuplicateDelivered`, `-OrphanCollected`, `-ReaderPinned`) | each VIOLATED:Canary_* (a canary is a deliberately failing check; VIOLATED is its EXPECTED outcome, not a finding) | not run (budget spent) | - | - | - | - |

In every pair of REST and STORE configurations that finished, the state counts are identical to the last digit (the three pairs above).

**Why the phase ran out.** The phase budget (7,200 s) is checked only before a run starts. After the seven small runs (32 s), `three-rest-ser` (2,481 s) and the timed-out `three-store-ser` (2,700 s) the clock stood at 5,213 s, so `three-rest-snap` started; it finished at 2,650 s and the clock stood at 7,863 s (the phase took 7,864 s), after which 14 runs were refused. The 8-vCPU machine, not a violation, set the limit.

**`three-store-ser` was close to done (an inference, not a result).** Its last progress line (02:19:59, 2,644 s after it started) reads 661,016,772 states generated, 113,880,982 distinct, 3,146,046 left on the queue, depth 45. `three-rest-ser` showed 663,593,191 generated, 114,238,285 distinct, 2,856,061 on the queue, depth 46 at 2,400 s, and finished with 683,202,106 generated 76 s later. If the STORE configuration has the same state space as the REST one (true in all three pairs that finished, but not shown here), `three-store-ser` needed a few more minutes; it ran about 10% slower than `three-rest-ser` at the same state count. **No invariant violation was seen in its log, and nothing was concluded about it.**

**What `three-rest-ser` and `three-rest-snap` show.** For the bounded model (up to 3 writers each doing one operation: an append, a delete-where and a compaction; at most 2 attempts per writer; budgets of one crash, one duplicate delivery and one lost request; one pinned reader), TLC explored the whole reachable state graph and no invariant I1-I3, I5-I7 was violated under serializable isolation (`three-rest-ser`, which also checks I4) and under snapshot isolation (`three-rest-snap`, which does not check I4). TLC's own estimate of the chance that a fingerprint collision hid a state is printed in the logs: 0.0036 (optimistic) and 0.0043 (from the actual fingerprints) for `three-rest-ser`; 0.0044 and 2.1E-4 for `three-rest-snap`. These are small, not zero.

### Mutants (`model-mutants`, 12 s)

The baseline (unmutated copy) passed on `smoke-rest` (755 distinct states). **9 of 9 mutants CAUGHT, 0 SURVIVED, 0 INVALID** (the 7 required ones and the 2 extra ones):

| Mutant | Caught by | Configuration | Distinct states | Seconds |
|---|---|---|---|---|
| no-cas | I6_SeqIncreases | appends2-rest | 343 | 1 |
| skip-validation | I2_NoLostUpdate | dc-rest | 21,433 | 2 |
| blind-retry-after-unknown | I5_AtMostOnce | smoke-rest | 665 | 1 |
| commit-before-durable | I1_FilesExist | smoke-rest | 73 | 1 |
| reuse-sequence-number | I6_SeqIncreases | smoke-rest | 48 | 1 |
| compaction-deletes-live-files | I1_FilesExist | dc-rest | 461 | 1 |
| accept-stale-base-on-retry | I2_NoLostUpdate | appends2-rest | 18,623 | 2 |
| gc-ignores-active-writers | I1_FilesExist | smoke-rest | 114 | 1 |
| gc-ignores-inflight-requests | I1_FilesExist | smoke-rest | 185 | 1 |

Every mutant was caught on the first configuration in its list (section 2). The section 2 prediction (the invariant I expected from a hand trace) held for 8 of 9; for `no-cas` I predicted I2 (also I6, I7) and TLC reported I6 first, which is consistent with "also I6" but is not I2. TLC reports one violated invariant per mutant, so for `skip-validation` the I4 violation I predicted for the delete-misses-an-insert case was not exercised. The mutants were run only on small configurations; none was run against a `three-*` configuration.

### The commit spike against the Apache REST reference fixture (`spike-rest-catalog`, 20 s)

Fixture 1.12.0 started on port 8181 (`catalog-start.txt` lists the endpoints it advertises). `SUMMARY backend=rest forced=18 (known-bad writers caught 5/5) crash-matrix=12 stress-correct=6/6 failures=0`, `SUITE PASS`:

- 18 forced interleavings (lost update, unknown outcome, lost request, duplicate request, delete-then-compact, compact-then-delete, phantom under serializable and under snapshot isolation, append-then-compact, unknown-then-conflict): the correct writer passes every scenario; the five known-bad writer/scenario pairs are flagged with the expected invariant: `bad-no-cas` I2, `bad-blind-retry` I5, `bad-skip-validation` I2 (delete-then-compact), I2+I4 (compact-then-delete) and I4 (phantom, serializable). Under snapshot isolation `bad-skip-validation` is expected to pass and does.
- 12 crash points (a writer dies after plan, files, list and commit, for each of append, delete and compact): all pass, each leaving 5-8 orphan files, which the checker treats as expected.
- 6 stress runs with writer processes, lost responses and requests, duplicates, crash points and one SIGKILL each (seeds 1-6, 4/8/16 writers, 3 operations each): all pass; 7 to 26 snapshots checked per run, 8 to 27 metadata versions, 11 to 47 journalled operations, 30 to 266 orphan files.
- Stress with the known-bad writers (informational, not a gate): `bad-no-cas` flagged in 3 of 3 runs, `bad-skip-validation` 3 of 3, **`bad-blind-retry` 0 of 3**. A stress run only exposes a blind retry if a race happens to occur; the forced scenario is what detects it.

### The object-store mode (`spike-object-store`, 6 s)

Conditional create on a local file system (`PutMode::Create`, an atomic hard link). `SUMMARY backend=store forced=18 (known-bad writers caught 5/5) crash-matrix=12 stress-correct=6/6 failures=0`, `SUITE PASS`; the same five detections, stress seeds 1-6 all pass (10 to 24 snapshots checked, 11 to 25 metadata versions, 13 to 47 journalled operations, 18 to 207 orphan files); the known-bad writers in stress: `bad-no-cas` 3 of 3, `bad-skip-validation` 3 of 3, `bad-blind-retry` 0 of 3. The Rust tests (`rust-tests`): 8 unit tests and 4 scenario tests (the checker detects tampering; the mock REST catalog and the store mode with forced scenarios and the crash matrix; stress with writer processes) all pass.

### The skipped phase

`interop-pyiceberg` (not gating) exited 77 after 13 s with `pyiceberg could not load the table: ModuleNotFoundError('Could not load a FileIO, please consider installing one: pip3 install "pyiceberg[pyarrow]" ...')`. The VM had been set up before the commit that installs `pyiceberg[pyarrow]` instead of `pyiceberg` (that fix is in the repository now). **No independent reader has yet read the manifests our writers produce.**

### What this does not show

- **No model result for `three-store-ser`** (timeout), for **`three-rest-snap-i4`** (that I4 can fail under snapshot isolation, which shows I4 is a live check), for **liveness** (`live-2`, `live-3`: termination and progress have not been checked at all) or for **any of the 11 canaries** (so at three writers it is not shown that an unknown outcome, a conflict, a committed retry, a validation abort, a duplicate delivery, a collected orphan or a pinned reader are reachable; a configuration that cannot reach them would pass vacuously). The mutants, caught on small configurations, show some of these situations are reachable there (unknown outcome with retry, compaction, garbage collection), not all.
- The passes are for a bounded model: at most 3 writers with one operation each, 2 attempts, budgets of one crash, one duplicate delivery and one lost request, one pinned reader, one table. A larger bound, a second operation per writer, or equality deletes and v3 deletion vectors (not modelled) could behave differently. TLC's fingerprint-collision estimate is small but not zero (above).
- The model is not tied to the code by a refinement check: the model passing and the spike passing are two separate facts about two descriptions of the protocol.
- The spike ran against one catalog, the Apache reference fixture 1.12.0 (a test fixture, JDBC/SQLite backed), not against any product (Polaris, Lakekeeper, Nessie, a cloud catalog), and the object-store mode used a local file system's atomic create, not S3, GCS or Azure over a network (no 409/404 races, no listing behaviour, no latency). Only ARM64 ran; x86-64 did not.
- The stress runs are six seeds of at most 16 writers with three operations each, one SIGKILL; `bad-blind-retry` is not found by stress (0 of 3), only by its forced scenario.
- Independent interop (pyiceberg reading our manifests) did not run.
- The cost $0.224 is VM time only.

## 9. Plan for the re-run of the unchecked model configurations (job `icebergmodel`; written 2026-11-05; run the same day, result in section 10)

The job runs only the 15 TLC runs that have no result (section 8), with no Rust build, catalog, spike, interop or mutant phase, on one `n4a-highcpu-16` Spot VM (16 vCPU, 32 GB; one TLC worker per vCPU, TLC heap about 19,800 MB). Description, variables and tests: `scripts/gcp/README.md`, section "`icebergmodel`". The verdict is FAIL if any run is a TIMEOUT, NOT-RUN, ERROR or unexpected result.

**Limits (defaults):** 4,500 s for `three-*` runs and `live-3`, 2,700 s for the others, 9,000 s (2 h 30 min) for the whole model phase, each run cut to the time left. **Order:** `three-store-ser`, `three-rest-snap-i4`, `live-2`, the 11 canaries, `live-3`.

**How long each run needs (what is known and what is not):**

| Run | Estimate |
|---|---|
| `three-store-ser` | About the time of `three-rest-ser`, which took 2,481 s on 8 vCPU. It did not finish in 2,700 s, and at the same number of distinct states (about 114 million) it was about 10% slower than `three-rest-ser` (2,644 s against about 2,400 s), which suggests about 2,750 s on 8 vCPU (an inference from one pair of progress lines, not a measurement). What 16 workers change is **unknown**. |
| `three-rest-snap-i4` | **Unknown.** TLC stops at the first violation of I4; how deep that is in the snapshot-isolation space was not measured. |
| `live-2` | **Unknown.** It checks liveness (termination and progress) on two writers; no liveness run has ever been timed. |
| `live-3` | **Unknown**, and the most likely to be expensive: three writers with liveness checking. |
| the 11 canaries | **Unknown**, each. Each stops at the first state where its situation is reached, in the `three-rest-ser` state space; the depth at which each is reached was not measured. |

So the 9,000 s budget is a cap, not a prediction: if the unknown runs are cheap the job ends early, and if not, the runs at the end of the order are `NOT-RUN` (a FAIL that names them), to be run in a second `icebergmodel` with `IC_MODEL_ONLY` set to those names.

**Cost** (prices from `scripts/gcp/prices.env`; the `n4a-highcpu-16` price of $0.1968/h is derived from the core and RAM prices of 2026-11-03, not fetched, and must be recomputed before the run): the first run cost $0.224 for 2 h 17 min of VM time on `n4a-highcpu-8` ($0.0984/h). A run that uses the full 9,000 s model budget is about 2 h 35 min of VM time, about $0.51 for the VM. **Proposed cap: `--max-vm-hours 3.0`**, for which `orchestrate.sh plan` prints a worst case of **$0.646** (VM, external IP, disks and the Hyperdisk allowance), so `--approved-usd 0.65`. It needs the owner's explicit yes first.

## 10. GCP run `icebergmodel-arm64`, 2026-11-05: verdict PASS for the model phases it covers

**Run.** Job `icebergmodel` (section 9), one `n4a-highcpu-16` Spot VM (Neoverse-N3, 16 vCPU, 32 GB, Debian 12 ARM64, kernel 6.1.0-54-cloud-arm64), approved cap $0.65 (`plan` worst case $0.646). Started 03:28 UTC, finished 04:19 UTC. The run's own report: finished, 1 incarnation, 0 preemptions, 4 steps completed, 3,028 s of VM time counted, resources left after cleanup: none; VM cost estimate $0.166 (VM time only). I also listed instances and disks afterwards: both lists were empty.
TLC: tla2tools v1.7.4 (sha256 `936a2622...e050e88`), 16 workers, heap 19,870 MB. `IcebergCommit.tla` sha256 `365cab76...fe54d88763`, the same as in section 8. Limits: 2,700 s per run, 4,500 s per three-writer run (`three-*`, `live-3`), 9,000 s in total.

**Verdict line:** `VERDICT PASS (every requested model run ended as expected; only the model was re-run: the other phases of the full check are NOT covered by this verdict)`; `TOTAL mismatches=0`. Phases: tools, model-lint and model-parse-and-edits PASS (8, 11 and 5 s); `model-full` PASS (2,848 s).

| Run | Expected | Result | Generated | Distinct | Depth | Seconds |
|---|---|---|---|---|---|---|
| three-store-ser | PASS | **PASS** | 683,202,106 | 116,840,733 | 52 | 1,485 |
| three-rest-snap-i4 | VIOLATED:I4_NoPhantom | VIOLATED I4_NoPhantom (expected) | 543,022 | 167,721 | 19 | 2 |
| live-2 | PASS | **PASS** | 1,165,651 | 280,106 | 35 | 23 |
| live-3 | PASS | **PASS** | 40,580,220 | 8,591,956 | 51 | 1,326 |
| 11 canaries (`canary-Unknown`, `-Conflict`, `-RetryCommitted`, `-CompactCommitted`, `-DeleteCommitted`, `-ValidationAbort`, `-CrashedButCommitted`, `-LateOldAttempt`, `-DuplicateDelivered`, `-OrphanCollected`, `-ReaderPinned`) | each VIOLATED:Canary_* | each REACHED | 81 to 127,474 | 69 to 42,909 | 7 to 17 | 1 to 2 each |

**What it shows.**
- `three-store-ser` (three writers, conditional-create store, serializable) explored the whole reachable state graph with no invariant violated. Its generated, distinct and depth numbers equal those of `three-rest-ser` in section 8 to the last digit, so all three REST/STORE pairs now agree. It took 1,485 s on 16 workers; the 8-worker run timed out at 2,700 s, so the section 9 guess of about 2,750 s on 8 vCPU was consistent with this.
- `three-rest-snap-i4` ended with I4_NoPhantom violated, as expected: I4 is a live check under snapshot isolation, not a vacuous one.
- Liveness (`live-2`, `live-3`) passed: no counterexample to termination and progress in the bounded model, with 0 states left on the queue. These are the first liveness results.
- All 11 coverage canaries were reached, so the situations the model claims to check (unknown outcome, conflict, committed retry, compaction, delete, validation abort, crash-but-committed, late old attempt, duplicate delivery, orphan collection, pinned reader) are reachable in it; the earlier passes were not vacuous for these.
- TLC fingerprint-collision estimates: `three-store-ser` 3.6E-3 (calculated, optimistic) and 4.9E-4 (from the actual fingerprints); `live-3` 1.5E-5 and 9.5E-6; `live-2` 1.3E-8 and 5.3E-9. These are small but not zero, the same caveat as section 8.

**What this does not show.**
- It closes the model part of the item 24 gate. It does not show the spike or the product code correct, and the model is not tied to the code by a refinement check.
- The bounds are the same as in section 8 (at most 3 writers with one operation each, 2 attempts, one crash, one duplicate delivery, one lost request, one pinned reader, one table). Equality deletes and v3 deletion vectors are not modelled.
- Only the model was re-run, on ARM64 only. The pyiceberg read of our manifests (section 8, skipped) was not re-run; the spike against the REST fixture and the object-store mode were not re-run; x86-64 did not run; no product catalog or real object store was involved.
- Liveness holds in the bounded model; no fairness or timing claim is made about a real catalog.

## 11. GCP runs `icebergspikes-arm64-r2` and `icebergspikes-x86-r2`, 2026-11-05: verdict PASS for the code side (the model was not run)

**Runs.** Job `icebergspikes` (`IC_MODE=spikes` of `gcp-check.sh`: every phase except the TLA+ model; `interop-pyiceberg` is GATING in this mode), one Spot VM each: `n4a-highcpu-8` (Neoverse-N3, kernel 6.1.0-54-cloud-arm64) and `c3d-highcpu-8` (AMD EPYC 9B14, kernel 6.1.0-54-cloud-amd64), started 05:05 UTC, both finished at about 05:11 UTC: 357 s and 346 s of VM time, 1 incarnation, 0 preemptions, resources left after cleanup: none, VM cost $0.010 and $0.007. (A first pair of runs, `icebergspikes-arm64` and `-x86`, was stopped after 3 minutes and deleted at a cost of $0.000, because `vm-startup.sh` did not yet install `python3-venv` for this job name: the interop phase would have been skipped, which is a FAIL in this mode. Fixed in `da8ef51` with a test that every job running the interop is listed there.)

**Verdict line (both):** `VERDICT PASS (every phase of the code side passed, including the pyiceberg interop; the TLA+ model was NOT run in this mode: its result is in the model runs, not here)`.

| Phase | ARM64 | x86-64 |
|---|---|---|
| tools | PASS 11 s | PASS 4 s |
| rust-build | PASS 13 s | PASS 17 s |
| rust-tests | PASS 10 s | PASS 9 s |
| catalog-build (Apache REST fixture 1.12.0 from the source release) | PASS 179 s | PASS 168 s |
| catalog-start | PASS 1 s | PASS 1 s |
| spike-rest-catalog | PASS 19 s | PASS 13 s |
| interop-pyiceberg (pyiceberg 0.12.0) | **PASS 12 s** | **PASS 14 s** |
| spike-object-store | PASS 7 s | PASS 4 s |

**What it shows.**
- On both architectures the commit spike's gating checks pass against the REST fixture and against conditional create on a local file system: `forced=18 (known-bad writers caught 5/5) crash-matrix=12 stress-correct=6/6 failures=0`, `SUITE PASS`, for each backend. Section 8 had this on ARM64 only.
- **The independent reader now reads our manifests:** pyiceberg 0.12.0 read the table our commit spike wrote and agreed with the spike on all 6 snapshots on each architecture (data file and delete file counts equal, 0 mismatches), `INTEROP PASS`. In section 8 this phase had been skipped (setup defect); this closes that gap for the REST fixture on both architectures.

**What it does not show.** The TLA+ model was not run (its result is section 10, ARM64 only; the model does not depend on the VM). The same limits as section 8 hold: one catalog (the Apache fixture, a test fixture), the object-store mode used a local file system's atomic create and not S3, GCS or Azure, the stress runs are six seeds of at most 16 writers with three operations each, the model is not tied to the code by a refinement check, and pyiceberg reads only what this spike wrote (6 snapshots), not a general set of tables.

