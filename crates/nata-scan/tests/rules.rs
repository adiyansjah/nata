//! One test group per rule of the table in the crate docs: the exact boundary of each rule (the last value that is skipped and the first that is kept),
//! with the DuckDB 2.0.0 check that confirmed the semantics noted in the crate docs.

use nata_scan::{
    Bound, CmpOp::*, ColumnStats, ColumnType as T, Decision, KeepReason as K, Predicate,
    RowGroupStats, SkipReason as S, Term, Value as V, can_skip,
};

// ---------------------------------------------------------------------------------------------------- helpers

fn one(col: ColumnStats, rows: u64) -> RowGroupStats {
    RowGroupStats::new(rows).with_column("x", col)
}

fn cmp(op: nata_scan::CmpOp, v: V) -> Predicate {
    Predicate::new(vec![Term::cmp("x", op, v)])
}

fn decide(rg: &RowGroupStats, op: nata_scan::CmpOp, v: V) -> Decision {
    can_skip(rg, &cmp(op, v))
}

fn skips(rg: &RowGroupStats, op: nata_scan::CmpOp, v: V) -> bool {
    decide(rg, op, v).is_skip()
}

fn ints(min: i64, max: i64) -> RowGroupStats {
    one(
        ColumnStats::none(T::Int64)
            .with_null_count(0)
            .with_range(V::Int64(min), V::Int64(max)),
        10,
    )
}

fn floats(min: f64, max: f64, nan_count: Option<u64>) -> RowGroupStats {
    let mut c = ColumnStats::none(T::Float64)
        .with_null_count(0)
        .with_range(V::Float64(min), V::Float64(max));
    c.nan_count = nan_count;
    one(c, 10)
}

fn f(x: f64) -> V {
    V::Float64(x)
}

fn text(min: &str, max: &str) -> RowGroupStats {
    one(
        ColumnStats::none(T::Utf8)
            .with_null_count(0)
            .with_range(V::Utf8(min.into()), V::Utf8(max.into())),
        10,
    )
}

/// Text with truncated (inexact) bounds, as a writer with a truncation length leaves them.
fn text_truncated(min: &str, max: &str) -> RowGroupStats {
    let mut c = ColumnStats::none(T::Utf8).with_null_count(0);
    c.min = Some(Bound::truncated(V::Utf8(min.into())));
    c.max = Some(Bound::truncated(V::Utf8(max.into())));
    one(c, 10)
}

fn s(x: &str) -> V {
    V::Utf8(x.into())
}

// ---------------------------------------------------------------------------------------------------- integers: the boundary of every operator

#[test]
fn integer_boundaries_of_each_operator() {
    let g = ints(10, 20);
    // x < v: nothing matches when every value is >= v.
    assert!(skips(&g, Lt, V::Int64(10)));
    assert!(!skips(&g, Lt, V::Int64(11)));
    // x <= v: nothing matches when every value is > v.
    assert!(skips(&g, Le, V::Int64(9)));
    assert!(!skips(&g, Le, V::Int64(10)));
    // x > v
    assert!(skips(&g, Gt, V::Int64(20)));
    assert!(!skips(&g, Gt, V::Int64(19)));
    // x >= v
    assert!(skips(&g, Ge, V::Int64(21)));
    assert!(!skips(&g, Ge, V::Int64(20)));
    // x = v: out of [min, max] on either side, kept at both ends and inside.
    assert!(skips(&g, Eq, V::Int64(9)));
    assert!(!skips(&g, Eq, V::Int64(10)));
    assert!(!skips(&g, Eq, V::Int64(15)));
    assert!(!skips(&g, Eq, V::Int64(20)));
    assert!(skips(&g, Eq, V::Int64(21)));
    // x <> v: only a constant group can be excluded.
    assert!(!skips(&g, Ne, V::Int64(10)));
    assert!(!skips(&g, Ne, V::Int64(15)));
    let c = ints(7, 7);
    assert!(skips(&c, Ne, V::Int64(7)));
    assert!(!skips(&c, Ne, V::Int64(8)));
    assert!(!skips(&c, Eq, V::Int64(7)));
    assert!(skips(&c, Eq, V::Int64(6)));
    assert!(skips(&c, Eq, V::Int64(8)));
}

#[test]
fn skip_reason_is_out_of_range() {
    assert_eq!(
        decide(&ints(10, 20), Gt, V::Int64(20)),
        Decision::Skip(S::OutOfRange)
    );
    assert_eq!(
        decide(&ints(10, 20), Gt, V::Int64(19)),
        Decision::Keep(K::MayMatch)
    );
}

#[test]
fn integer_extremes_do_not_overflow() {
    // The bounds and literals sit at the ends of i64: nothing may be added to or subtracted from them.
    let all = ints(i64::MIN, i64::MAX);
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        assert!(!skips(&all, op, V::Int64(0)), "{op:?}");
    }
    assert!(skips(&all, Lt, V::Int64(i64::MIN)), "x < MIN is empty");
    assert!(!skips(&all, Le, V::Int64(i64::MIN)));
    assert!(skips(&all, Gt, V::Int64(i64::MAX)), "x > MAX is empty");
    assert!(!skips(&all, Ge, V::Int64(i64::MAX)));
    let lo = ints(i64::MIN, i64::MIN);
    assert!(skips(&lo, Lt, V::Int64(i64::MIN)));
    assert!(!skips(&lo, Le, V::Int64(i64::MIN)));
    assert!(!skips(&lo, Lt, V::Int64(i64::MIN + 1)));
    assert!(skips(&lo, Gt, V::Int64(i64::MIN)));
    assert!(skips(&lo, Ne, V::Int64(i64::MIN)));
    assert!(skips(&lo, Eq, V::Int64(i64::MAX)));
    let hi = ints(i64::MAX, i64::MAX);
    assert!(skips(&hi, Lt, V::Int64(i64::MAX)));
    assert!(!skips(&hi, Ge, V::Int64(i64::MAX)));
    assert!(skips(&hi, Le, V::Int64(i64::MAX - 1)));
    assert!(skips(&hi, Eq, V::Int64(i64::MIN)));
}

