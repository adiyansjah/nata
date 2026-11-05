//! DIFFERENTIAL against DuckDB on EDGE-CASE files (four row groups of 2,048 rows each, designed so that some row groups are pruned and some must not
//! be): NaN, -0.0 and infinities, an all-NULL row group, NULLs in some row groups only, decimals of every storage width, Unicode and truncated text,
//! integers of every width including unsigned, dates, timestamps, booleans, bytes with the high bit set. Each file is written by DuckDB, by parquet-rs
//! (default, truncated statistics, delta, byte-stream-split, data page v2) and by pyarrow (when available).
//!
//! `NATA_DISCOVER=1` prints every case with its skip count (to design expectations); the default run asserts them.

mod common;
use common::*;
use std::path::{Path, PathBuf};

/// Which writer made the file: statistics differ (column order of floats, exactness flags, truncation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum W {
    DuckDb,
    ParquetRs,
    PyArrow,
}

struct Case {
    label: String,
    filters: Vec<Filter>,
    /// Expectation by writer: (DuckDB, parquet-rs, pyarrow).
    skip: [Skip; 3],
}

fn case(label: &str, filters: Vec<Filter>, skip: Skip) -> Case {
    Case {
        label: label.to_string(),
        filters,
        skip: [skip; 3],
    }
}

impl Case {
    fn per_writer(mut self, duck: Skip, prs: Skip, pa: Skip) -> Case {
        self.skip = [duck, prs, pa];
        self
    }
}

fn discover() -> bool {
    std::env::var("NATA_DISCOVER").is_ok()
}

fn run(
    duck: &str,
    file: &Path,
    w: W,
    columns: Option<&[&str]>,
    cases: &[Case],
    tag: &str,
) -> usize {
    let specs: Vec<Spec> = cases
        .iter()
        .map(|c| Spec {
            columns: columns.map(|c| c.iter().map(|s| s.to_string()).collect()),
            filters: c.filters.clone(),
            expect: if discover() {
                Skip::Any
            } else {
                c.skip[w as usize]
            },
            what: format!("{tag} {:?} {}", w, c.label),
        })
        .collect();
    let reports = check_cases(duck, &[file], &specs);
    if discover() {
        for (c, r) in cases.iter().zip(&reports) {
            println!(
                "DISCOVER {tag:<28} {:<10?} {:<44} skipped {}/{}",
                w, c.label, r.row_groups_skipped, r.row_groups_total
            );
        }
    }
    cases.len()
}

/// Every file of one base: DuckDB's, parquet-rs in 5 styles, pyarrow in the variants that apply.
fn variants(
    duck: &str,
    dir: &Path,
    name: &str,
    pyarrow_variants: &[&str],
) -> Vec<(PathBuf, W, String)> {
    let base = data::write_edge(duck, dir, name);
    let mut v = vec![(base.clone(), W::DuckDb, "duckdb".to_string())];
    let wanted = [
        "plain-dict-v1-none",
        "snappy-dict-v2",
        "delta-v2",
        "bss-v2",
        "truncate4",
    ];
    for style in writers::styles()
        .into_iter()
        .filter(|s| wanted.contains(&s.name))
    {
        let f = data::rewrite_parquet_rs(&base, dir, name, &style);
        v.push((f, W::ParquetRs, format!("prs-{}", style.name)));
    }
    for variant in pyarrow_variants {
        if let Some(f) = data::rewrite_pyarrow(&base, dir, name, variant) {
            v.push((f, W::PyArrow, format!("pa-{variant}")));
        }
    }
    v
}

fn dbl(op: &str, text: &str, sql: &str) -> Filter {
    flt("d", op, text, sql)
}

