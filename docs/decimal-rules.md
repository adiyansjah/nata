# Decimal rules

Status: decided by the project owner on 2026-11-03; reviewed by an independent agent (findings applied). Not implemented yet: this page is the contract for the slice 2 kernels and for `nata-check`. Anything not listed here is `not supported yet`.

Evidence: `bench/results/20261103-decimal-rules-duckdb.md` (the rules checked against DuckDB 2.0.0) and `bench/results/20261103-kernels-arrow-rs.md` (kernel costs). Storage and kernels: `decisions/0003-batch-format-and-expression-kernels.md`.

## 1. Principles

1. **Exact.** Decimal arithmetic is exact. A result is never silently rounded, truncated, or turned into a floating-point number.
2. **Strict.** Overflow, division by zero and invalid conversions are errors, never wrong results (`vision.md`). An error is raised only for rows that are selected and valid (ADR 0003, decision 4), so `WHERE b <> 0 AND a / b > 1` and `CASE` guards work.
3. **Same as DuckDB unless stated.** `nata-check` compares with DuckDB, so every difference in section 8 is deliberate and has a test.
4. **Errors say what to do.** Every bind-time and run-time error names the problem and suggests a fix (section 7).

## 2. Types and storage

- `DECIMAL(p, s)` with 1 <= p <= 38 and 0 <= s <= p. Negative scale and p > 38 are `not supported yet`.
- Stored as Arrow `Decimal64` when p <= 18, otherwise `Decimal128` (ADR 0003, decision 3). **The result precision of an operation decides the stored type.**
- Integer types take part in decimal arithmetic as exact decimals: `TINYINT` is (3,0), `SMALLINT` (5,0), `INTEGER` (10,0), `BIGINT` (19,0).
- Literals: `1.5` is `DECIMAL(2,1)`; whole numbers are `INTEGER` or `BIGINT` by size; **`1e2` is an exact decimal** (here `DECIMAL(3,0)`; `1e-3` is `DECIMAL(3,3)`); a literal with more than 38 digits is an error. A `DOUBLE` is written `CAST(x AS DOUBLE)` or `x::DOUBLE`.

## 3. Result types

Write the operand types as (p1,s1) and (p2,s2). "Checked" means every result is compared with the result precision and an overflow is an error. "Bind error" means the query is rejected before it runs.

| operation | scale S | precision P |
|---|---|---|
| `a + b`, `a - b` | `max(s1, s2)` | `min(38, max(p1-s1, p2-s2) + S + 1)`; checked. At the cap the scale is kept, never cut |
| `a * b` | `s1 + s2`; bind error if above 38 | `p1 + p2`, but if both p1 and p2 are <= 18 and S < 18 then `min(p1 + p2, 18)`; otherwise `min(p1 + p2, 38)`; checked |
| `a / b` | `min(s1 + 6, 38)` | `(p1 - s1) + s2 + S`; bind error if above 38; rounded half away from zero at scale S; checked |
| `a % b` | `max(s1, s2)` | `max(p1-s1, p2-s2) + S`; bind error if above 38; the sign follows the dividend; checked |
| `a // b` (integers only) | | integer division truncating toward zero; decimals are `not supported yet` |
| `-a`, `ABS(a)` | `s1` | `p1` |
| `FLOOR`, `CEIL`, `TRUNC` | 0 | `p1` |
| `ROUND(a, n)`, n a constant | `n` if `0 <= n < s1`, unchanged if `n >= s1`, 0 if `n < 0` | `p1`, except `min(p1 + 1, 38)` if `n < 0`; half away from zero |

Examples (all derived from the formulas above; the `+`, `-`, `*` and `%` columns agree with DuckDB wherever DuckDB returns a decimal, see the evidence file):

| operands | `+` / `-` | `*` | `/` | `%` |
|---|---|---|---|---|
| `DECIMAL(15,2)` and `DECIMAL(15,2)` | (16,2) | (18,4) | (23,8) | (15,2) |
| `DECIMAL(15,2)` and `DECIMAL(5,3)` | (17,3) | (18,5) | (24,8) | (16,3) |
| `DECIMAL(18,2)` and `DECIMAL(18,2)` | (19,2) | (18,4) | (26,8) | (18,2) |
| `DECIMAL(38,2)` and `DECIMAL(38,2)` | (38,2) | (38,4) | bind error | (38,2) |
| `DECIMAL(10,2)` and `DECIMAL(10,2)` | (11,2) | (18,4) | (18,8) | (10,2) |
| `DECIMAL(10,9)` and `DECIMAL(10,9)` | (11,9) | (20,18) | (25,15) | (10,9) |
| `DECIMAL(15,2)` and `DECIMAL(20,2)` | (21,2) | (35,4) | (23,8) | (20,2) |
| `DECIMAL(20,20)` and `DECIMAL(20,20)` | (21,20) | bind error | bind error | (20,20) |
| `DECIMAL(10,0)` and `DECIMAL(10,0)` | (11,0) | (18,0) | (16,6) | (10,0) |
| `DECIMAL(19,0)` and `DECIMAL(19,0)` | (20,0) | (38,0) | (25,6) | (19,0) |
| `DECIMAL(38,0)` and `DECIMAL(38,0)` | (38,0) | (38,0) | bind error | (38,0) |
| `DECIMAL(10,8)` and `DECIMAL(10,8)` | (11,8) | (18,16) | (24,14) | (10,8) |
| `DECIMAL(38,38)` and `DECIMAL(1,0)` | (38,38) | (38,38) | (38,38) | bind error |
| `DECIMAL(38,0)` and `DECIMAL(38,38)` | (38,38) | (38,38) | bind error | bind error |

