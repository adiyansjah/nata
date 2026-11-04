//! A seed -> a scenario (workload, network and store faults, crashes, partitions, a cancel) -> one simulated run
//! -> the verdict of the invariant checks. Eight scenario families, chosen by `seed % 8`, so that any 8
//! consecutive seeds cover every kind of fault.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::engine;
use crate::exec::{Nanos, NodeId};
use crate::frame::*;
use crate::protocol::{self, COORD, Config, MS};
use crate::rng::Rng;
use crate::sim::*;

pub const FAMILIES: u64 = 8;
pub const DEADLINE: Nanos = 5_000 * MS;
pub const MAX_POLLS: u64 = 3_000_000;

pub const FAMILY_NAMES: [&str; 8] = [
    "calm",
    "lossy-network",
    "worker-crash",
    "crash-after-first-frame",
    "crash-at-final-frame",
    "partition",
    "cancel",
    "store-faults",
];

pub struct Scenario {
    pub seed: u64,
    pub family: u32,
    pub cfg: Config,
    pub files: Vec<Vec<i64>>,
    pub net: NetConfig,
    pub store: StoreConfig,
    pub timed: Vec<(Nanos, Ev)>,
    pub crash_points: Vec<CrashPoint>,
}

impl Scenario {
    pub fn from_seed(seed: u64) -> Scenario {
        let mut r = Rng::stream(seed, 0x5CE_AA10);
        let family = (seed % FAMILIES) as u32;
        let nfiles = r.range(5, 9) as u32;
        let batch_rows = r.range(6, 16) as usize;
        let window = r.range(2, 4) as u32;
        let mut files = Vec::new();
        for f in 0..nfiles {
            let n = r.range(40, 160) as i64;
            files.push((0..n).map(|i| f as i64 * 1_000_000 + i).collect::<Vec<i64>>());
        }
        let mut net = NetConfig { base: MS, jitter: r.range(1, 4) * MS, drop_ppm: 0, dup_ppm: 0, corrupt_ppm: 0 };
        let mut store = StoreConfig { lat_min: MS, lat_max: r.range(2, 6) * MS, fail_ppm: 0 };
        let mut timed: Vec<(Nanos, Ev)> = Vec::new();
        let mut cps: Vec<CrashPoint> = Vec::new();
        let ms = |x: u64| x * MS;
        match family {
            0 => {
                net.jitter = r.range(0, 2) * MS;
            }
            1 => {
                net.drop_ppm = r.range(50_000, 150_000) as u32;
                net.dup_ppm = r.range(50_000, 100_000) as u32;
                net.corrupt_ppm = r.range(10_000, 30_000) as u32;
            }
            2 => {
                net.drop_ppm = 20_000;
                let v = r.range(1, 3) as NodeId;
                let t = ms(r.range(15, 80));
                timed.push((t, Ev::Crash(v)));
                let restart = match r.below(3) {
                    0 => None,
                    1 => Some(ms(r.range(5, 50))),
                    _ => Some(ms(r.range(150, 300))),
                };
                if let Some(d) = restart {
                    timed.push((t + d, Ev::Restart(v)));
                }
            }
            3 => {
                net.drop_ppm = 20_000;
                net.dup_ppm = 20_000;
                let restart = match r.below(3) {
                    0 => None,
                    1 => Some(ms(r.range(3, 12))),
                    _ => Some(ms(r.range(60, 100))),
                };
                cps.push(CrashPoint::new("worker.first_frame_sent", r.range(1, 3) as u32, Victim::Trigger, restart));
            }
            4 => {
                net.drop_ppm = r.range(50_000, 100_000) as u32;
                let restart = if r.below(2) == 0 { None } else { Some(ms(r.range(60, 90))) };
                cps.push(CrashPoint::new("worker.final_frame_sent", r.range(1, 3) as u32, Victim::Trigger, restart));
            }
            5 => {
                net.drop_ppm = 20_000;
                net.dup_ppm = 30_000;
                let w = r.range(1, 3) as NodeId;
                let t0 = ms(r.range(15, 50));
                let dur = ms(r.range(45, 90));
                timed.push((t0, Ev::Partition(w, COORD)));
                timed.push((t0 + dur, Ev::Heal(w, COORD)));
                if r.below(2) == 0 {
                    let w2 = (w % 3) + 1;
                    let t1 = t0 + dur + ms(r.range(10, 40));
                    let dur2 = ms(r.range(45, 90));
                    timed.push((t1, Ev::Partition(w2, COORD)));
                    timed.push((t1 + dur2, Ev::Heal(w2, COORD)));
                }
            }
            6 => {
                net.drop_ppm = r.range(30_000, 80_000) as u32;
                match r.below(5) {
                    0..=2 => {
                        let v = r.range(1, 3) as NodeId;
                        timed.push((ms(r.range(8, 45)), Ev::ClientCancel));
                        cps.push(CrashPoint::new("coord.cancel_sent", 1, Victim::Node(v), Some(ms(r.range(1, 4)))));
                    }
                    3 => {
                        // a crash and restart that is over (and noticed) well before the cancel
                        let v = r.range(1, 3) as NodeId;
                        timed.push((ms(2), Ev::Crash(v)));
                        timed.push((ms(2 + r.range(3, 10)), Ev::Restart(v)));
                        timed.push((ms(r.range(60, 90)), Ev::ClientCancel));
                    }
                    _ => timed.push((ms(r.range(8, 45)), Ev::ClientCancel)),
                }
            }
            _ => {
                net.drop_ppm = r.range(100_000, 150_000) as u32;
                net.corrupt_ppm = 10_000;
                store.fail_ppm = r.range(100_000, 200_000) as u32;
                if r.below(2) == 0 {
                    timed.push((ms(r.range(60, 200)), Ev::ClientCancel));
                }
            }
        }
        Scenario {
            seed,
            family,
            cfg: Config { files: nfiles, workers: 3, window, batch_rows },
            files,
            net,
            store,
            timed,
            crash_points: cps,
        }
    }

