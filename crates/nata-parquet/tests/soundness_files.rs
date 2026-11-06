//! PROPERTY on real files: for random Parquet files (written by DuckDB, row groups of 2,048 rows with random per-group behaviour: clustered, constant,
//! all NULL, NaN-laden, mixed NULLs) and random predicates (1 to 3 terms over integers, doubles with NaN / -0.0 / infinities, decimals with literals at other
//! scales, dates, Unicode text, booleans, `IS [NOT] NULL`), the rows of the PRUNED scan equal the rows of the UNPRUNED scan equal the rows DuckDB returns
//! (compared as count, sum of ids and sum of squares of ids, ids being unique). Pruning is therefore sound end to end: footer, adapter, `can_skip`, row filter.
//! The generator reaches the interesting situations (the test counts the row groups skipped and requires many of them).
//!
//! `NATA_SCAN_FILES` (default 30) files, `NATA_SCAN_PREDICATES` (default 40) predicates per file, `NATA_SCAN_SEED` (default fixed) for a replay.

mod common;
use common::*;

use std::fmt::Write as _;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Behaviour {
    Clustered,
    Constant,
    AllNull,
    SomeNull,
    Wide,
    Special,
}

fn behaviour(rng: &mut Rng) -> Behaviour {
    [
        Behaviour::Clustered,
        Behaviour::Clustered,
        Behaviour::Constant,
        Behaviour::AllNull,
        Behaviour::SomeNull,
        Behaviour::Wide,
        Behaviour::Special,
    ][rng.below(7) as usize]
}

const WORDS: [&str; 10] = [
    "",
    "a",
    "apple",
    "été",
    "日本",
    "𝄞g",
    "zz",
    "Zebra",
    "apple pie",
    "é",
];

/// The model of one file: its CSV text and, per column, the values (as text, `None` = NULL) to draw literals from.
struct File {
    csv: String,
    pool: Vec<Vec<String>>,
}

const COLS: [&str; 7] = ["i", "d", "dec", "dt", "s", "b", "i64"];
const SQL_TYPES: [&str; 7] = [
    "INTEGER",
    "DOUBLE",
    "DECIMAL(10,2)",
    "DATE",
    "VARCHAR",
    "BOOLEAN",
    "BIGINT",
];

fn generate(rng: &mut Rng) -> File {
    let groups = 2 + rng.below(4) as usize; // 2..=5 row groups
    let rows = groups * 2048
        - if rng.chance(50) {
            rng.below(2000) as usize
        } else {
            0
        };
    let mut beh: Vec<Vec<Behaviour>> = (0..COLS.len())
        .map(|_| (0..groups).map(|_| behaviour(rng)).collect())
        .collect();
    beh[6] = vec![Behaviour::Clustered; groups]; // i64: always clustered, for guaranteed pruning
    let mut csv = String::new();
    let mut pool: Vec<Vec<String>> = vec![Vec::new(); COLS.len()];
    for r in 0..rows {
        let g = r / 2048;
        let mut cells: Vec<String> = Vec::new();
        for (c, _) in COLS.iter().enumerate() {
            let b = beh[c][g];
            let null = match b {
                Behaviour::AllNull => true,
                Behaviour::SomeNull => rng.chance(30),
                _ => rng.chance(2),
            };
            let cell = if null {
                None
            } else {
                Some(match c {
                    0 => {
                        let v = match b {
                            Behaviour::Constant => g as i64 * 7,
                            Behaviour::Wide => rng.range(-2_000_000_000, 2_000_000_000),
                            Behaviour::Special => {
                                [i64::from(i32::MIN), i64::from(i32::MAX), 0, -1, 1]
                                    [rng.below(5) as usize]
                            }
                            _ => g as i64 * 1000 + rng.range(0, 1500),
                        };
                        v.to_string()
                    }
                    1 => {
                        let v = match b {
                            Behaviour::Constant => g as f64 * 1.5,
                            Behaviour::Wide => rng.range(-1000, 1000) as f64 / 8.0,
                            Behaviour::Special => [
                                "nan", "inf", "-inf", "-0.0", "0.0", "nan", "1e308", "5e-324",
                            ][rng.below(8) as usize]
                                .parse::<f64>()
                                .unwrap_or(0.0),
                            _ => {
                                if rng.chance(3) {
                                    f64::NAN
                                } else {
                                    g as f64 * 100.0 + rng.range(0, 150) as f64 / 4.0
                                }
                            }
                        };
                        if v.is_nan() {
                            "nan".to_string()
                        } else if v.is_infinite() {
                            if v > 0.0 { "inf".into() } else { "-inf".into() }
                        } else {
                            format!("{v:?}")
                        }
                    }
                    2 => {
                        let v = match b {
                            Behaviour::Constant => g as i64 * 100,
                            Behaviour::Wide => rng.range(-99_999_999, 99_999_999),
                            Behaviour::Special => {
                                [0, -1, 1, 99_999_999, -99_999_999][rng.below(5) as usize]
                            }
                            _ => g as i64 * 5000 + rng.range(0, 7000),
                        };
                        format!(
                            "{}{}.{:02}",
                            if v < 0 { "-" } else { "" },
                            v.abs() / 100,
                            v.abs() % 100
                        )
                    }
                    3 => {
                        let day = match b {
                            Behaviour::Constant => 8000 + g as i64,
                            Behaviour::Wide => rng.range(0, 30000),
                            _ => 8000 + g as i64 * 400 + rng.range(0, 500),
                        };
                        let (y, m, d) = nata_parquet::civil::civil_from_days(day);
                        format!("{y:04}-{m:02}-{d:02}")
                    }
                    4 => match b {
                        Behaviour::Constant => WORDS[g % WORDS.len()].to_string(),
                        Behaviour::Wide | Behaviour::Special => format!(
                            "{}{}",
                            WORDS[rng.below(10) as usize],
                            WORDS[rng.below(10) as usize]
                        ),
                        _ => format!(
                            "{}{:03}",
                            WORDS[(g + (rng.below(2) as usize)) % WORDS.len()],
                            rng.below(40)
                        ),
                    },
                    5 => match b {
                        Behaviour::Constant => (g % 2 == 0).to_string(),
                        _ => rng.chance(50).to_string(),
                    },
                    _ => (g as i64 * 4_000_000_000 + rng.range(0, 3_000_000_000)).to_string(),
                })
            };
            match cell {
                Some(v) => {
                    let _ = write!(csv, "{}{}", if c > 0 { "," } else { "" }, v);
                    pool[c].push(v);
                }
                None => {
                    let _ = write!(csv, "{}NULL", if c > 0 { "," } else { "" });
                }
            }
            let _ = &mut cells;
        }
        csv.push('\n');
    }
    // CSV needs the id: prepend it in the table definition by position (id is generated by DuckDB with row_number)
    File { csv, pool }
}