Notes:

- `*` stays 64-bit for ordinary operands: `price * (1 - discount) * (1 + tax)` on `DECIMAL(15,2)` columns is `DECIMAL(18,6)`, stored as `Decimal64`. The cost is a possible run-time overflow error if a result needs more than 18 digits; the message suggests casting an operand to a wider decimal.
- `INTEGER / INTEGER` is the decimal rule applied to (10,0) and (10,0), so `7 / 2` is `3.500000` of type `DECIMAL(16,6)`; use `7 // 2` for `3`.
- Division by a column with a large scale can need many integer digits; that is what the bind error on precision above 38 catches. Cast the operands to a narrower decimal first.
- `%` and `//` by zero are errors, like `/`.

## 4. Mixing types

| operands | rule |
|---|---|
| decimal with integer | the integer is an exact decimal (section 2); result per section 3 |
| decimal with `DOUBLE`/`FLOAT`, arithmetic **or** comparison | **bind error.** Exact and approximate numbers are not mixed implicitly. Write `CAST(x AS DOUBLE)` for an approximate result or `CAST(y AS DECIMAL(p,s))` for an exact one |
| integer with `DOUBLE` | not specified here (integer rules); `not supported yet` until they are |
| `CASE`, `COALESCE`, `UNION`, `IN` over decimals | scale `max(s_i)`, integer digits `max(p_i - s_i)`, precision their sum; **bind error if above 38**, asking for a `CAST` to a common type. Never a silent rounding |

## 5. Comparison, grouping and joins

- Comparisons are exact across different scales, precisions and storage widths, and against integers: `1.50::DECIMAL(5,2) = 1.5::DECIMAL(10,1)` is true.
- `GROUP BY`, `DISTINCT`, joins and hash tables treat equal values as equal whatever their scale or storage width, so the hash is computed from the numeric value, not the stored representation.
- The same value must hash identically on x86-64 and ARM64 (`portability.md`, section 5).

## 6. Rounding and casts

Rounding is **half away from zero** everywhere: division, `ROUND`, and every cast below.

| cast | rule |
|---|---|
| `DECIMAL` to `DECIMAL(p2,s2)` | rounds to scale s2; error if the value does not fit p2 |
| `DECIMAL` to an integer type | rounds (2.5 gives 3, -2.5 gives -3); error if it does not fit |
| `DECIMAL` to `DOUBLE` | the nearest double |
| `DOUBLE` to `DECIMAL(p,s)` or integer | rounds the **exact binary value** of the double; `NaN` and infinity are errors. So `2.675::DOUBLE` becomes `2.67` (the double is slightly below 2.675), where DuckDB gives 2.68. Cast through a string or write the decimal literal to get 2.68 |
| string to `DECIMAL(p,s)` | trims spaces; accepts a sign, digits, a decimal point and an exponent (`'1e2'` is 100); rounds at scale s (`'1.005'` to `DECIMAL(5,2)` is 1.01); error if it is not a number or does not fit |

## 7. Errors

Bind-time errors reject the query. Run-time errors apply to selected, valid rows only. NULL input gives NULL, never an error.

| error | example message |
|---|---|
| scale too large (bind) | `cannot multiply DECIMAL(20,20) by DECIMAL(20,20): the result scale would be 40, above the maximum of 38. Cast an operand to a smaller scale, for example CAST(x AS DECIMAL(20,10)).` |
| precision too large (bind) | `cannot divide DECIMAL(38,0) by DECIMAL(38,0): the result needs 44 digits, above the maximum of 38. Cast the operands to narrower decimals first.` |
| decimal with `DOUBLE` (bind) | `cannot add DECIMAL(5,2) and DOUBLE: exact and approximate numbers are not mixed implicitly. Use CAST(x AS DOUBLE) for an approximate result or CAST(y AS DECIMAL(p,s)) for an exact one.` |
| overflow (run) | `decimal overflow: 99999999999999999999999999999999999999 + 1 does not fit DECIMAL(38,0). Restrict the rows, or use a smaller scale.` |
| division by zero (run) | `division by zero in a / b. Use NULLIF(b, 0) to turn zero into NULL, or TRY(a / b).` |