#[test]
fn floats_nan_zeros_infinities_and_an_all_null_group() {
    let Some(duck) = duckdb_or_skip("floats") else {
        return;
    };
    let dir = scratch("edge-floats");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "floats", &["default", "v2-zstd"]) {
        let mut cases = Vec::new();
        // literals: ordinary, zeros of both signs, NaN, infinities
        let lits: [(&str, &str); 9] = [
            ("0", "0"),
            ("0.0", "0.0"),
            ("-0.0", "-0.0"),
            ("1.5", "1.5"),
            ("100", "100"),
            ("5000", "5000"),
            ("nan", "'NaN'::DOUBLE"),
            ("inf", "'inf'::DOUBLE"),
            ("-inf", "'-inf'::DOUBLE"),
        ];
        for op in OPS {
            for (t, s) in lits {
                cases.push(case(&format!("d {op} {t}"), vec![dbl(op, t, s)], Skip::Any));
            }
        }
        for op in OPS {
            for (t, s) in [
                ("0.1", "0.1::FLOAT"),
                ("2", "2::FLOAT"),
                ("nan", "'NaN'::FLOAT"),
                ("-0.0", "-0.0::FLOAT"),
            ] {
                cases.push(case(
                    &format!("f {op} {t}"),
                    vec![flt("f", op, t, s)],
                    Skip::Any,
                ));
            }
        }
        // designed cases. Group 0 holds 0..2047, group 1 holds numbers and NaN, group 2 is all NULL, group 3 holds zeros and infinities.
        // `d > 5000`: the maximum of group 0 is below 5000, but Parquet statistics leave NaN out, so no NaN-free proof exists: only the all-NULL group goes.
        cases.push(case(
            "d > 5000 (NaN-blind pruning is wrong)",
            vec![dbl(">", "5000", "5000")],
            Skip::Exactly(1),
        ));
        cases.push(case(
            "d >= 5000 (same)",
            vec![dbl(">=", "5000", "5000")],
            Skip::Exactly(1),
        ));
        cases.push(case(
            "d <> 5000 (same)",
            vec![dbl("<>", "5000", "5000")],
            Skip::Exactly(1),
        ));
        cases.push(case(
            "d > nan (nothing is above NaN)",
            vec![dbl(">", "nan", "'NaN'::DOUBLE")],
            Skip::All,
        ));
        cases.push(case("d is null", vec![is_null("d")], Skip::Exactly(3)));
        cases.push(case(
            "d is not null",
            vec![is_not_null("d")],
            Skip::Exactly(1),
        ));
        cases.push(case(
            "d > 5000 and id < 100",
            vec![dbl(">", "5000", "5000"), flt("id", "<", "100", "100")],
            Skip::Exactly(3),
        ));
        // `<` ignores NaN rows, so a bound can prune: not for parquet-rs 60, which writes float bounds in IEEE total order (unused here)
        cases.push(
            case("d < 0", vec![dbl("<", "0", "0")], Skip::Any).per_writer(
                Skip::AtLeast(1),
                Skip::Exactly(1),
                Skip::AtLeast(2),
            ),
        );
        cases.push(
            case(
                "d < -inf",
                vec![dbl("<", "-inf", "'-inf'::DOUBLE")],
                Skip::Any,
            )
            .per_writer(Skip::AtLeast(1), Skip::Exactly(1), Skip::AtLeast(2)),
        );
        n += run(&duck, &file, w, None, &cases, &tag);
        n += run(&duck, &file, w, Some(&["id"]), &cases[..12], &tag);
        n += run(
            &duck,
            &file,
            w,
            Some(&["f", "id", "d"]),
            &cases[60..64],
            &tag,
        );
    }
    eprintln!("DIFFERENTIAL floats: {n} cases");
    for d in pushdown_differences() {
        eprintln!("DUCKDB-PUSHDOWN-DIFFERENCE {d}");
    }
}

