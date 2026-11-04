//! Metadata-only tables of any size for the planning benchmark: manifests full of plausible data-file entries (partition tuple, counts, bounds for every column),
//! no data files behind them. Two layouts: `clustered` (files sorted by day, so a manifest covers a few days and the manifest list can prune) and `scattered`
//! (every manifest holds every day: only the entries can prune).
use crate::manifest::*;
use crate::meta::*;
use crate::table::{Table, simple_schema};
use crate::transform::Transform;
use crate::types::*;

pub const DAYS: i64 = 1000;
pub const BUCKETS: u32 = 16;

fn splitmix(mut x: u64) -> u64 { x = x.wrapping_add(0x9E37_79B9_7F4A_7C15); let mut z = x; z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); z ^ (z >> 31) }

pub fn schema() -> Schema {
    simple_schema(&[("id", Type::Long), ("k", Type::Int), ("day", Type::Date), ("ts", Type::Timestamp), ("name", Type::String), ("score", Type::Double), ("amt", Type::Decimal(10, 2)), ("blob", Type::Binary)])
}

fn file_entry(i: u64, n: u64, clustered: bool, snap: i64) -> Entry {
    let per_day = (n / DAYS as u64).max(1);
    let day = if clustered { (i / per_day).min(DAYS as u64 - 1) as i64 } else { (splitmix(i) % DAYS as u64) as i64 };
    let bucket = (splitmix(i ^ 0xB) % BUCKETS as u64) as i32;
    let epoch_day = 15_000 + day as i32;
    let id_lo = i as i64 * 1000;
    let mut f = DataFile { content: DATA, path: format!("/synthetic/data/d={epoch_day}/b={bucket}/f{i:09}.parquet"), format: "parquet".into(), record_count: 1000, size: 1 << 20,
                           partition: vec![Some(Value::Date(epoch_day)), Some(Value::Int(bucket))], sort_order_id: Some(0), ..Default::default() };
    let put = |f: &mut DataFile, id: i32, lo: Value, hi: Value, nulls: i64| {
        f.value_counts.insert(id, 1000); f.null_counts.insert(id, nulls);
        f.lower.insert(id, serialize(&lo)); f.upper.insert(id, serialize(&hi));
    };
    put(&mut f, 1, Value::Long(id_lo), Value::Long(id_lo + 999), 0);
    put(&mut f, 2, Value::Int(bucket * 1000), Value::Int(bucket * 1000 + 999), (splitmix(i) % 5) as i64);
    put(&mut f, 3, Value::Date(epoch_day), Value::Date(epoch_day), 0);
    let t0 = epoch_day as i64 * MICROS_PER_DAY;
    put(&mut f, 4, Value::Ts(t0), Value::Ts(t0 + MICROS_PER_DAY - 1), 0);
    let c = (b'a' + (splitmix(i ^ 0xC) % 26) as u8) as char;
    put(&mut f, 5, Value::Str(format!("{c}aaaaaaaaaaaaaaa")), Value::Str(format!("{c}zzzzzzzzzzzzzzz")), 0);
    put(&mut f, 6, Value::Double((i % 1000) as f64), Value::Double((i % 1000) as f64 + 500.0), 0);
    f.nan_counts.insert(6, 0);
    put(&mut f, 7, Value::Decimal((i % 5000) as i128), Value::Decimal((i % 5000) as i128 + 100_000), 0);
    put(&mut f, 8, Value::Bin(vec![0, c as u8]), Value::Bin(vec![255, c as u8]), 0);
    Entry { status: ADDED, snapshot_id: Some(snap), seq: None, file_seq: None, file: f }
}

/// Writes `files` data-file entries (and one position-delete entry per 100 files) into manifests of `per_manifest` entries, as one snapshot.
pub fn build(dir: &std::path::Path, files: u64, clustered: bool, per_manifest: usize) -> Table {
    let schema = schema();
    let spec = Spec { id: 0, fields: vec![PartField { source_id: 4, field_id: 1000, name: "ts_day".into(), transform: Transform::Day }, PartField { source_id: 2, field_id: 1001, name: "k_bucket".into(), transform: Transform::Bucket(BUCKETS) }] };
    // the partition tuple of the synthetic files is (day as date, bucket): `ts_day` is a Date, `k_bucket` an Int
    let mut t = Table::create(dir, schema, spec.clone(), 7);
    let snap = 1_000_000 + 3;
    let mut list = vec![];
    let mut i = 0u64;
    while i < files {
        let end = (i + per_manifest as u64).min(files);
        let mut entries: Vec<Entry> = (i..end).map(|j| file_entry(j, files, clustered, snap)).collect();
        // a few position deletes pointing at data files of the same manifest, in the same partition
        let dels: Vec<Entry> = (i..end).step_by(100).map(|j| { let d = &entries[(j - i) as usize].file; Entry { status: ADDED, snapshot_id: Some(snap), seq: None, file_seq: None,
            file: DataFile { content: POS_DELETES, path: format!("{}.del", d.path), format: "parquet".into(), partition: d.partition.clone(), record_count: 3, size: 1000, referenced: Some(d.path.clone()), ..Default::default() } } }).collect();
        let m = t.write_manifest_file(&spec, DATA, &std::mem::take(&mut entries), snap, 1);
        list.push(m);
        if !dels.is_empty() { list.push(t.write_manifest_file(&spec, 1, &dels, snap, 1)); }
        i = end;
    }
    t.snapshot_commit("append", list);
    t
}
