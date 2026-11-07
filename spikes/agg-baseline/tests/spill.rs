#![cfg(not(nata_loom))]
use agg_baseline::spill::*;
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestRunner},
};
use std::sync::atomic::{AtomicUsize, Ordering};
// Test-only allocation observer. It records requested sizes and delegates every
// operation unchanged to System. Reads run serially in the reproduction command.
struct Observed;
static MAX_ALLOC: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOC: Observed = Observed;
unsafe impl std::alloc::GlobalAlloc for Observed {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        MAX_ALLOC.fetch_max(layout.size(), Ordering::Relaxed);
        // SAFETY: forwards the allocator contract unchanged.
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: System allocated this pointer with this layout.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        MAX_ALLOC.fetch_max(size, Ordering::Relaxed);
        // SAFETY: forwards the allocator contract unchanged.
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
}
static SEQ: AtomicUsize = AtomicUsize::new(0);
fn file() -> std::path::PathBuf {
    let root = std::env::var("TMPDIR").expect("set TMPDIR to an on-disk directory");
    std::path::PathBuf::from(root).join(format!(
        "agg-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}
fn roundtrip(states: &[State], format: Format) {
    let p = file();
    write(&p, states, format).unwrap();
    assert_eq!(read(&p, 8 * MAX_PAYLOAD).unwrap(), states);
    assert!(read(&p, 8 * MAX_PAYLOAD - 1).is_err());
    std::fs::remove_file(p).unwrap();
}
#[test]
fn seeded_exact_roundtrips() {
    let cfg = Config {
        cases: 64,
        rng_seed: RngSeed::Fixed(0x0005_2026_1107),
        failure_persistence: None,
        ..Config::default()
    };
    TestRunner::new(cfg)
        .run(&(any::<u64>(), 0usize..129), |(seed, n)| {
            let states = generated(seed, n);
            roundtrip(&states, Format::Ipc);
            roundtrip(&states, Format::RecordControl);
            Ok(())
        })
        .unwrap();
    let mut extremes = generated(5, 2);
    for (i, s) in extremes.iter_mut().enumerate() {
        let v = if i == 0 {
            10i128.pow(38) - 1
        } else {
            -10i128.pow(38) + 1
        };
        s.key.decimal = Some(v);
        s.sum = Some(v);
        s.avg_sum = Some(v);
        s.min = Some(v);
        s.max = Some(v);
    }
    for f in [Format::Ipc, Format::RecordControl] {
        roundtrip(&extremes, f);
    }
    for n in [0, 1, MAX_ROWS] {
        for f in [Format::Ipc, Format::RecordControl] {
            roundtrip(&generated(5, n), f);
        }
    }
}
static MAX_BAD_ALLOC: AtomicUsize = AtomicUsize::new(0);
fn reject(frame: &[u8]) {
    let p = file();
    std::fs::write(&p, frame).unwrap();
    // A panic is a failing test; external timeout and ulimit catch hangs/OOM.
    MAX_ALLOC.store(0, Ordering::SeqCst);
    assert!(read(&p, 8 * MAX_PAYLOAD).is_err(), "accepted damaged frame");
    let largest = MAX_ALLOC.load(Ordering::SeqCst);
    assert!(largest <= MAX_PAYLOAD, "oversize allocation {largest}");
    MAX_BAD_ALLOC.fetch_max(largest, Ordering::SeqCst);
    std::fs::remove_file(p).unwrap();
}
/// Location of a Flatbuffer scalar in test fixtures, independent of Arrow's
/// accessor. No unsafe code or unchecked pointer dereference.
fn field_loc(bytes: &[u8], table: usize, slot: usize) -> usize {
    let back = i32::from_le_bytes(bytes[table..table + 4].try_into().unwrap());
    let vt = (table as i64 - back as i64) as usize;
    let off = u16::from_le_bytes(bytes[vt + slot..vt + slot + 2].try_into().unwrap()) as usize;
    assert_ne!(off, 0);
    table + off
}
#[test]
fn damaged_files() {
    let mut cases = 0;
    for f in [Format::Ipc, Format::RecordControl] {
        let good = encode(&generated(5, 32), f).unwrap();
        for n in [0, 1, 7, 31, 32, 33, good.len() / 2, good.len() - 1] {
            reject(&good[..n]);
            cases += 1;
        }
        for range in [16..20, 20..28] {
            let mut b = good.clone();
            b[range].fill(255);
            seal(&mut b);
            reject(&b);
            cases += 1;
        }
        // Every single-bit flip position, sampled at fixed stride, no CRC repair.
        for pos in (0..good.len()).step_by(31) {
            let mut b = good.clone();
            b[pos] ^= 1;
            reject(&b);
            cases += 1;
        }
        let mut b = good.clone();
        b.push(0);
        reject(&b);
        cases += 1;
        match f {
            Format::RecordControl => {
                let mut b = good.clone();
                b[HEADER..HEADER + 4].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
                seal(&mut b);
                reject(&b);
                cases += 1;
            }
            Format::Ipc => {
                let mut p = HEADER;
                let mut messages = Vec::new();
                while p + 8 < good.len() {
                    let n = u32::from_le_bytes(good[p + 4..p + 8].try_into().unwrap()) as usize;
                    if n == 0 {
                        break;
                    }
                    let start = p + 8;
                    let m = arrow::ipc::root_as_message(&good[start..start + n]).unwrap();
                    messages.push((p, start, n, m.bodyLength() as usize));
                    p = start + n + m.bodyLength() as usize;
                }
                for (prefix, start, n, body) in messages {
                    // CRC-repaired malicious metadata and body lengths.
                    let mut b = good.clone();
                    b[prefix + 4..prefix + 8].copy_from_slice(&u32::MAX.to_le_bytes());
                    seal(&mut b);
                    reject(&b);
                    cases += 1;
                    if body > 0 {
                        let table = start
                            + u32::from_le_bytes(good[start..start + 4].try_into().unwrap())
                                as usize;
                        let loc =
                            field_loc(&good, table, arrow::ipc::Message::VT_BODYLENGTH as usize);
                        let mut b = good.clone();
                        b[loc..loc + 8].copy_from_slice(&i64::MAX.to_le_bytes());
                        seal(&mut b);
                        reject(&b);
                        cases += 1;
                        let m = arrow::ipc::root_as_message(&good[start..start + n]).unwrap();
                        let rb = m
                            .header_as_record_batch()
                            .or_else(|| m.header_as_dictionary_batch().and_then(|d| d.data()))
                            .unwrap();
                        let loc = field_loc(
                            &good,
                            start + rb._tab.loc(),
                            arrow::ipc::RecordBatch::VT_LENGTH as usize,
                        );
                        let mut b = good.clone();
                        b[loc..loc + 8].copy_from_slice(&i64::MAX.to_le_bytes());
                        seal(&mut b);
                        reject(&b);
                        cases += 1;
                        // The buffer struct is inline in the validated metadata.
                        let buf = rb.buffers().unwrap().get(0);
                        let loc = buf as *const _ as usize - good.as_ptr() as usize;
                        let mut b = good.clone();
                        b[loc..loc + 8].copy_from_slice(&i64::MAX.to_le_bytes());
                        seal(&mut b);
                        reject(&b);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!(
        "damaged_cases={cases} all rejected; largest_decode_allocation={}",
        MAX_BAD_ALLOC.load(Ordering::SeqCst)
    );
}
#[test]
fn table_fit_and_reuse() {
    use agg_baseline::work::*;
    for collide in [false, true] {
        let r = rows(2048, collide);
        let expected = std_control(&r);
        let mut t = ReusedTable::default();
        assert_eq!(t.aggregate(&r), expected);
        let cap = t.capacity();
        assert_eq!(t.aggregate(&r), expected);
        assert_eq!(cap, t.capacity());
    }
}
#[test]
fn identical_scheduler_work() {
    use agg_baseline::work::*;
    for q in [Queue::Fifo, Queue::Deque] {
        for pin in [false, true] {
            schedule(q, pin, 2, 8, &rows(256, true));
        }
    }
}