/// The same with no all-NULL group: the NaN group sits between ordinary ones, and `>` must not prune at all.
#[test]
fn a_nan_group_is_never_proven_away_by_a_maximum() {
    let Some(duck) = duckdb_or_skip("floats_nonull") else {
        return;
    };
    let dir = scratch("edge-floats2");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "floats_nonull", &["default"]) {
        let cases = vec![
            // group 0: 0..2047, group 1: NaN every 10th and numbers 2048..4095, group 2: 100000 + 4096.., group 3: zeros and infinities
            case(
                "d > 1e9 designed NOT to prune",
                vec![dbl(">", "1e9", "1e9")],
                Skip::Zero,
            ),
            case(
                "d >= 1e9 designed NOT to prune",
                vec![dbl(">=", "1e9", "1e9")],
                Skip::Zero,
            ),
            case(
                "d <> 1 designed NOT to prune",
                vec![dbl("<>", "1", "1")],
                Skip::Zero,
            ),
            case("d > -1e9", vec![dbl(">", "-1e9", "-1e9")], Skip::Zero),
            case(
                "d > inf",
                vec![dbl(">", "inf", "'inf'::DOUBLE")],
                Skip::Zero,
            ),
            case("d = nan", vec![dbl("=", "nan", "'NaN'::DOUBLE")], Skip::Any),
            case(
                "d >= nan",
                vec![dbl(">=", "nan", "'NaN'::DOUBLE")],
                Skip::Any,
            ),
            case("d < 1e9", vec![dbl("<", "1e9", "1e9")], Skip::Zero),
            case(
                "d <= nan",
                vec![dbl("<=", "nan", "'NaN'::DOUBLE")],
                Skip::Zero,
            ),
            case(
                "d < nan",
                vec![dbl("<", "nan", "'NaN'::DOUBLE")],
                Skip::Zero,
            ),
            case(
                "d <> nan",
                vec![dbl("<>", "nan", "'NaN'::DOUBLE")],
                Skip::Zero,
            ),
            case("d > nan", vec![dbl(">", "nan", "'NaN'::DOUBLE")], Skip::All),
            // `<` and `=` ignore NaN rows, so the ordinary groups can be skipped where the writer's bounds are usable
            case("d < -1", vec![dbl("<", "-1", "-1")], Skip::Any),
            case("d = 50000", vec![dbl("=", "50000", "50000")], Skip::Any),
            case(
                "d = 0 (zeros of both signs)",
                vec![dbl("=", "0", "0")],
                Skip::Any,
            ),
            case("d = -0.0", vec![dbl("=", "-0.0", "-0.0")], Skip::Any),
            case(
                "d < 0 (-0.0 is not below 0)",
                vec![dbl("<", "0", "0")],
                Skip::Any,
            ),
            case("d > -0.0", vec![dbl(">", "-0.0", "-0.0")], Skip::Any),
        ];
        n += run(&duck, &file, w, None, &cases, &tag);
    }
    eprintln!("DIFFERENTIAL floats_nonull: {n} cases");
}

// ------------------------------------------------------------------------------------------------------------------------------ decimals

/// The decimal text of `id` in column `col` (the SQL of `edge_sql("decimals")`), without extra digits.
fn dec_of(col: &str, id: u64) -> String {
    match col {
        "d4" => format!("{}.{:02}", id / 100, id % 100),
        "d9" => format!("{}.{:03}", id / 1000, id % 1000),
        "d15" => format!("{}.25", id * 100),
        "d18" => {
            let v = id * 10001; // id * 1.0001 at scale 4
            format!("{}.{:04}", v / 10000, v % 10000)
        }
        "d28" => format!("{}00000000000000000000.50", id),
        "d38" => format!("{}{}", id, "0".repeat(29)),
        _ => unreachable!(),
    }
}

