//! The footer-statistics adapter, on hand-made column metadata (no file): each rule of `src/adapter.rs` with the statistic that would be unsound if the
//! rule were missing, and the pruning decision that statistic would wrongly allow.

use std::sync::Arc;

use nata_parquet::adapter::{column_order, column_stats, row_group_stats};
use nata_parquet::schema::{Kind, Leaf, Unit, classify, leaves};
use nata_scan::{
    Bound, CmpOp, ColumnType, Decision, Predicate, RowGroupStats, Term, Value, can_skip,
};
use parquet::basic::{ColumnOrder, ConvertedType, LogicalType, SortOrder, Type as Physical};
use parquet::data_type::{ByteArray, FixedLenByteArray};
use parquet::file::metadata::{ColumnChunkMetaData, FileMetaData, RowGroupMetaData};
use parquet::file::statistics::{Statistics, ValueStatistics};
use parquet::schema::types::{ColumnDescPtr, ColumnDescriptor, ColumnPath, SchemaDescriptor, Type};

const SIGNED: ColumnOrder = ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::SIGNED);
const UNSIGNED: ColumnOrder = ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::UNSIGNED);

fn desc(
    physical: Physical,
    logical: Option<LogicalType>,
    converted: ConvertedType,
    len: i32,
    precision: i32,
    scale: i32,
) -> ColumnDescPtr {
    let mut b = Type::primitive_type_builder("c", physical)
        .with_converted_type(converted)
        .with_logical_type(logical);
    if len > 0 {
        b = b.with_length(len);
    }
    if precision > 0 {
        b = b.with_precision(precision).with_scale(scale);
    }
    let t = b.build().expect("type");
    Arc::new(ColumnDescriptor::new(
        Arc::new(t),
        1,
        0,
        ColumnPath::from("c"),
    ))
}

fn chunk(d: ColumnDescPtr, stats: Statistics) -> ColumnChunkMetaData {
    ColumnChunkMetaData::builder(d)
        .set_statistics(stats)
        .build()
        .expect("chunk")
}

fn text_desc() -> ColumnDescPtr {
    desc(
        Physical::BYTE_ARRAY,
        Some(LogicalType::String),
        ConvertedType::UTF8,
        0,
        0,
        0,
    )
}

fn bytes(s: &str) -> Option<ByteArray> {
    Some(ByteArray::from(s))
}

fn text_stats(min: &str, max: &str, nulls: u64, deprecated: bool, exact: bool) -> Statistics {
    Statistics::ByteArray(
        ValueStatistics::new(bytes(min), bytes(max), None, Some(nulls), deprecated)
            .with_min_is_exact(exact)
            .with_max_is_exact(exact),
    )
}

fn keep(decision: Decision) -> bool {
    matches!(decision, Decision::Keep(_))
}

fn skip(decision: Decision) -> bool {
    matches!(decision, Decision::Skip(_))
}

fn lt_text(v: &str) -> Predicate {
    Predicate::new(vec![Term::cmp("c", CmpOp::Lt, Value::Utf8(v.to_string()))])
}

fn one(stats: nata_scan::ColumnStats, rows: u64) -> RowGroupStats {
    RowGroupStats::new(rows).with_column("c", stats)
}

#[test]
fn exact_text_bounds_with_the_right_column_order_are_used() {
    let s = column_stats(
        &chunk(text_desc(), text_stats("b", "d", 0, false, true)),
        UNSIGNED,
        Some(Kind::Utf8),
    );
    assert_eq!(s.ty, ColumnType::Utf8);
    assert_eq!(s.null_count, Some(0));
    assert_eq!(s.min, Some(Bound::exact(Value::Utf8("b".into()))));
    assert_eq!(s.max, Some(Bound::exact(Value::Utf8("d".into()))));
    assert!(
        skip(can_skip(&one(s, 100), &lt_text("b"))),
        "min 'b' proves nothing is below 'b'"
    );
}

