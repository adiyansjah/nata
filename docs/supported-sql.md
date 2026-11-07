# Supported SQL

This page is the contract: if it is not listed here, it is not supported, and Nata must answer `not supported yet: <what>`. The rules behind it are
`docs/dialect.md` (names, literals, operators, NULL, CASE and CAST, LIMIT, errors, the differences from DuckDB) and `docs/decimal-rules.md` (numbers).
Slice 4 (card `docs/slices/0002-select-where-limit.md`) built this: `crates/nata-sql` (parse, bind, plan) and `crates/nata-exec` (execution).

```
nata sql -c "<query>"        the query is the argument ...
nata sql < query.sql         ... or stdin (at most 1 MiB)
  --csv                      CSV (header row, NULL as NULL; the text nata-check compares) instead of an aligned table
  --threads N                worker threads (default: the available parallelism)
  --window N, --unordered    developer options: row groups in flight; rows in the order row groups finish
  --report                   developer option: after a successful run, the scan's one-line report and the peak resident memory on stderr
```

`EXPLAIN` and `EXPLAIN ANALYZE` print their plan as text, with or `--csv` or without it.

Exit codes: `0` success (also when the reader of stdout went away: `nata sql ... | head` is quiet); `1` a data or I/O error (a missing, truncated or damaged
file, a division by zero or an overflow in the data); `2` `not supported yet`, a syntax or bind error, a wrong use of the command; `130` interrupted
(Ctrl-C: the query stops, every thread is joined, every handle and buffer is released; a Ctrl-C in the first milliseconds, before the handler is installed, or while the query is read from stdin, ends the process by the signal itself, which a shell also reports as 130). If rows were printed before an error, the exit code is not 0 and
the message says that the output is INCOMPLETE. Errors name the class, the message, the line and column, and put a caret under the position; they quote the
query and never a value from the data. A failure that comes from a file names the file and the row group.

## What works

Every example runs: `check/queries/sql/` runs 251 of them against DuckDB (`nata-check`), `check/queries/sql-differences/` the 37 deliberate differences.

