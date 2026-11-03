# Nata feature map

The full list of components and features, grouped by layer, with a priority tier on each. Use it to pick the next slice (see `way-of-working.md`).

## Tiers

| Tier | Meaning | Target |
|---|---|---|
| **T0** | Needed for the internal data team to run real queries | Phases 1-2 |
| **T1** | Needed to run in production and scale out | Phase 3 |
| **T2** | Differentiators: the reason to choose Nata | Phases 3-4 |
| **T3** | Later, only when pulled by a real user | After launch |
| **NO** | Explicit non-goal for now | - |

## Layer map

```
 Clients ──► Frontend ──► Planner ──► Scheduler ──► Executors ──► Storage / Connectors
 (A)         (B)          (C)         (E)           (D)           (F)       ▲
                                                                            │
 Cross-cutting: Cache (G) · Catalog (H) · Security (I) · Observability (J)
                Operability (K) · Quality tooling (L) · Ecosystem (M)
```

---

## A. Clients and protocols

| Feature | Tier | Notes |
|---|---|---|
| Postgres wire protocol (simple query) | T0 | Unlocks `psql`, JDBC, BI tools |
| Postgres wire: extended protocol (prepared statements, parameters) | T0 | Needed by most drivers |
| Postgres wire: result streaming and cancellation | T0 | Cancel must free memory and tasks |
| Interactive CLI / REPL (`nata sql`) | T0 | History, multiline, pretty tables, timing |
| Arrow Flight SQL | T1 | Fast bulk results, ADBC clients |
| TLS for client connections | T1 | |
| HTTP API (submit, status, fetch) | T3 | Only if a non-pg client needs it |
| Result pagination / large result handling | T1 | Spool large results to object storage |
| COPY TO (export Parquet/CSV to object storage) | T1 | |
| Web UI | NO | Revisit after internal adoption |

## B. SQL frontend

### B1. Parsing and binding
| Feature | Tier | Notes |
|---|---|---|
| Lexer and parser (Postgres-style base) | T0 | Start on `sqlparser`, own it if extensions outgrow it |
| Pipe syntax (`FROM t \|> WHERE \|> AGGREGATE`) | T2 | Part of the "clearer SQL" identity |
| Name resolution, scoping, aliases | T0 | |
| Type checker with strict coercion rules | T0 | No silent coercion, overflow is an error |
| Error diagnostics (position, suggestion) | T0 | "Did you mean ..." is a feature, not polish |
| Prepared statements and parameters | T0 | |
| Dialect spec document | T0 | Write before parser work starts |

### B2. Query constructs
| Feature | Tier | Notes |
|---|---|---|
| SELECT, WHERE, GROUP BY, HAVING, ORDER BY, LIMIT/OFFSET | T0 | |
| Joins: inner, left, right, full, cross | T0 | |
| Joins: semi, anti (explicit and from EXISTS/IN) | T0 | |
| Joins: lateral | T1 | |
| Multi-table joins (10+ tables) | T0 | Depends on join ordering (C) |
| CTEs (WITH) | T0 | Inline vs materialize decision |
| Subqueries: uncorrelated | T0 | |
| Subqueries: correlated (decorrelation) | T1 | Hard; common in BI SQL |
| Set operations: UNION [ALL], INTERSECT, EXCEPT | T0 | |
| DISTINCT, DISTINCT ON | T0 | |
| GROUP BY ALL, ORDER BY ALL | T2 | Friendly syntax |
| GROUPING SETS, ROLLUP, CUBE | T1 | |
| QUALIFY | T2 | Pairs with window functions |
| VALUES, table-less SELECT | T0 | |
| Recursive CTEs | T3 | |
| PIVOT / UNPIVOT | T3 | |
| TABLESAMPLE | T3 | |