#[test]
fn deprecated_min_max_are_never_used() {
    // A writer that put SIGNED-ordered values in the old fields: the claim 'b'..'d' may be wrong for bytes with the high bit. No bound is handed over.
    let s = column_stats(
        &chunk(text_desc(), text_stats("b", "d", 3, true, true)),
        UNSIGNED,
        Some(Kind::Utf8),
    );
    assert_eq!((s.min.clone(), s.max.clone()), (None, None));
    assert_eq!(
        s.null_count,
        Some(3),
        "the null count does not depend on the order and is kept"
    );
    assert!(keep(can_skip(&one(s, 100), &lt_text("b"))));
}

#[test]
fn a_missing_exactness_flag_means_not_exact() {
    // `is_min_value_exact` absent: the parquet crate reports false. A constant group is then not proven constant, so `<>` keeps it.
    let s = column_stats(
        &chunk(text_desc(), text_stats("k", "k", 0, false, false)),
        UNSIGNED,
        Some(Kind::Utf8),
    );
    assert_eq!(s.min, Some(Bound::truncated(Value::Utf8("k".into()))));
    let ne = Predicate::new(vec![Term::cmp("c", CmpOp::Ne, Value::Utf8("k".into()))]);
    assert!(
        keep(can_skip(&one(s, 100), &ne)),
        "an inexact bound must not prove that every value equals it"
    );
    let exact = column_stats(
        &chunk(text_desc(), text_stats("k", "k", 0, false, true)),
        UNSIGNED,
        Some(Kind::Utf8),
    );
    assert!(
        skip(can_skip(&one(exact, 100), &ne)),
        "control: with exact bounds the constant group is skipped"
    );
}

#[test]
fn an_exactness_flag_per_side() {
    let st = Statistics::ByteArray(
        ValueStatistics::new(bytes("abcd"), bytes("abcz"), None, Some(0), false)
            .with_min_is_exact(true)
            .with_max_is_exact(false),
    );
    let s = column_stats(&chunk(text_desc(), st), UNSIGNED, Some(Kind::Utf8));
    assert_eq!(s.min.as_ref().map(|b| b.exact), Some(true));
    assert_eq!(s.max.as_ref().map(|b| b.exact), Some(false));
}

#[test]
fn only_the_expected_type_defined_order_gives_bounds() {
    let st = || text_stats("b", "d", 0, false, true);
    for (order, label) in [
        (
            ColumnOrder::UNDEFINED,
            "undefined (no column_orders in the footer)",
        ),
        (ColumnOrder::UNKNOWN, "unknown"),
        (ColumnOrder::IEEE_754_TOTAL_ORDER, "IEEE total order"),
        (SIGNED, "a signed order on text"),
        (
            ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::UNDEFINED),
            "type defined, sort order undefined",
        ),
    ] {
        let s = column_stats(&chunk(text_desc(), st()), order, Some(Kind::Utf8));
        assert_eq!((s.min.clone(), s.max.clone()), (None, None), "{label}");
        assert!(keep(can_skip(&one(s, 100), &lt_text("b"))), "{label}");
    }
    // floats: the signed type-defined order is used, the IEEE total order of parquet-rs 60 is not
    let d = desc(Physical::DOUBLE, None, ConvertedType::NONE, 0, 0, 0);
    let st = || {
        Statistics::Double(
            ValueStatistics::new(Some(1.0), Some(2.0), None, Some(0), false)
                .with_min_is_exact(true)
                .with_max_is_exact(true),
        )
    };
    let ok = column_stats(&chunk(d.clone(), st()), SIGNED, Some(Kind::Float64));
    assert_eq!(ok.min, Some(Bound::exact(Value::Float64(1.0))));
    let total = column_stats(
        &chunk(d, st()),
        ColumnOrder::IEEE_754_TOTAL_ORDER,
        Some(Kind::Float64),
    );
    assert!(total.min.is_none() && total.max.is_none());
    assert_eq!(total.nan_count, None, "Parquet has no NaN count");
}

