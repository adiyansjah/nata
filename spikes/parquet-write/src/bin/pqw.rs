//! pqw gen <dir> [--rows N] [--seed S]            write the adversarial table as <dir>/src.parquet
//! pqw rewrite <src.parquet> <dir> <config>        rewrite it with one of the writer configurations (see CONFIGS)
//! pqw verify [--ids] [--trunc N] [--bloom] <file...>   independent verification; exit 1 on any error
//! pqw faults <dir>                                write the known-bad statistic files
use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::EnabledStatistics;
use spike_parquet_write::data::{self, Profile};
use spike_parquet_write::faulty::{FAULTS, write_fault};
use spike_parquet_write::verify::{VerifyOpts, verify_file};
use spike_parquet_write::write::*;

const CONFIGS: &[&str] = &["default", "small-groups", "trunc8", "chunk-stats", "bloom", "nodict-uncompressed", "no-truncation", "sorted-id-desc", "sorted-u8-id", "rolling", "unmitigated"];

fn opts(name: &str) -> Opts {
    let d = Opts::default();
    match name {
        "default" => d,
        "small-groups" => Opts { row_group_rows: 1500, page_rows: 200, ..d },
        "trunc8" => Opts { trunc: Some(8), index_trunc: Some(8), ..d },
        "chunk-stats" => Opts { stats: EnabledStatistics::Chunk, ..d },
        "bloom" => Opts { bloom: true, ..d },
        "nodict-uncompressed" => Opts { dict: false, compression: Compression::UNCOMPRESSED, ..d },
        "no-truncation" => Opts { trunc: None, index_trunc: None, ..d },
        "sorted-id-desc" => Opts { sort: vec![SortKey { col: 0, desc: true, nulls_first: false }], row_group_rows: 4000, ..d },
        "sorted-u8-id" => Opts { sort: vec![SortKey { col: 5, desc: false, nulls_first: true }, SortKey { col: 0, desc: false, nulls_first: false }], row_group_rows: 4000, ..d },
        "unmitigated" => Opts { nan_mitigation: false, row_group_rows: 5000, ..d },
        "rolling" => Opts { target_file_bytes: Some(600_000), row_group_rows: 2000, ..d },
        other => { eprintln!("unknown config {other}; known: {CONFIGS:?}"); std::process::exit(2) }
    }
}

fn read(path: &Path) -> (arrow::datatypes::SchemaRef, Vec<RecordBatch>) {
    let b = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(path).unwrap()).unwrap();
    let schema = b.schema().clone();
    (schema, b.build().unwrap().map(|x| x.unwrap()).collect())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let get = |k: &str, d: &str| a.iter().position(|x| x == k).map_or(d.to_string(), |i| a[i + 1].clone());
    match a.get(1).map(String::as_str) {
        Some("gen") => {
            let dir = PathBuf::from(&a[2]);
            let batches = data::table(get("--seed", "1").parse().unwrap(), get("--rows", "60000").parse().unwrap(), 8192, &Profile::default());
            let f = write_files(&dir, "src", &data::schema(), &batches, &Opts { row_group_rows: 20000, ..Default::default() });
            println!("{}", f[0].display());
        }
        Some("rewrite") => {
            let (schema, batches) = read(Path::new(&a[2]));
            let o = opts(&a[4]);
            for f in write_files(Path::new(&a[3]), &a[4], &schema, &batches, &o) { println!("{}", f.display()); }
        }
        Some("verify") => {
            let files: Vec<&String> = a[2..].iter().filter(|x| !x.starts_with("--") && Path::new(x).exists()).collect();
            let vo = VerifyOpts { field_ids: a.contains(&"--ids".into()).then(data::expected_field_ids), trunc: a.iter().position(|x| x == "--trunc").map(|i| a[i + 1].parse().unwrap()),
                                  index_trunc: a.iter().position(|x| x == "--trunc").map(|i| a[i + 1].parse().unwrap()), check_bloom: a.contains(&"--bloom".into()) };
            let (mut errs, mut warns, mut kinds) = (0, 0, std::collections::BTreeMap::<String, usize>::new());
            for f in &files {
                let r = verify_file(Path::new(f), &vo);
                errs += r.errors().len(); warns += r.warns().len();
                for v in &r.violations { *kinds.entry(format!("{:?} {} {}", v.sev, v.kind, v.col)).or_default() += 1; }
                println!("{f}: {} chunks, {} pages, {} values checked; {} errors, {} warnings", r.chunks, r.pages, r.values, r.errors().len(), r.warns().len());
            }
            for (k, n) in &kinds { println!("  {n:5} x {k}"); }
            std::process::exit(if errs > 0 { 1 } else { 0 });
        }
        Some("faults") => {
            let dir = PathBuf::from(&a[2]);
            std::fs::create_dir_all(&dir).unwrap();
            for (n, want) in FAULTS { let p = dir.join(format!("{n}.parquet")); write_fault(&p, n); println!("{}\t{}", p.display(), want.unwrap_or("control")); }
        }
        _ => { eprintln!("usage: pqw gen|rewrite|verify|faults ... (configs: {CONFIGS:?})"); std::process::exit(2) }
    }
}