#[test]
fn int32_column_with_wide_literals_is_compared_exactly() {
    let g = one(
        ColumnStats::none(T::Int32)
            .with_null_count(0)
            .with_range(V::Int32(0), V::Int32(10)),
        5,
    );
    // A literal outside the range of i32 is not truncated to 32 bits: 5_000_000_000 as i32 would be 705_032_704.
    assert!(
        skips(&g, Gt, V::Int64(5_000_000_000)),
        "no int32 is above 5e9"
    );
    assert!(skips(&g, Eq, V::Int64(5_000_000_000)));
    assert!(!skips(&g, Lt, V::Int64(5_000_000_000)));
    assert!(
        skips(&g, Lt, V::Int64(i64::MIN)),
        "nothing is below i64::MIN"
    );
    assert!(skips(&g, Le, V::Int64(i64::MIN)));
    assert!(!skips(&g, Gt, V::Int64(i64::MIN)));
    assert!(
        skips(&g, Eq, V::Int64(i64::from(u32::MAX) + 1)),
        "4294967296 is 0 as a wrapped i32 and would match the minimum"
    );
    assert!(!skips(&g, Eq, V::Int32(5)));
    // The other way round: a narrow literal against a wide column.
    let h = ints(i64::from(i32::MAX) + 1, i64::from(i32::MAX) + 10);
    assert!(skips(&h, Le, V::Int32(i32::MAX)));
    assert!(skips(&h, Eq, V::Int32(i32::MIN)));
}

// ---------------------------------------------------------------------------------------------------- empty, one row, all-NULL

#[test]
fn empty_row_group_is_skipped_for_every_predicate() {
    let e = RowGroupStats::new(0).with_column("x", ColumnStats::none(T::Int64));
    assert_eq!(
        can_skip(&e, &Predicate::default()),
        Decision::Skip(S::EmptyRowGroup)
    );
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        assert_eq!(
            decide(&e, op, V::Int64(1)),
            Decision::Skip(S::EmptyRowGroup)
        );
    }
    assert_eq!(
        can_skip(&e, &Predicate::new(vec![Term::is_null("x")])),
        Decision::Skip(S::EmptyRowGroup)
    );
    assert_eq!(
        can_skip(&e, &Predicate::new(vec![Term::is_not_null("x")])),
        Decision::Skip(S::EmptyRowGroup)
    );
    assert_eq!(
        can_skip(&e, &Predicate::new(vec![Term::is_null("nope")])),
        Decision::Skip(S::EmptyRowGroup)
    );
    // ... and a non-empty group with no predicate is kept.
    assert_eq!(
        can_skip(&ints(1, 2), &Predicate::default()),
        Decision::Keep(K::EmptyPredicate)
    );
}

#[test]
fn all_null_row_group_with_each_operator() {
    let g = one(ColumnStats::none(T::Int64).with_null_count(5), 5);
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        assert_eq!(
            decide(&g, op, V::Int64(1)),
            Decision::Skip(S::AllNull),
            "{op:?}: NULL <op> v is never true"
        );
    }
    assert_eq!(
        can_skip(&g, &Predicate::new(vec![Term::is_not_null("x")])),
        Decision::Skip(S::AllNull)
    );
    assert_eq!(
        can_skip(&g, &Predicate::new(vec![Term::is_null("x")])),
        Decision::Keep(K::MayMatch),
        "every row is NULL"
    );
    // The same for the other types, and for NaN and mismatched-width literals.
    let fl = one(ColumnStats::none(T::Float64).with_null_count(3), 3);
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        assert!(skips(&fl, op, f(f64::NAN)), "{op:?}");
        assert!(skips(&fl, op, f(0.0)), "{op:?}");
    }
    let st = one(ColumnStats::none(T::Utf8).with_null_count(2), 2);
    assert!(skips(&st, Ne, s("a")));
    // Without the null count the group is not known to be all NULL.
    let unknown = one(ColumnStats::none(T::Int64), 5);
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        assert!(!skips(&unknown, op, V::Int64(1)), "{op:?}");
    }
}

#[test]
fn all_null_with_bounds_is_a_contradiction_and_kept() {
    let g = one(
        ColumnStats::none(T::Int64)
            .with_null_count(5)
            .with_range(V::Int64(1), V::Int64(2)),
        5,
    );
    assert_eq!(
        decide(&g, Gt, V::Int64(100)),
        Decision::Keep(K::UnsoundStatistics)
    );
    assert_eq!(
        can_skip(&g, &Predicate::new(vec![Term::is_not_null("x")])),
        Decision::Keep(K::UnsoundStatistics)
    );
    assert_eq!(
        can_skip(&g, &Predicate::new(vec![Term::is_null("x")])),
        Decision::Keep(K::MayMatch)
    );
}

#[test]
fn is_null_and_is_not_null_use_the_null_count() {
    let p_null = Predicate::new(vec![Term::is_null("x")]);
    let p_not = Predicate::new(vec![Term::is_not_null("x")]);
    let with = |n: Option<u64>| {
        let mut c = ColumnStats::none(T::Int64);
        c.null_count = n;
        one(c, 4)
    };
    assert_eq!(
        can_skip(&with(Some(0)), &p_null),
        Decision::Skip(S::NoNulls)
    );
    assert!(!can_skip(&with(Some(1)), &p_null).is_skip());
    assert!(!can_skip(&with(Some(4)), &p_null).is_skip());
    assert_eq!(
        can_skip(&with(None), &p_null),
        Decision::Keep(K::NoStatistics),
        "an unknown null count is not zero"
    );
    assert!(!can_skip(&with(Some(0)), &p_not).is_skip());
    assert!(!can_skip(&with(Some(3)), &p_not).is_skip());
    assert_eq!(can_skip(&with(Some(4)), &p_not), Decision::Skip(S::AllNull));
    assert!(!can_skip(&with(None), &p_not).is_skip());
}

