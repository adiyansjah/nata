//! Tests of the hand-written manifest decoder (src/hand.rs) against the reference reader of the iceberg-plan spike, and against bad input.
//! Every test that could hang runs under a timeout: a hang is a failure. `mutants/run.sh` re-runs this file against deliberately broken builds.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mdeclib::hand::{data_offset, decode_manifest};
use mdeclib::{canon_generic, canon_hand, dec_generic, recode_bytes};

fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).unwrap_or_else(|_| panic!("TIMED OUT after {secs} s (a hang, or a panic in the thread): {what}"))
}

/// Manifests (uncompressed) of a small synthetic table: 4,000 files in manifests of 1,000, plus one position-delete manifest for each.
fn sample() -> &'static Vec<Vec<u8>> {
    static S: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    S.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("mdec-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        iceplan::synth::build(&dir, 4000, true, 1000);
        let mut names: Vec<_> = std::fs::read_dir(dir.join("metadata")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().ends_with("-m.avro")).collect();
        names.sort();
        let v: Vec<Vec<u8>> = names.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(v.len() >= 8, "expected data and delete manifests, got {}", v.len());
        v
    })
}

const CODECS: [&str; 4] = ["null", "deflate", "snappy", "zstandard"];

/// The data manifest with the most entries.
fn biggest(codec: &str) -> Vec<u8> {
    let b = sample().iter().max_by_key(|b| b.len()).unwrap();
    if codec == "null" { b.clone() } else { recode_bytes(b, codec) }
}

#[test]
fn matches_the_reference_reader_entry_for_entry_on_every_codec() {
    for codec in CODECS {
        let mut entries = 0;
        for b in sample() {
            let b = if codec == "null" { b.clone() } else { recode_bytes(b, codec) };
            let want = canon_generic(&dec_generic(&b).unwrap());
            let got = canon_hand(&decode_manifest(&b, None).unwrap_or_else(|e| panic!("{codec}: {e}")));
            assert_eq!(want.len(), got.len(), "{codec}: entry count");
            for (i, (w, g)) in want.iter().zip(&got).enumerate() {
                assert_eq!(w, g, "{codec}: entry {i}");
            }
            entries += want.len();
        }
        assert!(entries >= 4000, "{codec}: only {entries} entries compared");
    }
}

#[test]
fn keeps_delete_manifests_their_content_flag_and_their_referenced_files() {
    let mut deletes = 0;
    for b in sample() {
        let c = decode_manifest(b, None).unwrap();
        if c.content == 1 {
            deletes += 1;
            assert!(c.entries.iter().all(|e| e.content == 1 && e.referenced.len > 0), "a position delete without its referenced data file");
        }
    }
    assert!(deletes > 0, "the sample has no delete manifest");
}

#[test]
fn the_column_filter_keeps_only_the_wanted_columns_and_changes_nothing_else() {
    let b = biggest("null");
    let full = canon_hand(&decode_manifest(&b, None).unwrap());
    let part = canon_hand(&decode_manifest(&b, Some(&[1, 3])).unwrap());
    assert_eq!(full.len(), part.len());
    for (f, p) in full.iter().zip(&part) {
        let keep = |m: &std::collections::BTreeMap<i32, Vec<u8>>| m.iter().filter(|(k, _)| **k == 1 || **k == 3).map(|(k, v)| (*k, v.clone())).collect::<std::collections::BTreeMap<_, _>>();
        let keepc = |m: &std::collections::BTreeMap<i32, i64>| m.iter().filter(|(k, _)| **k == 1 || **k == 3).map(|(k, v)| (*k, *v)).collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!((&f.path, &f.part, f.rc, f.size, f.status, f.content), (&p.path, &p.part, p.rc, p.size, p.status, p.content));
        assert_eq!(keep(&f.lower), p.lower);
        assert_eq!(keep(&f.upper), p.upper);
        assert_eq!(keepc(&f.nulls), p.nulls);
        assert_eq!(keepc(&f.vals), p.vals);
        assert_eq!(keepc(&f.nans), p.nans);
    }
    assert!(part.iter().any(|p| !p.lower.is_empty()), "the filter kept nothing");
}

#[test]
fn input_that_is_not_a_container_is_an_error_not_a_panic() {
    for junk in [&b""[..], b"Obj", b"Obj\x01", b"Obj\x01\x00", b"not avro at all, just text", &[0xFF; 64][..], &[0u8; 64][..]] {
        assert!(decode_manifest(junk, None).is_err(), "{junk:?}");
    }
}

#[test]
fn a_truncated_file_is_never_decoded_as_the_complete_one_and_never_panics() {
    for codec in ["null", "snappy"] {
        let b = biggest(codec);
        let full = decode_manifest(&b, None).unwrap().entries.len();
        let mut cuts: Vec<usize> = (0..400.min(b.len())).collect();
        cuts.extend((400..b.len()).step_by(137));
        cuts.push(b.len() - 1);
        for cut in cuts {
            let r = catch_unwind(AssertUnwindSafe(|| decode_manifest(&b[..cut], None))).unwrap_or_else(|_| panic!("{codec}: panic at cut {cut}"));
            // Avro has no end-of-file marker: a cut exactly at a block boundary is a valid (shorter) file. It must never claim the full count.
            if let Ok(c) = r {
                assert!(c.entries.len() < full, "{codec}: a file cut at {cut} of {} decoded {} entries, the full count", b.len(), c.entries.len());
            }
        }
    }
}

/// A tiny deterministic generator.
fn next(x: &mut u64) -> u64 {
    *x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *x >> 33
}

