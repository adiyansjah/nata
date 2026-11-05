//! Manifest decoder bake-off (ADR 0005 rule 7). Three decoders on the same files:
//!   generic       the iceberg-plan spike's reader: apache-avro generic values, then typed entries
//!   iceberg-rust  `iceberg::spec::Manifest::parse_avro` (iceberg-rust 0.11)
//!   hand          src/hand.rs: schema-compiled decoder into compact arenas
//!
//!   mdec check <table-dir>                       decode every manifest with all three and compare entry by entry
//!   mdec check-x <dir>                           manifests written by ANOTHER engine (any *.avro under <dir>): hand against iceberg-rust
//!   mdec recode <table-dir> <out-dir> <null|deflate|snappy|zstandard>   rewrite every manifest with another Avro codec
//!   mdec bench <table-dir> <arm> <rounds> [--want 1,3] [--threads N]
//!                                                 time and memory of decoding all manifests of the table (one arm per process)
use mdeclib::hand;
use mdeclib::*;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use iceplan::manifest as im;

type R<T> = Result<T, String>;

fn manifests(table: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(table.join("metadata"))
        .expect("table/metadata")
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().ends_with("-m.avro"))
        .collect();
    v.sort();
    v.into_iter().map(|p| (p.file_name().unwrap().to_string_lossy().to_string(), std::fs::read(&p).unwrap())).collect()
}

// ----- arm 2: iceberg-rust
fn dec_rust(bytes: &[u8]) -> R<iceberg::spec::Manifest> {
    iceberg::spec::Manifest::parse_avro(bytes).map_err(|e| e.to_string())
}
fn canon_rust(m: &iceberg::spec::Manifest) -> Vec<Canon> {
    use iceberg::spec::{DataContentType, Literal, ManifestStatus, PrimitiveLiteral};
    let sorted_u = |h: &std::collections::HashMap<i32, u64>| h.iter().map(|(k, v)| (*k, *v as i64)).collect::<BTreeMap<_, _>>();
    let sorted_d = |h: &std::collections::HashMap<i32, iceberg::spec::Datum>| h.iter().map(|(k, v)| (*k, v.to_bytes().map(|b| b.to_vec()).unwrap_or_default())).collect::<BTreeMap<_, _>>();
    m.entries()
        .iter()
        .map(|e| {
            let d = e.data_file();
            Canon {
                status: match e.status() {
                    ManifestStatus::Existing => 0,
                    ManifestStatus::Added => 1,
                    ManifestStatus::Deleted => 2,
                },
                snap: e.snapshot_id(),
                seq: e.sequence_number(),
                fseq: e.file_sequence_number,
                content: match d.content_type() {
                    DataContentType::Data => 0,
                    DataContentType::PositionDeletes => 1,
                    DataContentType::EqualityDeletes => 2,
                },
                path: d.file_path().to_string(),
                part: d
                    .partition()
                    .iter()
                    .map(|l| match l {
                        Some(Literal::Primitive(PrimitiveLiteral::Int(x))) => Some(*x as i64),
                        Some(Literal::Primitive(PrimitiveLiteral::Long(x))) => Some(*x),
                        _ => None,
                    })
                    .collect(),
                rc: d.record_count() as i64,
                size: d.file_size_in_bytes() as i64,
                nulls: sorted_u(d.null_value_counts()),
                vals: sorted_u(d.value_counts()),
                nans: sorted_u(d.nan_value_counts()),
                lower: sorted_d(d.lower_bounds()),
                upper: sorted_d(d.upper_bounds()),
                eq: d.equality_ids().unwrap_or_default(),
                referenced: d.referenced_data_file(),
            }
        })
        .collect()
}