### B3. Window functions (core to the real workload)
| Feature | Tier | Notes |
|---|---|---|
| PARTITION BY, ORDER BY | T0 | |
| Ranking: row_number, rank, dense_rank | T0 | |
| Aggregates over windows: sum, count, avg, min, max | T0 | Incremental frame evaluation, not recompute |
| Frames: ROWS | T0 | |
| Frames: RANGE (with offsets) | T0 | |
| Frames: GROUPS, EXCLUDE | T1 | |
| lag, lead, first_value, last_value, nth_value | T0 | IGNORE NULLS option |
| ntile, percent_rank, cume_dist | T1 | |
| Named windows (`WINDOW w AS (...)`) | T1 | |
| FILTER on window aggregates | T1 | |
| Multiple windows sharing sort order | T1 | Reuse one sort across compatible windows |
| Window over data larger than memory (spill) | T0 | Whole-table windows make this unavoidable |
| Parallel partitioned window execution | T0 | Hash-partition by PARTITION BY keys |

### B4. Types
| Feature | Tier | Notes |
|---|---|---|
| Integers, floats, boolean, text | T0 | |
| Decimal (up to 38 digits) | T0 | Finance data needs exact math |
| Date, time, timestamp, timestamptz, interval | T0 | Time zone handling is a correctness risk |
| Binary / bytes, UUID | T1 | |
| Arrays, structs | T1 | Iceberg and Parquet nested types |
| Maps | T1 | |
| JSON (parse, extract, operators) | T1 | |
| Collations | T3 | Start with binary/UTF-8 ordering |

### B5. Functions
| Feature | Tier | Notes |
|---|---|---|
| Arithmetic, comparison, boolean logic | T0 | |
| String functions (concat, substr, like, regex, etc.) | T0 | |
| Date/time functions (trunc, extract, add, diff, format) | T0 | |
| Conditional (CASE, COALESCE, NULLIF, IF) | T0 | |
| Aggregates: count, sum, avg, min, max, count distinct | T0 | |
| Aggregates: stddev, variance, median, percentiles | T1 | |
| Aggregates: approx_distinct (HLL), approx_percentile | T1 | |
| Aggregates: array_agg, string_agg, bool_and/or | T1 | |
| Table functions: unnest, generate_series | T1 | |
| Table functions: read_parquet, read_csv, read_iceberg | T1 | Query files without creating a table |
| User-defined functions (SQL macros) | T3 | |
| User-defined functions (WASM) | T3 | |

### B6. Statements
| Feature | Tier | Notes |
|---|---|---|
| SELECT | T0 | |
| EXPLAIN, EXPLAIN ANALYZE | T0 | |
| SHOW (catalogs, schemas, tables, columns) / DESCRIBE | T0 | |
| SET / SHOW settings (session) | T0 | |
| CREATE / DROP CATALOG | T0 | Catalogs defined by SQL, not property files |
| CREATE TABLE AS SELECT (Iceberg) | T3 | Write path comes after read path |
| INSERT / DELETE / UPDATE / MERGE | NO | Revisit after internal adoption |
| CREATE VIEW | T1 | |
| CREATE MATERIALIZED VIEW | T3 | |
| Transactions | NO | Read snapshots only for now |

---

## C. Planner and optimizer

| Feature | Tier | Notes |
|---|---|---|
| Logical plan representation | T0 | Design carefully: everything builds on it |
| Predicate pushdown | T0 | |
| Projection (column) pushdown / pruning | T0 | Huge win on columnar scans |
| Constant folding, expression simplification | T0 | |
| Filter reordering by selectivity/cost | T1 | |
| Limit / TopN pushdown | T0 | |
| Join type rewrites (outer to inner, semi/anti) | T0 | |
| Statistics: row counts, NDV, min/max, null counts | T0 | From Iceberg manifests and Parquet footers first |
| Cardinality estimation | T0 | Wrong estimates ruin join order |
| Cost model (CPU, memory, I/O, network) | T1 | |
| Join ordering: greedy heuristic | T0 | Enough to handle 10+ table joins sanely |
| Join ordering: dynamic programming / cost-based | T1 | |
| Join algorithm selection (hash, sort-merge, nested loop) | T1 | |
| Subquery decorrelation | T1 | |
| Common subexpression / CTE reuse | T1 | |
| Window: sort sharing, partition-aware planning | T1 | |
| Aggregation: partial/final split, pre-aggregation | T1 | |
| Partition-aware planning (Iceberg partition pruning) | T0 | |
| Dynamic / runtime filters (min-max, bloom, in-list) | T2 | Biggest speedup for star-schema joins |
| Adaptive execution (runtime re-plan of joins, skew) | T2 | |
| Auto-collected statistics (ANALYZE, background) | T2 | |
| Plan cache | T3 | |
| Materialized view rewrite | T3 | |
| EXPLAIN with cost and row estimates | T0 | |
| Optimizer tracing (why this plan) | T1 | Debugging aid |

