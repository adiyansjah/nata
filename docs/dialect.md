# The Nata SQL dialect (the subset of slice 4)

Status: written 2026-11-06 before the parser work (feature map B1, slice 4 card 0002); decided in ADR 0007. Grows with each slice.
Rule for everything here: **strict**. If a query's meaning is not stated on this page, Nata answers `not supported yet: <what>` at the
position of the construct; it never guesses, never converts silently, never returns a wrong result. Numbers follow `docs/decimal-rules.md`,
which this page does not repeat. "DuckDB" below means DuckDB 2.0.0, checked by hand on 2026-11-06; every difference is listed in section 9.

## 1. Statements

`SELECT <select list> FROM <source> [WHERE <expr>] [LIMIT <n> [OFFSET <m>]]`, `EXPLAIN <select>`, `EXPLAIN ANALYZE <select>`. One
statement per call; a trailing `;` is allowed. `<source>` is a single-quoted path or glob of Parquet files: `FROM 'lineitem.parquet'`,
`FROM 'data/*.parquet'` (all files must have the same schema). Not yet: `GROUP BY`, `HAVING`, `ORDER BY`, joins, subqueries, `WITH`,
`DISTINCT`, set operations, more than one source, a table name.

## 2. Names

- An unquoted identifier is folded to lower case: `SELECT L_OrderKey` reads the column `l_orderkey`.
- A double-quoted identifier is exact and case-sensitive: `"L_OrderKey"` reads only a column spelled that way. **DuckDB: quoted names
  are case-insensitive** (section 9). A `"` inside a quoted name is written `""`.
- `SELECT *` expands to every column of the source in file order. An alias (`expr AS name`, `AS` required) names an output column;
  aliases cannot be used inside the same `SELECT` or `WHERE` (as in Postgres and DuckDB's standard mode).
- Two output columns with the same name are allowed in the output; referring to an ambiguous name is an error naming both.
- Keywords: the words of this page are reserved and need quotes to be used as names.

## 3. Literals

| literal | type | example |
|---|---|---|
| integer | the smallest of `INTEGER`, `BIGINT` that holds it; larger is a decimal up to 38 digits, then an error | `42`, `3000000000` (BIGINT) |
| decimal | exact `DECIMAL(p,s)` from its digits, never through a float (`decimal-rules.md` section 2) | `0.1` is `DECIMAL(1,1)`, `1e2` is exact |
| float | only through `CAST(... AS DOUBLE)` or a column; there is no float literal syntax | `CAST('1.5' AS DOUBLE)` |
| string | single quotes, `''` for a quote, no backslash escapes, UTF-8 | `'it''s'` |
| date | `DATE 'yyyy-mm-dd'`; an impossible date is an error | `DATE '1994-01-01'`; `DATE '2024-02-30'` is an error |
| boolean | `TRUE`, `FALSE` | |
| null | `NULL`, which takes the type its context needs; `SELECT NULL` alone has no type and is `not supported yet` | `x IS NULL` |

## 4. Operators and precedence

From tightest to loosest: unary `-`; `*` `/` `%` `//`; binary `+` `-`; comparisons `=` `<>` `!=` `<` `<=` `>` `>=`, `BETWEEN`, `IN`,
`IS [NOT] NULL`; `NOT`; `AND`; `OR`. Parentheses override. Arithmetic is `decimal-rules.md` sections 3 and 4 (integer overflow, division
by zero and out-of-range results are errors, for selected rows only: `WHERE b <> 0 AND a / b > 1` never divides by zero).

## 5. Comparisons, NULL and three-valued logic

- Both sides of a comparison must have the same type, or types that `decimal-rules.md` section 4 allows to mix (integers and decimals).
  **No implicit conversion between strings and numbers or dates**: `1 = '1'` is a bind error naming the two types (DuckDB: `true`).
- Strings compare bytewise in UTF-8 (no collation yet). Dates compare as dates.
- NULL: any comparison with NULL is NULL; `NOT NULL` is NULL; `NULL AND FALSE` is `FALSE`, `NULL OR TRUE` is `TRUE` (SQL three-valued
  logic, as DuckDB). `WHERE` keeps a row only when the condition is `TRUE`.
- `x BETWEEN a AND b` is `x >= a AND x <= b` (so `1 BETWEEN 3 AND 1` is `FALSE`; no `SYMMETRIC`).
- `x IN (v1, v2, ...)` is `x = v1 OR x = v2 OR ...`: `2 IN (1, NULL)` is NULL. The list holds literals of a type comparable to `x`.
- Floating point (`DOUBLE`, `REAL` columns): `NaN = NaN` is `TRUE`, `NaN` is greater than every other value, `-0.0 = 0.0` is `TRUE`, as in
  DuckDB (and as the scan's pruning of slice 3 assumes). Comparing a float with a decimal is a bind error (`decimal-rules.md`).

## 6. CASE and CAST

- `CASE WHEN c THEN r ... [ELSE e] END` and `CASE x WHEN v THEN r ... END`. All results must have one type, or integers and decimals that
  `decimal-rules.md` widens without exceeding 38 digits; otherwise a bind error. A missing `ELSE` is `ELSE NULL`. Only the taken branch
  is evaluated for a row (errors in other branches are not raised).
- `CAST(x AS type)` for `BOOLEAN`, `INTEGER`, `BIGINT`, `DECIMAL(p,s)`, `DOUBLE`, `VARCHAR`, `DATE`. A value that does not convert is an
  error naming the value's position in the query, not the value (no cell values in errors); rounding is half away from zero
  (`decimal-rules.md` section 6). `x::type` is the same as `CAST`. `TRY_CAST` is not supported yet.

## 7. LIMIT and OFFSET

`LIMIT n` and `OFFSET m` take non-negative integer literals (a negative or non-literal value is an error, as in DuckDB). Without
`ORDER BY` (not yet supported) the rows are those of the files in file order, row-group order and row order, so `LIMIT` is deterministic.

## 8. Errors

Every error has a class (`syntax`, `bind`, `not supported yet`, `execution`), a message, and the line and column of the construct it is
about (1-based, in characters of the query text). Messages quote the query, never data values. Where a name is unknown, the message
suggests the closest existing column name ("did you mean `l_orderkey`?") when one is within an edit distance of 2.

Limits: a query text of at most 1 MB, nesting (parentheses, `CASE`, unary operators) at most 1,000 deep, and at most 10,000 terms in one
chain of the same binary operator; beyond them, a positioned error before parsing.

## 9. Where this differs from DuckDB 2.0.0

| case | DuckDB | Nata |
|---|---|---|
| quoted identifiers | case-insensitive: `"A"` finds column `a` | exact and case-sensitive |
| string compared with a number: `1 = '1'` | `true` (implicit cast) | bind error |
| `date + integer`: `DATE '1994-01-01' + 1` | `1994-01-02` | `not supported yet` (intervals come with the date-function slice) |
| float literal syntax (`1.5e0`) | `DOUBLE` | exact decimal (`decimal-rules.md`) |
| division and the other numeric differences | | `decimal-rules.md` section 9 |

The differential check (`nata-check`) holds each row as a Nata-only expectation, as `check/queries/decimal-differences/` does for numbers.
