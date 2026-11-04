//! The verifier must FAIL on files with known-bad statistics (and pass on legitimately loose ones): a check is evidence only if it can fail.
use std::sync::Arc;

use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::file::metadata::SortingColumn;

use spike_parquet_write::data::{self, Profile};
use spike_parquet_write::faulty::{FAULTS, write_fault};
use spike_parquet_write::verify::*;

fn tmp(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("pqw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn each_known_bad_statistic_is_flagged_and_the_controls_pass() {
    let d = tmp("faults");
    for (name, want) in FAULTS {
        let p = d.join(format!("{name}.parquet"));
        write_fault(&p, name);
        let r = verify_file(&p, &VerifyOpts { trunc: Some(8), ..Default::default() });
        match want {
            Some(kind) => assert!(r.error_kinds().contains(kind), "{name}: expected `{kind}`, got {:?}", r.violations.iter().map(|v| (v.kind, v.detail.clone())).collect::<Vec<_>>()),
            None => assert!(r.errors().is_empty(), "{name}: a control must pass, got {:?}", r.violations.iter().map(|v| (v.kind, v.detail.clone())).collect::<Vec<_>>()),
        }
    }
}

#[test]
fn a_file_that_claims_to_be_sorted_but_is_not_is_flagged() {
    let d = tmp("unsorted");
    let batches = data::table(1, 4000, 4000, &Profile::default());
    let props = WriterProperties::builder().set_sorting_columns(Some(vec![SortingColumn { column_idx: 3, descending: false, nulls_first: false }])).build(); // i32: random
    let p = d.join("u.parquet");
    let mut w = ArrowWriter::try_new(std::fs::File::create(&p).unwrap(), data::schema(), Some(props)).unwrap();
    w.write(&batches[0]).unwrap();
    w.close().unwrap();
    assert!(verify_file(&p, &VerifyOpts::default()).error_kinds().contains("unsorted"));
}

#[test]
fn field_id_problems_are_flagged() {
    let d = tmp("ids");
    let batches = data::table(1, 100, 100, &Profile::default());
    // 1. no ids at all (the schema without the metadata)
    let plain = Arc::new(arrow::datatypes::Schema::new(data::schema().fields().iter().map(|f| arrow::datatypes::Field::new(f.name(), f.data_type().clone(), f.is_nullable())).collect::<Vec<_>>()));
    let b = arrow::array::RecordBatch::try_new(plain.clone(), batches[0].columns().to_vec()).unwrap_or_else(|_| batches[0].clone());
    let p = d.join("noids.parquet");
    let mut w = ArrowWriter::try_new(std::fs::File::create(&p).unwrap(), plain, None).unwrap();
    w.write(&b).unwrap();
    w.close().unwrap();
    let r = verify_file(&p, &VerifyOpts { field_ids: Some(data::expected_field_ids()), ..Default::default() });
    assert!(r.error_kinds().contains("field-id-missing"), "{:?}", r.error_kinds());
    // 2. a wrong id expectation on a file that has ids
    let good = spike_parquet_write::write::write_files(&d, "good", &data::schema(), &batches, &Default::default()).remove(0);
    let mut exp = data::expected_field_ids();
    exp[1].1 += 1000;
    assert!(verify_file(&good, &VerifyOpts { field_ids: Some(exp), ..Default::default() }).error_kinds().contains("field-id-wrong"));
    assert!(verify_file(&good, &VerifyOpts { field_ids: Some(data::expected_field_ids()), ..Default::default() }).errors().is_empty());
}
