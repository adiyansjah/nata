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

## 8b. Readings decided while building the front end (2026-11-06)

The front end of slice 4 found ten places where this page was silent; the strict reading is the rule (decided by the assistant under the
owner's delegation, `bench/results/20261106-sql-frontend.md`):
1. **Unaliased output names** are PostgreSQL's: a column keeps its name (also under a `CAST`), a `CASE` is `case`, any other expression is
   `?column?`. DuckDB names an expression by its text: the differential check compares rows, and test queries alias their expressions.
2. **`DECIMAL` without arguments** is `DECIMAL(18,3)` and `DECIMAL(p)` is `DECIMAL(p,0)` (DuckDB's defaults).
3. **Float arithmetic** (`d + d`, `-d`, `d + 1`) and **comparing a float with an integer** are `not supported yet` in this slice; a float
   with a decimal is a bind error (`decimal-rules.md` section 4); `REAL` and `DOUBLE` are two types, and comparing them is a bind error.
4. **CAST** converts between the numeric types, from and to `VARCHAR` for numbers, booleans and dates, and from `VARCHAR` to `DOUBLE`;
   boolean to and from numbers and `DOUBLE` to `VARCHAR` are `not supported yet`; a meaningless cast (date to integer) is a bind error.
   `TINYINT` and `SMALLINT` are column types but not `CAST` targets yet.
5. **`IN` lists hold literals only** (an expression in the list is `not supported yet`). `x = NULL` is not folded: it is NULL for every
   row, so `WHERE x = NULL` keeps nothing.
6. **The size limits** (1 MB, nesting 1,000, chains of 10,000) are `syntax` errors starting "query too large or too deeply nested:". A chain
   counts each operator kind separately per nesting level; `::` and `IS` count as operators.
7. **An ambiguous name** cannot arise yet (no name can refer to an output column; a source with two columns of one name is refused when its
   footer is read). Two output columns with one name are allowed.
8. **Reserved words** are DuckDB's 112 (a superset of the words of this page): a column named `end` or `order` needs quotes.
9. **`LIMIT` and `OFFSET`:** a negative, fractional or too large number is a `bind` error; a non-literal, `OFFSET` without `LIMIT`, and
   `OFFSET m ROWS` are `not supported yet`; `LIMIT 5, 10` is a syntax error.
10. **A negative literal is typed by its value:** `-2147483648` is an `INTEGER`, `-9223372036854775808` a `BIGINT`.

## 9. Where this differs from DuckDB 2.0.0

| case | DuckDB | Nata |
|---|---|---|
| quoted identifiers | case-insensitive: `"A"` finds column `a` | exact and case-sensitive |
| string compared with a number: `1 = '1'` | `true` (implicit cast) | bind error |
| `date + integer`: `DATE '1994-01-01' + 1` | `1994-01-02` | `not supported yet` (intervals come with the date-function slice) |
| float literal syntax (`1.5e0`) | `DOUBLE` | exact decimal (`decimal-rules.md`) |
| division and the other numeric differences | | `decimal-rules.md` section 9 |
| `ti + 1` with `ti` a TINYINT (or SMALLINT) column | the literal takes the column's type: TINYINT, overflow at 127 | INTEGER (the literal is typed by its value, section 3; the wider type wins): `128`. Comparisons do narrow a literal to the column |
| a decimal literal below one: `0.1`, `.5`, `0.000` | `DECIMAL(18,1)`, printed `0.1` | `DECIMAL(1,1)` (section 3), printed `.1`, as DuckDB prints every `DECIMAL(p,p)`; `CAST(0.20 AS VARCHAR)` is `.20` |
| `WHERE i` with an integer `i` | the integer is read as a boolean | bind error: a `WHERE` needs a boolean |
| `CAST('1.5' AS INTEGER)`, `'1e2'`, `'0x10'`, `'1_000'` to an integer | converts (rounding, hex, underscores) | error: an integer is an optional sign and digits (spaces around are trimmed) |
| `CAST(s AS BOOLEAN)` with `'t'`, `'yes'`, `'1'`, `'on'` | converts | error: only `true` and `false`, in any case |
| `CAST(s AS DATE)` with `'2024/02/29'`, `'2024-2-9'`, `'2024-02-29 10:00:00'` | converts | error: `YYYY-MM-DD` of a date that exists (spaces around are trimmed) |
| `d IN (CAST(1.5 AS DOUBLE))` | allowed | `not supported yet`: the list holds literals (a `CAST` of a literal is not one) |
| what is evaluated for which rows | see section 9b | see section 9b |

## 9b. What is evaluated for which rows (decided while building step 3, 2026-11-06)

Errors (division by zero, overflow, a value that does not convert) are raised only for a row that reaches the operation with valid (non-NULL) operands. The
executor evaluates:
- **`AND`, `OR`:** the next operand only for the rows the earlier ones left undecided (`AND`: TRUE or NULL so far; `OR`: FALSE or NULL). In a `WHERE`
  only TRUE rows matter, so a top-level `AND` also drops the rows whose earlier operand is NULL. DuckDB evaluates every operand of an `AND`/`OR` in a projection.
- **`CASE`:** a branch only for the rows that take it. **`BETWEEN`:** the upper bound only for the rows the lower bound did not decide as FALSE (in a `WHERE`: only for the rows whose lower bound is TRUE).
- **`LIMIT`:** the select list is evaluated for the first `offset + n` rows the `WHERE` keeps and for no later row (the rows `OFFSET` drops are still
  evaluated), as DuckDB does with its limit pushed below the projection. The `WHERE` is never skipped: it decides which rows count.
- **an operand next to a NULL:** both operands of `+ - * % // =` and the rest are evaluated for the selected rows; DuckDB does not evaluate the other
  operand when one is a constant NULL (`(a * -1) + CAST(NULL AS INTEGER)` is NULL there, an overflow error here for `a = -2147483648`).
- **constant expressions:** `CAST('abc' AS INTEGER)` as a `CASE` condition fails only for a row that reaches that condition; DuckDB evaluates it when it plans the
  query and fails even if every row takes an earlier branch. A select list made only of literals that fails (`4294967296 * -4294967296`, a bad cast) fails in
  DuckDB over its own tables even when no row is selected; over a Parquet file DuckDB agreed with Nata in the cases tried (`WHERE id < 0`, `WHERE 1 = 2`).

These are consequences of one rule (a row that does not reach an operation cannot make it fail), so each is a difference in which errors appear, never in a value.

The differential check (`nata-check`) holds each row as a Nata-only expectation, as `check/queries/decimal-differences/` does for numbers.
