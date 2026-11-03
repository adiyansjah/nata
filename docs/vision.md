# Nata: vision

> The lakehouse query engine you can run in 60 seconds and scale to a cluster in 60 more.

Nata is an open-source, distributed SQL engine written in Rust. It queries Apache Iceberg tables (and Parquet on object storage) with a clean, strict SQL dialect, and federates to operational databases. It is aimed at being easier to run than Trino and faster than both Trino and DuckDB on repeat lakehouse workloads.

## Decisions so far

| Topic | Decision |
|---|---|
| Goal | Real product: open source plus paid support |
| First workloads | Lakehouse analytics, then federated ad-hoc queries |
| Scale target | Large elastic cluster (single-node first, same design) |
| Language | Rust |
| SQL | New, strict, readable dialect on a Postgres-style base. No Trino compatibility. |
| Client protocols | Postgres wire protocol, Arrow Flight SQL |
| Table format | Iceberg first, Delta later |
| Build vs reuse | Reuse commodity crates; write original code where it drives performance or differentiation, decided by a benchmark bake-off |
| First users | Our own internal data team (dogfood) |
| License | Apache-2.0 |
| Real workload | SQL-only users; window functions over whole tables or partitions; many-table joins |
| Primary benchmark | TPC-DS (window- and join-heavy), then ClickBench |
| Product name | Nata |

## Pain points we solve

| Pain (Trino and others) | Nata answer |
|---|---|
| Heavy setup: coordinator, workers, JVM, per-catalog files | One static binary. `nata run` starts a node, `nata join <addr>` adds one. Catalogs are SQL: `CREATE CATALOG`. |
| Memory tuning | Engine measures memory, admits queries, spills by default. No knobs required. |
| Slow repeat reads from S3 | Local NVMe cache of column chunks, with split affinity so caches stay warm. |
| Slow small queries | Single-node fast path; no distributed shuffle when one node is enough. |
| Fragile long queries | Stateless workers, retry per task, shuffle spills to object storage only when needed. |
| Awkward clients | Postgres wire and Arrow Flight SQL. |

## Architecture sketch

- **Stateless nodes.** All table state lives in the Iceberg catalog (REST, Glue, etc.). Any node can coordinate a query.
- **Execution.** Vectorized, columnar (Arrow layout), morsel-driven parallelism with work stealing.
- **Pruning.** Iceberg manifests, then Parquet row-group and page stats, then runtime filters (min-max, bloom) pushed into scans.
- **Adaptive.** Broadcast vs shuffle joins and skew handling decided at runtime from observed row counts.
- **Exchange abstraction from day one.** Even on one node, operators talk through the same exchange interface the distributed scheduler will use.
- **Federation.** Connectors declare capabilities; the planner pushes filters, projections, aggregates and joins down.

## The SQL dialect: what "clearer" means

- No silent coercion; overflow is an error.
- Explicit NULL and ordering semantics.
- Friendly syntax: `GROUP BY ALL`, pipe syntax (`FROM t |> WHERE ... |> AGGREGATE ...`), trailing commas.
- Error messages that point at the problem and suggest a fix.
- Learnable in an afternoon by anyone who knows Postgres.

## Build vs reuse

Rule: reuse a commodity component unless a benchmark shows it is the bottleneck.

| Component | Start with | Own it when |
|---|---|---|
| Object storage I/O | `object_store` | I/O scheduling shows up in profiles |
| In-memory format | Arrow arrays | Never; it is the interop format |
| Parquet decode | `parquet` crate | Page-level pruning or late materialization needs it |
| SQL parser | `sqlparser` (Postgres dialect) | Pipe syntax and extensions outgrow it |
| Optimizer, operators, scheduler, shuffle, cache, memory manager | Original | Always: this is the product |

## Competitors

DuckDB/MotherDuck, ClickHouse, Athena, Starburst/Trino, StarRocks. Our edge is ease plus elasticity plus lakehouse cache speed, not raw speed alone.

## Success metrics

1. **Time to first query** under 60 seconds from download.
2. **Beat DuckDB** on single-node ClickBench and TPC-H (repeat runs, warm cache, Iceberg on S3).
3. **Beat Trino** on TPC-DS at SF1000 on an equal-cost cluster.
4. **Our internal data team** moves real dashboards onto Nata.

## Roadmap

| Phase | Months | Deliverable | Gate |
|---|---|---|---|
| 1 | 0-3 | Single-node core: Parquet scan, filter, project, hash agg, hash join, sort with spill, window functions, join ordering, benchmark harness | Within reach of DuckDB on TPC-H SF10 and a TPC-DS subset |
| 2 | 3-6 | Iceberg reads, SQL dialect, Postgres wire, cache | Internal data team runs real queries |
| 3 | 6-10 | Distributed execution, scheduler, runtime filters | Beat Trino on TPC-DS SF1000 |
| 4 | 10+ | Federated connectors, adaptive execution, Kubernetes operator | Open-source launch |

## Risks

- **Correctness.** Wrong results lose trust faster than slow results. Build a differential test suite against DuckDB from phase 1.
- **Scope.** Distributed execution and a custom optimizer are where engine projects stall. Keep phase gates honest.
- **Moat.** DataFusion-based engines and Presto C++ are closing in. Differentiate on ease, elasticity and cache.
- **License.** Apache-2.0 (decided). Revenue must come from support and paid operational features, not from license restrictions.

## Open questions

1. Which paid features sit above the open core (SSO, audit, governance, managed control plane).
3. Interview 3-5 teams with Trino or Athena pain before phase 3.
4. Dialect name and spec: write a one-page strictness spec before the parser work starts.