#[test]
fn one_row_group() {
    let g = one(
        ColumnStats::none(T::Int32)
            .with_null_count(0)
            .with_range(V::Int32(7), V::Int32(7)),
        1,
    );
    assert!(!skips(&g, Eq, V::Int32(7)));
    assert!(skips(&g, Eq, V::Int32(8)));
    assert!(skips(&g, Ne, V::Int32(7)));
    assert!(!skips(&g, Ne, V::Int32(8)));
    assert!(skips(&g, Lt, V::Int32(7)));
    assert!(!skips(&g, Le, V::Int32(7)));
    assert!(skips(&g, Gt, V::Int32(7)));
    assert!(!skips(&g, Ge, V::Int32(7)));
    // The one row is NULL.
    let n = one(ColumnStats::none(T::Int32).with_null_count(1), 1);
    assert!(skips(&n, Ne, V::Int32(7)));
    assert!(!can_skip(&n, &Predicate::new(vec![Term::is_null("x")])).is_skip());
}

// ---------------------------------------------------------------------------------------------------- missing and unsound statistics

#[test]
fn missing_statistics_never_skip() {
    let none = one(ColumnStats::none(T::Int64).with_null_count(0), 10);
    for op in [Lt, Le, Gt, Ge, Eq, Ne] {
        for v in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(
                decide(&none, op, V::Int64(v)),
                Decision::Keep(K::NoStatistics),
                "{op:?} {v}: an absent statistic is not zero"
            );
        }
    }
}

#[test]
fn one_sided_statistics_are_used_on_their_side_only() {
    let mut only_min = ColumnStats::none(T::Int64).with_null_count(0);
    only_min.min = Some(Bound::exact(V::Int64(10)));
    let g = one(only_min, 10);
    assert!(skips(&g, Lt, V::Int64(10)));
    assert!(skips(&g, Eq, V::Int64(9)));
    assert!(!skips(&g, Gt, V::Int64(1_000_000)), "no upper bound");
    assert!(!skips(&g, Eq, V::Int64(1_000_000)));
    let mut only_max = ColumnStats::none(T::Int64).with_null_count(0);
    only_max.max = Some(Bound::exact(V::Int64(10)));
    let g = one(only_max, 10);
    assert!(skips(&g, Gt, V::Int64(10)));
    assert!(skips(&g, Eq, V::Int64(11)));
    assert!(!skips(&g, Lt, V::Int64(-1_000_000)), "no lower bound");
    assert!(!skips(&g, Ne, V::Int64(10)), "a constant needs both bounds");
}

#[test]
fn statistics_without_a_null_count_still_prune_comparisons() {
    let mut c = ColumnStats::none(T::Int64).with_range(V::Int64(10), V::Int64(20));
    c.null_count = None;
    let g = one(c, 10);
    assert!(skips(&g, Gt, V::Int64(20)));
    assert!(!skips(&g, Gt, V::Int64(19)));
}

#[test]
fn contradictory_statistics_are_kept() {
    // min above max
    assert_eq!(
        decide(&ints(20, 10), Gt, V::Int64(100)),
        Decision::Keep(K::UnsoundStatistics)
    );
    // a null count above the row count
    let g = one(
        ColumnStats::none(T::Int64)
            .with_null_count(11)
            .with_range(V::Int64(1), V::Int64(2)),
        10,
    );
    assert_eq!(
        decide(&g, Gt, V::Int64(100)),
        Decision::Keep(K::UnsoundStatistics)
    );
    assert_eq!(
        can_skip(&g, &Predicate::new(vec![Term::is_null("x")])),
        Decision::Keep(K::UnsoundStatistics)
    );
    // a bound of the wrong type
    let mut c = ColumnStats::none(T::Int64).with_null_count(0);
    c.min = Some(Bound::exact(V::Float64(1.0)));
    assert_eq!(
        decide(&one(c, 3), Gt, V::Int64(100)),
        Decision::Keep(K::UnsoundStatistics)
    );
    let mut c = ColumnStats::none(T::Int32).with_null_count(0);
    c.max = Some(Bound::exact(V::Int64(5)));
    assert_eq!(
        decide(&one(c, 3), Gt, V::Int32(100)),
        Decision::Keep(K::UnsoundStatistics)
    );
}

#[test]
fn unknown_column_unsupported_type_and_mismatched_literal_are_kept() {
    let g = ints(1, 2);
    let p = Predicate::new(vec![Term::cmp("nope", Gt, V::Int64(100))]);
    assert_eq!(can_skip(&g, &p), Decision::Keep(K::UnknownColumn));
    let u = one(ColumnStats::none(T::Unsupported).with_null_count(0), 3);
    assert_eq!(
        decide(&u, Gt, V::Int64(1)),
        Decision::Keep(K::UnsupportedType)
    );
    assert_eq!(
        can_skip(&u, &Predicate::new(vec![Term::is_null("x")])),
        Decision::Keep(K::UnsupportedType)
    );
    // Literal of another type: the binder must cast.
    assert_eq!(decide(&g, Gt, f(1.0)), Decision::Keep(K::TypeMismatch));
    assert_eq!(decide(&g, Gt, s("a")), Decision::Keep(K::TypeMismatch));
    assert_eq!(
        decide(&g, Gt, V::Bool(true)),
        Decision::Keep(K::TypeMismatch)
    );
    assert_eq!(
        decide(&g, Gt, V::Date32(1)),
        Decision::Keep(K::TypeMismatch)
    );
    let d = one(
        ColumnStats::none(T::Date32)
            .with_null_count(0)
            .with_range(V::Date32(1), V::Date32(2)),
        3,
    );
    assert_eq!(
        decide(&d, Gt, V::Int32(100)),
        Decision::Keep(K::TypeMismatch),
        "a date is not an integer"
    );
    assert_eq!(
        decide(&d, Gt, V::TimestampMicros(100)),
        Decision::Keep(K::TypeMismatch)
    );
    let b = one(
        ColumnStats::none(T::Binary)
            .with_null_count(0)
            .with_range(V::Binary(vec![1]), V::Binary(vec![2])),
        3,
    );
    assert_eq!(
        decide(&b, Gt, s("zzz")),
        Decision::Keep(K::TypeMismatch),
        "text is not binary"
    );
    // An all-NULL group with a literal of the wrong type is not skipped either: the term is a bind error, not a proof.
    let n = one(ColumnStats::none(T::Int64).with_null_count(3), 3);
    assert_eq!(decide(&n, Gt, s("a")), Decision::Keep(K::TypeMismatch));
}