#[test]
fn decimals_of_every_storage_width_with_literals_at_other_scales() {
    let Some(duck) = duckdb_or_skip("decimals") else {
        return;
    };
    let dir = scratch("edge-dec");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "decimals", &["default", "int-decimals"]) {
        let mut cases = Vec::new();
        for col in ["d4", "d9", "d15", "d18", "d28", "d38"] {
            // group 2 starts at id 4096, group 3 at id 6144
            let (lo2, lo3, last) = (dec_of(col, 4096), dec_of(col, 6144), dec_of(col, 8191));
            for op in OPS {
                for (name, lit) in [
                    ("g2start", lo2.clone()),
                    ("g3start", lo3.clone()),
                    ("max", last.clone()),
                    ("zero", "0".to_string()),
                ] {
                    cases.push(case(
                        &format!("{col} {op} {name}"),
                        vec![flt(col, op, &lit, &lit)],
                        Skip::Any,
                    ));
                }
            }
            // designed: the start of group 3 prunes groups 0 to 2 (>=), the end of group 0 prunes 1 to 3 (<)
            cases.push(case(
                &format!("{col} >= start of group 3"),
                vec![flt(col, ">=", &lo3, &lo3)],
                Skip::Exactly(3),
            ));
            cases.push(case(
                &format!("{col} > max"),
                vec![flt(col, ">", &last, &last)],
                Skip::All,
            ));
            cases.push(case(
                &format!("{col} = absent"),
                vec![flt(col, "=", "-1", "-1")],
                Skip::All,
            ));
            cases.push(case(
                &format!("{col} >= 0 (prunes nothing)"),
                vec![flt(col, ">=", "0", "0")],
                Skip::Zero,
            ));
            // an integer literal for a wide-scale column and the reverse
            cases.push(case(
                &format!("{col} < 1"),
                vec![flt(col, "<", "1", "1")],
                Skip::Any,
            ));
        }
        // literals with MORE fraction digits than the column: never rounded to the column's scale
        for (col, lit) in [
            ("d4", "40.955"),
            ("d4", "40.9600"),
            ("d4", "0.005"),
            ("d9", "4.0955"),
            ("d15", "409600.2500"),
            ("d15", "409600.255"),
            ("d18", "4096.40960000"),
            ("d28", "4096000000000000000000000.5000"),
        ] {
            for op in OPS {
                cases.push(case(
                    &format!("{col} {op} {lit} (more digits than the column)"),
                    vec![flt(col, op, lit, lit)],
                    Skip::Any,
                ));
            }
        }
        // two decimal columns at once
        cases.push(case(
            "d4 and d15",
            vec![
                flt("d4", ">=", "40.96", "40.96"),
                flt("d15", "<", "500000", "500000"),
            ],
            Skip::AtLeast(1),
        ));
        n += run(&duck, &file, w, None, &cases, &tag);
        n += run(
            &duck,
            &file,
            w,
            Some(&["id", "d38", "d4"]),
            &cases[..40],
            &tag,
        );
    }
    eprintln!("DIFFERENTIAL decimals: {n} cases");
}

// ------------------------------------------------------------------------------------------------------------------------------ text

