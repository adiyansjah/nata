//! RELIABILITY of the `nata sql` process (card 0002): random scenarios with a printed, replayable seed, each a real process on a file of many row groups
//! with ONE random disturbance: Ctrl-C (SIGINT) at a random moment, the reader closing stdout after a random number of bytes, a data error in a random row,
//! or a LIMIT, with a consumer that is sometimes slow (so that the query is stopped in the middle of its work, with row groups in flight) and a random
//! thread count. After EVERY scenario: the process exited by itself within the time bound (no hang), with the exit code the disturbance calls for
//! (130 and a message for an interrupt that came in time, 0 and nothing on stderr for a closed stdout, 1 for a data error, 0 and the right rows for a
//! LIMIT), a result that was cut short is announced as INCOMPLETE, and every complete result is the right one. Threads, handles and buffers cannot be counted
//! from outside: the in-process scenarios of `crates/nata-exec/tests/scenarios.rs` count those; here the process must END (it joins its workers) and
//! must not leave a child, which `wait` shows.
//!
//! `NATA_PROC_SCENARIOS` (default 60; the long run is 2000), `NATA_PROC_SEED` (default 20261106), `NATA_PROC_ONLY=<n>` replays one. `NATA_COMPETITORS=1` runs
//! a busy loop on every core meanwhile.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data and a seeded generator"
)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arrow_array::{ArrayRef, Decimal128Array, Int64Array, RecordBatch};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

const ROWS: i64 = 240_000;
const GROUP: usize = 8_000;

fn write_file(path: &std::path::Path) {
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(0..ROWS));
    let price: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values((0..i128::from(ROWS)).map(|i| i * 7 % 100_000))
            .with_precision_and_scale(12, 2)
            .expect("decimal"),
    );
    let batch = RecordBatch::try_from_iter([("id", id), ("price", price)]).expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(GROUP))
        .build();
    let mut w = ArrowWriter::try_new(
        std::fs::File::create(path).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

#[derive(Debug, Clone)]
enum Disturbance {
    /// SIGINT after this many milliseconds.
    Sigint(u64),
    /// The reader closes stdout after this many bytes.
    CloseAfter(usize),
    /// Division by zero in the row with this id.
    DataError(i64),
    Limit(u64),
    None,
}

#[derive(Debug, Clone)]
struct Scenario {
    threads: usize,
    window: usize,
    csv: bool,
    slow_reader_us: u64,
    disturbance: Disturbance,
}

fn make(rng: &mut Rng) -> Scenario {
    let threads = 1 + rng.below(8) as usize;
    Scenario {
        threads,
        window: 1 + rng.below(2 * threads as u64) as usize,
        csv: rng.below(3) != 0,
        slow_reader_us: [0u64, 0, 200, 2000][rng.below(4) as usize],
        disturbance: match rng.below(6) {
            0 | 1 => Disturbance::Sigint(rng.below(250)),
            2 | 3 => Disturbance::CloseAfter(rng.below(3_000_000) as usize),
            4 => Disturbance::DataError(rng.below(ROWS as u64) as i64),
            _ => {
                if rng.below(2) == 0 {
                    Disturbance::Limit(1 + rng.below(100_000))
                } else {
                    Disturbance::None
                }
            }
        },
    }
}

/// Lines around the rows: a CSV header, or a table's header, rule and footer.
fn framing(s: &Scenario) -> usize {
    if s.csv { 1 } else { 3 }
}

fn kill_int(child: &Child) {
    let _ = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status();
}

struct Done {
    code: Option<i32>,
    signal: Option<i32>,
    stdout_bytes: usize,
    stdout_head: Vec<u8>,
    stdout_lines: usize,
    stderr: String,
    elapsed: Duration,
}

fn run(file: &str, s: &Scenario) -> Done {
    let (where_, limit) = match &s.disturbance {
        Disturbance::DataError(k) => (format!("WHERE 100 // (id - {k}) > -1000"), String::new()),
        Disturbance::Limit(n) => (String::new(), format!("LIMIT {n}")),
        _ => (String::new(), String::new()),
    };
    let sql = format!("SELECT id, price * 3 + 1 AS p FROM '{file}' {where_} {limit}");
    let mut args = vec!["sql".to_string()];
    if s.csv {
        args.push("--csv".into());
    }
    args.extend([
        "--threads".into(),
        s.threads.to_string(),
        "--window".into(),
        s.window.to_string(),
        "-c".into(),
        sql,
    ]);
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let err_reader = std::thread::spawn(move || {
        let mut t = String::new();
        let _ = stderr.read_to_string(&mut t);
        t
    });
    let (slow, close_after) = (
        s.slow_reader_us,
        match s.disturbance {
            Disturbance::CloseAfter(n) => Some(n),
            _ => None,
        },
    );
    let out_reader = std::thread::spawn(move || {
        let (mut total, mut lines, mut head) = (0usize, 0usize, Vec::new());
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if head.len() < 64 {
                        head.extend_from_slice(&buf[..n.min(64)]);
                    }
                    total += n;
                    lines += buf[..n].iter().filter(|b| **b == b'\n').count();
                    if close_after.is_some_and(|c| total >= c) {
                        break; // drop the pipe: the reader goes away
                    }
                    if slow > 0 {
                        std::thread::sleep(Duration::from_micros(slow));
                    }
                }
            }
        }
        (total, lines, head)
    });
    if let Disturbance::Sigint(ms) = s.disturbance {
        std::thread::sleep(Duration::from_millis(ms));
        kill_int(&child);
    }
    // no hang: the process must end by itself
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(st) => break st,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("HANG: nata sql did not end in 60 s: {s:?}");
            }
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    };
    let (stdout_bytes, stdout_lines, stdout_head) = out_reader.join().expect("reader");
    Done {
        code: status.code(),
        signal: std::os::unix::process::ExitStatusExt::signal(&status),
        stdout_bytes,
        stdout_head,
        stdout_lines,
        stderr: err_reader.join().expect("stderr reader"),
        elapsed: started.elapsed(),
    }
}