// ---------------------------------------------------------------------------------------------------- floating point

#[test]
fn float_group_with_unknown_nan_rows() {
    // Parquet leaves NaN out of min and max, so a group [1, 5] may hold NaN rows. NaN is above every number (DuckDB).
    let g = floats(1.0, 5.0, None);
    // `<`, `<=` and `=` cannot match a NaN row: pruned as for any number.
    assert!(skips(&g, Lt, f(1.0)));
    assert!(!skips(&g, Lt, f(1.5)));
    assert!(skips(&g, Le, f(0.5)));
    assert!(!skips(&g, Le, f(1.0)));
    assert!(skips(&g, Eq, f(0.5)));
    assert!(skips(&g, Eq, f(6.0)));
    assert!(!skips(&g, Eq, f(5.0)));
    // `>`, `>=` and `<>` match a NaN row for every number: max < v does NOT prove the absence of a match.
    assert_eq!(decide(&g, Gt, f(5.0)), Decision::Keep(K::NanRows));
    assert_eq!(decide(&g, Gt, f(100.0)), Decision::Keep(K::NanRows));
    assert_eq!(decide(&g, Ge, f(6.0)), Decision::Keep(K::NanRows));
    assert_eq!(decide(&g, Ne, f(100.0)), Decision::Keep(K::NanRows));
    let c = floats(3.0, 3.0, None);
    assert!(
        !skips(&c, Ne, f(3.0)),
        "a constant group may still hold NaN rows, which are <> 3"
    );
}

#[test]
fn float_group_without_nan_rows() {
    let g = floats(1.0, 5.0, Some(0));
    assert!(skips(&g, Gt, f(5.0)));
    assert!(!skips(&g, Gt, f(4.0)));
    assert!(skips(&g, Ge, f(5.5)));
    assert!(!skips(&g, Ge, f(5.0)));
    assert!(!skips(&g, Ne, f(3.0)));
    let c = floats(3.0, 3.0, Some(0));
    assert!(skips(&c, Ne, f(3.0)));
    assert!(!skips(&c, Ne, f(4.0)));
    // A NaN count above zero is no better than an unknown one.
    let some = floats(1.0, 5.0, Some(2));
    assert!(!skips(&some, Gt, f(100.0)));
    assert!(skips(&some, Lt, f(1.0)));
}

#[test]
fn float_infinities_are_ordinary_numbers() {
    let g = floats(f64::NEG_INFINITY, 1.0, Some(0));
    assert!(skips(&g, Lt, f(f64::NEG_INFINITY)));
    assert!(!skips(&g, Le, f(f64::NEG_INFINITY)));
    assert!(skips(&g, Gt, f(1.0)));
    let h = floats(1.0, f64::INFINITY, Some(0));
    assert!(skips(&h, Gt, f(f64::INFINITY)));
    assert!(!skips(&h, Ge, f(f64::INFINITY)));
    assert!(skips(&h, Eq, f(f64::NEG_INFINITY)));
    assert!(!skips(&h, Eq, f(f64::INFINITY)));
}

#[test]
fn zeros_are_equal_whatever_their_sign() {
    // DuckDB: -0.0 = 0.0, -0.0 < 0.0 is false. A bound of either sign stands for both zeros.
    for (lo, hi) in [(0.0, 0.0), (-0.0, -0.0), (-0.0, 0.0), (0.0, -0.0)] {
        let g = floats(lo, hi, Some(0));
        for lit in [0.0, -0.0] {
            assert!(!skips(&g, Eq, f(lit)), "[{lo:?},{hi:?}] = {lit:?}");
            assert!(!skips(&g, Le, f(lit)), "[{lo:?},{hi:?}] <= {lit:?}");
            assert!(!skips(&g, Ge, f(lit)), "[{lo:?},{hi:?}] >= {lit:?}");
            assert!(skips(&g, Lt, f(lit)), "[{lo:?},{hi:?}] < {lit:?}");
            assert!(skips(&g, Gt, f(lit)), "[{lo:?},{hi:?}] > {lit:?}");
            assert!(
                skips(&g, Ne, f(lit)),
                "[{lo:?},{hi:?}] <> {lit:?}: every row is a zero"
            );
        }
    }
    // The group of the writer that gives +0.0 as the minimum although -0.0 occurs: [+0.0, 3].
    let g = floats(0.0, 3.0, Some(0));
    assert!(!skips(&g, Le, f(-0.0)));
    assert!(!skips(&g, Eq, f(-0.0)));
    let g = floats(-3.0, -0.0, Some(0));
    assert!(!skips(&g, Ge, f(0.0)));
    assert!(!skips(&g, Eq, f(0.0)));
}

#[test]
fn nan_literal() {
    let unknown = floats(1.0, 5.0, None);
    let nan_free = floats(1.0, 5.0, Some(0));
    // Nothing is above NaN, not even NaN: always empty, with or without statistics.
    assert_eq!(
        decide(&unknown, Gt, f(f64::NAN)),
        Decision::Skip(S::Unsatisfiable)
    );
    assert_eq!(
        decide(&one(ColumnStats::none(T::Float64), 5), Gt, f(f64::NAN)),
        Decision::Skip(S::Unsatisfiable)
    );
    // Only a NaN row equals NaN or is at least NaN.
    assert_eq!(
        decide(&unknown, Eq, f(f64::NAN)),
        Decision::Keep(K::NanRows)
    );
    assert_eq!(
        decide(&unknown, Ge, f(f64::NAN)),
        Decision::Keep(K::NanRows)
    );
    assert_eq!(decide(&nan_free, Eq, f(f64::NAN)), Decision::Skip(S::NoNan));
    assert_eq!(decide(&nan_free, Ge, f(f64::NAN)), Decision::Skip(S::NoNan));
    // Every number is below NaN and NaN <= NaN: these match whatever the NaN count.
    for g in [&unknown, &nan_free] {
        assert!(!skips(g, Lt, f(f64::NAN)));
        assert!(!skips(g, Le, f(f64::NAN)));
        assert!(!skips(g, Ne, f(f64::NAN)));
    }
    // Float32 column, the same.
    let f32g = one(
        ColumnStats::none(T::Float32)
            .with_null_count(0)
            .with_nan_count(0)
            .with_range(V::Float32(1.0), V::Float32(2.0)),
        4,
    );
    assert!(skips(&f32g, Eq, V::Float32(f32::NAN)));
    assert!(skips(&f32g, Gt, V::Float32(f32::NAN)));
    assert!(!skips(&f32g, Lt, V::Float32(f32::NAN)));
    // A NaN literal against an integer column is a type error, not a proof.
    assert_eq!(
        decide(&ints(1, 2), Gt, f(f64::NAN)),
        Decision::Keep(K::TypeMismatch)
    );
}