#[test]
fn flipping_any_byte_never_panics_or_hangs_and_a_checksummed_block_never_decodes_wrongly() {
    let (mut ok_same, mut ok_diff, mut errs) = (0, 0, 0);
    let mut snappy_wrong = 0;
    for codec in ["null", "snappy"] {
        let b = biggest(codec);
        let reference = canon_hand(&decode_manifest(&b, None).unwrap());
        let data_start = data_offset(&b).unwrap();
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        for _ in 0..1500 {
            let pos = if codec == "snappy" { data_start + (next(&mut seed) as usize) % (b.len() - data_start) } else { (next(&mut seed) as usize) % b.len() };
            let mut bad = b.clone();
            bad[pos] ^= 1 << (next(&mut seed) % 8);
            let t = Instant::now();
            let r = catch_unwind(AssertUnwindSafe(|| decode_manifest(&bad, None))).unwrap_or_else(|_| panic!("{codec}: panic after flipping byte {pos}"));
            assert!(t.elapsed() < Duration::from_secs(2), "{codec}: byte {pos} made the decoder run for {:?}", t.elapsed());
            match r {
                Err(_) => errs += 1,
                Ok(c) => {
                    for e in &c.entries {
                        let r = e.path.off as usize..(e.path.off + e.path.len) as usize;
                        assert!(std::str::from_utf8(&c.strings[r]).is_ok(), "{codec}: byte {pos} produced a file path that is not valid UTF-8");
                    }
                    if canon_hand(&c) == reference {
                        ok_same += 1;
                    } else {
                        ok_diff += 1;
                        if codec == "snappy" {
                            snappy_wrong += 1;
                        }
                    }
                }
            }
        }
    }
    println!("flip outcomes over 3000 flips: error {errs}, same result {ok_same}, silently different {ok_diff} (of which in snappy blocks: {snappy_wrong})");
    assert_eq!(snappy_wrong, 0, "a flipped byte in a snappy block decoded to a different result: the checksum is not enforced");
    assert!(errs > 1000, "only {errs} of 3000 corruptions were detected");
}

// ---- hand-built containers for hostile counts

fn zz(v: i64, out: &mut Vec<u8>) {
    let mut u = ((v << 1) ^ (v >> 63)) as u64;
    loop {
        let b = (u & 0x7F) as u8;
        u >>= 7;
        if u == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn container(schema: &str, count: i64, block: &[u8]) -> Vec<u8> {
    let sync = [7u8; 16];
    let mut o = b"Obj\x01".to_vec();
    zz(1, &mut o);
    zz(11, &mut o);
    o.extend_from_slice(b"avro.schema");
    zz(schema.len() as i64, &mut o);
    o.extend_from_slice(schema.as_bytes());
    zz(0, &mut o);
    o.extend_from_slice(&sync);
    zz(count, &mut o);
    zz(block.len() as i64, &mut o);
    o.extend_from_slice(block);
    o.extend_from_slice(&sync);
    o
}

/// A manifest-shaped schema whose `junk` field is an array of `null`: its items take ZERO bytes each.
const JUNK_SCHEMA: &str = r#"{"type":"record","name":"manifest_entry","fields":[
  {"name":"status","type":"int"},
  {"name":"data_file","type":{"type":"record","name":"data_file","fields":[
     {"name":"file_path","type":"string"},{"name":"record_count","type":"long"},{"name":"file_size_in_bytes","type":"long"},
     {"name":"junk","type":{"type":"array","items":"null"}}]}}]}"#;

fn one_entry(junk_count: i64) -> Vec<u8> {
    let mut b = vec![];
    zz(1, &mut b); // status
    zz(1, &mut b);
    b.push(b'x'); // file_path "x"
    zz(10, &mut b); // record_count
    zz(20, &mut b); // size
    if junk_count != 0 {
        zz(junk_count, &mut b); // one block of that many items (null items take no bytes)
    }
    zz(0, &mut b); // end of the array
    b
}

#[test]
fn the_container_builder_itself_produces_a_decodable_file() {
    let c = decode_manifest(&container(JUNK_SCHEMA, 1, &one_entry(0)), None).expect("a plain entry decodes");
    assert_eq!(c.entries.len(), 1);
    assert_eq!((c.entries[0].record_count, c.entries[0].size), (10, 20));
}

#[test]
fn a_huge_count_of_zero_size_items_is_rejected_at_once_instead_of_looping() {
    within(5, "zero-size items", || {
        let r = decode_manifest(&container(JUNK_SCHEMA, 1, &one_entry(1 << 60)), None);
        assert!(r.is_err(), "a count of 2^60 null items was accepted");
    });
}

#[test]
fn a_block_that_claims_more_records_than_it_has_bytes_is_rejected_without_reserving_memory() {
    within(5, "record count", || {
        let r = decode_manifest(&container(JUNK_SCHEMA, 1 << 40, &one_entry(0)), None);
        assert!(r.is_err());
    });
}

#[test]
fn a_corrupted_sync_marker_is_detected() {
    for codec in ["null", "snappy"] {
        let mut b = biggest(codec);
        let n = b.len();
        b[n - 1] ^= 0xFF; // the sync marker that ends the last block
        assert!(decode_manifest(&b, None).is_err(), "{codec}: a wrong sync marker was accepted");
    }
}

#[test]
fn an_unknown_codec_is_an_error_and_not_read_as_raw_bytes() {
    let mut b = biggest("null");
    // rewrite the codec name in the header: "null" is not present as avro.codec in null-codec files from this writer, so use a snappy file
    let mut s = biggest("snappy");
    let pos = s.windows(6).position(|w| w == b"snappy").expect("codec name in header");
    s[pos..pos + 6].copy_from_slice(b"snappx");
    assert!(decode_manifest(&s, None).is_err());
    b.clear();
}
