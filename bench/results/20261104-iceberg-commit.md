# The Iceberg commit under concurrent writers, retries and crashes (verification-plan item 24, gate)

Status: **built, parse-checked and compile-checked only. Nothing has been run**: not the model checker, not the Rust tests,
not the catalog. On the shared dev box only `cargo check --tests --bins` and a SANY parse of the spec were allowed, and every
run goes to GCP by the owner's decision. **GCP results: to be filled.** Every statement below about how the model or the
spike behaves is therefore *reasoned* (the traces in "Mutants" were walked by hand), not measured. The facts about Iceberg,
the REST API and object stores are from primary sources read on 2026-11-04 (section 1).

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

**State counts and diameters: to be filled** from `model/model-summary.txt` of the GCP run (`generated`, `distinct`, `depth` per configuration). Nothing is known about them yet; if a configuration hits the per-run limit (`IC_TLC_TIMEOUT`, default 45 min) it reports TIMEOUT, which is a failure, and the bound must then be reduced.

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

**Results: CAUGHT / SURVIVED / INVALID per mutant: to be filled.**

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

## 5. GCP results: to be filled

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
