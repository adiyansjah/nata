# Decimal rules: exhaustive verification, and client support for Decimal64

Date: 2026-11-04. Machine: shared 8-vCPU x86-64 VM (heavily loaded, so timings are not recorded). DuckDB 2.0.0 (CLI). Python 3.13.5, `fractions`/`decimal` from the standard library. No GCP, no cost.

What was checked: `docs/decimal-rules.md` (the rules as written), against (1) an independent reference implementation written from the document, (2) invariants and oracles over every type pair, (3) DuckDB 2.0.0, (4) Python's `decimal` module as a second implementation of the values. Nothing here tests Nata code, because none exists; it tests that the contract is right, unambiguous and implementable.

## Reproduce

```
cd spikes/decimal-verify
python3 -I verify.py                  # everything in item 14, incl. the 31 mutations
                                      # DuckDB answers are cached in /tmp/nata-dv-cache; delete it to re-query (not timed; roughly 20-40 min on the loaded VM)
python3 -I verify.py --skip-mutations # baseline only
# item 17
CARGO_BUILD_JOBS=2 cargo build --release && ./target/release/decimal-verify-writer /tmp/nata-dv-clients
uv venv .venv && uv pip install --python .venv/bin/python pyarrow polars duckdb pandas   # .venv is git-ignored
.venv/bin/python clients.py /tmp/nata-dv-clients
```

Files: `rules_ref.py` (reference + the 31 mutations), `typecheck.py` (type invariants, DuckDB type matrix), `values.py` / `extras.py` (value cases, ties, AVG, ROUND/CAST, overflow analysis), `duck.py` (DuckDB batching: one process runs many statements, errors attributed with `stdbuf -o0` and markers), `verify.py` (driver), `clients.py` + `src/main.rs` (item 17).

## Item 14: results of the baseline run (unmutated reference): 0 failures

The brief said "about 2M tuples per operator"; the real number is **779 types x 779 types = 606,841** (types with 1 <= p <= 38, 0 <= s <= p), all of them were checked.

| check | cases | result |
|---|---:|---|
| document's own examples (14 operand rows x 5 columns, 6 AVG, Q1 chain `(18,6)`, `7/2` is `(16,6)`) | 78 | all equal |
| invariants per operator (`+ - * / %`): P in 1..38, 0 <= S <= P, S as specified, bind error exactly where the digit budget says, P-monotone in p1 and p2 (a wider operand never narrows the result, a bind error never disappears), S monotone | 606,841 x 5 | 0 violations |
| oracle: exact largest |result| computed from first principles (`bound()` in `typecheck.py`) against P | 606,841 x 5 | `/`: P is exactly the digit count of the largest quotient, so the division bind error is exactly "needs more than 38 digits"; `+ -`: never too narrow below 38, exact when the scales are equal, at most 1 digit wider otherwise; `*`: P = min(38, p1+p2) except where the 64-bit cap engaged; `%`: never too narrow |
| DuckDB `typeof`: `+` | 606,841 | all equal |
| DuckDB `typeof`: `-` | 606,841 | all equal |
| DuckDB `typeof`: `*` | 606,841 | 505,571 equal; 101,270 both error (exactly the pairs with s1+s2 > 38) |
| DuckDB `typeof`: `/` | 606,841 | DuckDB is `DOUBLE` for every pair (documented difference); Nata type from the formula, checked by the oracle |
| DuckDB `typeof`: `%` | 606,841 | 404,301 equal; 202,540 where the rules give a bind error and DuckDB returns `DOUBLE` (documented difference) |
| DuckDB `typeof(COALESCE(a,b))` | 606,841 | 404,301 equal; 202,540 where Nata has a bind error: DuckDB returns exactly `DECIMAL(38, 38 - maxIntDigits)` (silent scale cut, documented difference) |
| values, small types (p <= 2, 5 types, every value) x every ordered type pair, `+ - * / %` | 5 x 403,225 = 2,016,125 | see below |
| values, boundary-biased (24 types from (1,0) to (38,38), at most 44 values each: 0, +-1, +-max, +-max/2, powers of ten and 10^k-1, tie digits, 2^63 edge and its square-root edge) x all ordered pairs where the type is valid | `+` 868,624, `-` 868,624, `*` 778,152, `/` 557,046, `%` 643,304 | see below |
| division, exact half-way cases (built from exact arithmetic, 9,712 true ties) plus near-misses, 4 sign combinations | 24,074 | all equal to the reference, to `decimal` (ROUND_HALF_UP) and, for the 24,046 that fit 120 bits, to DuckDB's HUGEINT integer arithmetic |
| AVG: 441 random and boundary groups (12 types) | 441 | nearest, ties away (24 exact ties), SUM equal to DuckDB's SUM, AVG equal to DuckDB HUGEINT arithmetic on that SUM, DuckDB `AVG` is `DOUBLE` and within tolerance. AVG type for all 779 types: P <= 38, no bind error, integer digits unchanged, scale = min(s+6, 38-(p-s)) |
| ROUND(x, n), n in {-3..3, 5, 10, 38}, 15 types | 10,154 equal + 6 both-error | equal |
| CAST DECIMAL to DECIMAL(p2,s2), 10 targets | 8,470 equal + 1,690 both-error | equal |
| FLOOR, CEIL, TRUNC | 3 x 1,016 | equal |
| result types of ROUND (n = -3..40), FLOOR, CEIL, TRUNC, `-x`, `ABS` over all 779 types | 38,171 | equal except ROUND with n < 0 (finding F2) |

Value comparison details (`+ - *` `%`): result type and exact text equal to DuckDB, and NULL (from `TRY`) exactly where the reference says overflow or modulo by zero. Small types: `+`, `-`, `*` 403,225 equal each; `%` 400,050 equal + 3,175 modulo-by-zero, error in both. Boundary set: `+`/`-` 763,424 equal + 102,556 both overflow + 2,644 finding F1; `*` 674,996 equal + 103,156 both overflow; `%` 626,308 equal + 16,996 both error. Every one of these also agrees with Python `decimal` (ROUND_HALF_UP) as a second implementation, and a result the reference calls an overflow is confirmed to need more than P digits by `decimal`.

Division (DuckDB returns `DOUBLE`, so the documented expectation is used): small types 400,050 and boundary set 542,390 non-zero-divisor cases satisfy (a) the exact rational property "the stored value is the nearest value at scale S to the true quotient, and an exact tie goes away from zero" and (b) |DuckDB double - Nata value| <= 0.5 x 10^-S + 1e-15 relative. Division by zero: 3,175 + 14,656 cases, error in the reference, `inf`/`-inf`/`nan` in DuckDB (the documented difference); 150 + 2,352 reference-only checks that every typed pair raises division by zero. The reference never raises overflow for a valid division.

### Differences from DuckDB, classified