    /// What the query must return: every input row through the filter and the map, each exactly once.
    pub fn expected(&self) -> Vec<i64> {
        let mut all: Vec<i64> = self.files.iter().flat_map(|f| engine::reference(f)).collect();
        all.sort();
        all
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: &'static str,
    pub msg: String,
}

pub struct Outcome {
    pub seed: u64,
    pub family: u32,
    pub fail: Option<Failure>,
    pub hash: u64,
    pub events: u64,
    pub end_ms: u64,
    pub status: &'static str,
    pub stats: BTreeMap<&'static str, u64>,
    pub log: Vec<String>,
}

impl Outcome {
    /// One line, identical for a sweep and its replay.
    pub fn line(&self) -> String {
        match &self.fail {
            None => format!(
                "PASS seed={} family={} status={} hash={:016x} events={} end_ms={}",
                self.seed, FAMILY_NAMES[self.family as usize], self.status, self.hash, self.events, self.end_ms
            ),
            Some(f) => format!(
                "FAIL seed={} family={} kind={} hash={:016x} events={} end_ms={} msg={}",
                self.seed, FAMILY_NAMES[self.family as usize], f.kind, self.hash, self.events, self.end_ms, f.msg
            ),
        }
    }
}

fn fail(kind: &'static str, msg: String) -> Option<Failure> {
    Some(Failure { kind, msg })
}

/// Sorted multiset comparison: (missing, extra) counts of `got` against `want`.
fn diff_sorted(want: &[i64], got: &[i64]) -> (usize, usize) {
    let (mut i, mut j, mut missing, mut extra) = (0, 0, 0, 0);
    while i < want.len() || j < got.len() {
        if j >= got.len() || (i < want.len() && want[i] < got[j]) {
            missing += 1;
            i += 1;
        } else if i >= want.len() || got[j] < want[i] {
            extra += 1;
            j += 1;
        } else {
            i += 1;
            j += 1;
        }
    }
    (missing, extra)
}

/// Check the committed result against the expected rows. Shared by the simulated and the real runtime.
pub fn verify_result(
    outcome: i64,
    rows_obj: Option<Vec<u8>>,
    manifest: Option<Vec<u8>>,
    expected: &[i64],
) -> Option<Failure> {
    match outcome {
        1 => {
            let (Some(rows_obj), Some(manifest)) = (rows_obj, manifest) else {
                return fail("result", "coordinator reported completion but the result objects are missing".into());
            };
            let mut got = decode_i64s(&rows_obj);
            let consistent = manifest.len() == 16
                && u64::from_le_bytes(manifest[..8].try_into().expect("8")) == got.len() as u64
                && u64::from_le_bytes(manifest[8..].try_into().expect("8")) == protocol::checksum(&got);
            got.sort();
            let (missing, extra) = diff_sorted(expected, &got);
            if missing != 0 || extra != 0 {
                return fail(
                    "rows",
                    format!("result rows differ: expected {} got {} (lost {missing}, duplicated or invented {extra})", expected.len(), got.len()),
                );
            }
            if !consistent {
                return fail("result", "manifest does not describe the rows object".into());
            }
            None
        }
        2 => {
            if manifest.is_some() {
                return fail("result", "query was cancelled but a result manifest was committed".into());
            }
            None
        }
        _ => fail("outcome", "coordinator ended without a Completed or Cancelled outcome".into()),
    }
}

fn check(w: &World, sc: &Scenario, end: crate::exec::RunEnd) -> (Option<Failure>, &'static str) {
    use crate::exec::RunEnd;
    let status = match w.gauge(COORD, "outcome") {
        1 => "completed",
        2 => "cancelled",
        _ => "none",
    };
    if let Some(v) = w.violations().first() {
        return (fail("violation", v.clone()), status);
    }
    match end {
        RunEnd::AllDone => {}
        RunEnd::StepLimit => return (fail("livelock", format!("poll budget of {} spent", MAX_POLLS)), status),
        _ => {
            let stuck: Vec<NodeId> = w.node_ids().into_iter().filter(|&n| w.node_up(n) && !w.node_finished(n)).collect();
            return (
                fail("hang", format!("nodes still running at the {} ms deadline: {:?}", DEADLINE / MS, stuck)),
                status,
            );
        }
    }
    let outcome = w.gauge(COORD, "outcome");
    if let Some(f) = verify_result(outcome, w.peek_object("result/rows"), w.peek_object("result/manifest"), &sc.expected()) {
        return (Some(f), status);
    }
    let live = w.buffers.load(std::sync::atomic::Ordering::SeqCst);
    if live != 0 {
        return (fail("leak", format!("{live} buffer(s) still allocated after every node ended")), status);
    }
    if w.exec.live_tasks() != 0 {
        return (fail("leak", format!("{} task(s) still alive after every node ended", w.exec.live_tasks())), status);
    }
    (None, status)
}

/// Run one seed. Pure function of the seed (and of the code): this is the unit of replay. A panic inside the
/// run (a broken assumption in the code under test) is a finding, reported as a failure of kind `panic`.
pub fn run_seed(seed: u64, keep_log: bool) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_seed_inner(seed, keep_log))) {
        Ok(o) => o,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "non-string panic".to_string());
            Outcome {
                seed,
                family: (seed % FAMILIES) as u32,
                fail: Some(Failure { kind: "panic", msg }),
                hash: 0,
                events: 0,
                end_ms: 0,
                status: "none",
                stats: BTreeMap::new(),
                log: Vec::new(),
            }
        }
    }
}

