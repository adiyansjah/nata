//! Shared pieces of the bake-off: the canonical form that decoders are compared in, the generic reference arm, the hand-written decoder's
//! canonical form, and a re-encoder to other Avro codecs. Public so that tests/hand.rs can use them.
use std::collections::BTreeMap;

use iceplan::manifest as im;
use iceplan::types::{Type, Value};

pub mod hand;

type R<T> = Result<T, String>;

#[derive(Debug, PartialEq)]
pub struct Canon {
    pub status: i64,
    pub snap: Option<i64>,
    pub seq: Option<i64>,
    pub fseq: Option<i64>,
    pub content: i64,
    pub path: String,
    pub part: Vec<Option<i64>>,
    pub rc: i64,
    pub size: i64,
    pub nulls: BTreeMap<i32, i64>,
    pub vals: BTreeMap<i32, i64>,
    pub nans: BTreeMap<i32, i64>,
    pub lower: BTreeMap<i32, Vec<u8>>,
    pub upper: BTreeMap<i32, Vec<u8>>,
    pub eq: Vec<i32>,
    pub referenced: Option<String>,
}


pub fn part_types() -> Vec<Type> {
    vec![Type::Date, Type::Int] // the synthetic table's spec: (ts_day as date, k_bucket as int)
}

pub fn pval(v: &Option<Value>) -> Option<i64> {
    match v {
        Some(Value::Date(x) | Value::Int(x)) => Some(*x as i64),
        Some(Value::Long(x) | Value::Ts(x)) => Some(*x),
        _ => None,
    }
}


// ----- arm 1: the spike's generic reader
pub fn dec_generic(bytes: &[u8]) -> R<Vec<im::Entry>> {
    im::read_manifest(bytes, &part_types()).map(|(_, e)| e)
}
pub fn canon_generic(es: &[im::Entry]) -> Vec<Canon> {
    es.iter()
        .map(|e| Canon {
            status: e.status as i64,
            snap: e.snapshot_id,
            seq: e.seq,
            fseq: e.file_seq,
            content: e.file.content as i64,
            path: e.file.path.clone(),
            part: e.file.partition.iter().map(pval).collect(),
            rc: e.file.record_count,
            size: e.file.size,
            nulls: e.file.null_counts.clone(),
            vals: e.file.value_counts.clone(),
            nans: e.file.nan_counts.clone(),
            lower: e.file.lower.clone(),
            upper: e.file.upper.clone(),
            eq: e.file.equality_ids.clone(),
            referenced: e.file.referenced.clone(),
        })
        .collect()
}


// ----- arm 3: hand-written
pub fn canon_hand(c: &hand::Compact) -> Vec<Canon> {
    let rng = |r: hand::Range| r.off as usize..(r.off + r.len) as usize;
    let cnt = |r: hand::Range| c.counts[rng(r)].iter().map(|(k, v)| (*k, *v)).collect::<BTreeMap<_, _>>();
    let bnd = |r: hand::Range| c.bounds[rng(r)].iter().map(|(k, o, l)| (*k, c.bytes[*o as usize..(*o + *l) as usize].to_vec())).collect::<BTreeMap<_, _>>();
    let st = |r: hand::Range| String::from_utf8(c.strings[rng(r)].to_vec()).unwrap();
    c.entries
        .iter()
        .map(|e| Canon {
            status: e.status as i64,
            snap: e.has_snapshot.then_some(e.snapshot_id),
            seq: e.has_seq.then_some(e.seq),
            fseq: e.has_fseq.then_some(e.fseq),
            content: e.content as i64,
            path: st(e.path),
            part: c.part[rng(e.part)].iter().enumerate().map(|(i, v)| if i < 32 && e.part_nulls & (1 << i) != 0 { None } else { Some(*v) }).collect(),
            rc: e.record_count,
            size: e.size,
            nulls: cnt(e.null_counts),
            vals: cnt(e.value_counts),
            nans: cnt(e.nan_counts),
            lower: bnd(e.lower),
            upper: bnd(e.upper),
            eq: c.ids[rng(e.eq_ids)].to_vec(),
            referenced: (e.referenced.len > 0).then(|| st(e.referenced)),
        })
        .collect()
}


/// Rewrites one synthetic-table manifest with another Avro codec: null, deflate, snappy or zstandard (the spec and schema of `iceplan::synth`).
pub fn recode_bytes(b: &[u8], codec: &str) -> Vec<u8> {
    use apache_avro::Codec;
    use iceplan::meta::{PartField, Spec};
    use iceplan::transform::Transform;
    let codec = match codec {
        "null" => Codec::Null,
        "deflate" => Codec::Deflate(apache_avro::DeflateSettings::default()),
        "snappy" => Codec::Snappy,
        "zstandard" => Codec::Zstandard(apache_avro::ZstandardSettings::default()),
        other => panic!("unknown codec {other}"),
    };
    let spec = Spec { id: 0, fields: vec![PartField { source_id: 4, field_id: 1000, name: "ts_day".into(), transform: Transform::Day }, PartField { source_id: 2, field_id: 1001, name: "k_bucket".into(), transform: Transform::Bucket(iceplan::synth::BUCKETS) }] };
    let schema = iceplan::synth::schema();
    let (hdr, entries) = im::read_manifest(b, &part_types()).unwrap();
    im::write_manifest(&spec, &schema, hdr.content, &entries, codec).unwrap().0
}