/// One random predicate: the scan text and the SQL for each term.
fn predicate(rng: &mut Rng, pool: &[Vec<String>]) -> Vec<Filter> {
    let n = 1 + rng.below(3) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let c = rng.below(COLS.len() as u64) as usize;
        let name = COLS[c];
        let k = rng.below(10);
        if k == 0 {
            out.push(is_null(name));
            continue;
        }
        if k == 1 {
            out.push(is_not_null(name));
            continue;
        }
        let op = OPS[rng.below(6) as usize];
        // a literal: a value of the column, a neighbour of it, or a special
        let fallback = ["0", "0", "0.00", "1992-01-01", "x", "true", "0"][c];
        let base = if pool[c].is_empty() {
            fallback.to_string()
        } else {
            pool[c][rng.below(pool[c].len() as u64) as usize].clone()
        };
        let (scan, sql) = match c {
            0 | 6 => {
                let v: i128 = base.parse().unwrap_or(0);
                let v = match rng.below(6) {
                    0 => v + 1,
                    1 => v - 1,
                    2 => 5_000_000_000,
                    3 => -3_000_000_000,
                    _ => v,
                };
                (v.to_string(), v.to_string())
            }
            1 => {
                let specials = ["nan", "inf", "-inf", "0", "0.0", "-0.0", "1e308", "100.5"];
                let t = if rng.chance(40) {
                    specials[rng.below(8) as usize].to_string()
                } else {
                    base.clone()
                };
                let sql = match t.as_str() {
                    "nan" => "'NaN'::DOUBLE".to_string(),
                    "inf" => "'inf'::DOUBLE".to_string(),
                    "-inf" => "'-inf'::DOUBLE".to_string(),
                    other => format!("{other}::DOUBLE"),
                };
                (t, sql)
            }
            2 => {
                // fewer or more fraction digits than the column's 2
                let t = match rng.below(5) {
                    0 => base.split('.').next().unwrap_or("0").to_string(),
                    1 => format!("{base}0"),
                    2 => format!("{base}5"),
                    3 => format!(
                        "{}.{}",
                        base.split('.').next().unwrap_or("0"),
                        &base.split('.').nth(1).unwrap_or("00")[..1]
                    ),
                    _ => base.clone(),
                };
                (t.clone(), t)
            }
            3 => (base.clone(), format!("DATE '{base}'")),
            4 => {
                let q = base.replace('\'', "''");
                (format!("'{q}'"), format!("'{q}'"))
            }
            _ => (base.clone(), base.clone()),
        };
        out.push(flt(name, op, &scan, &sql));
    }
    out
}

fn ids(csv: &str) -> (u64, i128, i128) {
    let (mut n, mut s, mut sq) = (0u64, 0i128, 0i128);
    for line in records(csv).iter().skip(1) {
        let v: i128 = line.trim().parse().expect("id");
        n += 1;
        s += v;
        sq += v * v;
    }
    (n, s, sq)
}