#[test]
fn nan_in_the_bounds_is_not_trusted() {
    // A page of NaN merged with others leaves NaN as minimum and maximum: the group may hold numbers on both sides of any literal.
    for (lo, hi) in [(f64::NAN, f64::NAN), (f64::NAN, 5.0), (1.0, f64::NAN)] {
        let g = floats(lo, hi, None);
        for op in [Lt, Le, Gt, Ge, Eq, Ne] {
            for v in [-10.0, 0.0, 3.0, 100.0] {
                assert_eq!(
                    decide(&g, op, f(v)),
                    Decision::Keep(K::UnsoundStatistics),
                    "[{lo},{hi}] {op:?} {v}"
                );
            }
        }
        let g = floats(lo, hi, Some(0));
        assert!(!skips(&g, Lt, f(0.0)));
        assert!(!skips(&g, Eq, f(100.0)));
    }
}

#[test]
fn float_nan_count_above_the_row_count_is_contradictory() {
    let g = one(
        ColumnStats::none(T::Float64)
            .with_null_count(4)
            .with_nan_count(7)
            .with_range(f_v(1.0), f_v(2.0)),
        10,
    );
    assert_eq!(
        decide(&g, Lt, f(0.0)),
        Decision::Keep(K::UnsoundStatistics),
        "7 NaN rows cannot fit in 6 non-NULL rows"
    );
    let ok = one(
        ColumnStats::none(T::Float64)
            .with_null_count(4)
            .with_nan_count(6)
            .with_range(f_v(1.0), f_v(2.0)),
        10,
    );
    assert!(skips(&ok, Lt, f(0.0)));
}

fn f_v(x: f64) -> V {
    V::Float64(x)
}

#[test]
fn float32_columns_compare_as_the_double_they_widen_to() {
    // DuckDB: 0.1::FLOAT = 0.1::DOUBLE is false (the float is 0.10000000149011612).
    let g = one(
        ColumnStats::none(T::Float32)
            .with_null_count(0)
            .with_nan_count(0)
            .with_range(V::Float32(0.1), V::Float32(0.1)),
        4,
    );
    assert!(skips(&g, Eq, f(0.1)), "0.1f64 is not the float 0.1");
    assert!(skips(&g, Lt, f(f64::from(0.1f32))) && !skips(&g, Le, f(f64::from(0.1f32))));
    assert!(!skips(&g, Eq, V::Float32(0.1)));
    assert!(!skips(&g, Eq, f(f64::from(0.1f32))));
    assert!(skips(&g, Ne, V::Float32(0.1)));
    assert!(skips(&g, Gt, f(0.2)));
    // and a float64 column against a float32 literal
    let h = one(
        ColumnStats::none(T::Float64)
            .with_null_count(0)
            .with_nan_count(0)
            .with_range(f_v(0.1), f_v(0.1)),
        4,
    );
    assert!(
        skips(&h, Eq, V::Float32(0.1)),
        "the column holds the double 0.1; the literal is the float 0.1"
    );
}

// ---------------------------------------------------------------------------------------------------- decimals

fn dec(unscaled: i128, precision: u8, scale: u8) -> V {
    V::Decimal {
        unscaled,
        precision,
        scale,
    }
}

fn decimals(precision: u8, scale: u8, min: i128, max: i128) -> RowGroupStats {
    one(
        ColumnStats::none(T::Decimal { precision, scale })
            .with_null_count(0)
            .with_range(dec(min, precision, scale), dec(max, precision, scale)),
        10,
    )
}

#[test]
fn decimal_literal_with_more_fraction_digits_than_the_column() {
    // DuckDB: 1.50::DECIMAL(5,2) = 1.505::DECIMAL(5,3) is false and < is true: the comparison is exact; the literal is never rounded to the column's scale.
    let c = decimals(5, 2, 150, 150); // exactly 1.50
    assert!(
        skips(&c, Eq, dec(1505, 4, 3)),
        "1.50 <> 1.505, not rounded to 1.51 or 1.50"
    );
    assert!(skips(&c, Ge, dec(1505, 4, 3)));
    assert!(skips(&c, Gt, dec(1505, 4, 3)));
    assert!(!skips(&c, Lt, dec(1505, 4, 3)));
    assert!(!skips(&c, Le, dec(1505, 4, 3)));
    assert!(!skips(&c, Ne, dec(1505, 4, 3)));
    assert!(skips(&c, Ne, dec(1500, 4, 3)), "1.500 equals 1.50");
    assert!(skips(&c, Ne, dec(15, 2, 1)), "1.5 equals 1.50");
    assert!(!skips(&c, Eq, dec(15, 2, 1)));
    // 1.495 is below 1.50
    assert!(skips(&c, Le, dec(1495, 4, 3)));
    assert!(skips(&c, Lt, dec(1495, 4, 3)));
    assert!(skips(&c, Eq, dec(1495, 4, 3)));
    // A range [1.50, 2.00] contains 1.505
    let r = decimals(5, 2, 150, 200);
    assert!(!skips(&r, Eq, dec(1505, 4, 3)));
    assert!(skips(&r, Eq, dec(2001, 4, 3)));
    assert!(skips(&r, Gt, dec(2000, 4, 3)));
    assert!(!skips(&r, Ge, dec(2000, 4, 3)));
    // A literal with fewer fraction digits, and an integer literal
    assert!(skips(&r, Gt, dec(2, 1, 0)));
    assert!(!skips(&r, Ge, dec(2, 1, 0)));
    assert!(skips(&r, Gt, V::Int32(2)));
    assert!(skips(&r, Lt, V::Int64(1)));
    assert!(!skips(&r, Lt, V::Int64(2)));
    assert!(skips(&r, Eq, V::Int64(3)));
    assert!(!skips(&r, Eq, V::Int64(2)));
}