| difference | scope | classification |
|---|---|---|
| `/` returns `DOUBLE` | all 606,841 type pairs, 942k value cases | documented (section 9); Nata expectation checked as above |
| `/` by zero: `inf` / `nan` | 17,831 cases | documented |
| `%` where P > 38: DuckDB `DOUBLE`, Nata bind error | 202,540 type pairs | documented |
| `COALESCE` where I + S > 38: DuckDB silently `(38, 38-I)` | 202,540 type pairs | documented |
| `AVG` is `DOUBLE` | 441 groups | documented |
| **F1** `+`/`-` at the 38-digit cap: the exact result fits `DECIMAL(38,s)`, but DuckDB raises overflow because it first casts an operand to the result type | 2,644 cases per operator | **not in the document** (below) |
| **F2** `ROUND(x, n)` with n < 0: result precision | 2,112 of the 2,337 (type, n in -3..-1) cases | **not in the document** (below) |
| DuckDB prints `DECIMAL(p,p)` without the leading zero (`.9801`, `-.05`) | all values of such types | not in the document; matters for "compared as text" |

## Findings (each is a check in `verify.py`; none changes a result type of `+ - * /`)

**F1. Add/subtract at the cap.** `10000000000000000000000000000000000000::DECIMAL(38,0) + (-0.9)::DECIMAL(1,1)`: the rules give `DECIMAL(38,1)` and the exact sum 9999999999999999999999999999999999999.1 has 38 digits, so by "checked against the result precision" it fits. DuckDB 2.0.0 raises overflow (`TRY` gives NULL). Rescaling the 38-digit operand to scale 1 needs 39 digits (10^39 is above `i128::MAX`, about 1.7 x 10^38), so a 128-bit kernel cannot produce the exact answer either. The rules should say which is intended; erroring like DuckDB is the only one a 128-bit kernel can do. (Run: `values big +`, finding line in the baseline.)

**F2. ROUND with a negative digit count.** The document says precision `min(p1+1, 38)` for n < 0. DuckDB: `typeof(round(NULL::DECIMAL(5,2), -1))` is `DECIMAL(5,0)`, `round(NULL::DECIMAL(5,0), -1)` is `DECIMAL(6,0)`, `round(NULL::DECIMAL(5,0), -6)` is `DECIMAL(5,0)`. Over all 779 types and n = -3..-1 DuckDB's rule is exactly: `min(38, p+1)` if s = 0 and -n <= p, otherwise `p` (checked, 0 exceptions). The document's rule is correct but 1 digit wider than needed in 2,112 cases; the evidence file only tested one example. ROUND is also not marked "checked" in the table, but at 38 digits it must be: `ROUND(99999999999999999999999999999999999999::DECIMAL(38,0), -1)` is 10^38 and fails in DuckDB (`Out of Range Error: Overflow in ROUND of DECIMAL(38)`) and in the reference; the ROUND value cases include 6 such both-error cases.

**F3. AVG rounding mode is not stated in section 8** (section 6 lists "division, ROUND, and every cast"). The reference uses half away from zero; `AVG` of 2 and 3 as `DECIMAL(38,0)` gives 3 (half-even would give 2). Mutation `avg_round_half_even` shows the difference is detectable.

**F4. Integer times integer is unspecified.** Section 2 makes integers decimals, section 4 says integer arithmetic is "integer rules", none of which is written. With the decimal reading `INTEGER * INTEGER` is `(10,0)*(10,0)` = `(18,0)`, so `2147483647 * 2147483647` (4611686014132420609, 19 digits, fits BIGINT) is an overflow error. DuckDB: `INTEGER * INTEGER` is `INTEGER` and also errors; `BIGINT * BIGINT` gives 4611686014132420609. Needs a sentence.

**F5. `%` bind error is conservative.** The type `P = max(p1-s1, p2-s2) + S` is not tight, because |a % b| < min(|a|, |b|). `DECIMAL(38,0) % DECIMAL(5,2)` is a bind error in the rules (needs 40), yet the result is below 1000 (DuckDB returns `DOUBLE`, 121.90266580222885 for 99999999999999999999999999999999999999 % 123.45). The tight type `min(p1-s1, p2-s2) + S` never exceeds 38 and was never too narrow over all 606,841 pairs (202,540 pairs would change from bind error to a type). This is a design choice for the owner, not a bug.