`TRY(expr)` returns NULL when `expr` raises a run-time data error (overflow, division by zero, invalid cast). It never hides a bind error. (DuckDB has the same function; its place in the dialect is for the strictness spec.)

## 8. Functions and aggregates

| function | result |
|---|---|
| `SUM(DECIMAL(p,s))` | `DECIMAL(38,s)`; one overflow check against 38 digits at the end, over an accumulator wide enough that the order of rows (and so the thread count) cannot change the result |
| `AVG(DECIMAL(p,s))` | scale `min(s + 6, 38 - (p - s))`, precision `(p - s) + scale`; computed from an exact wide sum; never a bind error, because an average cannot need more integer digits than its inputs |
| `MIN`, `MAX`, `MEDIAN`, `QUANTILE_DISC` | the input type |
| `STDDEV`, `VAR_*` | `DOUBLE` (so `(x - AVG(x)) / STDDEV(x)` needs an explicit cast, and the error says so) |
| `COUNT` | not decimal-specific |

`AVG` examples:

| input | result |
|---|---|
| `DECIMAL(15,2)` | `DECIMAL(21,8)` |
| `DECIMAL(18,2)` | `DECIMAL(24,8)` |
| `DECIMAL(10,8)` | `DECIMAL(16,14)` |
| `DECIMAL(38,2)` | `DECIMAL(38,2)` |
| `DECIMAL(38,0)` | `DECIMAL(38,0)` |
| `DECIMAL(5,5)` | `DECIMAL(11,11)` |

Anything else (`SQRT`, `POWER`, `LN`, `QUANTILE_CONT`, `SUM` of integers, window `RANGE` offsets on decimals, unbounded `numeric` from federated sources) is `not supported yet` until specified.

## 9. Where this differs from DuckDB 2.0.0

Each row has a `nata-check` test in `check/queries/decimal-differences/` (Nata-only expectation or a tolerance), because the reference cannot be compared as written.

| case | DuckDB | Nata |
|---|---|---|
| `a / b` on decimals or integers | `DOUBLE` | exact decimal (section 3) |
| decimal with `DOUBLE`, arithmetic or comparison | `DOUBLE`, silently | bind error |
| division by zero | `inf` for `/`; error for `%` | error, `TRY()` for NULL |
| `AVG(DECIMAL)` | `DOUBLE` | exact decimal |
| `CASE`/`COALESCE`/`UNION` of decimals that do not fit 38 | silently rounds | bind error |
| `1e2` literal | `DOUBLE` | exact decimal |
| `DOUBLE` to integer or `DECIMAL` | half to even for integers; inconsistent for decimals | half away from zero on the exact value |
| `%` where the type would exceed 38 | falls back to `DOUBLE` | bind error |

Everywhere else the rules match DuckDB's result types and errors (`+`, `-`, `*`, `%`, `SUM`, `ROUND`/`FLOOR`/`CEIL`, casts from strings and between decimals, modulo sign), which is what the differential tests rely on.

### Testing

- Same-as-DuckDB behaviour: queries in `check/queries/decimal/`, compared as text.
- Both engines must error (overflow, scale, modulo by zero): `-- expect-error` header.
- Differences: `-- expect-error-nata: <text>` (Nata must fail with that text) or `-- tolerance: <relative>` (numeric cells compared within a relative tolerance, for exact decimals against DuckDB's `DOUBLE`; `0` compares values and ignores formatting). A tolerance must allow for the decimal scale: a quotient rounded to 8 places differs from the double by up to about 1e-8 relative.
- `DOUBLE` against `DOUBLE` (for example `STDDEV`): `-- tolerance: 1e-12`.
- A tolerance query with several rows must have an `ORDER BY`, because unordered rows are sorted as text.
- Limits: DuckDB computes in `DOUBLE`, so comparisons are only meaningful up to about 15 significant digits.

## 10. Kernel notes (ADR 0003)

- Result precision <= 18 means `Decimal64` output. `*` of two 64-bit operands keeps a checked `i64` multiply per row, with the error masked by selection and validity.
- `SUM` and `AVG` over `Decimal64` inputs fit an `i128` accumulator; over `Decimal128` inputs they need a wider accumulator and a single check at the end. This keeps the result independent of thread count (ADR 0001).
- Comparisons and hash keys across `Decimal64` and `Decimal128` widen and rescale before comparing or hashing.
- Every optimized kernel has a scalar reference (`portability.md`).