#[test]
fn decimal_rescaling_does_not_overflow() {
    // DECIMAL(38,0) holds up to 10^38 - 1; the literal has scale 20; the column's values times 10^20 do not fit 128 bits.
    let c = decimals(38, 0, 1, 10i128.pow(37));
    let lit = dec(5 * 10i128.pow(37), 38, 20); // 5e17
    assert!(
        !skips(&c, Gt, lit.clone()),
        "the column reaches 1e37, far above 5e17"
    );
    assert!(!skips(&c, Ge, lit.clone()));
    assert!(!skips(&c, Eq, lit.clone()), "5e17 lies inside [1, 1e37]");
    assert!(!skips(&c, Lt, lit.clone()), "the column starts at 1");
    let neg = decimals(38, 0, -(10i128.pow(37)), -1);
    assert!(
        !skips(&neg, Lt, lit.clone()),
        "negative column, positive literal: some value is below"
    );
    assert!(
        skips(&neg, Gt, lit.clone()),
        "every negative value is below 5e17"
    );
    assert!(skips(&neg, Eq, lit.clone()));
    let big = decimals(38, 0, 10i128.pow(37), 10i128.pow(38) - 1);
    assert!(
        skips(&big, Lt, lit.clone()),
        "every value (>= 1e37) is above 5e17: the rescaled column bound overflows"
    );
    assert!(skips(&big, Eq, lit.clone()));
    assert!(skips(&big, Le, lit.clone()));
    assert!(!skips(&big, Gt, lit));
    // The same with the column at the larger scale and the literal at scale 0 and a huge value
    let tiny = decimals(38, 38, -1, 1); // +-1e-38
    assert!(skips(&tiny, Eq, dec(10i128.pow(37), 38, 0)));
    assert!(skips(&tiny, Ge, dec(10i128.pow(37), 38, 0)));
    assert!(!skips(&tiny, Le, dec(10i128.pow(37), 38, 0)));
    assert!(skips(&tiny, Le, dec(-(10i128.pow(37)), 38, 0)));
    assert!(!skips(&tiny, Ge, dec(-(10i128.pow(37)), 38, 0)));
}

#[test]
fn decimal_extremes_and_validation() {
    let max = 10i128.pow(38) - 1;
    let g = decimals(38, 0, -max, max);
    assert!(!skips(&g, Eq, dec(max, 38, 0)));
    assert!(skips(&g, Gt, dec(max, 38, 0)));
    assert!(skips(&g, Lt, dec(-max, 38, 0)));
    // A literal that does not fit its stated precision, or a malformed type, is a type error and never prunes.
    assert_eq!(
        decide(&g, Gt, dec(1000, 3, 0)),
        Decision::Keep(K::TypeMismatch)
    );
    assert_eq!(
        decide(&g, Gt, dec(1, 39, 0)),
        Decision::Keep(K::TypeMismatch)
    );
    assert_eq!(
        decide(&g, Gt, dec(1, 3, 4)),
        Decision::Keep(K::TypeMismatch)
    );
    assert_eq!(
        decide(&g, Gt, f(1.0)),
        Decision::Keep(K::TypeMismatch),
        "decimal against DOUBLE is a bind error (decimal-rules section 4)"
    );
    let bad_col = one(
        ColumnStats::none(T::Decimal {
            precision: 39,
            scale: 0,
        })
        .with_null_count(0),
        3,
    );
    assert_eq!(
        decide(&bad_col, Gt, dec(1, 1, 0)),
        Decision::Keep(K::UnsupportedType)
    );
    // A bound with another precision or scale than the column is not trusted.
    let mut c = ColumnStats::none(T::Decimal {
        precision: 5,
        scale: 2,
    })
    .with_null_count(0);
    c.min = Some(Bound::exact(dec(100, 5, 3)));
    c.max = Some(Bound::exact(dec(200, 5, 2)));
    assert_eq!(
        decide(&one(c, 3), Gt, dec(10000, 5, 2)),
        Decision::Keep(K::UnsoundStatistics)
    );
    let mut c = ColumnStats::none(T::Decimal {
        precision: 5,
        scale: 2,
    })
    .with_null_count(0);
    c.max = Some(Bound::exact(dec(100000, 6, 2)));
    assert_eq!(
        decide(&one(c, 3), Gt, dec(10000, 5, 2)),
        Decision::Keep(K::UnsoundStatistics)
    );
    // A float column against a decimal literal
    let fl = floats(1.0, 2.0, Some(0));
    assert_eq!(
        decide(&fl, Gt, dec(1, 1, 0)),
        Decision::Keep(K::TypeMismatch)
    );
    assert_eq!(
        decide(&fl, Gt, V::Int64(1)),
        Decision::Keep(K::TypeMismatch)
    );
}

// ---------------------------------------------------------------------------------------------------- dates, timestamps, booleans

#[test]
fn year_range_on_dates() {
    // the shape of the card: d >= a AND d < b
    let g = |min, max| {
        one(
            ColumnStats::none(T::Date32)
                .with_null_count(0)
                .with_range(V::Date32(min), V::Date32(max)),
            100,
        )
    };
    let p = Predicate::new(vec![
        Term::cmp("x", Ge, V::Date32(8766)),
        Term::cmp("x", Lt, V::Date32(9131)),
    ]); // 1994-01-01 .. 1995-01-01
    assert!(
        can_skip(&g(8000, 8765), &p).is_skip(),
        "ends the day before"
    );
    assert!(
        !can_skip(&g(8000, 8766), &p).is_skip(),
        "touches the first day"
    );
    assert!(
        !can_skip(&g(9130, 9500), &p).is_skip(),
        "touches the last day"
    );
    assert!(
        can_skip(&g(9131, 9500), &p).is_skip(),
        "starts on the excluded end"
    );
    assert!(!can_skip(&g(8000, 9500), &p).is_skip(), "spans the range");
    assert!(!can_skip(&g(8800, 8900), &p).is_skip(), "inside");
    // extremes
    let all = g(i32::MIN, i32::MAX);
    assert!(!skips(&all, Ge, V::Date32(i32::MAX)));
    assert!(skips(&all, Gt, V::Date32(i32::MAX)));
    assert!(skips(&all, Lt, V::Date32(i32::MIN)));
}

