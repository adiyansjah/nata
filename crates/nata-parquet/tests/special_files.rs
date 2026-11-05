//! Special files against DuckDB: empty and one-row files, all-NULL columns of every type, every type in one file, the CSV text itself (all ASCII characters,
//! every byte value, 12,000 floats of both widths), and the one known difference from DuckDB 2.0.0 (exact decimal comparison).

mod common;
use common::*;

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMillisecondArray,
};
use parquet::arrow::ArrowWriter;

fn full_scan_equals_duckdb(duck: &str, file: &Path, what: &str) {
    check_case(duck, &[file], None, &[], Skip::Zero, what);
}

#[test]
fn empty_and_one_row_files() {
    let Some(duck) = duckdb_or_skip("empty") else {
        return;
    };
    let dir = scratch("empty");
    let cols = "1::BIGINT AS a, 'x'::VARCHAR AS s, 1.5::DECIMAL(9,2) AS d, DATE '2020-01-01' AS dt, 2.5::DOUBLE AS f, true AS b";
    let empty = dir.join("empty-duckdb.parquet");
    duckdb_csv(
        &duck,
        &format!(
            "COPY (SELECT {cols} WHERE false) TO '{}' (FORMAT parquet)",
            empty.display()
        ),
    );
    let one = dir.join("one-duckdb.parquet");
    duckdb_csv(
        &duck,
        &format!(
            "COPY (SELECT {cols}) TO '{}' (FORMAT parquet)",
            one.display()
        ),
    );
    // parquet-rs: a file with zero row groups at all
    let zero_rg = dir.join("zero-row-groups-prs.parquet");
    let ids: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    let txt: ArrayRef = Arc::new(StringArray::from(Vec::<&str>::new()));
    let batch = RecordBatch::try_from_iter([("a", ids), ("s", txt)]).expect("batch");
    let w = ArrowWriter::try_new(
        File::create(&zero_rg).expect("create"),
        batch.schema(),
        None,
    )
    .expect("writer");
    w.close().expect("close");
    let mut n = 0;
    for f in [&empty, &one, &zero_rg] {
        let tag = f.file_name().expect("name").to_string_lossy().into_owned();
        full_scan_equals_duckdb(&duck, f, &tag);
        n += 1;
        for filter in [
            flt("a", "=", "1", "1"),
            flt("a", "<>", "1", "1"),
            is_null("a"),
            is_not_null("a"),
            flt("s", ">=", "''", "''"),
        ] {
            let r = check_case(
                &duck,
                &[f],
                None,
                std::slice::from_ref(&filter),
                Skip::Any,
                &format!("{tag} {}", filter.scan),
            );
            assert_eq!(
                r.rows_returned,
                u64::from(f == &one && filter.scan != "a <> 1" && filter.scan != "a IS NULL"),
                "{tag} {}: {}",
                filter.scan,
                r.line()
            );
            n += 1;
        }
    }
    // an empty row group is skipped by the statistics (no row can match), also with an unrelated predicate
    let r = check_case(
        &duck,
        &[&one],
        Some(&["a"]),
        &[flt("a", "=", "2", "2")],
        Skip::All,
        "one row, no match",
    );
    assert_eq!((r.row_groups_total, r.rows_read), (1, 0));
    // and the one-row file with a matching predicate reads its row
    let r = check_case(
        &duck,
        &[&one],
        Some(&["a", "s"]),
        &[flt("a", "=", "1", "1")],
        Skip::Zero,
        "one row, match",
    );
    assert_eq!((r.rows_read, r.rows_returned), (1, 1));
    eprintln!("DIFFERENTIAL empty_and_one_row_files: {} cases", n + 2);
}