#[test]
fn unicode_text_with_truncated_statistics() {
    let Some(duck) = duckdb_or_skip("strings") else {
        return;
    };
    let dir = scratch("edge-str");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "strings", &["default", "v2-zstd"]) {
        let mut cases = Vec::new();
        let lits = [
            "",
            " ",
            "a",
            "apple-",
            "apple-00006abcdefghij",
            "apple-99999",
            "b",
            "é",
            "été-",
            "été-02050abcdefghij",
            "日",
            "日本-",
            "日本-04200abcdefghij",
            "𝄞",
            "𝄞-06150abcdefghij",
            "zzzz",
            "\u{10ffff}",
            "NULL",
            "it's",
            "a,b",
            "a\"b",
            "tab\there",
            "new\nline",
        ];
        for op in OPS {
            for l in lits {
                let q = l.replace('\'', "''");
                cases.push(case(
                    &format!("s {op} {l:?}"),
                    vec![flt("s", op, &format!("'{q}'"), &format!("'{q}'"))],
                    Skip::Any,
                ));
            }
        }
        // designed: 'apple-' < 'b' < 'été-' < '日本-' < '𝄞-' bytewise
        cases.push(case(
            "s < 'b' keeps group 0 only",
            vec![flt("s", "<", "'b'", "'b'")],
            Skip::Exactly(3),
        ));
        cases.push(case(
            "s >= 'é' skips group 0",
            vec![flt("s", ">=", "'é'", "'é'")],
            Skip::Exactly(1),
        ));
        cases.push(case(
            "s >= '𝄞' keeps group 3 only",
            vec![flt("s", ">=", "'𝄞'", "'𝄞'")],
            Skip::Exactly(3),
        ));
        cases.push(case(
            "s > '日' skips groups 0 and 1",
            vec![flt("s", ">", "'日'", "'日'")],
            Skip::Exactly(2),
        ));
        cases.push(case(
            "s = 'apple-00006abcdefghij'",
            vec![flt(
                "s",
                "=",
                "'apple-00006abcdefghij'",
                "'apple-00006abcdefghij'",
            )],
            Skip::Exactly(3),
        ));
        cases.push(case(
            "s = absent between groups",
            vec![flt("s", "=", "'b'", "'b'")],
            Skip::All,
        ));
        cases.push(case(
            "s >= 'a' prunes nothing",
            vec![flt("s", ">=", "'a'", "'a'")],
            Skip::Zero,
        ));
        // t is constant per group: 'aaaa', 'aaab', 'aaaa', 'zzzz'
        cases.push(case(
            "t = 'aaaa'",
            vec![flt("t", "=", "'aaaa'", "'aaaa'")],
            Skip::Exactly(2),
        ));
        // `<>` prunes a constant group only when the bound is known to be exact; DuckDB, parquet-rs and pyarrow all write the exactness flags
        cases.push(
            case(
                "t <> 'aaaa' (constant groups, exactness flag)",
                vec![flt("t", "<>", "'aaaa'", "'aaaa'")],
                Skip::Any,
            )
            .per_writer(Skip::Exactly(2), Skip::Exactly(2), Skip::Any),
        );
        cases.push(case("t is null", vec![is_null("t")], Skip::All));
        cases.push(case("s is null", vec![is_null("s")], Skip::Zero));
        n += run(&duck, &file, w, None, &cases, &tag);
        n += run(&duck, &file, w, Some(&["t"]), &cases[..30], &tag);
    }
    eprintln!("DIFFERENTIAL strings: {n} cases");
}

// ------------------------------------------------------------------------------------------------------------------------------ NULLs

#[test]
fn nulls_in_some_row_groups_only() {
    let Some(duck) = duckdb_or_skip("nullsome") else {
        return;
    };
    let dir = scratch("edge-null");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "nullsome", &["default"]) {
        // group 0: all NULL; group 1: 2048..4095, no NULL; group 2: 4096.. with every third NULL; group 3: the constant 7
        let x = |op: &str, v: &str| flt("x", op, v, v);
        let mut cases = vec![
            case("x is null", vec![is_null("x")], Skip::Exactly(2)),
            case("x is not null", vec![is_not_null("x")], Skip::Exactly(1)),
            case("x = 7", vec![x("=", "7")], Skip::Exactly(3)),
            case("x > 5000", vec![x(">", "5000")], Skip::Exactly(3)),
            case("x < 0", vec![x("<", "0")], Skip::All),
            case("x >= 2048", vec![x(">=", "2048")], Skip::Exactly(2)),
            case("x <= 7", vec![x("<=", "7")], Skip::Exactly(3)),
            // the all-NULL group never matches a comparison, also `<>` (NULL <> 7 is NULL)
            // the constant group 3 goes too when the writer says its bounds are exact (all three do)
            case("x <> 7", vec![x("<>", "7")], Skip::Any).per_writer(
                Skip::Exactly(2),
                Skip::Exactly(2),
                Skip::Any,
            ),
            case("x <> 100000", vec![x("<>", "100000")], Skip::Exactly(1)),
            case(
                "x is null and x = 7",
                vec![is_null("x"), x("=", "7")],
                Skip::AtLeast(3),
            ),
            case(
                "x is not null and x > 5000",
                vec![is_not_null("x"), x(">", "5000")],
                Skip::Exactly(3),
            ),
            case(
                "x is null and id < 100 (the all-NULL group 0 holds ids below 100)",
                vec![is_null("x"), flt("id", "<", "100", "100")],
                Skip::Exactly(3),
            ),
            case(
                "x = 5000 or nothing",
                vec![x("=", "5000")],
                Skip::Exactly(3),
            ),
        ];
        for op in OPS {
            for v in ["-1", "0", "7", "2048", "4097", "9999999999999999999999"] {
                cases.push(case(&format!("x {op} {v}"), vec![x(op, v)], Skip::Any));
            }
        }
        n += run(&duck, &file, w, None, &cases, &tag);
        n += run(&duck, &file, w, Some(&["id"]), &cases[..13], &tag);
    }
    eprintln!("DIFFERENTIAL nullsome: {n} cases");
}

