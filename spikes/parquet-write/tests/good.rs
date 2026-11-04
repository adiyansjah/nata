//! Files written by our writer must pass the independent verifier, for many data profiles and writer options.
use std::path::PathBuf;

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::EnabledStatistics;

use spike_parquet_write::data::{self, Profile};
use spike_parquet_write::verify::*;
use spike_parquet_write::write::*;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("pqw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn vopts(o: &Opts) -> VerifyOpts {
    VerifyOpts { field_ids: Some(data::expected_field_ids()), trunc: o.trunc, index_trunc: o.index_trunc, check_bloom: o.bloom }
}

fn read_all(files: &[PathBuf]) -> RecordBatch {
    let mut bs = vec![];
    for f in files {
        for b in ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(f).unwrap()).unwrap().build().unwrap() { bs.push(b.unwrap()); }
    }
    concat_batches(&bs[0].schema(), &bs).unwrap()
}

fn show(r: &Report) -> String {
    r.violations.iter().take(12).map(|v| format!("[{:?}] rg{:?} {} {}: {}", v.sev, v.rg, v.col, v.kind, v.detail)).collect::<Vec<_>>().join("\n")
}

#[test]
fn good_files_pass_the_verifier_for_many_options() {
    let schema = data::schema();
    let mut checked_pages = 0;
    let mut seen: std::collections::BTreeMap<(String, &'static str, String), (u64, String)> = Default::default();
    for (seed, rows, chunk, profile, opts) in [
        (1u64, 3000usize, 1000usize, Profile::default(), Opts::default()),
        (2, 20000, 4096, Profile::default(), Opts { row_group_rows: 3000, page_rows: 400, ..Default::default() }),
        (3, 12000, 5000, Profile { null_pct: 0, special_pct: 0, max_len: 10 }, Opts::default()),
        (4, 12000, 5000, Profile { null_pct: 60, special_pct: 40, max_len: 200 }, Opts { trunc: Some(8), index_trunc: Some(8), ..Default::default() }),
        (5, 9000, 3000, Profile::default(), Opts { stats: EnabledStatistics::Chunk, ..Default::default() }),
        (6, 9000, 3000, Profile::default(), Opts { bloom: true, ..Default::default() }),
        (7, 9000, 3000, Profile::default(), Opts { dict: false, compression: Compression::UNCOMPRESSED, ..Default::default() }),
        (8, 9000, 3000, Profile::default(), Opts { trunc: None, index_trunc: None, ..Default::default() }),
        (9, 6000, 2000, Profile { null_pct: 100, special_pct: 0, max_len: 5 }, Opts::default()),
        (10, 1, 1, Profile::default(), Opts::default()),
    ] {
        let batches = data::table(seed, rows, chunk, &profile);
        let files = write_files(&tmp(&format!("good{seed}")), "f", &schema, &batches, &opts);
        for f in &files {
            let r = verify_file(f, &vopts(&opts));
            checked_pages += r.pages;
            for v in &r.violations { seen.insert((format!("{:?}", v.sev), v.kind, v.col.clone()), (seed, v.detail.clone())); }
        }
        let back = read_all(&files);
        assert_eq!(back.num_rows(), rows, "seed {seed}: row count after the round trip");
    }
    let errs: Vec<_> = seen.iter().filter(|(k, _)| k.0 == "Error").collect();
    for ((sev, kind, col), (seed, d)) in &seen { eprintln!("{sev:5} {kind:22} {col:12} (first at seed {seed}) {d}"); }
    assert!(errs.is_empty(), "{} distinct error kinds on good files (see above)", errs.len());
    assert!(checked_pages > 100, "the page index was not exercised ({checked_pages} pages)");
}