#[test]
fn all_null_columns_of_every_type() {
    let Some(duck) = duckdb_or_skip("allnull") else {
        return;
    };
    let dir = scratch("allnull");
    let f = dir.join("allnull.parquet");
    let types = [
        ("i8", "TINYINT", "1"),
        ("i32", "INTEGER", "1"),
        ("i64", "BIGINT", "1"),
        ("u32", "UINTEGER", "1"),
        ("f", "FLOAT", "1.5"),
        ("d", "DOUBLE", "1.5"),
        ("dec4", "DECIMAL(4,2)", "1.50"),
        ("dec18", "DECIMAL(18,4)", "1.5"),
        ("dec38", "DECIMAL(38,0)", "1"),
        ("dt", "DATE", "1992-01-01"),
        ("ts", "TIMESTAMP", "1992-01-01 00:00:00"),
        ("s", "VARCHAR", "'x'"),
        ("b", "BLOB", "X'00'"),
        ("bo", "BOOLEAN", "true"),
    ];
    let sel = types
        .iter()
        .map(|(n, t, _)| format!("NULL::{t} AS {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    duckdb_csv(
        &duck,
        &format!(
            "COPY (SELECT i AS id, {sel} FROM range(0, 5000) t(i)) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048)",
            f.display()
        ),
    );
    full_scan_equals_duckdb(&duck, &f, "all-NULL columns, all columns");
    let mut n = 1;
    for (name, ty, lit) in types {
        let sql_lit = match ty {
            "DATE" => format!("DATE '{lit}'"),
            "TIMESTAMP" => format!("TIMESTAMP '{lit}'"),
            "BLOB" => "from_hex('00')".to_string(),
            "FLOAT" => format!("{lit}::FLOAT"),
            _ => lit.to_string(),
        };
        // an unsigned column has no pruning type, so not even its null counts are used (nata-scan: unsupported type = keep)
        let all = if name == "u32" { Skip::Zero } else { Skip::All };
        for op in OPS {
            let r = check_case(
                &duck,
                &[&f],
                Some(&["id", name]),
                &[flt(name, op, lit, &sql_lit)],
                all,
                &format!("all-NULL {name} {op}"),
            );
            assert_eq!(r.rows_returned, 0);
            n += 1;
        }
        check_case(
            &duck,
            &[&f],
            Some(&["id", name]),
            &[is_null(name)],
            Skip::Zero,
            &format!("all-NULL {name} IS NULL"),
        );
        let r = check_case(
            &duck,
            &[&f],
            Some(&["id", name]),
            &[is_not_null(name)],
            all,
            &format!("all-NULL {name} IS NOT NULL"),
        );
        assert_eq!(r.rows_returned, 0);
        n += 2;
    }
    eprintln!("DIFFERENTIAL all_null_columns_of_every_type: {n} cases");
}

#[test]
fn every_type_in_one_file() {
    let Some(duck) = duckdb_or_skip("types") else {
        return;
    };
    let dir = scratch("types");
    let f = dir.join("types.parquet");
    duckdb_csv(&duck, &format!("COPY (SELECT i,
      CASE WHEN i % 7 = 0 THEN NULL ELSE (i % 5 - 2)::TINYINT END AS i8,
      (i*100-15000)::SMALLINT AS i16, (i*100000)::INTEGER AS i32, (i*10000000000)::BIGINT AS i64,
      (i % 200)::UTINYINT AS u8, (i*200)::USMALLINT u16, (i*100000)::UINTEGER u32, (i*10000000000)::UBIGINT u64,
      (i/3.0)::FLOAT f32, CASE WHEN i%4=0 THEN 'NaN'::DOUBLE WHEN i%4=1 THEN 'inf'::DOUBLE WHEN i%4=2 THEN -0.0 ELSE i/7.0 END f64,
      (i/100.0)::DECIMAL(4,2) d4, (i/100.0)::DECIMAL(9,3) d9, (i*1000.5)::DECIMAL(15,2) d15, (i*1000.5)::DECIMAL(18,4) d18, (i*1e20+0.25)::DECIMAL(28,2) d28, (i*1e30)::DECIMAL(38,0) d38,
      DATE '1990-01-01' + i::INTEGER * 37 AS dt, TIMESTAMP '2020-01-01 00:00:00' + INTERVAL (i*1234567) MICROSECOND ts,
      CASE WHEN i%3=0 THEN 'é日𝄞 ' || i WHEN i%3=1 THEN 'a,b\"c' || i ELSE 'plain' END s, from_hex('ab' || printf('%02x', i%256)) b, i%2=0 AS bo
      FROM range(1, 301) t(i)) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 100)", f.display()));
    full_scan_equals_duckdb(&duck, &f, "every type");
    // every column alone, and a filter on every column kind
    let names = [
        "i", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "d4", "d9", "d15",
        "d18", "d28", "d38", "dt", "ts", "s", "b", "bo",
    ];
    let mut n = 1;
    for c in names {
        check_case(
            &duck,
            &[&f],
            Some(&[c]),
            &[],
            Skip::Zero,
            &format!("project {c}"),
        );
        n += 1;
    }
    for (flts, what) in [
        (vec![flt("f32", "<", "10.5", "10.5::FLOAT")], "float"),
        (
            vec![flt("f64", ">", "nan", "'NaN'::DOUBLE")],
            "double above nan",
        ),
        (vec![flt("f64", "=", "0", "0")], "double zero"),
        (
            vec![flt(
                "d28",
                ">=",
                "5000000000000000000000.25",
                "5000000000000000000000.25",
            )],
            "decimal 28",
        ),
        (
            vec![flt("u64", ">", "20000000000000", "20000000000000")],
            "ubigint",
        ),
        (vec![flt("u8", "<>", "5", "5")], "utinyint"),
        (vec![flt("i16", "<", "0", "0")], "smallint"),
        (vec![flt("b", ">=", "X'ab80'", "from_hex('ab80')")], "blob"),
        (vec![flt("s", "=", "'plain'", "'plain'")], "text"),
        (vec![flt("bo", "=", "false", "false")], "boolean"),
        (
            vec![is_null("i8"), flt("i", "<", "100", "100")],
            "null and range",
        ),
    ] {
        check_case(&duck, &[&f], None, &flts, Skip::Any, what);
        n += 1;
    }
    eprintln!("DIFFERENTIAL every_type_in_one_file: {n} cases");
}

#[test]
fn csv_text_of_every_ascii_character_and_every_byte() {
    let Some(duck) = duckdb_or_skip("csvtext") else {
        return;
    };
    let dir = scratch("csvtext");
    let f = dir.join("text.parquet");
    duckdb_csv(&duck, &format!("COPY (
        SELECT i AS id, 'a' || chr(i::INTEGER) || 'b' AS s, from_hex(printf('%02x', i)) AS blob1, from_hex('41' || printf('%02x', i) || '42') AS blob3 FROM range(1, 256) t(i) WHERE i < 128 OR i > 255
        UNION ALL SELECT 1000 + i, 'a' || chr(i::INTEGER + 160) || 'b', from_hex(printf('%02x', i)), from_hex('41' || printf('%02x', i) || '42') FROM range(0, 256) t(i)
        UNION ALL SELECT 2000, '', NULL, from_hex('') UNION ALL SELECT 2001, 'NULL', from_hex('00'), NULL UNION ALL SELECT 2002, NULL, NULL, NULL
        UNION ALL SELECT 2003, ' lead and trail ', from_hex('20'), from_hex('2c22275c') UNION ALL SELECT 2004, 'é日𝄞', from_hex('c3a9'), from_hex('f09d849e')
        ORDER BY id) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048)", f.display()));
    full_scan_equals_duckdb(&duck, &f, "csv text of every ASCII character and byte");
    for (c, filt) in [
        ("s", flt("s", ">", "'a'", "'a'")),
        ("blob1", flt("blob1", "<", "X'80'", "from_hex('80')")),
    ] {
        check_case(
            &duck,
            &[&f],
            Some(&["id", c]),
            &[filt],
            Skip::Any,
            "text filter",
        );
    }
}

/// Timestamps in MILLISECONDS (written by parquet-rs; DuckDB reads them as TIMESTAMP_MS): statistics converted to microseconds, the row filter, the text.
#[test]
fn timestamps_in_milliseconds() {
    let Some(duck) = duckdb_or_skip("ms") else {
        return;
    };
    let dir = scratch("ms");
    let f = dir.join("ms.parquet");
    // 6,000 rows, 1.5 s apart, in row groups of 2,000 (writer default row group limit lowered)
    let ts: ArrayRef = Arc::new(TimestampMillisecondArray::from_iter_values(
        (0..6000i64).map(|i| 1_577_836_800_000 + i * 1500),
    ));
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(0..6000));
    let batch = RecordBatch::try_from_iter([("id", ids), ("ts", ts)]).expect("batch");
    let props = parquet::file::properties::WriterProperties::builder()
        .set_max_row_group_row_count(Some(2000))
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(&f).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    full_scan_equals_duckdb(&duck, &f, "timestamp ms, text");
    // row group 1 starts at 2020-01-01 00:00:00 + 3000 s
    let ts = |op: &str, l: &str| flt("ts", op, l, &format!("TIMESTAMP '{l}'"));
    for op in OPS {
        for l in [
            "2020-01-01 00:00:00",
            "2020-01-01 00:50:00",
            "2020-01-01 00:49:59.999999",
            "2020-01-01 00:50:00.001",
            "2020-01-01 01:40:00",
            "2020-01-01 02:30:00",
            "2019-01-01 00:00:00",
        ] {
            check_case(
                &duck,
                &[&f],
                None,
                &[ts(op, l)],
                Skip::Any,
                &format!("ms ts {op} {l}"),
            );
        }
    }
    let r = check_case(
        &duck,
        &[&f],
        None,
        &[ts(">=", "2020-01-01 00:50:00")],
        Skip::Exactly(1),
        "ms: group 0 ends before 00:50:00",
    );
    assert_eq!(r.row_groups_total, 3);
    check_case(
        &duck,
        &[&f],
        None,
        &[ts("<", "2020-01-01 00:50:00")],
        Skip::Exactly(2),
        "ms: groups 1 and 2 start at or after 00:50:00",
    );
    check_case(
        &duck,
        &[&f],
        None,
        &[ts(">", "2030-01-01 00:00:00")],
        Skip::All,
        "ms: above every value",
    );
}