// ------------------------------------------------------------------------------------------------------------------------------ integers, dates, timestamps, booleans

#[test]
fn integers_dates_timestamps_booleans() {
    let Some(duck) = duckdb_or_skip("ints") else {
        return;
    };
    let dir = scratch("edge-ints");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "ints", &["default", "delta", "ms"]) {
        let ms = tag.ends_with("-ms");
        let mut cases = Vec::new();
        // each column: (name, literal of the start of group 3, last value, text of a literal below the type's minimum)
        for (col, g3, last) in [
            ("i8", "30", "36"),
            ("i16", "3000", "3099"),
            ("i32", "18432", "24573"),
            ("i64", "18432000000000", "24573000000000"),
            ("dt", "2008-10-27", "2014-07-22"),
        ] {
            let sql = |l: &str| {
                if col == "dt" {
                    format!("DATE '{l}'")
                } else {
                    l.to_string()
                }
            };
            for op in OPS {
                let lits: [&str; 5] = if col == "dt" {
                    [g3, last, "1992-01-01", "1970-01-01", "0001-01-01"]
                } else {
                    [g3, last, "1992", "0", "-1"]
                };
                for l in lits {
                    cases.push(case(
                        &format!("{col} {op} {l}"),
                        vec![flt(col, op, l, &sql(l))],
                        Skip::Any,
                    ));
                }
            }
            cases.push(case(
                &format!("{col} >= start of group 3"),
                vec![flt(col, ">=", g3, &sql(g3))],
                Skip::Exactly(3),
            ));
            cases.push(case(
                &format!("{col} > last"),
                vec![flt(col, ">", last, &sql(last))],
                Skip::All,
            ));
        }
        // literals beyond the column's width are never cast to it
        cases.push(case(
            "i8 > 1000 (above every tinyint)",
            vec![flt("i8", ">", "1000", "1000")],
            Skip::All,
        ));
        cases.push(case(
            "i8 < 1000 (all rows)",
            vec![flt("i8", "<", "1000", "1000")],
            Skip::Zero,
        ));
        cases.push(case(
            "i8 = 300 (would be 44 as a tinyint)",
            vec![flt("i8", "=", "300", "300")],
            Skip::All,
        ));
        cases.push(case(
            "i32 < 5000000000 (all rows)",
            vec![flt("i32", "<", "5000000000", "5000000000")],
            Skip::Zero,
        ));
        cases.push(case(
            "i32 = 4294967296 (would be 0)",
            vec![flt("i32", "=", "4294967296", "4294967296")],
            Skip::All,
        ));
        cases.push(case(
            "i16 <> -40000",
            vec![flt("i16", "<>", "-40000", "-40000")],
            Skip::Zero,
        ));
        cases.push(case(
            "i64 > 99999999999999999999 (beyond i64: row filter only)",
            vec![flt(
                "i64",
                ">",
                "99999999999999999999",
                "99999999999999999999",
            )],
            Skip::Zero,
        ));
        // unsigned types are never pruned on their values
        for (col, g3) in [("u8", "150"), ("u32", "18432"), ("u64", "18432000000000")] {
            for op in OPS {
                cases.push(case(
                    &format!("{col} {op} {g3}"),
                    vec![flt(col, op, g3, g3)],
                    Skip::Zero,
                ));
            }
        }
        // timestamps: group 3 starts at 2020-01-01 02:33:36
        let ts = |op: &str, l: &str| flt("ts", op, l, &format!("TIMESTAMP '{l}'"));
        for op in OPS {
            for l in [
                "2020-01-01 00:00:00",
                "2020-01-01 02:33:36",
                "2020-01-01 02:33:35.999999",
                "2020-01-01 02:33:36.000001",
                "2020-01-01 04:15:00.5",
                "1999-12-31 23:59:59",
            ] {
                cases.push(case(&format!("ts {op} {l}"), vec![ts(op, l)], Skip::Any));
            }
        }
        cases.push(case(
            "ts >= start of group 3",
            vec![ts(">=", "2020-01-01 02:33:36")],
            Skip::Exactly(3),
        ));
        cases.push(case(
            "ts > last",
            vec![ts(">", "2020-01-01 04:15:58.5")],
            Skip::All,
        ));
        cases.push(case(
            "ts T-separator and quotes",
            vec![flt(
                "ts",
                ">=",
                "'2020-01-01T02:33:36'",
                "TIMESTAMP '2020-01-01 02:33:36'",
            )],
            Skip::Exactly(3),
        ));
        let _ = ms;
        // booleans: false in groups 0 and 1, true in groups 2 and 3
        for op in OPS {
            for v in ["true", "false"] {
                cases.push(case(
                    &format!("b {op} {v}"),
                    vec![flt("b", op, v, v)],
                    Skip::Any,
                ));
            }
        }
        cases.push(case(
            "b = true",
            vec![flt("b", "=", "true", "true")],
            Skip::Exactly(2),
        ));
        cases.push(case(
            "b = false",
            vec![flt("b", "=", "false", "false")],
            Skip::Exactly(2),
        ));
        cases.push(case("b is null", vec![is_null("b")], Skip::All));
        n += run(&duck, &file, w, None, &cases, &tag);
        n += run(
            &duck,
            &file,
            w,
            Some(&["id", "u32", "ts"]),
            &cases[..20],
            &tag,
        );
    }
    eprintln!("DIFFERENTIAL ints: {n} cases");
}