---

## D. Execution engine

| Feature | Tier | Notes |
|---|---|---|
| Columnar batch format (Arrow layout) | T0 | |
| Selection vectors, null handling | T0 | |
| Vectorized expression evaluation (kernels) | T0 | |
| SIMD-friendly kernels | T1 | Via compiler autovectorization first |
| Operators: scan, filter, project, limit | T0 | |
| Operators: hash aggregate (incl. spill) | T0 | |
| Operators: hash join (incl. spill) | T0 | Inner, outer, semi, anti |
| Operators: sort (external merge sort) | T0 | |
| Operators: TopN | T0 | |
| Operators: window | T0 | See B3 |
| Operators: union, distinct | T0 | |
| Operators: nested loop / cross join | T1 | |
| Operators: sort-merge join | T1 | |
| Operators: unnest, table generators | T1 | |
| Morsel-driven parallelism, work stealing | T0 | |
| Pipeline model (breakers, push vs pull) | T0 | Core design decision, write an ADR |
| Memory manager (per-query budget, reservations) | T0 | |
| Spill to local disk | T0 | |
| Admission control (memory-aware) | T1 | |
| Cancellation and timeouts | T0 | |
| Compressed execution (dictionary, RLE) | T2 | |
| Late materialization | T2 | |
| Radix-partitioned hash join/agg (cache-friendly) | T2 | |
| Expression JIT | T3 | Only if kernels prove insufficient |
| NUMA / thread-per-core tuning | T3 | |
| GPU | NO | |

---

## E. Distributed runtime

| Feature | Tier | Notes |
|---|---|---|
| Exchange abstraction (local first) | T0 | Operators use it from day one |
| Node membership and discovery | T1 | Seed address, gossip or equivalent |
| Any node can coordinate a query | T1 | No dedicated coordinator |
| Plan fragmenting and stage scheduling | T1 | |
| Split assignment | T1 | |
| Split affinity (consistent hashing for cache locality) | T2 | |
| Shuffle: network streaming | T1 | |
| Shuffle: spill to object storage | T2 | Fault tolerance without a separate cluster |
| Broadcast and hash-partitioned exchanges | T1 | |
| Task retry on node failure | T2 | |
| Straggler mitigation (speculative tasks) | T3 | |
| Graceful node drain and join | T1 | |
| Autoscaling hooks (metrics for K8s/KEDA, scale-from-zero) | T2 | |
| Workload management: queues, priorities, per-user limits | T1 | |
| Resource isolation between queries | T1 | |
| Multi-tenancy | T3 | |
| Cluster state storage (what is stateless, what is not) | T1 | Decision needed, see open questions |

---

## F. Storage and connectors

### F1. Object storage and files
| Feature | Tier | Notes |
|---|---|---|
| Local filesystem | T0 | |
| S3 and S3-compatible (MinIO, R2) | T0 | |
| GCS, Azure Blob | T1 | |
| Parquet read: footer cache, row-group and page pruning | T0 | |
| Parquet read: column projection, predicate evaluation in scan | T0 | |
| Parquet read: bloom filters | T1 | |
| Parquet read: nested types | T1 | |
| Async I/O: prefetch, range coalescing, parallel reads | T0 | Dominates remote-scan performance |
| CSV and NDJSON read | T1 | |
| Parquet write / COPY TO | T1 | |
| ORC, Avro | T3 | |