/// Many doubles and floats (random bit patterns, decimal-looking values, integers) printed by Nata and by DuckDB. The VALUES must be identical for every cell
/// (the text parses back to the same bits, NaN signs included); the TEXT is identical for almost all, and the share is measured (see the csv module docs for
/// the known cosmetic difference).
#[test]
fn float_text_reads_back_to_the_same_values_and_mostly_the_same_text() {
    let Some(duck) = duckdb_or_skip("floattext") else {
        return;
    };
    let dir = scratch("floattext");
    let f = dir.join("floats.parquet");
    let mut s = 0x1234_5678_9ABC_DEF1u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut d: Vec<f64> = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        1e15,
        1e16,
        1e-4,
        1e-5,
        123456789012345678.0,
        f64::MAX,
        f64::MIN_POSITIVE,
        5e-324,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -f64::NAN,
    ];
    let mut fl: Vec<f32> = vec![
        0.0,
        -0.0,
        1.0,
        0.1,
        1e15,
        1e16,
        1e-4,
        1e-5,
        16_777_216.0,
        f32::MAX,
        f32::MIN_POSITIVE,
        1e-45,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
        1e10,
    ];
    for _ in 0..40_000 {
        d.push(f64::from_bits(next()));
        fl.push(f32::from_bits(next() as u32));
    }
    for _ in 0..40_000 {
        let m = (next() % 2_000_000) as f64 - 1_000_000.0;
        let k = (next() % 40) as i32 - 20;
        d.push(m * 10f64.powi(k));
        fl.push((m * 10f64.powi(k.clamp(-10, 10))) as f32);
    }
    for _ in 0..20_000 {
        d.push((next() % 100_000_000_000_000_000) as f64);
        fl.push((next() % 100_000_000) as f32);
    }
    let n = d.len().min(fl.len());
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n as i64));
    let dd: ArrayRef = Arc::new(Float64Array::from(d[..n].to_vec()));
    let ff: ArrayRef = Arc::new(Float32Array::from(fl[..n].to_vec()));
    let batch = RecordBatch::try_from_iter([("id", ids), ("d", dd), ("f", ff)]).expect("batch");
    let mut w = ArrowWriter::try_new(File::create(&f).expect("create"), batch.schema(), None)
        .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    let want = records(&duckdb_csv(
        &duck,
        &format!("SELECT * FROM read_parquet('{}')", f.display()),
    ));
    let got = records(&scan(&[&f], None, &[], true, None).expect("scan").csv);
    assert_eq!(want.len(), got.len());
    assert_eq!(want[0], got[0]);
    let (mut same_d, mut same_f) = (0usize, 0usize);
    for (w, g) in want[1..].iter().zip(&got[1..]) {
        let (w, g): (Vec<&str>, Vec<&str>) = (w.split(',').collect(), g.split(',').collect());
        assert_eq!(w[0], g[0]);
        let bits64 = |t: &str| -> u64 {
            match t {
                "nan" => f64::NAN.to_bits(),
                "-nan" => (-f64::NAN).to_bits(),
                "inf" => f64::INFINITY.to_bits(),
                "-inf" => f64::NEG_INFINITY.to_bits(),
                _ => t.parse::<f64>().expect("double").to_bits(),
            }
        };
        let bits32 = |t: &str| -> u32 {
            match t {
                "nan" => f32::NAN.to_bits(),
                "-nan" => (-f32::NAN).to_bits(),
                "inf" => f32::INFINITY.to_bits(),
                "-inf" => f32::NEG_INFINITY.to_bits(),
                _ => t.parse::<f32>().expect("float").to_bits(),
            }
        };
        assert_eq!(
            bits64(w[1]),
            bits64(g[1]),
            "double: DuckDB {:?}, Nata {:?}",
            w[1],
            g[1]
        );
        assert_eq!(
            bits32(w[2]),
            bits32(g[2]),
            "float: DuckDB {:?}, Nata {:?}",
            w[2],
            g[2]
        );
        same_d += usize::from(w[1] == g[1]);
        same_f += usize::from(w[2] == g[2]);
    }
    let (pd, pf) = (
        same_d as f64 * 100.0 / n as f64,
        same_f as f64 * 100.0 / n as f64,
    );
    eprintln!(
        "FLOAT-TEXT: {n} rows: every value reads back identical; text identical for {pd:.3}% of doubles and {pf:.2}% of floats"
    );
    assert!(pd > 99.9, "doubles: {pd}% text-identical");
    assert!(pf > 90.0, "floats: {pf}% text-identical");
}