// ------------------------------------------------------------------------------------------------------------------------------ bytes

#[test]
fn bytes_are_compared_as_unsigned() {
    let Some(duck) = duckdb_or_skip("blobs") else {
        return;
    };
    let dir = scratch("edge-blob");
    let mut n = 0;
    for (file, w, tag) in variants(&duck, &dir, "blobs", &["default"]) {
        // group 0: 00xxxx, group 1: 7fxxxx, group 2: 80xxxx, group 3: ffxxxx (xxxx = the row id in 4 hex digits)
        let b =
            |op: &str, hex: &str| flt("b", op, &format!("X'{hex}'"), &format!("from_hex('{hex}')"));
        let mut cases = vec![
            case(
                "b >= 0x80 skips groups 0 and 1",
                vec![b(">=", "80")],
                Skip::Exactly(2),
            ),
            case(
                "b < 0x80 skips groups 2 and 3",
                vec![b("<", "80")],
                Skip::Exactly(2),
            ),
            case("b > 0x7fffff", vec![b(">", "7fffff")], Skip::Exactly(2)),
            case("b = 0xff0000", vec![b("=", "ff0000")], Skip::Any),
            case("b < 0x00", vec![b("<", "00")], Skip::All),
            case("b > 0xffffff", vec![b(">", "ffffff")], Skip::All),
            case("b >= 0x00 prunes nothing", vec![b(">=", "00")], Skip::Zero),
        ];
        for op in OPS {
            for hex in [
                "00",
                "0000",
                "7f",
                "7fffff",
                "80",
                "800000",
                "ff",
                "ff0000",
                "ffffff",
                "fffffffff0",
            ] {
                cases.push(case(&format!("b {op} {hex}"), vec![b(op, hex)], Skip::Any));
            }
        }
        n += run(&duck, &file, w, None, &cases, &tag);
    }
    eprintln!("DIFFERENTIAL blobs: {n} cases");
}