fn check(table: &Path) {
    let ms = manifests(table);
    let (mut entries, mut bad_rust, mut bad_hand) = (0usize, 0usize, 0usize);
    let mut first_rust = None;
    let mut first_hand = None;
    for (name, b) in &ms {
        let g = canon_generic(&dec_generic(b).unwrap_or_else(|e| panic!("generic {name}: {e}")));
        let r = canon_rust(&dec_rust(b).unwrap_or_else(|e| panic!("iceberg-rust {name}: {e}")));
        let h = canon_hand(&hand::decode_manifest(b, None).unwrap_or_else(|e| panic!("hand {name}: {e}")));
        entries += g.len();
        if g.len() != r.len() || g.len() != h.len() {
            println!("{name}: entry counts differ: generic {} rust {} hand {}", g.len(), r.len(), h.len());
            bad_rust += 1;
            bad_hand += 1;
            continue;
        }
        for i in 0..g.len() {
            if g[i] != r[i] {
                bad_rust += 1;
                first_rust.get_or_insert_with(|| format!("{name} entry {i}:\n  generic {:?}\n  rust    {:?}", g[i], r[i]));
            }
            if g[i] != h[i] {
                bad_hand += 1;
                first_hand.get_or_insert_with(|| format!("{name} entry {i}:\n  generic {:?}\n  hand    {:?}", g[i], h[i]));
            }
        }
    }
    println!("{} manifests, {entries} entries: iceberg-rust differs from generic in {bad_rust}, hand differs from generic in {bad_hand}", ms.len());
    if let Some(s) = first_rust {
        println!("first difference generic vs iceberg-rust:\n{s}");
    }
    if let Some(s) = first_hand {
        println!("first difference generic vs hand:\n{s}");
    }
    if bad_hand != 0 {
        std::process::exit(1);
    }
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "avro") {
            out.push(p);
        }
    }
}

/// Manifests written by another engine: the generic arm is not used (it needs the partition types of OUR synthetic table). Hand and iceberg-rust must
/// agree on everything they both keep. Partition values of types the hand decoder does not keep (strings, decimals, ...) are None on both sides.
fn check_x(dir: &Path) {
    let mut files = vec![];
    walk(dir, &mut files);
    files.sort();
    let (mut manifests, mut lists, mut entries, mut agree, mut differ, mut hand_err, mut rust_err, mut unsupported) = (0, 0, 0usize, 0, 0, 0, 0, 0u64);
    let mut codecs: BTreeMap<String, usize> = BTreeMap::new();
    let mut first = vec![];
    for f in &files {
        let b = std::fs::read(f).unwrap();
        let h = match hand::decode_manifest(&b, None) {
            Ok(h) => h,
            Err(e) if e.contains("no data_file record") => {
                lists += 1;
                continue;
            }
            Err(e) => {
                hand_err += 1;
                first.push(format!("hand failed on {}: {e}", f.display()));
                continue;
            }
        };
        manifests += 1;
        unsupported += h.unsupported_part_fields;
        if let Some(c) = codec_of(&b) {
            *codecs.entry(c).or_default() += 1;
        }
        let r = match dec_rust(&b) {
            Ok(r) => r,
            Err(e) => {
                rust_err += 1;
                first.push(format!("iceberg-rust failed on {}: {e}", f.display()));
                continue;
            }
        };
        let (ch, cr) = (canon_hand(&h), canon_rust(&r));
        entries += ch.len();
        if ch == cr {
            agree += 1;
        } else {
            differ += 1;
            let i = ch.iter().zip(&cr).position(|(a, b)| a != b);
            first.push(format!("differ on {} ({} vs {} entries), first at {:?}:\n  hand {:?}\n  rust {:?}", f.display(), ch.len(), cr.len(), i, i.map(|i| &ch[i]), i.map(|i| &cr[i])));
        }
    }
    println!("{} avro files: {manifests} manifests ({lists} manifest lists skipped), {entries} entries; hand and iceberg-rust agree on {agree} manifests, differ on {differ}; hand failed on {hand_err}, iceberg-rust failed on {rust_err}; partition fields of a type the hand decoder does not keep: {unsupported}; codecs {codecs:?}", files.len());
    for l in first.iter().take(4) {
        println!("{l}");
    }
    if differ + hand_err > 0 {
        std::process::exit(1);
    }
}

/// The avro.codec of a container, for reporting.
fn codec_of(b: &[u8]) -> Option<String> {
    let key = b"avro.codec";
    let p = b.windows(key.len()).position(|w| w == key)?;
    let rest = &b[p + key.len()..];
    let n = (rest.first()? >> 1) as usize; // zigzag length of a short string
    Some(String::from_utf8_lossy(rest.get(1..1 + n)?).to_string())
}

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0)
}

enum Held {
    G(Vec<Vec<im::Entry>>),
    I(Vec<iceberg::spec::Manifest>),
    H(Vec<hand::Compact>),
}

