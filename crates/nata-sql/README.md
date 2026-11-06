# nata-sql

The SQL front end of slice 4 (card `docs/slices/0002-select-where-limit.md`): text to a typed logical plan. It does not execute anything; step 3 (`nata sql`, the evaluator, `EXPLAIN ANALYZE`) calls it.

```rust
let planned = nata_sql::plan_sql("SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' LIMIT 10", &nata_sql::FsSource)?;
println!("{}", planned.plan.explain_text());
let scan_options = planned.plan.scan.scan_options(); // columns and pushed conjuncts, for nata_parquet::ScanPlan::open
```

| module | job |
|---|---|
| `parse` | token guard (1 MB, nesting 1,000, chains of 10,000 terms), the 16 MB parse thread, `sqlparser` (PostgreSQL dialect) converted to `ast`, `not supported yet: <what>` for the rest |
| `bind` | names (folded or exact), `*`, aliases, exact types, the decimal rules, NULL typed by context, "did you mean" |
| `types`, `texpr` | the typing rules of `docs/dialect.md` and `docs/decimal-rules.md`; the typed expression tree (contract in its module doc) |
| `plan` | `Scan -> Filter -> Project -> Limit`, rule R2 pushdown, the plan text |
| `source`, `glob` | the files of a path or glob and their schema, from the FIRST footer only (`nata_parquet::read_leaves`; register item P10: the scan checks every other file's schema when it reaches it, `ScanOptions::strict_schema`; `BindOptions::all_footers` is the older binder that reads them all); `MemSource` for tests |

Rules: every error has a class (`syntax`, `bind`, `not supported yet`, `execution`) and a 1-based line and column in characters; messages quote the query and never data. A deep tree (a chain of 10,000 terms) is dropped, visited and printed with loops; the evaluator of step 3 must also run on a large stack or iterate.

Checks (`bench/results/20261106-sql-frontend.md`): `cargo test -p nata-sql` (rules of the dialect page, golden plans, pushdown property test against the scan's own row filter, parse fuzz, limits), and `python3 crates/nata-sql/mutate.py` (mutants of the binder, the typing and the pushdown; each must be CAUGHT).