### F2. Iceberg
| Feature | Tier | Notes |
|---|---|---|
| REST catalog | T0 | The standard way forward |
| Read table metadata, snapshots, manifests | T0 | |
| Partition pruning (hidden partitioning, transforms) | T0 | |
| File pruning with manifest stats | T0 | |
| Schema evolution on read | T0 | Column renames and adds must not break reads |
| Partition evolution on read | T1 | |
| Time travel (`FOR VERSION/TIMESTAMP AS OF`) | T1 | |
| Delete files: position deletes | T1 | Needed for correctness on tables with updates |
| Delete files: equality deletes | T1 | |
| Metadata tables (snapshots, files, manifests, history) | T1 | |
| Glue, Hive Metastore, JDBC, Nessie catalogs | T1/T3 | Glue T1, others T3 |
| Vended credentials | T1 | |
| Iceberg write (INSERT, MERGE, compaction) | NO | After read path is proven |
| Delta Lake read | T3 | |

### F3. Federation
| Feature | Tier | Notes |
|---|---|---|
| Connector interface (capabilities, splits, pushdown) | T1 | Design once, early |
| Postgres connector | T1 | |
| MySQL connector | T1 | |
| Pushdown: filter, projection, limit | T1 | |
| Pushdown: aggregate, join | T2 | |
| Parallel reads via partitioned splits | T2 | |
| Cache of federated results | T3 | |
| Other sources (SQLite, ClickHouse, Kafka, HTTP) | T3 | Pulled by user demand |

---

## G. Caching

| Feature | Tier | Notes |
|---|---|---|
| Metadata cache (Parquet footers, Iceberg manifests) | T0 | Cheap and big win |
| Local data cache (NVMe, column chunk granularity) | T2 | Headline performance feature |
| Memory cache tier | T2 | |
| Eviction policy and size limits | T2 | |
| Cache invalidation via immutable file identity | T2 | Iceberg files never change in place |
| Cache warming | T3 | |
| Result cache (same query, same snapshot) | T3 | |
| Cache stats in EXPLAIN ANALYZE and system tables | T2 | |

---

## H. Catalog and configuration

| Feature | Tier | Notes |
|---|---|---|
| In-memory catalogs defined at startup | T0 | |
| `CREATE CATALOG ... USING ...` | T0 | |
| Persisted catalog definitions | T1 | Where does this live in a stateless cluster? |
| `information_schema` | T0 | BI tools expect it |
| `pg_catalog` compatibility subset | T0 | Needed by many pg clients |
| System tables (queries, nodes, settings, cache) | T1 | |
| Secrets handling (env, file, secret store) | T1 | |
| Config: zero-config defaults, env vars, optional file | T0 | |

---

## I. Security

| Feature | Tier | Notes |
|---|---|---|
| Password auth | T1 | |
| TLS everywhere (client and node-to-node) | T1 | |
| OIDC / SSO | T2 | Candidate paid feature |
| mTLS between nodes | T1 | |
| RBAC (catalog/schema/table privileges) | T1 | |
| Row and column-level security | T2 | Candidate paid feature |
| Audit log | T2 | Candidate paid feature |
| Query-level resource limits per user | T1 | |
| Credential handling for object storage and connectors | T1 | |

---

## J. Observability

| Feature | Tier | Notes |
|---|---|---|
| Structured logs | T0 | |
| `EXPLAIN ANALYZE` (per-operator rows, time, memory, I/O) | T0 | |
| Metrics (Prometheus) | T1 | |
| Tracing (OpenTelemetry) | T1 | |
| Query history and profiles (system tables) | T1 | |
| Slow query log | T1 | |
| Cost/estimate-vs-actual report | T2 | Shows where the optimizer was wrong |
| Live progress for running queries | T2 | |
| Web UI | NO | |

---