/// DuckDB 2.0.0 FAILS on an exact decimal comparison that `docs/decimal-rules.md` section 5 defines: a DECIMAL(38,0) value of 38 nines against a literal of
/// scale 1 needs DECIMAL(38,1) and DuckDB reports a conversion error for that row. Nata compares exactly. This is a KNOWN DIFFERENCE: the differential tests
/// avoid such literals, and this test pins both sides.
#[test]
fn known_difference_exact_decimal_comparison_where_duckdb_errors() {
    let Some(duck) = duckdb_or_skip("decdiff") else {
        return;
    };
    let dir = scratch("decdiff");
    let f = dir.join("d38.parquet");
    let nines = "9".repeat(38);
    duckdb_csv(
        &duck,
        &format!(
            "COPY (SELECT * FROM (VALUES (1, 0::DECIMAL(38,0)), (2, 5::DECIMAL(38,0)), (3, -5::DECIMAL(38,0)), (4, {nines}::DECIMAL(38,0)), (5, -{nines}::DECIMAL(38,0)), (6, NULL)) t(id, d)) TO '{}' (FORMAT parquet)",
            f.display()
        ),
    );
    // DuckDB: an error, with or without its statistics
    let err = duckdb_try(
        &duck,
        &format!(
            "SELECT id FROM read_parquet('{}') WHERE d > 99.9",
            f.display()
        ),
    )
    .expect_err("DuckDB 2.0.0 is expected to fail here");
    assert!(
        err.contains("out of range") && err.contains("DECIMAL(38,1)"),
        "DuckDB's error changed: {err}"
    );
    // Nata: exact
    let ids = |filters: &[&str]| -> Vec<String> {
        let s = scan(&[&f], Some(&["id"]), filters, true, None)
            .unwrap_or_else(|e| panic!("{filters:?}: {e}"));
        records(&s.csv)[1..].to_vec()
    };
    assert_eq!(
        ids(&["d > 99.9"]),
        ["4"],
        "only the 38 nines are above 99.9"
    );
    assert_eq!(ids(&["d < -99.9"]), ["5"]);
    assert_eq!(ids(&["d = 5.0"]), ["2"]);
    assert_eq!(ids(&["d = 5.0000000000"]), ["2"]);
    assert_eq!(ids(&["d <> 5.0"]), ["1", "3", "4", "5"]);
    assert_eq!(ids(&["d >= 4.5", "d < 5.5"]), ["2"]);
    assert_eq!(
        ids(&["d > 0.5", "d <= 99999999999999999999999999999999999999"]),
        ["2", "4"]
    );
    // a literal with more digits than the column is never rounded: 5 is not equal to 5.0000000000000000000000000000000000001
    assert_eq!(
        ids(&["d = 5.0000000000000000000000000000000000001"]),
        Vec::<String>::new()
    );
    assert_eq!(
        ids(&["d < 5.0000000000000000000000000000000000001", "d > 4"]),
        ["2"]
    );
    // DuckDB answers the cases that do not need DECIMAL(38,1): compare those too
    check_case(
        &duck,
        &[&f],
        None,
        &[flt("d", ">", "5", "5")],
        Skip::Any,
        "d38 against an integer literal",
    );
}
