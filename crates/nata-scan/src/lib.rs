//! Row-group pruning for the Parquet scan (slice 3, `docs/slices/0001-parquet-scan.md`): a PURE function that decides, from the statistics of a
//! row group and a pushed-down predicate, whether the row group can be skipped. No I/O, no dependencies, no `unsafe`.
//!
//! ```
//! use nata_scan::{can_skip, CmpOp, ColumnStats, ColumnType, Predicate, RowGroupStats, Term, Value};
//!
//! // a sorted date column: this row group holds days 8000..=8100
//! let stats = RowGroupStats::new(122_880).with_column(
//!     "l_shipdate",
//!     ColumnStats::none(ColumnType::Date32).with_null_count(0).with_range(Value::Date32(8000), Value::Date32(8100)),
//! );
//! // l_shipdate >= 8766 AND l_shipdate < 9131
//! let year = Predicate::new(vec![
//!     Term::cmp("l_shipdate", CmpOp::Ge, Value::Date32(8766)),
//!     Term::cmp("l_shipdate", CmpOp::Lt, Value::Date32(9131)),
//! ]);
//! assert!(can_skip(&stats, &year).is_skip());
//! ```
//!
//! # Contract
//!
//! [`can_skip`] answers [`Decision::Skip`] ONLY when the statistics PROVE that no row of the row group satisfies the predicate, under SQL semantics
//! as DuckDB 2.0.0 implements them. Everything else is [`Decision::Keep`] with the reason: a missing statistic, a statistic that contradicts itself or
//! the row count, a NaN bound, a bound that may have been truncated, a type that is not supported, a literal of a type that is not comparable.
//! Wrongly keeping costs a read; wrongly skipping loses rows. A Skip is therefore only right if the row-level filter that runs on the kept row groups
//! has the SAME semantics (NaN above all numbers and equal to itself, `-0.0 = +0.0`, comparisons with NULL never true); Arrow's compare kernels use
//! IEEE rules for NaN and do not.
//!
//! Evidence (`bench/results/20261105-scan-pruning.md`): a property test of soundness against a row-by-row oracle (400,000 cases per release run,
//! seed replayable), a tightness test (exact statistics skip exactly when no value of `[min, max]` can match), R2 of the card on sorted dates and
//! integers, and mutants of every rule below. The oracle itself is compared with DuckDB (`tests/duckdb_oracle.rs`).
//!
//! # Rules
//!
//! `lo` and `hi` are the minimum and maximum of the row group (a lower and an upper bound of the non-NULL values), `v` the literal, `n` the row count,
//! `nulls` the null count. NaN-free means `nan_count == Some(0)`. "Check" names the DuckDB 2.0.0 facts (`FACTS` in `tests/duckdb_oracle.rs`) that
//! confirmed the semantics the rule rests on; every one of them is re-run by that test when the `duckdb` command line is present.
//!
//! | # | Term | Skip when | Because | DuckDB check |
//! |---|---|---|---|---|
//! | 0 | any | `n == 0` | no row, nothing can match (also for `IS NULL` and an empty predicate) | |
//! | 1 | `x IS NULL` | `nulls == 0` | an unknown null count (`None`) is NOT zero | |
//! | 2 | `x IS NOT NULL` | `nulls == n` | every row is NULL | |
//! | 3 | `x <op> v`, any op | `nulls == n` | `NULL <op> v` is NULL, never true, also for `<>` and a NaN literal | `(NULL::INT = 5) IS NULL`, same for `<>` and `<` |
//! | 4 | `x < v` | `lo >= v` | every value is at least `v` | |
//! | 5 | `x <= v` | `lo > v` | every value is above `v` | |
//! | 6 | `x > v` | `hi <= v` | every value is at most `v` | |
//! | 7 | `x >= v` | `hi < v` | every value is below `v` | |
//! | 8 | `x = v` | `v < lo` or `v > hi` | `v` is outside `[lo, hi]`; equal to either end keeps | |
//! | 9 | `x <> v` | `lo == hi == v`, both bounds exact | every value equals `v`; a truncated bound may stand for different values | |
//! | 10 | float `x > v`, `x >= v`, `x <> v` | the rule above AND NaN-free | Parquet leaves NaN out of `min` and `max`, so a group may hold NaN rows that they do not show; NaN is above every number, so such a row satisfies all three. `max < v` alone does NOT prove the absence of a match | `NaN > 5`, `NaN > Infinity`, `NaN <> 5`: true |
//! | 11 | float `x < v`, `x <= v`, `x = v` | the rule above (NaN rows do not matter) | a NaN row is not below, not equal to and not at most a number | `NaN < 5`, `NaN <= 5`, `NaN = 5`: false |
//! | 12 | float `x > NaN` | always (with or without statistics) | nothing is above NaN, not even NaN | `NaN > NaN` false; no number is above NaN |
//! | 13 | float `x = NaN`, `x >= NaN` | NaN-free | only a NaN row equals NaN or is at least NaN | `NaN = NaN`, `NaN >= NaN`: true; `5 >= NaN` false |
//! | 14 | float `x < NaN`, `x <= NaN`, `x <> NaN` | never | every number is below NaN, and `NaN <= NaN` | `5 < NaN` true, `NaN <= NaN` true |
//! | 15 | float, a bound that is NaN | never (Keep, unsound) | a page of NaN merged with others can leave NaN as both bounds of a group with numbers; the comparator has no order for NaN | |
//! | 16 | float, `-0.0` and `+0.0` | (no rule needed) | zeros compare as numbers: a bound of either sign stands for both, so a writer that stores `+0.0` as the minimum of a group that holds `-0.0` is harmless | `-0.0 = 0.0` true, `-0.0 < 0.0` false, `0.0 > -0.0` false |
//! | 17 | `Float32` column, `Float64` literal (or the reverse) | as for floats, on the exact widening to `f64` | the float `0.1` is 0.10000000149011612, not the double `0.1` | `0.1::FLOAT = 0.1::DOUBLE` false, `>` true |
//! | 18 | text / binary, bounds | bytewise unsigned order | for valid UTF-8 this is code point order; binary bytes are unsigned | `'é' > 'z'`, `'日' > 'é'`, `'𝄞' > '日'`, `'Z' < 'a'`, `from_hex('80') > from_hex('7f')` |
//! | 19 | text / binary, truncated MAXIMUM (`exact: false`) | rules 6, 7 and 8 (upper side) with `hi` replaced by the EXCLUSIVE bound `hi' = increment(max)`: skip when `hi' <= v` | a maximum cut to a prefix is only an upper bound on what the writer saw; a writer that forgot the increment the Parquet format asks for leaves a prefix BELOW the true maximum. Every value is below the increment of the prefix whichever the writer did. No increment exists for the empty string or for all `0xFF` bytes: then there is no upper bound | |
//! | 20 | text / binary, truncated MINIMUM | rules 4, 5, 8 with `lo` as is | a prefix is a lower bound; it is not a value that occurs, so rule 9 does not use it | |
//! | 21 | text bound that is not valid UTF-8 | never (`Bound::utf8_from_bytes` returns no bound) | a bound cut inside a character comes from a faulty writer; the rest of its statistics is suspect | |
//! | 22 | decimal column, any decimal literal | rules 4 to 9 on the exact comparison | values are compared exactly across scales and against integers; a literal with MORE fraction digits than the column is never rounded to the column's scale (`1.50 <> 1.505`). If rescaling does not fit 128 bits the magnitude decides by sign | `1.50::DECIMAL(5,2) = 1.505::DECIMAL(5,3)` false, `<` true; `= 1.5::DECIMAL(2,1)` true; `5.00::DECIMAL(5,2) = 5` true. **DuckDB 2.0.0 errors** on comparisons whose common type cannot hold the column value (`DECIMAL(38,0)` against `DECIMAL(3,1)`); Nata is exact (decimal-rules section 5), so those are not compared by the differential tests |
//! | 23 | integer columns, `Int32` or `Int64` literal | rules 4 to 9 on the exact value in 128 bits | no cast to the column width: `5_000_000_000` is not `705_032_704` | `2147483647::INT < 2147483648::BIGINT`, `0::INT = 4294967296::BIGINT` false |
//! | 24 | date, timestamp, boolean | rules 4 to 9 | integers (days, microseconds, 0/1); `false < true` | `false < true`; `DATE '1994-01-01' < DATE '1995-01-01'` |
//! | 25 | a bound with another type, a `min` above the `max`, a null count above `n`, a NaN count above the non-NULL rows, bounds on an all-NULL column, a decimal bound that does not fit its precision | never (Keep, unsound) | the statistics contradict themselves | |
//! | 26 | unknown column, unsupported column type, literal of another type | never (Keep) | nothing is known / the binder must cast: a decimal is never compared with a float (decimal-rules section 4), a date is not an integer | |
//!
//! Why a truncated maximum is only trusted up to its increment (rule 19): the test generator writes text statistics the way real writers do, cutting
//! at a random length with the increment and ALSO without it (flagged inexact in both cases). The reader is sound against both; a reader that takes the
//! truncated maximum as an exact one is rejected by the same test (`control_truncated_as_exact_reader_is_rejected`) and by a mutant.
//!
//! # Types and literals
//!
//! Literals are typed. Accepted without a cast: the column's own type; `Int32`/`Int64` against `Int32`/`Int64` columns; `Float32`/`Float64`
//! against `Float32`/`Float64` columns; any decimal (and `Int32`/`Int64` as a decimal of scale 0) against a decimal column. Everything else is
//! `Keep(TypeMismatch)`: casting is the binder's job. There is no NULL literal (`x = NULL` is folded to false before pruning). Timestamps are
//! microseconds (convert other units first).
//!
//! # Not covered
//!
//! Real Parquet statistics from writers other than the ones in the tests, page-level pruning (the column index), bloom filters, nested types and
//! lists, `IN` lists, `OR` and `NOT` (only an AND of simple terms is pushed down), `LIKE`/prefix predicates, collations other than bytewise,
//! `INT96`, unsigned integer types, time-of-day types, timestamps with a unit other than microseconds, DuckDB's infinity sentinels for dates and
//! timestamps at the extreme values of their integer types, and decoding the footer into these types.

mod predicate;
mod prune;
mod stats;

pub use predicate::{CmpOp, Predicate, Term};
pub use prune::{Decision, KeepReason, SkipReason, can_skip};
pub use stats::{Bound, ColumnStats, ColumnType, RowGroupStats, Value};
