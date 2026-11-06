# 0007: SQL dialect and strictness

Status: accepted (by the assistant under the owner's delegation of 2026-11-06; written before the parser work, as the feature map requires)
Date: 2026-11-06

## Context

Slice 4 (card `docs/slices/0002-select-where-limit.md`) adds SQL. The vision asks for "a clean, strict SQL dialect on a Postgres-style base"
(`docs/vision.md`) and the feature map's open question 5 asks for "exact coercion, overflow, NULL and ordering rules" before the parser. The
numeric rules exist (`docs/decimal-rules.md`, ADR 0003 decision 6: overflow is an error). DuckDB is our oracle for results, but it is
lenient in places (implicit string conversion, case-insensitive quoted names, float division of integers).

## Decision

The dialect is `docs/dialect.md`. Its rules, in short:
1. **Postgres-style names:** unquoted identifiers fold to lower case; quoted identifiers are exact and case-sensitive.
2. **No implicit conversion** between strings, numbers, dates and floats; numbers follow `decimal-rules.md` (exact decimals, integer and
   decimal widening only).
3. **Errors, not guesses:** overflow, division by zero, bad casts and impossible dates are errors for selected rows; unsupported
   constructs are `not supported yet: <what>` at their position; every error has a class and a line and column, and never quotes data.
4. **SQL three-valued logic** and DuckDB's float semantics (NaN equals NaN and is greatest; -0.0 equals 0.0).
5. **Determinism without ORDER BY:** a scan's rows come in file, row-group and row order, so `LIMIT` is reproducible.
6. Every difference from DuckDB is listed in `docs/dialect.md` section 9 and held by the differential check as a Nata-only expectation.

## Why

A strict dialect turns a class of silent wrong results (a string compared as a number, a case-mismatched column silently matched, a float
division of exact money) into errors with a position, which is the product's promise. Postgres-style names are what most clients and BI
tools expect. Matching DuckDB everywhere else keeps it usable as the oracle.

## Alternatives considered

- **Follow DuckDB exactly:** the oracle would be exact everywhere, but implicit casts and case-insensitive quoted names are exactly the
  leniency the vision rejects.
- **Follow Postgres exactly:** Postgres has no NaN ordering equal to DuckDB's and differs in decimal result types; we keep its name rules
  and three-valued logic and take DuckDB's float semantics so that the scan's pruning (slice 3) and the oracle agree.
- **Decide rule by rule during the parser work:** the feature map forbids it, and the rules would follow the parser's convenience.

## Evidence

- **Measured:** DuckDB 2.0.0's behaviour for every rule of section 9 was checked by hand on 2026-11-06 (`7 / 2` is 3.5, `1 / 0` is `inf`,
  `1 = '1'` is `true`, `"A"` finds `a`, `DATE '1994-01-01' + 1` is `1994-01-02`, `LIMIT -1` is refused, overflow is an error). The NaN and
  -0.0 semantics are the ones the slice 3 pruning was verified against (`bench/results/20261105-scan-pruning.md`).
- **Reasoned:** the choice of Postgres name rules and of strictness over leniency.
- **Not covered:** collations, time zones, timestamps, intervals, nested types (later slices extend the page).

## Confidence

High for the strictness rules (they follow the vision and ADR 0003). Medium for the name rules: if real client tools turn out to rely on
case-insensitive quoted names, revisit.

## Consequences

The binder must carry types exactly and refuse what the page does not state; the differential check needs a Nata-only expectation for each
difference; users porting DuckDB SQL meet errors where DuckDB was lenient, each with a position and a message.

## Revisit when

A real workload or client tool (slice 10) needs a rule this page refuses; the date-function slice adds intervals.

## Revision notes

## Amendments
