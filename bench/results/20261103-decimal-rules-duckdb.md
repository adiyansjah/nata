# Decimal rules checked against DuckDB 2.0.0

The rules are in `docs/decimal-rules.md`. This file records how they were derived and what was checked. No engine code exists yet, so nothing here is a benchmark; it is the evidence for the rules.

## Method

- DuckDB 2.0.0 CLI, `typeof(...)` on constant expressions and the values or errors of small queries. Type rules do not depend on the machine (x86-64 VM used).
- Type formulas were compared with DuckDB over **700 random pairs of operand types**: precision from {1, 3, 5, 9, 10, 15, 18, 19, 25, 30, 38}, scale from {0, 1, 2, 5, 9, 15, 18, 20, 30, 38} with scale not above precision (seeded shuffle, seed 1). A sample, not an exhaustive check.

## Result: result types

| operation | pairs | equal | DuckDB bind error | DuckDB returned `DOUBLE` | disagreements |
|---|---:|---:|---:|---:|---:|
| `+` and `-` | 700 each | 700 each | 0 | 0 | 0 |
| `*` | 700 | 670 | 30 | 0 | 0 |
| `%` | 700 | 564 | 0 | 136 | 0 |

- The 30 multiplication bind errors are exactly the pairs where the scale `s1 + s2` exceeds 38; the formula says "bind error" for them.
- For `%` DuckDB falls back to `DOUBLE` for 136 wide pairs (for example `(18,15) % (30,5)`, which would need 40 digits); the rules make these a bind error. For the 564 decimal results the formula matched.

### Multiplication precision (found by experiment, then confirmed on a grid)

With both operands of precision at most 18 and result scale `s1 + s2` below 18, DuckDB caps the result precision at 18; otherwise it uses `min(p1 + p2, 38)`.

- Grid of 158 operand combinations: a first hypothesis ("cap at 18 if both operands fit 64 bits") matched all 158, but the grid contained no case with result scale exactly 18.
- A targeted probe over result scales 12 to 24 for operand precisions 10, 12, 15 and 18: the cap is 18 for scales 12 to 17; at scale 18 and above the precision is `min(p1 + p2, 38)` (for example `(10,9) * (10,9)` is `(20,18)`, not `(18,18)`). The rule was corrected to "scale below 18".
- Q1 expression: `price * (1 - disc) * (1 + tax)` on `DECIMAL(15,2)` columns is `DECIMAL(18,6)`.

## Result: values, rounding and errors

- Casts round half away from zero: `CAST(2.345 AS DECIMAL(5,2))` is 2.35, `-2.345` is -2.35, `2.5` to `DECIMAL(5,0)` is 3, `3.5` is 4; `DECIMAL` to `INTEGER` rounds the same way (2.5 to 3, -2.5 to -3).
- `DOUBLE` to `INTEGER` rounds half to even (2.5 to 2, -2.5 to -2, 3.5 to 4, 0.5 to 0); `DOUBLE` to `DECIMAL(5,2)` is inconsistent (0.285 to 0.28, 2.675 to 2.68, 1.005 to 1.00).
- String casts: `'1.005'` to `DECIMAL(5,2)` is 1.01, `'1e2'` to `DECIMAL(10,2)` is 100.00, `' 12.5 '` is 12.50.
- Errors: overflow in `+`, `*`, `SUM` and `INTEGER +`; multiplication with needed scale 40 is a bind error ("Needed scale 40… add an explicit cast"); `%` by zero is an error, with a hint to use `TRY(...)`.
- No error: `/` by zero returns `inf` for decimals and integers.
- `/` on decimals and integers returns `DOUBLE` (`1.00/3.00` is 0.3333333333333333, `7/2` is 3.5); `//` on integers truncates toward zero (`-7 // 2` is -3, `7 // -2` is -3). On decimals `//` is plain division, not integer division (`-7.5 // 2` is -3.75), so the rules leave decimal `//` as not supported yet.
- `decimal + DOUBLE` is `DOUBLE`; `decimal + INTEGER` is `DECIMAL(13,2)` for `DECIMAL(5,2)` and `BIGINT` gives `(22,2)`.
- `1.5` is `DECIMAL(2,1)`, `100` is `INTEGER`, `10000000000` is `BIGINT`, `1e2` and `1.5e0` are `DOUBLE`.
- `SUM(DECIMAL(15,2))` is `DECIMAL(38,2)` and errors on overflow; `AVG` is `DOUBLE`; `MIN`, `MAX`, `MEDIAN` keep the type; `STDDEV` and `VAR_SAMP` are `DOUBLE`.
- `COALESCE(NULL::DECIMAL(36,2), 1.1234567891::DECIMAL(38,10))` is `DECIMAL(38,4)` and rounds to 1.1235 silently.
- Comparisons across scales are exact (`1.50::DECIMAL(5,2) = 1.5::DECIMAL(10,1)`), and decimal against `DOUBLE` compares without error.
- `ROUND(1.25::DECIMAL(5,2), 1)` is `DECIMAL(5,1)` 1.3; `ROUND(125::DECIMAL(5,0), -1)` is `DECIMAL(6,0)` 130; a larger digit count leaves the type unchanged; `FLOOR`, `CEIL`, `TRUNC` give scale 0 with the same precision; negation and `ABS` keep the type.
- `TRY(expr)` returns NULL for `% 0` and for a failing cast.
- `%` on decimals: `-5.5 % 2` is -1.5 (sign of the dividend), type `DECIMAL(6,1)` for `(5,1) % (5,0)`.

## Test queries

`check/queries/decimal/` (27: 17 same-as-DuckDB, 10 where both engines must error) and `check/queries/decimal-differences/` (12: 6 where Nata must error and DuckDB does not, 6 with a tolerance). Each was run on DuckDB to confirm the behaviour it assumes; with DuckDB standing in for Nata, the 27 and the tolerance queries pass and the six Nata-error queries report a mismatch because DuckDB returns a result (`inf`, 2.5, `true`, 1.0, 1.1235, 0.0).

## Limits

- DuckDB 2.0.0 only; a different version may differ.
- Statements about Postgres, Snowflake, SQL Server and Spark in the review were from memory and were not verified; the rules do not depend on them except that decimal-with-`DOUBLE` is an error and division by zero is an error, which are choices made for the dialect's strictness.
- The `AVG`, division-scale and unification formulas are original to these rules, so they were checked only for internal consistency by hand (the tables in the document), not against another engine.