**F6. Observations, no action needed.** (a) `+`, `-` precision is exact only when the scales are equal, otherwise up to 1 digit wider than needed (1,332 type pairs, e.g. `(1,0)+(1,1)` is `(3,1)` where `(2,1)` suffices; matches DuckDB). (b) Result precision is not monotone in a scale: with a scale growing by 1 at fixed precision P drops in 82,251 `+`/`-` pairs and 91,390 `%` pairs (e.g. `(1,0)+(1,1)` is `(3,1)`, `(1,1)+(1,1)` is `(2,1)`); `*` and `/` have none. The document does not promise it. (c) `DECIMAL(1,0) + DECIMAL(38,38)` has the type `(38,38)`, which has no integer digit, so the sum overflows whenever the `DECIMAL(1,0)` value is nonzero (`5::DECIMAL(1,0) + 0::DECIMAL(38,38)`: the reference raises overflow and DuckDB's `TRY` gives NULL, type `DECIMAL(38,38)` in both; `0.5::DECIMAL(1,1) + 0::DECIMAL(38,38)` works). (d) The brief's "about 2M tuples" is 606,841.

## 2d: Decimal64 and i64 (exhaustive over the 35,721 type pairs with both operands <= 18 digits)

| class | type pairs | result |
|---|---:|---|
| `+`/`-`, P <= 18 (Decimal64 output) | 19,210 | largest rescaled operand 99,999,999,999,999,999 (< 10^17), largest sum 199,999,999,999,999,998; both < 2^63 and < 10^P. **No i64 overflow is possible; 0 violations.** |
| `+`/`-`, P >= 19 (Decimal128 output) | 16,511 | in **11,970** of them rescaling an operand to the result scale exceeds i64 (example `(1,0)+(18,18)` is `(20,18)`): widen to `i128` before rescaling |
| `*`, cap off, P <= 18 | 6,936 | largest product 999,999,998,000,000,001 < 10^18 < 2^63. **Cannot overflow i64 or P; 0 violations.** |
| `*`, cap off, P >= 19 (S >= 18) | 7,296 | 7,290 can exceed i64 (example `(2,0)*(18,18)` is `(20,18)`): `i128` product |
| `*`, **cap engaged** (exactly: p1 <= 18, p2 <= 18, s1+s2 < 18, p1+p2 >= 19; the invariant checks that P < p1+p2 happens only there) | **21,489** | **all 21,489 can exceed P; none is safe.** 21,419 can also exceed i64 (a plain wrapping or even a checked-only-on-i64 multiply is wrong), 70 only reach [10^18, 2^63) (those are `(1,s1) * (18,s2)` and the mirror); for **every** cap-engaged pair a value pair with 10^18 <= a*b < 2^63 exists |
| `*` bind error with both operands <= 18 digits | 0 | (s1+s2 <= 36) |
| `/`, P <= 18 | 7,021 | numerator `a * 10^(s2+S-s1)` < 10^P (exactly, since P is its digit count), `numerator + |b|` and `2|b|+1` < 2^63 when the divisor is <= 18 digits; 0 violations |
| `/`, P <= 18 and the divisor wider than 18 digits | 8,840 (over all dividend types) | a Decimal64 output can need a Decimal128 input (example `(1,0) / (19,0)` is `(7,6)`) |
| `%`, P <= 18 | 23,751 | rescaled operands < 10^P; P <= 18 implies both operands <= 18 digits; 0 violations |

Examples, each run on DuckDB with `TRY` and in the reference (`TRY` = NULL means error):

| class | operands | product (unscaled) | type | reference | DuckDB |
|---|---|---:|---|---|---|
| cap engaged, fits i64, exceeds P | `10000000.00::DECIMAL(15,2) * 10000000.00::DECIMAL(15,2)` | 10^18 | `(18,4)` | overflow error | `DECIMAL(18,4)`, NULL (so DuckDB checks 10^18, not i64) |
| cap engaged, just below | `9999999.99 * 10000000.00` as above | 999,999,999,000,000,000 | `(18,4)` | 99999999900000.0000 | equal |
| cap engaged, exceeds i64 | `99::DECIMAL(2,0) * 99999999999999999::DECIMAL(17,0)` | 9,899,999,999,999,999,901 | `(18,0)` | overflow error | NULL |
| cap engaged, in [10^18, 2^63) with the largest 18-digit operand | `9::DECIMAL(1,0) * 999999999999999999::DECIMAL(18,0)` | 8,999,999,999,999,999,991 | `(18,0)` | overflow error | NULL |
| cap not engaged | `999999.99::DECIMAL(9,2) * 999999.99::DECIMAL(9,2)` | 9,999,999,800,000,001 | `(18,4)` | 999999980000.0001 | equal |
| cap off because S >= 18 | `(18,9) * (18,9)`, both 10^8 | 10^34 | `(36,18)` | 10000000000000000.000000000000000000 | reference only (DuckDB literal out of range in my query) |

## Mutation check: 31 mutations of the reference, 31 caught, 0 survivors

Columns: which kinds of check flagged it. **D** = compared with DuckDB (types, values, HUGEINT arithmetic); **I** = invariants/oracles over all type pairs; **E** = the document's own examples; **R** = reference-only exact property or second implementation (`decimal`), or the documented expectation (division by zero is an error). Command: `python3 -I verify.py` (the table is printed at the end). A mutation counts as caught if any check fails or a finding count changes.

| mutation | caught by |
|---|---|
| div scale s1+5 | D E I |
| div scale s1+7 | D E I R |
| div precision +1 | E I R |
| div scale cap 37 | D E I R |
| div ties to even | D R |
| div ties toward +inf | D R |
| div truncates | D R |
| div by zero returns 0 | R only (DuckDB returns `inf` in both versions, so only the documented expectation can see it) |
| `+ -` scale max -> min | D E I |
| `+ -` no carry digit | D E I |
| `+ -` cap cuts the scale | D E I |
| `+ -` max(int digits) -> min | D E I |
| `*` cap operands <= 19 | D E I |
| `*` cap when S <= 18 | D E I |
| `*` cap when either operand <= 18 | D E I |
| `*` cap value 17 | D E I |
| `*` no cap | D E |
| `*` scale bind error above 39 | D I |
| `*` overflow checked against i64 only (P <= 18) | D R (caught at the values 10^9 x 10^9 and the other [10^18, 2^63) products; DuckDB gives NULL) |
| `*` unchecked overflow | D R |
| `%` sign follows divisor | D R |
| `%` scale min | D E I |
| `%` bind error above 39 | D E I |
| `%` precision +1 | D E I R |
| AVG scale s+5 | D E R |
| AVG without the 38-(p-s) limit | D E R |
| AVG truncates | D R |
| AVG ties to even | D R |
| COALESCE precision max(p) | D |
| ROUND n < 0 precision p1 | D |
| ROUND/CAST ties to even | D |

Honest notes on the process: the first version of the harness skipped the DuckDB HUGEINT comparison after an exact-property failure, so the division and AVG rounding mutants were flagged by reference-only checks (R); the early `continue` was removed and the AVG HUGEINT check added, so DuckDB now sees them too. A mutation only the reference-only checks see is listed as such (division by zero). Division-scale mutants are mainly caught by the invariants and the document examples, not by DuckDB values, because DuckDB's division is `DOUBLE`.

## Item 17: client support for Arrow Decimal64

Files written by `spikes/decimal-verify/src/main.rs` (arrow 60.1.0, parquet 60.1.0): Arrow IPC file, Arrow IPC stream, Parquet; columns `d64` Decimal64(15,2) with NULL, `d128` Decimal128(15,2) with NULL, `d64_wide` Decimal64(18,0) with +-999999999999999999. The writer accepted Decimal64 in all three formats. arrow-rs wrote both `d64` and `d128` to Parquet as physical `INT64` with the Decimal logical type (precision 15, scale 2) and read them back as Decimal64 and Decimal128 respectively (the Arrow schema is stored in the file). Client versions: pyarrow 26.0.1, polars 2.1.0, DuckDB 2.0.0 (Python and CLI engine), pandas 3.1.0. Full output: run `clients.py`; 29 probes, 29 accepted, 0 errors.

| client / path | accepts Decimal64? | what it shows | values read back |
|---|---|---|---|
| pyarrow IPC file, IPC stream, Parquet | accepts, **keeps** `decimal64(15, 2)` / `decimal64(18, 0)` | schema equals the written schema | 123.45, NULL, -0.05, 9999999999999.99 (the 15-digit maximum), 0.00; `d64_wide` +-999999999999999999 |
| pandas (`read_parquet` via pyarrow, `Table.to_pandas` from IPC file and stream) | accepts, converts to Python `decimal.Decimal` in `object` columns | dtype `object` (no numeric dtype) | same values as above, exact |
| polars `read_parquet`, `read_ipc` (file), `read_ipc_stream`, `scan_ipc`, `from_arrow` | accepts, shows `Decimal(precision=15, scale=2)` / `(18, 0)` | one Decimal type, no separate 64-bit decimal in the schema | same values, exact |
| DuckDB `read_parquet` | accepts | `DECIMAL(15,2)`, `DECIMAL(18,0)` | same values; `d64*d64` is `DECIMAL(18,4)`; the 15-digit maximum squared raises `Out of Range Error: Overflow in multiplication of DECIMAL(18) (999999999999999 * 999999999999999)` |
| DuckDB Arrow scan of a pyarrow Table (from IPC file or stream) and of a `RecordBatchReader` | accepts | `DECIMAL(15,2)`, `DECIMAL(18,0)`; `sum(d64)` is `DECIMAL(38,2)` = 10000000000123.39 | same values |

No client rejected Decimal64, and none converted to float or string. pyarrow is the only one that visibly keeps the 64-bit decimal type; the others present a plain decimal with the right precision and scale. Not tested: Arrow Flight / Flight SQL clients, ADBC, JDBC/ODBC drivers, Parquet files without the embedded Arrow schema (readers then see only the physical and logical type; arrow-rs's choice of Decimal64 or Decimal128 in that case was not verified), Spark, Java/Go/C++ Arrow libraries, older client versions.

### Postgres wire protocol (answer about our server, from cited docs)

- `NUMERIC`'s limits: "up to 131072 digits before the decimal point; up to 16383 digits after the decimal point"; "the maximum precision that can be explicitly specified in a numeric type declaration is 1000" (PostgreSQL docs, *Numeric Types*, https://www.postgresql.org/docs/current/datatype-numeric.html, fetched 2026-11-04). `DECIMAL(38,s)` is inside both limits, so every Nata decimal is representable.
- Protocol: format code 0 is text, 1 is binary; the text representation "is whatever strings are produced and accepted by the input/output conversion functions for the particular data type"; for binary, only integers are described and "for other data types consult the documentation or source code" (PostgreSQL docs, *Protocol Overview*, section 54.1.3, https://www.postgresql.org/docs/current/protocol-overview.html, fetched 2026-11-04).
- Conclusion: the wire type is `NUMERIC` as a decimal string (text) or the server-defined binary layout, independent of the Arrow storage width. Decimal64 is invisible there: the server converts the unscaled `i64` or `i128` to a decimal string at the edge. Nothing in these sources makes Decimal64 a problem.
- Not verified: the exact binary `NUMERIC` layout (the documentation says to read the source; I did not), how a real Postgres client library handles the typmod (precision/scale) in `RowDescription`, and the text form a server should send for a value with trailing zeros at a declared scale.

## Proposed changes for the orchestrator (not made; I did not edit the rules or ADRs)

1. `decimal-rules.md` section 3, `+`/`-`: add that at the 38-digit cap an operand that does not fit the result type after rescaling is an overflow error even if the exact sum would fit (F1), and add it to section 9 as "same as DuckDB". Reason a 128-bit kernel cannot do otherwise.
2. Section 3, ROUND with n < 0: either adopt DuckDB's rule (`p1+1` only if s1 = 0 and -n <= p1, else `p1`, capped at 38) or list it in section 9 as a deliberate difference (F2). Mark ROUND as checked at 38 digits.
3. Section 8: state that AVG rounds half away from zero (F3).
4. Sections 2 and 4: state the type of integer `+ - *` integer (F4); the decimal reading makes `INTEGER * INTEGER` a `(18,0)` that overflows above 10^18.
5. Section 3, `%`: decide between the current conservative type (bind error for 202,540 pairs) and the tight `min(p1-s1, p2-s2) + S` (F5).
6. Section 9 "Testing": `nata-check` compares text; DuckDB prints `DECIMAL(p,p)` as `.9801` / `-.05`. Either Nata prints the same or the comparison normalises.
7. Section 10 (kernel notes): add the measured facts above: `+ -` with P <= 18 never overflow i64; with P >= 19 widen before rescaling (11,970 of 16,511 pairs with <= 18-digit operands need it); the `*` check must compare with 10^P, not i64 (21,489 cap-engaged pairs, all can exceed P, 21,419 can exceed i64); a Decimal64 `/` result can have a Decimal128 divisor (8,840 pairs).
8. ADR 0003: the "Decimal64 and client-edge widening: low" line can say that pyarrow, pandas, polars and DuckDB accept Decimal64 in IPC and Parquet (29 of 29 probes) and Postgres NUMERIC is independent of it; keep "not verified" for Flight SQL, ADBC, JDBC, and Parquet without the embedded Arrow schema.

## Limits

- DuckDB 2.0.0 and x86-64 only. Python's `decimal` and DuckDB were the only second implementations.
- Boundary-biased, not exhaustive, for p > 2: 24 types and at most 44 values per type; wide-type interior values and random values are only sampled (AVG groups, tie search). Ties for division were constructed for 140 type pairs of the 24-type set.
- The reference is my reading of the document; a misreading shared by the document's authors and me is only caught where DuckDB or a first-principles oracle disagrees. DuckDB cannot see division scale or precision, AVG, or division by zero; those rest on the document's examples, the oracle (division precision is tight) and the stated invariants.
- Not covered: casts from strings/DOUBLE/to integers, `//`, `SUM`, comparison and hashing across widths, `TRY`, literals, `ROUND` with non-constant n, NULL propagation, Kani/loom-level proofs of actual kernels, ARM64.
- The type `typeof` checks use `NULL` operands (types only); value checks use constants in tables, so DuckDB's constant folding or vectorised paths for very wide columns are exercised only through the cross joins of the tables.

## Update 2026-11-04 after the four rule decisions

Status: **the harness is updated and checked locally; the full run (all 779 types, full value sets, all 52 mutants) is moving to a quiet GCP VM, and every number marked "GCP" below is to be filled by that run.** Nothing marked "GCP" has been measured. No GCP was used to produce this section.

Machine for the local checks: shared 8-vCPU x86-64 VM, heavily loaded (load average about 49), so no timing here is a measurement. DuckDB 2.0.0, Python 3.13.5.

`docs/decimal-rules.md` now says (decisions of the project owner):

1. `+` `-`: an operand that does not fit the result type once rescaled to S is an overflow, even if the exact sum fits. (Earlier **F1: resolved by decision**.)
2. `ROUND(a, n)`, n < 0: precision `p1 + 1` only if `s1 = 0` and `-n <= p1`, else `p1`, capped at 38, checked. (Earlier **F2: resolved by decision**.)
3. `a % b`: scale `max(s1,s2)`, precision `min(p1-s1, p2-s2) + S`, never a bind error; deliberately tighter than DuckDB (section 9). (Earlier **F5: resolved by decision**.)
4. Integer with integer (`+ - * % //`): the wider integer type, overflow is an error; only `/` is a decimal; integer with decimal keeps the exact-decimal reading. (Earlier **F4: resolved by decision**.) AVG half away from zero (earlier F3) was already in.

### What changed in the harness (`spikes/decimal-verify/`)

| file | change |
|---|---|
| `rules_ref.py` | the four decisions, written from the document; integer arithmetic (`t_int`, `v_int`, `as_dec`); a `quick`/`full` profile (`NATA_DV_PROFILE`, set by `verify.py`); 52 mutants |
| `typecheck.py` | `%`: DuckDB's own type is modelled (wider operand type, or `DOUBLE` when it needs more than 38 digits), checked against all pairs, and Nata's type is classified against it; invariants for decision 3 from first principles; `check_mod_witnesses`, `check_mod_bound_bruteforce`; the document's `%` examples transcribed verbatim |
| `values.py` | the old F1 "finding" branch is gone: `+ -` where an operand does not fit must now be an error in both engines; `%` value comparison (exact for DuckDB decimals, tolerance for its `DOUBLE`); integer groups |
| `extras.py` | ROUND result types for every type and every n in -45..40 compared strictly (the F2 "finding" is gone) plus a first-principles oracle; more ROUND types and n; the `%` kernel facts |
| `intarith.py` (new) | integer with integer and integer with decimal (below) |
| `verify.py` | `--full`, `--cache-dir`, `--mutants`, `--list-mutants`, `--json`; exit code 1 on any failure or surviving mutant |

### Reproduce

```
cd spikes/decimal-verify
python3 -I verify.py                                  # quick profile: 29 types, small value sets, 4 mutants; seconds; checks the harness, is NOT the verification
python3 -I verify.py --mutants all                    # quick profile, all 52 mutants (11 s locally)
python3 -I verify.py --list-mutants
# the verification, on a quiet machine (tens of minutes, mostly DuckDB). Use a NEW cache directory:
python3 -I verify.py --full --cache-dir /tmp/nata-dv-cache-NEW --json /tmp/nata-dv-full.json
```
DuckDB answers are cached in `<cache-dir>/<profile>/` (a profile cannot read the other's cache). The mutation stage reuses the cached answers.

### What was run locally, and the result

All commands above on this machine, 2026-11-04.

| check | where it ran | result |
|---|---|---|
| `python3 -I verify.py --mutants all` (quick profile: 841 type pairs, 2 small + 8 boundary value types, 4 integer types with 40 values each, 5 integer-with-decimal types) | local, DuckDB included | baseline **0 failures**; **52 of 52 mutants caught, 0 survivors**; exit code 0 |
| invariants for `+ - * / %` over **all 606,841 type pairs** (pure Python, no DuckDB; P in 1..38, S as specified, bind errors exactly where the digit budget says, monotonicity; for `%`: never a bind error, P equals the digit count of the largest possible remainder) | local, full profile | **0 violations** for each of the five operators (18 s in total) |
| `%` witnesses: for all 606,841 pairs the reference's checked `%` at the extreme values (both maxima; `a = B'-1, b = B` when no rescaling of `a` is needed), both signs | local, full profile | no overflow raised in any pair; a remainder with exactly P digits was reached in 522,663 of the 606,841 pairs (the other 84,178: this witness set does not reach P, so P is an upper bound there; the argument below covers them) |
| `%` supremum formula `min(A', B'-1)` by brute force, A', B' in 1..40 | local | 1,600 cases, 0 wrong |
| ROUND n < 0 oracle (largest result from first principles) for all 779 types and n = -45..-1 | local, full profile | 35,055 cases; type never too narrow (except the 38 cap cases, which are the checked overflow); P wider than needed in 33,522 (DuckDB's rule is an upper bound, see N4) |
| ROUND against DuckDB: result types and values for 12 types x n in {-45,-40,-39,-38,-37,-20,-19,-18,-10,-6,-5,-4,-3,-2,-1,0,1,2,3,5,10,38}; the same 12 types x CAST/FLOOR/CEIL/TRUNC | local, full-profile lists, a subset of the types | 13,966 ROUND cases equal + 26 both error; 1,092 result types equal DuckDB (540 with n < 0); 0 differences. Mutants of decision 2 tried on this set: 6 of 6 caught |
| TINYINT x TINYINT exhaustive (all 65,536 pairs) for `*`, `//`, `%` against DuckDB `TRY` | local, full profile | 0 mismatches (excluding the one listed in N2) |
| integer with decimal: type matrix 4 integer types x 779 decimal types x 2 orders x 5 operators, and values (4 integer types, 12 decimal types, boundary values, both orders) against DuckDB | local, full profile of this piece | 31,160 types: 18,934 equal + 12,226 documented differences (`/` is `DOUBLE`; `%` tighter or `DOUBLE`) + **0 disagree**; values: `+` 697,420 equal + 3,624 both overflow + 77,916 operand-does-not-fit (F1) both error; `*` 709,320 equal + 69,640 both overflow; `%` 689,150 equal + 12,632 both error + 77,178 `DOUBLE` cases within tolerance or bounded; `/` 650,192 within tolerance + 11,378 by-zero; **0 mismatches** |
| `verify.py --full` complete run | **GCP** | to be filled by the GCP run |

### Case counts per check

Quick profile (local, measured). The "full" column is the number from the GCP run, to be filled; where the count is a closed form, the expected value is given in brackets and must match.

| check | quick (local) | full |
|---|---:|---|
| document examples (14 rows x 5 + 6 AVG + Q1 chain + `7/2`) | 78, 0 failures, 1 finding (N1) | 78 (GCP) |
| type pairs per operator for invariants | 841 | [606,841] (done locally, above) |
| DuckDB `typeof` `+`, `-` | 841 equal each | GCP: [606,841 equal each] |
| DuckDB `typeof` `*` | 774 equal + 67 both error | GCP |
| DuckDB `typeof` `/` | 841 `DOUBLE` (documented) | GCP |
| DuckDB `typeof` `%` | 91 equal, 592 Nata tighter (documented), 158 DuckDB `DOUBLE` (documented), **0 disagree** | GCP: [606,841 pairs, tighter or DOUBLE where P_old > 38: expected 202,540 DOUBLE] |
| DuckDB `typeof` COALESCE | 683 equal + 158 documented | GCP |
| values, small types, `+ - * / %` | 1,444 each; all equal | GCP: [2,016,125 over 5 operators for the full small set] |
| values, boundary-biased, `+` / `-` | 12,996 each: 10,168 equal, 372 both overflow, **2,456 operand-does-not-fit (F1) now both error** | GCP |
| values, boundary-biased, `*` | 11,111: 8,467 equal + 2,644 both overflow | GCP |
| values, boundary-biased, `/` | 7,116: 6,610 within the exact property and DuckDB tolerance + 506 by zero | GCP |
| values, boundary-biased, `%` | 12,996: 8,052 equal, 6,240 of which with a Nata type tighter than DuckDB's; 912 both error (by zero); 2,912 DuckDB `DOUBLE` within tolerance; 1,120 `DOUBLE` not comparable (quotient above 2^40), only `|r| <= |b|` checked | GCP |
| division ties, AVG | 1,900 ties (768 exact); 33 AVG groups (20 exact ties) | GCP |
| ROUND result types | 2,639 (1,305 with n < 0), 0 differences | GCP: all 779 types x n = -45..40 |
| integer result types, 16 ordered pairs x 6 operators | 80 equal + 16 documented (`/`) + 0 disagree | the same 96 (GCP; deterministic) |
| integer values per operator (quick) | 25,600 per operator, see below | GCP: 3,189,796 per operator |
| integer error category probes (DuckDB's own error message, no `TRY`) | 170 probes, 170 category equal | GCP: 490 probes |

### Integer with integer (decision 4)

Against DuckDB 2.0.0, for TINYINT, SMALLINT, INTEGER, BIGINT: all 16 ordered pairs, operators `+ - * % // /`.

- **Result types (96 expressions).** `+ - * % //` give the wider of the two integer types in all 80 cases and DuckDB agrees in all 80. `/` is a decimal by the decimal rule on (3,0), (5,0), (10,0), (19,0) (for example `INTEGER / INTEGER` is (16,6)); DuckDB gives `DOUBLE` in all 16 (documented). `INTEGER * INTEGER` is `INTEGER`, `TINYINT * SMALLINT` is `SMALLINT`, `BIGINT // INTEGER` is `BIGINT`.
- **Values (quick, local).** 25,600 cases per operator at the limits (MIN, MIN+1, MAX, MAX-1, 0, +-1, +-2, powers of two and their neighbours, the square-root edge of the limit, powers of ten, half the limit). `+` 24,038 equal + 1,562 overflow in both; `-` 24,031 + 1,569; `*` 17,392 + 8,208; `%` 24,950 equal + 640 division by zero in both + 10 listed in N2; `//` 24,950 + 10 overflow in both (`MIN // -1`) + 640 by zero in both; `/` 24,960 within the exact property and DuckDB's tolerance + 640 by zero (`inf` in DuckDB, documented). A second implementation (plain Python integers with an explicit range test, `oracle_int`) agrees with the reference in every case.
- **Error category** is compared as a category, never as text: for the error cases the harness runs the statement without `TRY` and reads DuckDB's message (`Overflow in multiplication of INT8 ...` is overflow; `Division by zero in expression (5 // 0)` is division by zero). 170 of 170 probes agree (quick); the full run has 490.
- **Exhaustive** (local, full profile): TINYINT x TINYINT, all 65,536 pairs for `*`, `//`, `%`: 0 mismatches.
- **Integer with decimal** keeps the exact-decimal reading: see the row in the table above; also covered by the mutant `intdec_bigint_as_18`.
- GCP numbers for the values: to be filled.

### F1, F2, F5: zero differences now

| earlier finding | check | local result | GCP |
|---|---|---|---|
| F1 `+ -` at the cap | `values big +/-`: every case where an operand does not fit once rescaled is now compared as an error with DuckDB's overflow | quick: **2,456 such cases per operator, all both-error, 0 mismatches**; with integers read as decimals, 77,916 more per operator (full profile of that piece); the mutants `add_cap_exact_sum`, `add_fit_check_first_operand_only`, `sub_no_fit_check` are each caught | full count: to be filled (the earlier run saw 2,644 per operator in the full set; the new count is expected to be the same or larger) |
| F2 ROUND n < 0 | strict equality of the result type for every type and n in -45..-1 (not only -3..-1) | quick: 1,305 (type, n<0) cases equal; full profile on 12 types x 22 n: 540 equal; **0 differences**; the earlier 2,112 differences are gone | all 779 types: to be filled |
| F5 `%` | invariants + witnesses over all 606,841 pairs (local, done) and DuckDB `typeof` | invariants **0 violations on 606,841 pairs**, never a bind error, P never above 38, no overflow at the witnesses; DuckDB: classification above, 0 disagree | DuckDB `typeof` over all pairs: to be filled |

**Why the remainder always fits the new type (reasoned, and checked by the items above).** Let A' and B' be the largest aligned magnitudes, `A' = (10^p1 - 1) * 10^(S-s1)` and `B' = (10^p2 - 1) * 10^(S-s2)`. For aligned integers `0 <= |a| <= A'`, `1 <= |b| <= B'`: `|a % b| <= |a|` and `|a % b| <= |b| - 1`, so `|r| <= min(A', B' - 1)`, which is exactly attained as a supremum over the aligned integers (a = B'-1, b = B' if B'-1 <= A'; a = A', b = A'+1 otherwise; brute force confirms the formula for A', B' <= 40). `min(A', B'-1)` has `min(p1-s1, p2-s2) + S` digits, because `A'` has `(p1-s1)+S` digits and `B'-1` has `(p2-s2)+S` digits (subtracting 1 from `99...9 x 10^k` does not remove a digit). So the precision is sufficient and equal to the digits of the bound; it is never above 38 because `min(i1,i2) + max(s1,s2) <= p1` when the larger scale is s1 and `<= p2` otherwise. The invariant check proves "P equals the digit count of the bound" for all 606,841 pairs.

### Mutation check

52 mutants in total; **52 of 52 caught in the quick profile (local)**; GCP: caught of 52 in the full run, to be filled.

Kept from the earlier 31: 30 (the 28 unchanged ones plus two with a changed meaning: `mod_prec_plus1` is now "tight + 1, capped at 38", and `round_neg_prec_same` is now "never widens", because `p1` instead of `p1 + 1` was the old rule's mutant). **Retired: 1.** `mod_bind_error_39` (the `%` bind error above 39 instead of 38): under decision 3 `%` has no bind error, so there is nothing to shift. Added: 22 (3 for decision 1, 5 for decision 2 besides the redefined one, 3 for decision 3 besides the three kept, 11 for decision 4). Decision 1: 3 mutants; decision 2: 6; decision 3: 6 (with `mod_sign_follows_divisor`, `mod_scale_min`, `mod_prec_plus1`); decision 4: 11. Columns: D = compared with DuckDB, I = invariants/oracles over all type pairs, E = the document's own examples, R = reference-only (decimal module, plain-integer oracle, exact property).

| mutant | caught by (quick, local) |
|---|---|
| `div_scale_plus5` | D E I |
| `div_scale_plus7` | D E I |
| `div_prec_plus1` | E I R |
| `div_scale_cap37` | D E I R |
| `div_round_half_even` | D R |
| `div_round_half_up_posinf` | D R |
| `div_round_trunc` | D R |
| `div_by_zero_gives_zero` | R only |
| `add_scale_min` | D E I |
| `add_prec_no_plus1` | D E I |
| `add_cap_cuts_scale` | D E I |
| `add_prec_max_to_min` | D E I |
| `mul_cap_threshold_19` | D E I |
| `mul_cap_scale_le18` | D E I |
| `mul_cap_or` | D E I |
| `mul_cap_value_17` | D E I |
| `mul_no_cap` | D E |
| `mul_scale_error_39` | D I |
| `mul_overflow_i64_only` | D R |
| `mul_overflow_unchecked` | D R |
| `mod_sign_follows_divisor` | D R |
| `mod_scale_min` | D E I |
| `avg_scale_plus5` | D E R |
| `avg_no_cap38` | D E R |
| `avg_round_trunc` | D R |
| `avg_round_half_even` | D R |
| `unify_prec_max` | D |
| `round_half_even` | D |
| **decision 1** `add_cap_exact_sum` (+ - at the cap compute the exact sum) | D |
| `add_fit_check_first_operand_only` | D |
| `sub_no_fit_check` (subtraction only) | D |
| **decision 2** `round_neg_prec_old_rule` (`min(p1+1,38)`) | D |
| `round_neg_prec_same` (never widens) | D |
| `round_neg_widen_without_range_test` | D |
| `round_neg_widen_without_scale_test` | D |
| `round_neg_range_strict` (`-n < p1`) | D |
| `round_neg_no_cap38` | D |
| **decision 3** `mod_prec_old_conservative` (old precision, bind error above 38) | D E I R |
| `mod_prec_max_capped` (old precision capped at 38, no bind error) | E I |
| `mod_prec_tight_minus1` | D E I R |
| `mod_prec_plus1` | D E I |
| **decision 4** `int_mul_widen_bigint` | D R |
| `int_mul_as_decimal` (`INTEGER * INTEGER` = (18,0)) | D R |
| `int_add_widen_one_step` | D R |
| `int_result_narrower` | D R |
| `int_overflow_wraps` | D R |
| `int_overflow_saturates` | D R |
| `int_div_min_by_minus1_unchecked` | D R |
| `int_div_floor` | D R |
| `int_mod_sign_follows_divisor` | D R |
| `int_slash_stays_integer` | D R |
| `intdec_bigint_as_18` | D R |

Notes. (a) `mod_prec_max_capped` (the old precision without the bind error) is not visible to DuckDB at all: DuckDB's `%` type is that very type, so only the document's examples (E) and the first-principles invariant (I: P must equal the digits of the largest remainder) catch it. That is the check that tells "tight" from "not tight"; without it this mutant would have survived. (b) The first version of the integer check skipped the DuckDB comparison after an oracle disagreement, so `int_div_floor` and `int_mod_sign_follows_divisor` showed as R only; the `continue` was removed and DuckDB now sees them too. (c) The quick profile shows that the checks can fail on each of the 52 mutants; it uses small type and value sets, so the GCP run is the authority for the full sets. A mutant that survives there means a missing check, and `verify.py` then exits with 1.

### New findings

**N1. The document's `%` example row contradicts its own formula.** Section 3 prints `DECIMAL(15,2)` and `DECIMAL(20,2)` with `%` = `(20,2)`; the formula `min(p1-s1, p2-s2) + S` gives `min(13, 18) + 2 = (15,2)`. Run: the example check (`check_doc_examples`) reports exactly this cell; the other 13 operand rows of the table agree with the formula in all five columns. DuckDB gives `DECIMAL(20,2)` there (`SELECT typeof(NULL::DECIMAL(15,2) % NULL::DECIMAL(20,2))`), so the example was probably copied from DuckDB's type, which is the wider operand type. The reference follows the formula (15,2); the first-principles check agrees: `|a % b| <= |a| < 10^13`, so 15 digits suffice. Proposed fix: print `(15,2)` in that cell.

**N2. `MIN % -1` for integers: the document says the result is 0, DuckDB raises an overflow error.** `SELECT (-128)::TINYINT % (-1)::TINYINT` gives `Out of Range Error: Overflow in division of -128 / -1`; the same for SMALLINT, INTEGER and BIGINT (`-9223372036854775808 % -1`). By section 2 ("overflow is an error") and the value of the remainder (0, which fits), the document's reading is 0. The harness follows that reading and reports it as a finding (10 cases in the quick set; counted, not a failure). `MIN // -1` is an overflow error in both. The natural kernel behaviour is DuckDB's: Rust's `i8::MIN.checked_rem(-1)` is `None` (checked, `rustc` run locally), `wrapping_rem` is 0. Proposed: state either "`MIN % -1` is an overflow error, as in DuckDB" in section 4 or list the difference in section 9.

**N3. The tight `%` type moves the 38-digit limit into the intermediate value (kernel consequence, reasoned from measured counts).** Aligning the operands to the result scale S can need far more than 38 digits even though the result is small: for `DECIMAL(1,0) % DECIMAL(38,38)` the aligned dividend has 39 digits (the type is (38,38)). Over all 606,841 pairs: in **202,540** the aligned operand needs more than 38 digits (and exceeds `i128::MAX`), up to 76 digits (`DECIMAL(38,0) % DECIMAL(38,38)`, computed over all pairs); these are exactly the pairs that were bind errors under the old rule. For pairs with both operands of at most 18 digits (35,721), P is at most 18 in all, so the output is `Decimal64`, yet the aligned operand exceeds `i64` in 11,968 of them (example `DECIMAL(1,1) % DECIMAL(18,0)`); and in 98,040 pairs over all types the output is `Decimal64` with an input of 19 or more digits (example `DECIMAL(1,0) % DECIMAL(19,0)`). A kernel cannot rescale first; it needs `a * 10^k mod b` without materialising the product (or a wide integer), or the shortcut "if the other aligned operand is above `i128` it exceeds `|a|`, so the result is `a`". The ADR 0003 kernel notes (section 10 of the rules) should say so. Not tested: no kernel exists.

**N4. ROUND n < 0: precision is DuckDB's, wider than the result needs in most cases (information, not a defect).** Over all 779 types and n = -45..-1 the rule gives a precision above the digits of the largest possible result in 33,522 of 35,055 cases (for example `ROUND(DECIMAL(10,2), -1)` is `DECIMAL(10,0)`, whose largest result has 9 digits). The rule is sufficient (0 too narrow) and equals DuckDB's; this only means the type is not tight. At the 38 cap the largest input of `DECIMAL(38,0)` rounds to 10^38 (39 digits): the checked run-time overflow, as in DuckDB (`Overflow in ROUND of DECIMAL(38)`).

**N5. Section 9's last paragraph lists `%` among the things that match DuckDB's result types and errors.** After decision 3 the result types differ (the table row above it says so); only the sign, the value and the errors still match (`typeof` over the 29 quick types: 91 equal, 592 tighter, 158 DuckDB `DOUBLE`). Proposed: drop `%` from that sentence or say "values and errors".

### Proposed changes (not made; `docs/` and ADRs were not edited)

1. Section 3, `%` example row `DECIMAL(15,2)` and `DECIMAL(20,2)`: `(15,2)` instead of `(20,2)` (N1).
2. Section 4 or 9: `MIN % -1` for integers (N2).
3. Section 10: `%` kernel notes (N3): aligned operand up to 76 digits, a `Decimal64` output with a `Decimal128` input, 11,968 pairs where the aligned operand exceeds `i64` with operands of at most 18 digits.
4. Section 9, last paragraph: `%` no longer matches DuckDB's result types (N5).
5. Section 3, `+ -` row: "2,644 cases per operator in the verification" is the count from the 24-type boundary set; the GCP run will give the count for the same set under the new rule (compare it before keeping the number in the document).
6. Not covered by anything here, so `not supported yet` or a test is needed before slice 2 relies on it: unary minus and `ABS` on `MIN` of an integer type, integer with `DOUBLE`, integer literals, `SUM` of integers, `//` of an integer with a decimal (DuckDB returns `DOUBLE`; the document says not supported yet).

### Limits of this update

- The local runs are the quick profile (29 types, small value sets) plus the full-scale pure-Python invariants and three full-profile DuckDB checks (ROUND on 12 types, TINYINT x TINYINT, integer with decimal). The full verification, including `typeof` for all 606,841 pairs and the full integer sets (3,189,796 cases per operator), has not been run.
- DuckDB 2.0.0 and x86-64 only; Python's `decimal` and plain-integer arithmetic are the only second implementations.
- `%` against DuckDB's `DOUBLE` is compared with a tolerance only where the quotient is below 2^40 (otherwise DuckDB's `fmod` of the rounded doubles says nothing about the exact remainder); for those cases only `|r| <= |b|` is checked (1,120 + 32,328 cases locally).
- The reference is my reading of the document; N1 and N2 are the two places where reading and the document's example or DuckDB differ.

## Update 2 (2026-11-04): `%` alignment bind error, `MIN % -1`, corrected examples

Quick local checks only; the full run is on the GCP VM and every number marked "GCP" is **to be filled by the GCP run**. No GCP was used here.

**What changed in the document, and in the harness.**

- N1 and N5 are fixed in the document; the example check now expects `DECIMAL(15,2) % DECIMAL(20,2)` = (15,2). All 78 example cells agree, **0 findings** (the finding mechanism stays, the list is empty). The two rows `(38,38) % (1,0)` and `(38,0) % (38,38)` are `bind error` again.
- N3 decided: `%` keeps the type `min(p1-s1, p2-s2) + S` but is a bind error (`not supported yet`) when `max(p1-s1, p2-s2) + S > 38`. `rules_ref.py` raises it; the invariants require the bind-error set to equal exactly that condition (written from the digit budget, not from the reference); a `%` that binds has P at most 38, its remainder fits (witnesses and the supremum argument of Update 1). DuckDB does not error there (`DOUBLE`): classified as a documented difference, and for every such pair the harness checks that DuckDB really returns `DOUBLE` for all queried values. The old DuckDB-`DOUBLE` tolerance comparison is gone, since Nata no longer returns a value in those cases.
- N2 decided: `MIN % -1` and `MIN // -1` are overflow errors for every integer type (when `a` is the minimum of the wider operand type; `TINYINT MIN % SMALLINT -1` is 0). The reference and the plain-integer oracle follow it; the 10 quick-set cases that were a counted finding agree with DuckDB now (`%` and `//` each: 10 both-overflow), error-category probes 180 of 180. The harness has no findings left.

**Quick profile result (local):** `python3 -I verify.py --mutants all --cache-dir <new dir>`: baseline **0 failures, 0 findings**; **56 of 56 mutants caught**, 0 survivors (9 s). Quick counts: `typeof %` over 841 pairs: 91 equal, 592 Nata type tighter than DuckDB's, 158 Nata bind error / DuckDB `DOUBLE`, **0 disagree**; boundary values `%`: 8,052 equal (6,240 with a tighter type), 602 both error by zero, 4,342 cases of the bind-error pairs checked as DuckDB `DOUBLE`; integer with decimal `%`: 4,292 equal, 388 by zero, 1,170 bind-error cases; 683 `%` pairs bind, `%` witnesses: no overflow, P reached in 627.

**Full-scale pure-Python checks (local, no DuckDB):** invariants for `%` over all 606,841 pairs: **0 violations**; bind-error pairs by the condition: **202,540**, reference disagreeing with the condition: **0**; witnesses over the 404,301 pairs that bind: no overflow, a remainder of exactly P digits reached in 362,619; for every pair that binds the aligned operand has at most 38 digits and never exceeds `i128` (0 pairs). Kernel facts for `%`: with operands of at most 18 digits (35,721 pairs) P is at most 18 and the aligned operand has at most 36 digits and exceeds `i64` in 11,968; over all bound pairs the aligned operand exceeds `i64` in 380,548 of 404,301 and a `Decimal64` output has an input of 19 or more digits in 86,070. So an `i128` intermediate is enough for `%`; `i64` is not.

**Mutants.** Retired: `mod_prec_max_capped` (the old precision without the bind error; the case is now `mod_no_alignment_bind_error`, with a tight type). Redefined: `mod_prec_old_conservative` keeps its meaning (precision `max(...)+S`) next to the alignment bind error; `int_div_min_by_minus1_unchecked` stays. Added 5: `mod_no_alignment_bind_error` (D E I), `mod_bind_error_threshold_39` (D E I), `mod_bind_error_threshold_37` (D E I R), `int_mod_min_by_minus1_returns_zero` (D R), `int_mod_by_minus1_always_overflows` (D R). 52 - 1 + 5 = 56. `mod_prec_old_conservative` is seen only by E and I (DuckDB's type is that type), as `mod_prec_max_capped` was.

**Full run (GCP), same as before:**

```
cd spikes/decimal-verify
python3 -I verify.py --full --cache-dir /tmp/nata-dv-cache-NEW --json /tmp/nata-dv-full.json
```

| item | expected from the local checks | full run |
|---|---|---|
| `typeof %` over 606,841 pairs: Nata bind error and DuckDB `DOUBLE` | 202,540 | to be filled by the GCP run |
| `typeof %`: 0 disagree; others tighter or equal | 404,301 | to be filled by the GCP run |
| integer values per operator, `MIN % -1` and `MIN // -1` both-overflow | no finding | to be filled by the GCP run |
| mutants caught | 56 of 56 | to be filled by the GCP run |
| all other full-run numbers of Update 1 | | to be filled by the GCP run |

The "Proposed changes" of Update 1 for items 1, 2 and 4 and the `%` part of item 3 are done in the document; remaining: item 5 (the `+ -` count in section 3 should be re-read against the GCP number) and item 6 (not covered).

## Full run on GCP (2026-11-04; supersedes every 'to be filled by the GCP run' above)

Run `decimal-arm64`: `python3 -I verify.py --full --mutants all` from a clean tarball on a GCP Spot VM (`n4a-highcpu-8`, 8 vCPU Neoverse-N3, Debian 12 arm64, DuckDB 2.0.0), 1,482 VM-seconds, estimated cost 0.041 USD (catalog prices; the bill was not checked), nothing left on GCP afterwards. The profile is the full one: all 779 decimal types (606,841 type pairs). **`FAILURES: none`. Mutations: 56 of 56 caught, 0 survivors.** The DuckDB queries took about 6 minutes (cached on the data disk), the mutation phase the rest. Raw output: the run's `results/decimal-full.txt` and `decimal-full.json` (not committed; the counts are copied here).

| check | result |
|---|---|
| document examples | 78, 0 failures |
| invariants for `+ - * / %` over all 606,841 type pairs | 0 violations each |
| `%`: witness remainders over the 404,301 pairs that bind | the full P-digit remainder is reached in 362,619 pairs, **0 overflows**; the bound `min(A', B'-1)` is exact in 1,600 of 1,600 brute-force cases |
| DuckDB `typeof` `+`, `-` | 606,841 equal each, 0 disagree |
| DuckDB `typeof` `*` | 505,571 equal + 101,270 both error, 0 disagree |
| DuckDB `typeof` `/` | 606,841 documented differences (DuckDB `DOUBLE`), 0 disagree |
| DuckDB `typeof` `%` | 20,463 equal, 383,838 Nata tighter (documented), **202,540 Nata bind error / DuckDB `DOUBLE` (documented; the predicted set)**, **0 disagree** |
| DuckDB `typeof` COALESCE/unify | 404,301 equal, 202,540 documented, 0 disagree |
| values, small types, `+ - * / %` | 403,225 cases per operator; `+ - *` all equal to DuckDB and to Python's `decimal`; `/` 400,050 exact-and-within-tolerance + 3,175 by zero; `%` 400,050 equal + 3,175 both error (by zero) |
| values, boundary-biased, `+ -` | 868,624 per operator: 763,424 equal, 4,356 both overflow, **100,844 'operand does not fit, exact sum fits': both error (F1 resolved)** |
| values, boundary-biased, `*` | 778,152: 674,996 equal + 103,156 both overflow |
| values, boundary-biased, `/` | 557,046: 542,390 within tolerance + 14,656 by zero |
| values, boundary-biased, `%` | 643,304: 626,308 equal, 16,996 both error (by zero), 225,320 are the documented bind-error cases (DuckDB `DOUBLE`) |
| division ties; AVG | 9,712 exact ties; 24,046 of 24,074 division cases equal to DuckDB HUGEINT arithmetic; 441 AVG groups equal, 24 exact ties, AVG types for all 779 types |
| ROUND/CAST/FLOOR/CEIL/TRUNC values | 27,738 ROUND equal, 9,868 CAST decimal-to-decimal equal, 2,752 CAST both error, 1,262 each of FLOOR/CEIL/TRUNC equal |
| function result types | 70,889 checked; ROUND `n < 0` equals DuckDB's rule (the wider-than-needed precision is information: 33,522 cases) |
| integer result types, 16 pairs x operators | 80 equal + 16 documented (`/`), 0 disagree |
| integer values per operator | 3,189,796 cases each: `+` 3,029,644 equal + 160,152 both overflow; `*` 1,570,798 equal + 1,618,998 both overflow; `%` and `//` 3,182,642 equal + 10 both overflow (`MIN` by `-1`) + 7,144 both by zero |
| integer error-category probes | 500 of 500 equal |
| integer with decimal, result types | 18,934 equal + 12,226 documented, 0 disagree |

Limits: ARM64 N3 and DuckDB 2.0.0 only; the Python reference is one reading of the document (the first-principles oracles and DuckDB are the checks against a shared misreading); casts from strings and DOUBLE, `SUM`, comparisons across widths, `TRY` and literals are not covered here.