fn decode_all(arm: &str, ms: &[(String, Vec<u8>)], want: Option<&[i32]>, threads: usize) -> (Held, usize) {
    let run = |chunk: &[(String, Vec<u8>)]| -> Held {
        match arm {
            "generic" => Held::G(chunk.iter().map(|(_, b)| dec_generic(b).unwrap()).collect()),
            "iceberg-rust" => Held::I(chunk.iter().map(|(_, b)| dec_rust(b).unwrap()).collect()),
            "hand" => Held::H(chunk.iter().map(|(_, b)| hand::decode_manifest(b, want).unwrap()).collect()),
            other => panic!("unknown arm {other}"),
        }
    };
    let count = |h: &Held| match h {
        Held::G(v) => v.iter().map(Vec::len).sum(),
        Held::I(v) => v.iter().map(|m| m.entries().len()).sum(),
        Held::H(v) => v.iter().map(|c| c.entries.len()).sum(),
    };
    if threads <= 1 {
        let h = run(ms);
        let n = count(&h);
        return (h, n);
    }
    let per = ms.len().div_ceil(threads);
    let parts: Vec<Held> = std::thread::scope(|s| {
        let hs: Vec<_> = ms.chunks(per).map(|ch| s.spawn(move || run(ch))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let n = parts.iter().map(count).sum();
    // keep them all alive: merge into one Held of the same variant
    let merged = match arm {
        "generic" => Held::G(parts.into_iter().flat_map(|p| if let Held::G(v) = p { v } else { vec![] }).collect()),
        "iceberg-rust" => Held::I(parts.into_iter().flat_map(|p| if let Held::I(v) = p { v } else { vec![] }).collect()),
        _ => Held::H(parts.into_iter().flat_map(|p| if let Held::H(v) = p { v } else { vec![] }).collect()),
    };
    (merged, n)
}

fn bench(table: &Path, arm: &str, rounds: usize, want: Option<Vec<i32>>, threads: usize) {
    let ms = manifests(table);
    let input: usize = ms.iter().map(|(_, b)| b.len()).sum();
    let base_rss = status_kb("VmRSS:");
    let mut times = vec![];
    let mut entries = 0;
    let mut held: Option<Held> = None;
    for r in 0..rounds.max(1) {
        drop(held.take()); // free the previous round's result before decoding again
        let t = Instant::now();
        let (h, n) = decode_all(arm, &ms, want.as_deref(), threads);
        let el = t.elapsed().as_secs_f64();
        entries = n;
        held = Some(h);
        if r > 0 || rounds <= 1 {
            times.push(el);
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = times[times.len() / 2];
    let peak = status_kb("VmHWM:");
    let rss = status_kb("VmRSS:");
    let extra = match &held {
        Some(Held::H(v)) => format!(" arenas={} MiB", v.iter().map(|c| c.bytes_used()).sum::<usize>() >> 20),
        _ => String::new(),
    };
    println!(
        "arm={arm} threads={threads} want={:?} manifests={} entries={entries} input={} MiB | decode min {:.3} s median {:.3} s = {:.2} M entries/s {:.0} MB/s of manifest bytes | peak RSS above the loaded input {} MiB, retained result {} MiB (peak {} MiB, rss {} MiB, input RSS {} MiB){extra}",
        want, ms.len(), input >> 20, times[0], med, entries as f64 / med / 1e6, input as f64 / med / 1e6,
        (peak.saturating_sub(base_rss)) / 1024, (rss.saturating_sub(base_rss)) / 1024, peak / 1024, rss / 1024, base_rss / 1024,
    );
    drop(held);
}

fn recode(src: &Path, dst: &Path, codec: &str) {
    std::fs::create_dir_all(dst.join("metadata")).unwrap();
    for (name, b) in manifests(src) {
        std::fs::write(dst.join("metadata").join(name), recode_bytes(&b, codec)).unwrap();
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("check-x") => check_x(Path::new(&a[2])),
        Some("recode") => recode(Path::new(&a[2]), Path::new(&a[3]), &a[4]),
        Some("check") => check(Path::new(&a[2])),
        Some("bench") => {
            let mut want = None;
            let mut threads = 1;
            let mut i = 5;
            while i < a.len() {
                match a[i].as_str() {
                    "--want" => {
                        want = Some(a[i + 1].split(',').map(|x| x.parse().unwrap()).collect());
                        i += 2;
                    }
                    "--threads" => {
                        threads = a[i + 1].parse().unwrap();
                        i += 2;
                    }
                    other => panic!("unknown option {other}"),
                }
            }
            bench(Path::new(&a[2]), &a[3], a[4].parse().unwrap(), want, threads);
        }
        _ => eprintln!("usage: mdec check <table> | mdec bench <table> <generic|iceberg-rust|hand> <rounds> [--want 1,3] [--threads N]"),
    }
}
