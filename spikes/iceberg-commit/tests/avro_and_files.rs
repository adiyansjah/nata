//! Unit tests of the file formats the spike writes: the Avro codec and the Iceberg v2 manifest, manifest list, data and
//! position-delete files, and the snapshot view that applies deletes by sequence number.
use iceberg_commit::avro::{self, Sch, Val};
use iceberg_commit::ice::{self, Entry, MFile, Row};
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ic-unit-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn varint_roundtrip_edge_values() {
    let schema = Sch::Rec("r".into(), vec![("x".into(), None, Sch::Long)]);
    let values = [0i64, 1, -1, 63, 64, -64, -65, 127, 128, 300, i32::MAX as i64, i32::MIN as i64, i64::MAX, i64::MIN];
    let recs: Vec<Val> = values.iter().map(|v| Val::Rec(vec![Val::Long(*v)])).collect();
    let bytes = avro::write_file(&schema, &[], &recs, 1);
    let back = avro::read_file(&bytes, &schema).unwrap();
    let got: Vec<i64> = back.records.iter().map(|r| r.rec()[0].long()).collect();
    assert_eq!(got, values);
}

#[test]
fn header_carries_schema_with_field_ids_and_metadata() {
    let bytes = avro::write_file(&ice::manifest_entry_schema(), &[("content", "data".into())], &[], 3);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"field-id\":134") && text.contains("\"field-id\":100"), "field ids must be in the header schema");
    assert!(text.contains("avro.schema") && text.contains("avro.codec"));
    let f = avro::read_file(&bytes, &ice::manifest_entry_schema()).unwrap();
    assert_eq!(f.meta("content"), Some("data"));
    assert!(f.records.is_empty());
}

#[test]
fn corrupt_files_are_rejected() {
    let schema = ice::data_schema();
    let rows: Vec<Val> = (0..5).map(|i| Val::Rec(vec![Val::Long(i), Val::Str(format!("k{i}"))])).collect();
    let good = avro::write_file(&schema, &[], &rows, 9);
    assert_eq!(avro::read_file(&good, &schema).unwrap().records.len(), 5);
    let mut bad_sync = good.clone();
    let n = bad_sync.len();
    bad_sync[n - 1] ^= 0xFF;
    assert!(avro::read_file(&bad_sync, &schema).is_err(), "a damaged sync marker must be detected");
    assert!(avro::read_file(&good[..good.len() - 7], &schema).is_err(), "a truncated file must be detected");
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    assert!(avro::read_file(&bad_magic, &schema).is_err());
}

#[test]
fn manifest_and_list_roundtrip() {
    let d = tmp("manifest");
    let base = d.display().to_string();
    let entries = vec![
        Entry::added(ice::CONTENT_DATA, &format!("{base}/a.avro"), 3, 100),
        Entry::deleted(ice::CONTENT_DATA, &format!("{base}/old.avro"), 5, 200),
    ];
    let m = ice::write_manifest(&format!("{base}/m.avro"), ice::CONTENT_DATA, &entries, 5).unwrap();
    assert_eq!((m.added_files, m.deleted_files, m.added_rows, m.deleted_rows), (1, 1, 3, 5));
    assert_eq!(ice::read_manifest(&m.path).unwrap(), entries);
    let mut list_entry: MFile = m.clone();
    list_entry.seq = 7;
    list_entry.min_seq = 7;
    list_entry.added_snapshot = 42;
    ice::write_manifest_list(&format!("{base}/l.avro"), 42, Some(41), 7, &[list_entry.clone()], 6).unwrap();
    assert_eq!(ice::read_manifest_list(&format!("{base}/l.avro")).unwrap(), vec![list_entry]);
}

#[test]
fn position_deletes_apply_by_sequence_number() {
    let d = tmp("view");
    let base = d.display().to_string();
    let rows: Vec<Row> = (0..4).map(|i| Row { id: i, key: if i % 2 == 0 { "a".into() } else { "b".into() } }).collect();
    let data = format!("{base}/d.avro");
    let size = ice::write_data_file(&data, &rows, 1).unwrap();
    let del = format!("{base}/x.avro");
    let dsize = ice::write_posdel_file(&del, &[(data.clone(), 0), (data.clone(), 2)], 2).unwrap();
    let m_data = ice::write_manifest(&format!("{base}/md.avro"), ice::CONTENT_DATA, &[Entry::added(ice::CONTENT_DATA, &data, 4, size)], 3).unwrap();
    let m_del = ice::write_manifest(&format!("{base}/mx.avro"), ice::CONTENT_POS_DELETES, &[Entry::added(ice::CONTENT_POS_DELETES, &del, 2, dsize)], 4).unwrap();
    let list = |seq_data: i64, seq_del: i64, name: &str| {
        let mut a = m_data.clone();
        a.seq = seq_data;
        a.added_snapshot = 1;
        let mut b = m_del.clone();
        b.seq = seq_del;
        b.added_snapshot = 2;
        let p = format!("{base}/{name}.avro");
        ice::write_manifest_list(&p, 2, Some(1), seq_del, &[a, b], 8).unwrap();
        p
    };
    // delete committed after the data (seq 2 >= 1): rows 0 and 2 are gone
    let v = ice::load_view(&list(1, 2, "l1"), false).unwrap();
    assert_eq!(v.live_rows().iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 3]);
    // same commit (seq equal): position deletes apply to data of the same commit
    let v = ice::load_view(&list(2, 2, "l2"), false).unwrap();
    assert_eq!(v.live_rows().len(), 2);
    // delete OLDER than the data (seq 1 < 2): it must not apply (a file rewritten after the delete)
    let v = ice::load_view(&list(2, 1, "l3"), false).unwrap();
    assert_eq!(v.live_rows().len(), 4);
}

#[test]
fn lenient_view_reports_a_missing_file_and_strict_view_fails() {
    let d = tmp("missing");
    let base = d.display().to_string();
    let data = format!("{base}/gone.avro");
    let m = ice::write_manifest(&format!("{base}/m.avro"), ice::CONTENT_DATA, &[Entry::added(ice::CONTENT_DATA, &data, 1, 1)], 3).unwrap();
    let mut e = m.clone();
    e.seq = 1;
    e.added_snapshot = 1;
    let l = format!("{base}/l.avro");
    ice::write_manifest_list(&l, 1, None, 1, &[e], 4).unwrap();
    assert!(ice::load_view(&l, false).is_err());
    let v = ice::load_view(&l, true).unwrap();
    assert_eq!(v.missing.len(), 1);
    assert!(v.missing[0].contains("gone.avro"));
}

#[test]
fn fault_plan_parses_and_rejects_garbage() {
    use iceberg_commit::catalog::FaultPlan;
    let p = FaultPlan::parse("drop-response@1, drop-request@3,dup@2,delay@1=200").unwrap();
    assert!(p.drop_response.contains(&1) && p.drop_request.contains(&3) && p.duplicate.contains(&2) && p.delay_ms[&1] == 200);
    assert!(FaultPlan::parse("").unwrap().is_empty());
    assert!(FaultPlan::parse("explode@1").is_err());
    assert!(FaultPlan::parse("drop-response@x").is_err());
}

#[test]
fn prng_is_deterministic_and_snapshot_ids_are_positive() {
    let mut a = iceberg_commit::Prng::new(5);
    let mut b = iceberg_commit::Prng::new(5);
    for _ in 0..100 {
        let x = a.snapshot_id();
        assert_eq!(x, b.snapshot_id());
        assert!(x > 0);
    }
}