#[test]
fn timestamps_and_booleans() {
    let t = one(
        ColumnStats::none(T::TimestampMicros)
            .with_null_count(0)
            .with_range(V::TimestampMicros(i64::MIN), V::TimestampMicros(i64::MAX)),
        3,
    );
    assert!(skips(&t, Gt, V::TimestampMicros(i64::MAX)));
    assert!(!skips(&t, Ge, V::TimestampMicros(i64::MAX)));
    assert!(skips(&t, Lt, V::TimestampMicros(i64::MIN)));
    let b = |lo, hi| {
        one(
            ColumnStats::none(T::Bool)
                .with_null_count(0)
                .with_range(V::Bool(lo), V::Bool(hi)),
            5,
        )
    };
    assert!(skips(&b(false, false), Eq, V::Bool(true)));
    assert!(!skips(&b(false, false), Eq, V::Bool(false)));
    assert!(skips(&b(false, false), Gt, V::Bool(false)));
    assert!(skips(&b(false, false), Ge, V::Bool(true)));
    assert!(skips(&b(true, true), Ne, V::Bool(true)));
    assert!(skips(&b(true, true), Lt, V::Bool(false)));
    assert!(
        skips(&b(true, true), Lt, V::Bool(true)),
        "x < true is false for x = true"
    );
    assert!(!skips(&b(false, true), Ne, V::Bool(true)));
    assert!(!skips(&b(false, true), Eq, V::Bool(true)));
}

// ---------------------------------------------------------------------------------------------------- text and binary

#[test]
fn text_with_exact_bounds_is_compared_bytewise() {
    let g = text("apple", "banana");
    assert!(skips(&g, Lt, s("apple")));
    assert!(!skips(&g, Le, s("apple")));
    assert!(skips(&g, Gt, s("banana")));
    assert!(!skips(&g, Ge, s("banana")));
    assert!(
        skips(&g, Gt, s("bananaa")) && skips(&g, Ge, s("bananaa")),
        "longer than the maximum"
    );
    assert!(skips(&g, Eq, s("cherry")));
    assert!(
        skips(&g, Eq, s("appl")),
        "a prefix of the minimum is below it"
    );
    assert!(!skips(&g, Eq, s("apricot")));
    assert!(
        skips(&g, Eq, s("")),
        "the empty string is below every other"
    );
    assert!(skips(&g, Le, s("")));
    assert!(!skips(&g, Gt, s("")));
}

#[test]
fn text_order_is_by_bytes_for_non_ascii() {
    // e acute is C3 A9, the CJK character E6 97 A5, the clef F0 9D 84 9E: bytewise order is code point order.
    let g = text("é", "日");
    assert!(skips(&g, Lt, s("é")));
    assert!(skips(&g, Gt, s("日")));
    assert!(skips(&g, Eq, s("z")), "z (7A) is below C3");
    assert!(skips(&g, Eq, s("𝄞")), "F0 is above E6");
    assert!(!skips(&g, Eq, s("ü")), "C3 BC lies between");
    assert!(!skips(&g, Eq, s("日")));
    let h = text("a", "日");
    assert!(skips(&h, Gt, s("𝄞")));
    assert!(
        skips(&h, Gt, s("日\u{0}")),
        "one byte longer than the maximum"
    );
    assert!(
        skips(&h, Gt, s("\u{65E5}")),
        "nothing is above the maximum itself"
    );
    assert!(!skips(&h, Ge, s("\u{65E5}")));
    // a constant column
    let c = text("é", "é");
    assert!(skips(&c, Ne, s("é")));
    assert!(!skips(&c, Ne, s("e")));
}

#[test]
fn truncated_bounds_are_bounds_only() {
    // A writer truncated at 3 bytes: the real minimum starts with "abc", the real maximum starts with "abc" or is incremented "abd".
    let g = text_truncated("abc", "abd");
    assert!(
        skips(&g, Lt, s("abc")),
        "a truncated minimum is still a lower bound"
    );
    assert!(skips(&g, Eq, s("abb")));
    assert!(!skips(&g, Le, s("abc")));
    assert!(
        !skips(&g, Eq, s("abcdef")),
        "the real minimum may be abcdef"
    );
    // The maximum is trusted only up to its own increment: "abd" may be a prefix of the real maximum "abdzzz".
    assert!(!skips(&g, Gt, s("abd")), "a value abdzzz may exist");
    assert!(!skips(&g, Ge, s("abd")));
    assert!(!skips(&g, Eq, s("abd")));
    assert!(!skips(&g, Gt, s("abdzzz")));
    assert!(
        skips(&g, Gt, s("abe")),
        "abe is above everything that starts with abd"
    );
    assert!(skips(&g, Ge, s("abe")));
    assert!(skips(&g, Eq, s("abe")));
    // A constant needs exact bounds: truncated "ab" and "ab" may stand for abc..abz.
    let c = text_truncated("ab", "ab");
    assert!(!skips(&c, Ne, s("ab")));
    assert!(
        !skips(&c, Gt, s("ab")),
        "a writer that forgot the increment leaves abz as the real maximum"
    );
    assert!(
        skips(&c, Gt, s("ac")),
        "everything starting with ab is below ac"
    );
    // Exact bounds are exact.
    let e = text("ab", "ab");
    assert!(skips(&e, Ne, s("ab")));
    assert!(skips(&e, Gt, s("ab")));
    // One exact and one truncated bound: still no constant.
    let mut half = ColumnStats::none(T::Utf8).with_null_count(0);
    half.min = Some(Bound::exact(s("ab")));
    half.max = Some(Bound::truncated(s("ab")));
    assert!(!skips(&one(half, 5), Ne, s("ab")));
    let mut half = ColumnStats::none(T::Utf8).with_null_count(0);
    half.min = Some(Bound::truncated(s("ab")));
    half.max = Some(Bound::exact(s("ab")));
    assert!(!skips(&one(half, 5), Ne, s("ab")));
}

