//! Seeded fuzzers for what comes from outside: the RESPONSES of the store (before the client crate sees them) and the BYTES of the object (before the
//! Parquet decoder sees them), through the real wrapper and the real scan. Sized by environment variables so one source serves the short run of
//! `cargo test` and the nightly size: `NATA_OBJ_FUZZ_CASES` (default 150), `NATA_OBJ_FUZZ_SEED` (default 20261107). A failure prints the seed and case
//! number; the same two values replay it exactly.
//!
//! Invariants: every read returns exactly the object's bytes or a bounded error (at most 4 attempts, inside the deadline, never a wrong byte); a scan
//! returns the rows of the identical local file or an error (never other rows, never a hang); a damaged object is an error, not a panic.

#[path = "../cases/mod.rs"]
mod cases;

use std::time::{Duration, Instant};

use cases::fault::{OBJ_LEN, Rig, object_bytes};
use cases::testdata::lineitem_like;
use nata_objfake::{Fault, Plan, Rng};
use nata_objstore::policy::{Config, Mutants};
use nata_objstore::remote::Kind;

/// `NATA_OBJ_FUZZ_ONLY=<case>` replays one case of the run.
fn only_case() -> Option<u64> {
    std::env::var("NATA_OBJ_FUZZ_ONLY")
        .ok()
        .and_then(|v| v.parse().ok())
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One random fault. No stalls here: a stall costs the attempt timeout per attempt and the bulk fuzz would spend its time asleep (stalls have their own
/// cases); everything else the HTTP layer can be made to say is in the list.
fn random_fault(r: &mut Rng) -> Fault {
    match r.below(12) {
        0 => Fault::Status {
            code: [500, 502, 503, 504][r.below(4) as usize],
            retry_after: None,
            body: r.below(300) as usize,
            endless: false,
        },
        1 => Fault::Status {
            code: 429,
            retry_after: [None, Some(0), Some(1)][r.below(3) as usize],
            body: r.below(50) as usize,
            endless: false,
        },
        2 => Fault::Status {
            code: [403, 404, 416][r.below(3) as usize],
            retry_after: None,
            body: r.below(50) as usize,
            endless: false,
        },
        3 => Fault::Reset,
        4 => Fault::TruncateAfter {
            after: r.below(60_000) as usize,
        },
        5 => Fault::TruncateChunked {
            after: r.below(60_000) as usize,
        },
        6 => Fault::RangeClaim {
            start_delta: r.below(2000) as i64 - 1000,
            end_delta: r.below(2000) as i64 - 1000,
        },
        7 => Fault::LengthMismatch {
            delta: r.below(200) as i64 - 100,
        },
        8 => Fault::IgnoreRange,
        9 => Fault::IgnoreIfMatch,
        10 => Fault::Delay { ms: r.below(20) },
        _ => {
            // raw bytes: random garbage, or a plausible status line followed by garbage headers
            let mut bytes = Vec::new();
            if r.below(2) == 0 {
                bytes.extend_from_slice(b"HTTP/1.1 206 Partial Content\r\n");
            }
            for _ in 0..r.below(200) {
                bytes.push(r.next_u64() as u8);
            }
            Fault::Raw { bytes }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_responses_never_produce_a_wrong_byte_or_an_unbounded_read() {
    let (cases, seed) = (
        env_u64("NATA_OBJ_FUZZ_CASES", 150),
        env_u64("NATA_OBJ_FUZZ_SEED", 20_261_107),
    );
    let cfg = Config {
        attempt_timeout: Duration::from_secs(2),
        op_deadline: Duration::from_secs(8),
        backoff_initial: Duration::from_millis(2),
        backoff_max: Duration::from_millis(10),
        ..Config::fast_tests()
    };
    let rig = Rig::new(cfg.clone(), Mutants::default()).await;
    let id = rig.id().await;
    let data = object_bytes(1);
    let (mut ok, mut failed) = (0u64, 0u64);
    for case in 0..cases {
        if only_case().is_some_and(|o| o != case) {
            continue;
        }
        let mut r = Rng::new(seed ^ case.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let faults: Vec<Fault> = (0..1 + r.below(4)).map(|_| random_fault(&mut r)).collect();
        rig.fake.set_plan(Plan {
            seeded: Some((r.next_u64(), 100 + r.below(700) as u32, faults)),
            ..Default::default()
        });
        let reads: Vec<_> = (0..6)
            .map(|_| {
                let a = r.below(OBJ_LEN as u64 - 1);
                let len = 1 + r.below(200_000).min(OBJ_LEN as u64 - 1 - a);
                let (q, t, idc) = (rig.q.clone(), rig.target.clone(), id.clone());
                tokio::spawn(async move {
                    let t0 = Instant::now();
                    (
                        a,
                        len,
                        q.fetch_range(&t, &idc, a..a + len, Kind::Data)
                            .await
                            .map(|f| f.data),
                        t0.elapsed(),
                    )
                })
            })
            .collect();
        for h in reads {
            let (a, len, res, took) = h.await.expect("a read must not panic");
            match res {
                Ok(b) => {
                    ok += 1;
                    assert_eq!(
                        b.as_ref(),
                        &data[a as usize..(a + len) as usize],
                        "seed {seed} case {case}: WRONG BYTES for {a}+{len}"
                    );
                }
                Err(e) => {
                    failed += 1;
                    assert!(
                        e.attempts <= 4,
                        "seed {seed} case {case}: {} attempts ({:?})",
                        e.attempts,
                        e.class
                    );
                    assert!(
                        took < cfg.op_deadline + Duration::from_secs(2),
                        "seed {seed} case {case}: took {took:?} ({:?})",
                        e.class
                    );
                }
            }
        }
    }
    println!(
        "transport fuzz: seed {seed}, {cases} cases, {ok} reads exact, {failed} bounded errors"
    );
    assert!(
        only_case().is_some() || (ok > 0 && failed > 0),
        "the fuzz must exercise both outcomes: {ok} ok, {failed} failed"
    );
}

/// Random damage to a valid object, read through the store and the scan: an answer or an error in bounded time, and when it answers the rows are those
/// of the same bytes read as a local file.
#[test]
fn damaged_objects_are_errors_or_the_local_answer_and_never_a_hang() {
    use cases::scan_support::ScanRig;
    let (cases, seed) = (
        env_u64("NATA_OBJ_FUZZ_CASES", 150),
        env_u64("NATA_OBJ_FUZZ_SEED", 20_261_107),
    );
    let rig = ScanRig::new("fuzz");
    let clean = lineitem_like(4000, 1000, 5);
    let (mut answered, mut errors) = (0u64, 0u64);
    for case in 0..cases {
        if only_case().is_some_and(|o| o != case) {
            continue;
        }
        let mut r = Rng::new(seed ^ case.wrapping_mul(0xD1B5_4A32_D192_ED03));
        let mut bytes = clean.clone();
        match r.below(4) {
            0 => {
                for _ in 0..1 + r.below(8) {
                    let at = r.below(bytes.len() as u64) as usize;
                    bytes[at] = r.next_u64() as u8;
                }
            }
            1 => bytes.truncate(r.below(bytes.len() as u64) as usize),
            2 => {
                let from = r.below(bytes.len() as u64) as usize;
                let to = (from + r.below(200) as usize).min(bytes.len());
                for b in &mut bytes[from..to] {
                    *b = r.next_u64() as u8;
                }
            }
            _ => {
                // damage the footer length and the thrift near the end
                let n = bytes.len();
                for _ in 0..1 + r.below(6) {
                    let at = n - 1 - r.below(300.min(n as u64 - 1)) as usize;
                    bytes[at] = r.next_u64() as u8;
                }
            }
        }
        rig.put("d.parquet", bytes);
        let q = "SELECT l_orderkey, l_comment FROM '{d}/d.parquet' WHERE l_orderkey > 10";
        let t0 = Instant::now();
        let local = rig.local_result(q);
        let remote = rig.remote_result(q);
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "seed {seed} case {case}: hang ({:?})",
            t0.elapsed()
        );
        match (&local, &remote) {
            (Ok(a), Ok(b)) => {
                answered += 1;
                assert_eq!(
                    a, b,
                    "seed {seed} case {case}: the object answers differently from the identical local file"
                );
            }
            (Err(_), Err(_)) => errors += 1,
            (l, r) => panic!(
                "seed {seed} case {case}: local {:?} but remote {:?}",
                l.as_ref().map(String::len),
                r.as_ref().map(String::len)
            ),
        }
    }
    println!(
        "damaged-object fuzz: seed {seed}, {cases} cases, {answered} answered like the local file, {errors} errors alike"
    );
}