#[test]
fn the_column_order_of_a_short_or_missing_list_is_undefined_and_never_panics() {
    let schema = Type::group_type_builder("s")
        .with_fields(vec![Arc::new(
            Type::primitive_type_builder("c", Physical::INT32)
                .build()
                .expect("t"),
        )])
        .build()
        .expect("schema");
    let descr = Arc::new(SchemaDescriptor::new(Arc::new(schema)));
    let none = FileMetaData::new(1, 0, None, None, descr.clone(), None);
    assert_eq!(column_order(&none, 0), ColumnOrder::UNDEFINED);
    let short = FileMetaData::new(1, 0, None, None, descr.clone(), Some(vec![]));
    assert_eq!(
        column_order(&short, 0),
        ColumnOrder::UNDEFINED,
        "a corrupt footer with too few column orders"
    );
    assert_eq!(column_order(&short, 7), ColumnOrder::UNDEFINED);
    let one = FileMetaData::new(1, 0, None, None, descr, Some(vec![SIGNED]));
    assert_eq!(column_order(&one, 0), SIGNED);
}

#[test]
fn text_bounds_that_are_not_utf8_are_dropped() {
    let st = Statistics::ByteArray(
        ValueStatistics::new(
            Some(ByteArray::from(vec![0xC3u8])),
            bytes("z"),
            None,
            Some(0),
            false,
        )
        .with_min_is_exact(true)
        .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(text_desc(), st), UNSIGNED, Some(Kind::Utf8));
    assert!(s.min.is_none(), "a prefix cut inside a character");
    assert!(s.max.is_some());
}

#[test]
fn binary_bounds_are_bytes_in_unsigned_order() {
    let d = desc(Physical::BYTE_ARRAY, None, ConvertedType::NONE, 0, 0, 0);
    let st = Statistics::ByteArray(
        ValueStatistics::new(
            Some(ByteArray::from(vec![0x80u8])),
            Some(ByteArray::from(vec![0xFFu8])),
            None,
            Some(0),
            false,
        )
        .with_min_is_exact(true)
        .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(d, st), UNSIGNED, Some(Kind::Binary));
    assert_eq!(s.min, Some(Bound::exact(Value::Binary(vec![0x80]))));
    let below = Predicate::new(vec![Term::cmp("c", CmpOp::Lt, Value::Binary(vec![0x7F]))]);
    assert!(
        skip(can_skip(&one(s, 10), &below)),
        "0x80 is above 0x7f as an unsigned byte"
    );
}