#[test]
fn pruned_equals_unpruned_equals_duckdb_on_random_files_and_predicates() {
    let Some(duck) = duckdb_or_skip("soundness_files") else {
        return;
    };
    let files: u64 = std::env::var("NATA_SCAN_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let preds: usize = std::env::var("NATA_SCAN_PREDICATES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let seed: u64 = std::env::var("NATA_SCAN_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5EED_0001);
    let dir = scratch("soundness");
    let (mut cases, mut skipped, mut total_rg, mut nonempty, mut with_skips) =
        (0usize, 0u64, 0u64, 0usize, 0usize);
    for fi in 0..files {
        let mut rng = Rng((seed ^ (fi.wrapping_mul(0x9E37_79B9_7F4A_7C15))) | 1);
        let f = generate(&mut rng);
        let csv_path = dir.join(format!("f{fi}.csv"));
        let pq = dir.join(format!("f{fi}.parquet"));
        std::fs::write(&csv_path, &f.csv).expect("csv");
        let cols = COLS
            .iter()
            .zip(SQL_TYPES)
            .map(|(c, t)| format!("'{c}': '{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        duckdb_csv(
            &duck,
            &format!(
                "COPY (SELECT row_number() OVER ()::BIGINT AS id, * FROM read_csv('{}', header=false, nullstr='NULL', columns={{{cols}}}, auto_detect=false) ) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048)",
                csv_path.display(),
                pq.display()
            ),
        );
        let mut specs = Vec::new();
        for _ in 0..preds {
            let filters = predicate(&mut rng, &f.pool);
            specs.push(Spec {
                columns: Some(vec!["id".to_string()]),
                filters,
                expect: Skip::Any,
                what: format!("seed {seed} file {fi}"),
            });
        }
        // DuckDB's answers as (count, sum, sum of squares) in one process; the scan's the same way
        let sqls: Vec<String> = specs
            .iter()
            .map(|s| {
                let wh = s.filters.iter().map(|f| f.sql.clone()).collect::<Vec<_>>().join(" AND ");
                format!("SELECT count(*), coalesce(sum(id), 0), coalesce(sum(id::HUGEINT * id), 0) FROM (SELECT * FROM read_parquet('{}') LIMIT 9000000000000000000) WHERE {wh}", pq.display())
            })
            .collect();
        let answers = duckdb_batch(&duck, &sqls);
        for (s, (sql, answer)) in specs.iter().zip(sqls.iter().zip(&answers)) {
            let line = answer.lines().nth(1).unwrap_or("").to_string();
            let want: Vec<i128> = line
                .split(',')
                .map(|x| x.trim().parse().unwrap_or(-1))
                .collect();
            let texts: Vec<&str> = s.filters.iter().map(|f| f.scan.as_str()).collect();
            // count, sum and sum of squares do not depend on the order of the rows
            let pruned = unordered_ok(|| scan(&[&pq], Some(&["id"]), &texts, true, None))
                .unwrap_or_else(|e| panic!("seed {seed} file {fi}: {texts:?}: {e}"));
            let full = unordered_ok(|| scan(&[&pq], Some(&["id"]), &texts, false, None))
                .unwrap_or_else(|e| panic!("seed {seed} file {fi}: {texts:?}: {e}"));
            let (n, sum, sq) = ids(&pruned.csv);
            let (nf, sumf, sqf) = ids(&full.csv);
            assert_eq!(
                (n as i128, sum, sq),
                (want[0], want[1], want[2]),
                "PRUNED scan differs from DuckDB\n  {sql}\n  scan filters {texts:?}\n  report {}\n  replay: NATA_SCAN_SEED={seed} (file {fi})",
                pruned.report.line()
            );
            assert_eq!(
                (nf, sumf, sqf),
                (n, sum, sq),
                "unpruned scan differs from the pruned one: {texts:?}"
            );
            cases += 1;
            skipped += pruned.report.row_groups_skipped;
            total_rg += pruned.report.row_groups_total;
            if n > 0 {
                nonempty += 1;
            }
            if pruned.report.row_groups_skipped > 0 {
                with_skips += 1;
            }
        }
    }
    eprintln!(
        "SOUNDNESS-FILES: {cases} predicates over {files} files (seed {seed}): {nonempty} returned rows, {with_skips} skipped at least one row group, {skipped} of {total_rg} row groups skipped in all"
    );
    // the generator must reach both situations: pruning that happens, and predicates that match
    assert!(
        with_skips * 5 > cases,
        "too few cases prune anything ({with_skips} of {cases}): the test would prove little"
    );
    assert!(
        nonempty * 5 > cases,
        "too few predicates match a row ({nonempty} of {cases})"
    );
}