#[test]
fn truncated_maximum_that_cannot_be_used() {
    // An empty truncated maximum is no upper bound at all (its increment does not exist): nothing is pruned from above.
    let mut c = ColumnStats::none(T::Utf8).with_null_count(0);
    c.min = Some(Bound::exact(s("")));
    c.max = Some(Bound::truncated(s("")));
    let g = one(c, 5);
    assert!(!skips(&g, Gt, s("zzzz")));
    assert!(!skips(&g, Eq, s("zzzz")));
    assert!(skips(&g, Lt, s("")), "the minimum still works");
    // The same for a binary maximum of only 0xFF bytes.
    let mut c = ColumnStats::none(T::Binary).with_null_count(0);
    c.min = Some(Bound::exact(V::Binary(vec![1])));
    c.max = Some(Bound::truncated(V::Binary(vec![0xFF, 0xFF])));
    let g = one(c, 5);
    assert!(!skips(&g, Gt, V::Binary(vec![0xFF, 0xFF, 0xFF, 0xFF])));
    assert!(skips(&g, Lt, V::Binary(vec![1])));
}

#[test]
fn binary_increment_skips_trailing_ff() {
    // truncated maximum 41 FF: everything that starts with it is below 42.
    let mut c = ColumnStats::none(T::Binary).with_null_count(0);
    c.min = Some(Bound::exact(V::Binary(vec![0x00])));
    c.max = Some(Bound::truncated(V::Binary(vec![0x41, 0xFF])));
    let g = one(c, 5);
    assert!(!skips(&g, Gt, V::Binary(vec![0x41, 0xFF, 0xFF, 0xFF])));
    assert!(skips(&g, Gt, V::Binary(vec![0x42])));
    assert!(!skips(&g, Gt, V::Binary(vec![0x41, 0xFF, 0x01])));
    assert!(skips(&g, Eq, V::Binary(vec![0x42, 0x00])));
    assert!(skips(&g, Ge, V::Binary(vec![0x42])));
    // unsigned bytes: 0x80 is above 0x7F
    assert!(!skips(&g, Eq, V::Binary(vec![0x41])));
}

#[test]
fn utf8_bound_from_bytes_refuses_a_cut_character() {
    assert!(Bound::utf8_from_bytes("日本".as_bytes(), true).is_some());
    assert!(
        Bound::utf8_from_bytes("日本".as_bytes().get(..4).unwrap_or_default(), false).is_none(),
        "cut inside the second character"
    );
    assert!(Bound::utf8_from_bytes(&[0xC3], false).is_none());
    assert!(Bound::utf8_from_bytes(&[0xFF], true).is_none());
    assert!(Bound::utf8_from_bytes(b"", true).is_some());
    let b = Bound::utf8_from_bytes("é".as_bytes(), false);
    assert_eq!(
        b,
        Some(Bound {
            value: s("é"),
            exact: false
        })
    );
}

#[test]
fn text_truncated_at_a_character_boundary_is_usable() {
    // "日本語" truncated to 4 bytes by a careful writer is "日" (3 bytes); the maximum increments the last character.
    let g = text_truncated("日", "日本語".get(..3).map(|_| "旦").unwrap_or("日"));
    assert!(skips(&g, Lt, s("日")));
    assert!(!skips(&g, Eq, s("日本")));
    // 旦 is U+65E6, one above 日 (U+65E5); everything starting with 旦 is below U+65E7
    assert!(skips(&g, Gt, s("旧")), "U+65E7 and above");
    assert!(!skips(&g, Gt, s("旦")));
}

// ---------------------------------------------------------------------------------------------------- conjunctions

#[test]
fn one_provably_empty_term_skips_the_conjunction() {
    let g = ints(10, 20);
    let p = Predicate::new(vec![
        Term::cmp("x", Ge, V::Int64(15)),
        Term::cmp("x", Gt, V::Int64(20)),
    ]);
    assert_eq!(can_skip(&g, &p), Decision::Skip(S::OutOfRange));
    let p = Predicate::new(vec![
        Term::cmp("x", Gt, V::Int64(20)),
        Term::cmp("x", Ge, V::Int64(15)),
    ]);
    assert!(can_skip(&g, &p).is_skip());
    // x >= 15 AND x < 15 is empty, but the min and max alone cannot show it: kept (sound, not tight)
    let p = Predicate::new(vec![
        Term::cmp("x", Ge, V::Int64(15)),
        Term::cmp("x", Lt, V::Int64(15)),
    ]);
    assert!(!can_skip(&g, &p).is_skip());
    // a term on an unknown column does not hide a term that skips
    let p = Predicate::new(vec![
        Term::cmp("nope", Gt, V::Int64(1)),
        Term::cmp("x", Gt, V::Int64(20)),
    ]);
    assert!(can_skip(&g, &p).is_skip());
    // all terms kept: the first reason is reported
    let p = Predicate::new(vec![
        Term::cmp("nope", Gt, V::Int64(1)),
        Term::cmp("x", Gt, V::Int64(15)),
    ]);
    assert_eq!(can_skip(&g, &p), Decision::Keep(K::UnknownColumn));
    // two columns
    let rg = RowGroupStats::new(10)
        .with_column(
            "a",
            ColumnStats::none(T::Int64)
                .with_null_count(0)
                .with_range(V::Int64(0), V::Int64(5)),
        )
        .with_column("b", ColumnStats::none(T::Utf8).with_null_count(10));
    assert!(
        can_skip(
            &rg,
            &Predicate::new(vec![
                Term::cmp("a", Lt, V::Int64(100)),
                Term::cmp("b", Eq, s("q"))
            ])
        )
        .is_skip(),
        "b is all NULL"
    );
    assert!(
        !can_skip(
            &rg,
            &Predicate::new(vec![Term::cmp("a", Lt, V::Int64(100)), Term::is_null("b")])
        )
        .is_skip()
    );
    assert!(
        can_skip(
            &rg,
            &Predicate::new(vec![Term::cmp("a", Gt, V::Int64(5)), Term::is_null("b")])
        )
        .is_skip()
    );
}