#[test]
fn decimals_from_every_storage_form() {
    let k = Kind::Decimal {
        precision: 9,
        scale: 2,
    };
    let want = |u: i128, p, s| {
        Some(Bound::exact(Value::Decimal {
            unscaled: u,
            precision: p,
            scale: s,
        }))
    };
    let exact = |st: Statistics| st;
    // INT32
    let d32 = desc(
        Physical::INT32,
        Some(LogicalType::decimal(2, 9)),
        ConvertedType::DECIMAL,
        0,
        9,
        2,
    );
    let st = Statistics::Int32(
        ValueStatistics::new(Some(-150), Some(99999), None, Some(0), false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(d32, exact(st)), SIGNED, Some(k));
    assert_eq!((s.min, s.max), (want(-150, 9, 2), want(99999, 9, 2)));
    // INT64
    let k = Kind::Decimal {
        precision: 15,
        scale: 2,
    };
    let d64 = desc(
        Physical::INT64,
        Some(LogicalType::decimal(2, 15)),
        ConvertedType::DECIMAL,
        0,
        15,
        2,
    );
    let st = Statistics::Int64(
        ValueStatistics::new(
            Some(5_000_000_000),
            Some(6_000_000_000),
            None,
            Some(0),
            false,
        )
        .with_min_is_exact(true)
        .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(d64, st), SIGNED, Some(k));
    assert_eq!(
        (s.min, s.max),
        (want(5_000_000_000, 15, 2), want(6_000_000_000, 15, 2))
    );
    // FIXED_LEN_BYTE_ARRAY(8), big-endian two's complement, a negative minimum
    let k = Kind::Decimal {
        precision: 18,
        scale: 4,
    };
    let dfl = desc(
        Physical::FIXED_LEN_BYTE_ARRAY,
        Some(LogicalType::decimal(4, 18)),
        ConvertedType::DECIMAL,
        8,
        18,
        4,
    );
    let (lo, hi) = (
        FixedLenByteArray::from(ByteArray::from((-7i64).to_be_bytes().to_vec())),
        FixedLenByteArray::from(ByteArray::from(1234i64.to_be_bytes().to_vec())),
    );
    let st = Statistics::FixedLenByteArray(
        ValueStatistics::new(Some(lo), Some(hi), None, Some(0), false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(dfl.clone(), st), SIGNED, Some(k));
    assert_eq!((s.min, s.max), (want(-7, 18, 4), want(1234, 18, 4)));
    // a FLBA bound of the wrong length for the declared width is dropped
    let short = FixedLenByteArray::from(ByteArray::from(vec![1u8, 2, 3]));
    let st = Statistics::FixedLenByteArray(
        ValueStatistics::new(Some(short.clone()), Some(short), None, Some(0), false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(dfl, st), SIGNED, Some(k));
    assert!(s.min.is_none() && s.max.is_none());
    // BYTE_ARRAY (3 bytes: 0xFF 0xFF 0x85 = -123)
    let k = Kind::Decimal {
        precision: 30,
        scale: 3,
    };
    let dba = desc(
        Physical::BYTE_ARRAY,
        Some(LogicalType::decimal(3, 30)),
        ConvertedType::DECIMAL,
        0,
        30,
        3,
    );
    let st = Statistics::ByteArray(
        ValueStatistics::new(
            Some(ByteArray::from(vec![0xFFu8, 0xFF, 0x85])),
            Some(ByteArray::from(vec![0x01u8, 0x00])),
            None,
            Some(0),
            false,
        )
        .with_min_is_exact(true)
        .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(dba, st), SIGNED, Some(k));
    assert_eq!((s.min, s.max), (want(-123, 30, 3), want(256, 30, 3)));
}

#[test]
fn dates_and_timestamps() {
    let dd = desc(
        Physical::INT32,
        Some(LogicalType::Date),
        ConvertedType::DATE,
        0,
        0,
        0,
    );
    let st = |a, b| {
        Statistics::Int32(
            ValueStatistics::new(Some(a), Some(b), None, Some(0), false)
                .with_min_is_exact(true)
                .with_max_is_exact(true),
        )
    };
    let s = column_stats(&chunk(dd.clone(), st(8000, 8100)), SIGNED, Some(Kind::Date));
    assert_eq!(s.min, Some(Bound::exact(Value::Date32(8000))));
    // DuckDB's infinity sentinels are not days
    let s = column_stats(
        &chunk(dd, st(-i32::MAX, i32::MAX)),
        SIGNED,
        Some(Kind::Date),
    );
    assert!(s.min.is_none() && s.max.is_none());
    // milliseconds become microseconds, exactly; an overflow drops the bound
    let ts = |unit| Some(LogicalType::timestamp(false, unit));
    let d_ms = desc(
        Physical::INT64,
        ts(parquet::basic::TimeUnit::MILLIS),
        ConvertedType::NONE,
        0,
        0,
        0,
    );
    let st64 = |a, b| {
        Statistics::Int64(
            ValueStatistics::new(Some(a), Some(b), None, Some(0), false)
                .with_min_is_exact(true)
                .with_max_is_exact(true),
        )
    };
    let s = column_stats(
        &chunk(d_ms.clone(), st64(1_500, 2_000)),
        SIGNED,
        Some(Kind::Timestamp(Unit::Millis)),
    );
    assert_eq!(s.ty, ColumnType::TimestampMicros);
    assert_eq!(
        (s.min, s.max),
        (
            Some(Bound::exact(Value::TimestampMicros(1_500_000))),
            Some(Bound::exact(Value::TimestampMicros(2_000_000)))
        )
    );
    let s = column_stats(
        &chunk(d_ms, st64(i64::MIN / 10, i64::MAX / 10)),
        SIGNED,
        Some(Kind::Timestamp(Unit::Millis)),
    );
    assert!(
        s.min.is_none() && s.max.is_none(),
        "x1000 overflows i64: no bound, never a wrapped one"
    );
    let d_us = desc(
        Physical::INT64,
        ts(parquet::basic::TimeUnit::MICROS),
        ConvertedType::NONE,
        0,
        0,
        0,
    );
    let s = column_stats(
        &chunk(d_us, st64(1_500, 2_000)),
        SIGNED,
        Some(Kind::Timestamp(Unit::Micros)),
    );
    assert_eq!(s.min, Some(Bound::exact(Value::TimestampMicros(1_500))));
}

#[test]
fn types_that_cannot_be_mapped_soundly_have_no_bounds() {
    // unsigned integers: the scan reads them but never prunes on their statistics
    let u = desc(
        Physical::INT32,
        Some(LogicalType::integer(32, false)),
        ConvertedType::UINT_32,
        0,
        0,
        0,
    );
    let kind = classify(&u).expect("uint32 is readable");
    assert_eq!(
        kind,
        Kind::Int {
            bits: 32,
            signed: false
        }
    );
    let st = Statistics::Int32(
        ValueStatistics::new(Some(-5), Some(7), None, Some(2), false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(
        &chunk(u, st),
        ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::UNSIGNED),
        Some(kind),
    );
    assert!(s.min.is_none() && s.max.is_none());
    assert_eq!(s.ty, ColumnType::Unsupported);
    assert_eq!(
        s.null_count,
        Some(2),
        "the null count is handed over (nata-scan does not use it for an unsupported type)"
    );
    // an unknown kind (INT96, float16, time zone, nested ...): only the null count
    let i96 = desc(Physical::INT96, None, ConvertedType::NONE, 0, 0, 0);
    assert!(classify(&i96).is_err());
    let st = Statistics::Int96(ValueStatistics::new(
        Some(parquet::data_type::Int96::from(vec![1, 2, 3])),
        Some(parquet::data_type::Int96::from(vec![4, 5, 6])),
        None,
        Some(1),
        false,
    ));
    let s = column_stats(&chunk(i96, st), ColumnOrder::INT96_TIMESTAMP_ORDER, None);
    assert!(s.min.is_none() && s.max.is_none());
    assert_eq!(s.null_count, Some(1));
    let f16 = desc(
        Physical::FIXED_LEN_BYTE_ARRAY,
        Some(LogicalType::Float16),
        ConvertedType::NONE,
        2,
        0,
        0,
    );
    assert_eq!(classify(&f16), Err("FLOAT16".to_string()));
    let tz = desc(
        Physical::INT64,
        Some(LogicalType::timestamp(
            true,
            parquet::basic::TimeUnit::MICROS,
        )),
        ConvertedType::NONE,
        0,
        0,
        0,
    );
    assert!(classify(&tz).expect_err("an error").contains("TIME ZONE"));
    let ns = desc(
        Physical::INT64,
        Some(LogicalType::timestamp(
            false,
            parquet::basic::TimeUnit::NANOS,
        )),
        ConvertedType::NONE,
        0,
        0,
        0,
    );
    assert!(classify(&ns).expect_err("an error").contains("nanosecond"));
    let wide = desc(
        Physical::FIXED_LEN_BYTE_ARRAY,
        Some(LogicalType::decimal(2, 50)),
        ConvertedType::DECIMAL,
        21,
        50,
        2,
    );
    assert!(
        classify(&wide)
            .expect_err("an error")
            .contains("DECIMAL(50,2)")
    );
}

#[test]
fn a_missing_statistics_block_is_no_statistic() {
    let c = ColumnChunkMetaData::builder(text_desc())
        .build()
        .expect("chunk");
    let s = column_stats(&c, UNSIGNED, Some(Kind::Utf8));
    assert_eq!(
        (s.null_count, s.min, s.max, s.nan_count),
        (None, None, None, None)
    );
    assert!(keep(can_skip(
        &one(column_stats(&c, UNSIGNED, Some(Kind::Utf8)), 100),
        &lt_text("a")
    )));
}

#[test]
fn a_missing_null_count_is_not_zero() {
    // statistics with bounds but no null count: the writer did not say how many NULLs there are
    let st = Statistics::ByteArray(
        ValueStatistics::new(bytes("a"), bytes("z"), None, None, false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(text_desc(), st), UNSIGNED, Some(Kind::Utf8));
    assert_eq!(s.null_count, None);
    let is_null = Predicate::new(vec![Term::is_null("c")]);
    assert!(
        keep(can_skip(&one(s, 100), &is_null)),
        "an unknown null count must not prove that there are no NULLs"
    );
}

#[test]
fn nan_count_is_never_invented() {
    let d = desc(Physical::FLOAT, None, ConvertedType::NONE, 0, 0, 0);
    let st = Statistics::Float(
        ValueStatistics::new(Some(1.0f32), Some(2.0), None, Some(0), false)
            .with_min_is_exact(true)
            .with_max_is_exact(true),
    );
    let s = column_stats(&chunk(d, st), SIGNED, Some(Kind::Float32));
    assert_eq!(s.nan_count, None);
    // so `x > 5` cannot be skipped by the maximum: a NaN row could be there
    let gt = Predicate::new(vec![Term::cmp("c", CmpOp::Gt, Value::Float32(5.0))]);
    assert!(keep(can_skip(&one(s, 100), &gt)));
}

#[test]
fn row_group_stats_of_a_real_schema_and_a_negative_row_count() {
    let schema = Type::group_type_builder("s")
        .with_fields(vec![Arc::new(
            Type::primitive_type_builder("c", Physical::INT64)
                .build()
                .expect("t"),
        )])
        .build()
        .expect("schema");
    let descr = Arc::new(SchemaDescriptor::new(Arc::new(schema)));
    let file = FileMetaData::new(1, 10, None, None, descr.clone(), Some(vec![SIGNED]));
    let all: Vec<Leaf> = leaves(&descr).expect("leaves");
    let col = chunk(
        descr.column(0),
        Statistics::Int64(
            ValueStatistics::new(Some(10), Some(20), None, Some(0), false)
                .with_min_is_exact(true)
                .with_max_is_exact(true),
        ),
    );
    let rg = RowGroupMetaData::builder(descr.clone())
        .set_num_rows(10)
        .set_column_metadata(vec![col.clone()])
        .build()
        .expect("rg");
    let pairs: Vec<(&str, &Leaf)> = vec![("c", &all[0])];
    let stats = row_group_stats(&rg, &file, &pairs).expect("stats");
    assert_eq!(stats.num_rows, 10);
    let lt = Predicate::new(vec![Term::cmp("c", CmpOp::Lt, Value::Int64(10))]);
    assert!(skip(can_skip(&stats, &lt)));
    // a footer with a negative row count is not a statistic
    let bad = RowGroupMetaData::builder(descr)
        .set_num_rows(-1)
        .set_column_metadata(vec![col])
        .build()
        .expect("rg");
    assert!(row_group_stats(&bad, &file, &pairs).is_none());
}