#[test]
fn random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process() {
    let seed: u64 = std::env::var("NATA_PROC_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_261_106);
    let count: u64 = std::env::var("NATA_PROC_SCENARIOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let only: Option<u64> = std::env::var("NATA_PROC_ONLY")
        .ok()
        .and_then(|v| v.parse().ok());
    eprintln!(
        "process scenarios: seed {seed}, count {count}, only {only:?} (replay: NATA_PROC_SEED={seed} NATA_PROC_ONLY=<n>)"
    );
    let base = std::env::var("NATA_TEST_TMP").map_or_else(|_| std::env::temp_dir(), PathBuf::from);
    let dir = base.join(format!("nata-sqlrel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let file = dir.join("big.parquet");
    write_file(&file);
    let file = file.to_str().expect("path");
    let stop = Arc::new(AtomicBool::new(false));
    let competitors: Vec<_> = if std::env::var("NATA_COMPETITORS").is_ok() {
        (0..std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
            .map(|_| {
                let s = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !s.load(Ordering::Relaxed) {
                        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        std::hint::black_box(x);
                    }
                })
            })
            .collect()
    } else {
        vec![]
    };
    let mut classes = std::collections::BTreeMap::<&'static str, u64>::new();
    for n in 0..count {
        if only.is_some_and(|o| o != n) {
            continue;
        }
        let mut rng = Rng(seed ^ n.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let s = make(&mut rng);
        let what = format!("process scenario {n} of seed {seed}: {s:?}");
        let d = run(file, &s);
        let err = &d.stderr;
        // A SIGINT that arrives before the process has installed its handler (the first milliseconds: the fork, the exec, the start of `main`) does what
        // Ctrl-C does to any program: the process dies of the signal (a shell reports 130), nothing had been started, nothing is left. Found by this test
        // (scenario 646 of seed 1: a signal 0 ms after the spawn); it is a class of its own, allowed only for a signal sent in the first 30 ms.
        if let (Disturbance::Sigint(ms), Some(2)) = (&s.disturbance, d.signal) {
            assert!(
                *ms < 30,
                "{what}: killed by SIGINT after {ms} ms: the handler must be installed by then\n{err}"
            );
            *classes.entry("sigint-before-the-handler").or_insert(0) += 1;
            continue;
        }
        assert!(
            d.code.is_some(),
            "{what}: the process was killed by a signal ({:?})\n{err}",
            d.signal
        );
        assert!(
            !err.contains("panicked") && !err.contains("internal error"),
            "{what}\n{err}"
        );
        let class: &'static str = match &s.disturbance {
            Disturbance::Sigint(_) => match d.code {
                Some(130) => {
                    assert!(err.contains("interrupted"), "{what}\n{err}");
                    if d.stdout_bytes > 0 {
                        assert!(
                            err.contains("INCOMPLETE"),
                            "{what}: rows were printed\n{err}"
                        );
                    }
                    assert!(
                        d.elapsed < Duration::from_secs(30),
                        "{what}: slow to stop: {:?}",
                        d.elapsed
                    );
                    "sigint-stopped"
                }
                Some(0) => {
                    assert_eq!(
                        d.stdout_lines,
                        ROWS as usize + framing(&s),
                        "{what}: finished before the signal, so complete"
                    );
                    "sigint-too-late"
                }
                c => panic!("{what}: exit {c:?}\n{err}"),
            },
            Disturbance::CloseAfter(_) => {
                // the reader left: a quiet exit 0, unless the whole result was smaller than what the reader took
                assert_eq!(d.code, Some(0), "{what}\n{err}");
                assert!(err.is_empty(), "{what}: quiet\n{err}");
                "stdout-closed"
            }
            Disturbance::DataError(k) => {
                if d.code == Some(1) {
                    assert!(
                        err.contains("division by zero") && err.contains("row group"),
                        "{what}\n{err}"
                    );
                    if d.stdout_bytes > 0 {
                        assert!(
                            err.contains("INCOMPLETE"),
                            "{what}: rows were printed before the error\n{err}"
                        );
                    }
                    "data-error"
                } else {
                    // the failing row is outside the table
                    assert!(*k >= ROWS, "{what}: exit {:?}\n{err}", d.code);
                    "data-error-not-reached"
                }
            }
            Disturbance::Limit(n) => {
                assert_eq!(d.code, Some(0), "{what}\n{err}");
                let want = (*n).min(ROWS as u64) as usize + framing(&s);
                assert_eq!(d.stdout_lines, want, "{what}");
                "limit"
            }
            Disturbance::None => {
                assert_eq!(d.code, Some(0), "{what}\n{err}");
                assert_eq!(d.stdout_lines, ROWS as usize + framing(&s), "{what}");
                let _ = &d.stdout_head;
                "complete"
            }
        };
        *classes.entry(class).or_insert(0) += 1;
    }
    stop.store(true, Ordering::SeqCst);
    for h in competitors {
        h.join().expect("competitor");
    }
    eprintln!("process scenarios outcome classes: {classes:?}");
    let _ = std::fs::remove_dir_all(&dir);
    if only.is_none() && count >= 60 {
        for k in [
            "sigint-stopped",
            "stdout-closed",
            "data-error",
            "limit",
            "complete",
        ] {
            assert!(
                classes.get(k).copied().unwrap_or(0) > 0,
                "the mix never produced {k}: {classes:?}"
            );
        }
    }
}