fn run_seed_inner(seed: u64, keep_log: bool) -> Outcome {
    let sc = Scenario::from_seed(seed);
    let w = World::new(seed, sc.net.clone(), sc.store.clone(), sc.crash_points.clone(), keep_log);
    for (f, ids) in sc.files.iter().enumerate() {
        w.put_object(&format!("input/{f}"), encode_i64s(ids));
    }
    let cfg = sc.cfg.clone();
    {
        let c = cfg.clone();
        let f: Factory = Rc::new(move |env| -> LocalFut { Box::pin(protocol::coordinator(env, c.clone())) });
        w.add_node(COORD, f);
    }
    for n in 1..=cfg.workers {
        let c = cfg.clone();
        let f: Factory = Rc::new(move |env| -> LocalFut { Box::pin(protocol::worker(env, c.clone())) });
        w.add_node(n, f);
    }
    for (t, ev) in &sc.timed {
        w.schedule(*t, ev.clone());
    }
    let end = w.exec.run(DEADLINE, MAX_POLLS, || false);
    let (fail, status) = check(&w, &sc, end);
    let out = Outcome {
        seed,
        family: sc.family,
        fail,
        hash: w.trace_hash(),
        events: w.trace_events(),
        end_ms: w.exec.now() / MS,
        status,
        stats: w.trace_stats(),
        log: w.trace_log(),
    };
    w.shutdown();
    out
}