| construct | example |
|---|---|
| `SELECT <list> FROM '<file or glob>'`; a trailing `;` | `SELECT l_orderkey, l_quantity FROM 'lineitem.parquet';` |
| a glob: the files in byte order of their names, one schema (the first file's footer binds the query; a later file whose schema differs is an error naming it, found when the scan reaches it) | `SELECT id FROM 'data/parts/*.parquet'` |
| an object in S3, MinIO or GCS, or a glob of them (`s3://bucket/key`, `gs://bucket/key`; walking skeleton of slice 11): the same rows as the identical local file. Credentials only from the environment (`AWS_*`, `AWS_PROFILE` with static keys in the shared credentials file, the instance metadata; `GOOGLE_*`, the metadata server), never from SQL or the command line; TLS for real stores, plain http only with `AWS_ALLOW_HTTP=true` for a local endpoint. At most 100,000 objects per glob; a registered `nata serve` table may name one object | `SELECT l_orderkey FROM 's3://bucket/lineitem/*.parquet' WHERE l_orderkey > 0 LIMIT 10` |
| `*` | `SELECT * FROM 'nation.parquet'` |
| unquoted names fold to lower case, `"quoted"` names are exact | `SELECT L_ORDERKEY, "l_quantity" FROM 'lineitem.parquet'` |
| `AS` alias (required for an alias) | `SELECT l_extendedprice * (1 - l_discount) AS net FROM 'lineitem.parquet'` |
| `WHERE <boolean>` | `... WHERE l_quantity > 49 AND l_shipmode = 'AIR'` |
| `LIMIT n`, `LIMIT n OFFSET m` (rows in file order) | `... LIMIT 10 OFFSET 20` |
| integer literals (INTEGER, BIGINT, then decimals up to 38 digits) | `SELECT 42, 3000000000, 99999999999999999999 FROM 't.parquet'` |
| exact decimal literals | `0.1`, `1.50`, `1e2` (exact, not a float) |
| string, date, boolean, NULL literals | `'it''s'`, `DATE '1994-01-01'`, `TRUE`, `NULL` (typed by its context) |
| `+ - * % //` on integers (checked against the width of the type) | `SELECT a * 2 + 1, a % 3, a // 2 FROM 't.parquet'` |
| `+ - * / %` on decimals and integers (exact, `decimal-rules.md`) | `SELECT price * (1 - discount) * (1 + tax) FROM 't.parquet'` |
| unary minus | `SELECT -a, -price FROM 't.parquet'` |
| `= <> != < <= > >=` on numbers, text (bytewise), dates, booleans, floats (NaN equals NaN and is the largest, `-0.0 = 0.0`) | `... WHERE d >= CAST('Infinity' AS DOUBLE)` |
| `AND`, `OR`, `NOT` (three-valued) | `... WHERE NOT (a > 0 AND f) OR s IS NULL` |
| `IS NULL`, `IS NOT NULL` | `... WHERE s IS NOT NULL` |
| `[NOT] BETWEEN x AND y` | `... WHERE dt BETWEEN DATE '1994-01-01' AND DATE '1994-12-31'` |
| `[NOT] IN (literal, ...)` | `... WHERE s IN ('a', 'b', NULL)` |
| `CASE WHEN ... THEN ... [ELSE ...] END` and `CASE x WHEN v THEN ... END` | `SELECT CASE WHEN i < 0 THEN 'neg' WHEN i = 0 THEN 'zero' ELSE 'pos' END AS sign FROM 't.parquet'` |
| `CAST(x AS BOOLEAN / INTEGER / BIGINT / DECIMAL(p,s) / DOUBLE / VARCHAR / DATE)` and `x::type` | `SELECT CAST(price AS DECIMAL(10,1)), id::VARCHAR, CAST('2024-02-29' AS DATE) FROM 't.parquet'` |
| `EXPLAIN <select>`: the plan (scan with the pushed conjuncts and the columns read, filter, projection, limit) | `EXPLAIN SELECT id FROM 't.parquet' WHERE i > 1 AND i + 1 < 10 LIMIT 3` |
| `EXPLAIN ANALYZE <select>`: runs the query (the rows are not printed) and adds the rows and the time of every operator and the scan's report | `EXPLAIN ANALYZE SELECT ... FROM 'lineitem.parquet' WHERE ...` |

Column types read from Parquet: `BOOLEAN`, `TINYINT`, `SMALLINT`, `INTEGER`, `BIGINT`, `REAL` (FLOAT), `DOUBLE`, `DECIMAL(p,s)` with p up to 38, `VARCHAR`,
`DATE`. The result of the plan of `EXPLAIN` for the example above:

```
Limit 3 offset 0
  Project id: INTEGER
    Filter ((i + 1) < 10)
      Scan 't.parquet' (1 file)
        columns: id, i
        pushed: i > 1
```

`column <op> literal` conjuncts, `IS [NOT] NULL` and `column BETWEEN literal AND literal` are pushed into the scan (they prune row groups by their statistics and
are enforced by the scan's exact row filter); everything else is evaluated by the executor on Arrow batches with checked integer and exact decimal arithmetic.
What is evaluated for which rows, and so which errors appear, is `docs/dialect.md` section 9b: an error is raised only for a row that reaches the operation
with valid operands (`WHERE b <> 0 AND a // b > 1` never divides by zero; a `CASE` evaluates only the branch a row takes).

Results stream: the rows are printed as the row groups come, in the order of the files, row groups and rows (also with many threads); the aligned table
takes its column widths from the header and the first 1,000 rows (a wider cell later is printed in full). Memory is the scan's: the window of row groups in flight
times the largest row group, plus the evaluator's temporaries for one batch of 8,192 rows.

## Differences from DuckDB

`docs/dialect.md` section 9 (and `docs/decimal-rules.md` section 9) lists every one with its example; the differential queries for them are in
`check/queries/sql-differences/` and `check/queries/decimal-differences/`. The ones a user meets first: `/` is an exact decimal (DuckDB: a DOUBLE), quoted
names are case-sensitive, `1 = '1'` is a bind error, a string is cast only from its plain form, a `WHERE` needs a boolean, a decimal literal below one is a
`DECIMAL(p,p)` printed without the leading zero, and an operation fails only for the rows that reach it.

## Not supported yet (each answers `not supported yet: <what>` with a position, and each is a test)

| construct | example | when |
|---|---|---|
| `GROUP BY`, `HAVING`, aggregates (`COUNT`, `SUM`, ...) | `SELECT count(*) FROM 't.parquet'` | slice 5 |
| `ORDER BY` | `... ORDER BY id` | slice 6 |
| joins, subqueries, `WITH`, `DISTINCT`, set operations, more than one source, a table name, `SELECT` without `FROM` | `SELECT 1`, `SELECT * FROM a JOIN b ON ...` | 7 and later |
| functions (`ABS`, `COALESCE`, `NULLIF`, `TRY`, `LIKE`, `SUBSTR`, date functions, ...) | `SELECT abs(a) FROM 't.parquet'` | the function slices |
| float arithmetic (`d + d`, `-d`), a float compared with an integer, `CAST(d AS VARCHAR)`, `CAST(bool AS INTEGER)` | `SELECT d + d FROM 't.parquet'` | not specified yet |
| `date + integer`, intervals | `SELECT dt + 1 FROM 't.parquet'` | the date-function slice |
| an expression in an `IN` list, `TRY_CAST`, `CAST(x AS TINYINT / SMALLINT / REAL)` | `... WHERE a IN (id, 2)` | not specified yet |
| non-literal `LIMIT`/`OFFSET`, `OFFSET` without `LIMIT`, `LIMIT ALL` | `LIMIT 1 + 1` | not specified yet |
| `TIMESTAMP`, `BLOB`, unsigned integer and nested columns | a column of such a type, when the query reads it | later slices |
| `https://` paths, Azure and R2 locations, a remote table that is a glob in `nata serve`'s registry, an Iceberg table | `FROM 'https://host/x.parquet'` | slice 12 and later |

An unknown column is a bind error with a suggestion, not a `not supported yet`.

## Developer command (not SQL)

`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv` reads Parquet files (slice 3, closed 2026-11-06: parallel, ordered by default, `--threads`, `--unordered`, `--count`) and prints CSV: projection pushdown, row-group
pruning from the footer statistics and an exact row filter. Example, the year range of the slice card:
`nata scan lineitem.parquet --columns l_orderkey,l_shipdate --filter 'l_shipdate >= 1994-01-01' --filter 'l_shipdate < 1995-01-01' --csv`. Types, literals and limits: `crates/nata-parquet/README.md`.

## Registered sources and metadata over Postgres wire (slice 10)

With `nata serve --catalog registry.tsv`, `FROM example`, `FROM public.example`
and `FROM nata.public.example` resolve the same explicitly registered local
Parquet source. Unquoted components fold to lower case; quoted components are
exact. The server pins one read-only catalog snapshot per connection. The
standalone `nata sql` provider remains file-only and refuses registered names.
See [serve.md](serve.md#local-registry-and-metadata-browsing-slice-10)
for the tab-separated registry and the precise information_schema projections.

```sql
SELECT * FROM nata.public.example LIMIT 3;
SELECT table_name FROM information_schema.tables ORDER BY table_name;
SELECT column_name, data_type FROM information_schema.columns;
SELECT * FROM information_schema.columns WHERE table_schema = 'public';
```

Metadata uses a separate bounded snapshot adapter. It also recognizes the
frozen default-client browsing shapes, including filtered information_schema
stars and the captured joins/helpers; see serve.md and the slice result record
for the exact corpus. These shapes do not extend ordinary data SQL. Catalog SQL
outside that surface returns 0A000. Typed empty results describe absent Nata
capabilities only for the recognized queries. SQL DECLARE/FETCH/CLOSE remain
explicit unsupported errors.

The wire session also exposes snapshot-backed `SHOW CATALOGS`,
`SHOW SCHEMAS [FROM catalog]`, `SHOW TABLES [FROM [catalog.]schema]`,
`SHOW COLUMNS FROM table` and `DESCRIBE table`. For example,
`SHOW TABLES FROM public` and `DESCRIBE public.example` agree with the
registered catalog views. These commands are served by the bounded wire
metadata adapter; settings helpers and their exact restrictions are documented
in [serve.md](serve.md#session-compatibility-statements).