## K. Operability and packaging

| Feature | Tier | Notes |
|---|---|---|
| Single static binary | T0 | |
| `nata run` zero-config start | T0 | |
| `nata join <addr>` | T1 | |
| Auto-detect CPU/memory (cgroup aware) | T0 | |
| Health and readiness endpoints | T1 | |
| Graceful shutdown | T0 | |
| Docker image | T1 | |
| Helm chart / Kubernetes operator | T2 | |
| Rolling upgrade, version skew policy | T1 | |
| Homebrew / install script | T1 | |
| Config diagnostics (`nata doctor`) | T2 | |

---

## L. Quality and developer tooling

| Feature | Tier | Notes |
|---|---|---|
| `nata-check`: differential test vs DuckDB | T0 | Correctness oracle |
| SQL logic tests (sqllogictest-style) | T0 | |
| Unit tests per kernel and operator | T0 | |
| `nata-bench` with TPC-H, TPC-DS, ClickBench data generators | T0 | |
| Benchmark result history and regression alerts in CI | T1 | |
| SQL fuzzing (random query generation, compare with oracle) | T1 | Finds the bugs tests miss |
| Property tests (operators vs naive reference) | T1 | |
| Spill/OOM fault injection | T1 | |
| Concurrency tests (loom or similar) | T2 | |
| Chaos tests (kill nodes mid-query) | T2 | |
| Supported-SQL page generated from tests | T1 | Keeps the contract honest |
| ADRs (`docs/decisions/`) | T0 | |
| Docs site | T1 | |

---

## M. Ecosystem

| Feature | Tier | Notes |
|---|---|---|
| Works with `psql` and a JDBC driver | T0 | |
| Works with Metabase / Superset / Grafana through pg | T1 | |
| dbt adapter | T2 | Data teams live in dbt |
| Python client (ADBC / Flight SQL) | T2 | |
| Trino-to-Nata SQL transpiler | T3 | Migration aid |
| Terraform provider | T3 | |
| Jupyter integration | T3 | |

---

## Suggested slice order for T0

Each is a complete slice by the definition of done.

1. Workspace, CI, `nata-check` and `nata-bench` skeletons, a tiny TPC-H dataset.
2. Batch format and expression kernels (arithmetic, comparison, boolean, cast with strict errors).
3. Parquet scan with projection pushdown and row-group pruning, local files.
4. Parser and binder for `SELECT ... WHERE ... LIMIT` over a Parquet file; CLI prints results; `EXPLAIN`.
5. Hash aggregate with spill; `GROUP BY`, `HAVING`, common aggregates.
6. Sort and TopN with spill.
7. Hash join (inner, left, semi, anti) with spill; greedy join ordering.
8. Window functions: ranking, then frames, then lag/lead, with partitioned parallelism and spill.
9. CTEs, subqueries (uncorrelated), set operations.
10. Postgres wire protocol (simple, then extended); `information_schema`.
11. S3 and the async I/O layer; metadata cache.
12. Iceberg REST catalog read path with partition and file pruning.
13. TPC-H full and a TPC-DS subset passing against DuckDB, with benchmark results recorded.

After 13, the internal data team can try real queries. T1 and T2 follow in the order their real workload demands.

## Open design questions (each becomes an ADR)

1. **Pipeline model:** push-based pipelines (morsel-driven) versus pull-based iterators. Affects every operator.
2. **Cluster state:** what lives where in a stateless cluster (catalog definitions, query state, membership)? Options: object storage plus gossip, embedded Raft, external store.
3. **Async runtime:** `tokio` for I/O with a dedicated CPU thread pool for operators, versus everything on `tokio`.
4. **Shuffle design:** direct streaming with optional object-storage spill; format and compression.
5. **Strictness spec:** exact coercion, overflow, NULL and ordering rules for the dialect.
6. **Statistics source and freshness:** manifests and footers only, or background ANALYZE too?
7. **Decorrelation strategy:** how general, and when to fall back to "not supported yet".
