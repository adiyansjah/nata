//! Cost of slicing on a CPU-bound many-morsel pipeline, paired and interleaved.
//!
//! Pipeline per task: source (generates `rows` i64 per morsel) -> operator (x*3+1, then keep all)
//! -> sink (wrapping sum). The total work is fixed (2^27 rows) and split over `tasks` tasks.
//! Arms (all in the same process, rotated every round): `A` unlimited slice (= the scheduler
//! before the change), `A'` a second unlimited arm (CONTROL: shows the noise between two identical
//! arms), and slices of 1, 4, 16, 64 morsels and the default (32 morsels or 1 ms).
//! Every run's result is checked against a serial reference BEFORE its time is used.
//!
//! usage: overhead [--rounds N] [--workers W] [--total-log2 L]
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spike_fairness::*;

#[derive(Clone, Copy)]
struct Shape {
    rows: usize,
    morsels_per_task: usize,
}

struct Gen(Shape);

struct GenSource {
    shape: Shape,
    left: usize,
    base: i64,
}

impl SourceFactory for Gen {
    fn make(&self) -> Box<dyn Source> {
        Box::new(GenSource {
            shape: self.0,
            left: self.0.morsels_per_task,
            base: 0,
        })
    }
}

impl Source for GenSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.left == 0 {
            return Ok(SourceResult::Finished);
        }
        self.left -= 1;
        let b = self.base;
        self.base += self.shape.rows as i64;
        let col: Vec<i64> = (0..self.shape.rows as i64).map(|j| b + j).collect();
        Ok(SourceResult::Batch(Batch::from_cols(vec![col])))
    }
}

struct Map;
struct MapFactory;

impl OperatorFactory for MapFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(Map)
    }
}

impl Operator for Map {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let col: Vec<i64> = input.cols[0]
            .iter()
            .map(|&x| x.wrapping_mul(3).wrapping_add(1))
            .collect();
        *out = Batch::from_cols(vec![col]);
        Ok(OpResult::NeedMoreInput)
    }
}

struct Sum(Arc<AtomicU64>);
struct SumLocal(Arc<AtomicU64>, u64);

impl GlobalSink for Sum {
    fn make(&self, _t: usize) -> Box<dyn LocalSink> {
        Box::new(SumLocal(Arc::clone(&self.0), 0))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for SumLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.1 = input.cols[0].iter().fold(self.1, |a, &x| a.wrapping_add(x as u64));
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        self.0.fetch_add(self.1, SeqCst);
        Ok(())
    }
}

/// Serial reference: the same sum computed without the scheduler.
fn reference(shape: Shape, tasks: usize) -> u64 {
    let mut total = 0u64;
    for _ in 0..tasks {
        let mut base = 0i64;
        for _ in 0..shape.morsels_per_task {
            for j in 0..shape.rows as i64 {
                total = total.wrapping_add((base + j).wrapping_mul(3).wrapping_add(1) as u64);
            }
            base += shape.rows as i64;
        }
    }
    total
}

fn run_once(shape: Shape, tasks: usize, workers: usize, slice: SliceConfig) -> (Duration, usize, u64) {
    let sum = Arc::new(AtomicU64::new(0));
    let p = Pipeline {
        name: "cpu".into(),
        source: Arc::new(Gen(shape)),
        ops: vec![Arc::new(MapFactory)],
        sink: Arc::new(Sum(Arc::clone(&sum))),
        deps: vec![],
        tasks,
    };
    let t0 = Instant::now();
    let rq = Query::new(vec![p]).with_slice(slice).start(workers).unwrap();
    let (r, yields) = rq.wait_with_stats();
    let dt = t0.elapsed();
    r.unwrap();
    (dt, yields, sum.load(SeqCst))
}

/// Process CPU time (user + system, all threads, including exited ones) in clock ticks of 10 ms.
fn cpu_ticks() -> u64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit_once(')').map_or("", |x| x.1);
    let f: Vec<&str> = rest.split_whitespace().collect();
    // fields after ")": state(0) ... utime is stat field 14 = index 11 here, stime index 12
    let g = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    g(11) + g(12)
}

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok()))
        .unwrap_or(f64::NAN)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str, d: usize| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let rounds = get("--rounds", 15);
    let workers = get("--workers", 2);
    let total_rows: usize = 1 << get("--total-log2", 27);
    let arms: Vec<(&str, SliceConfig)> = vec![
        ("A unlimited (old)", SliceConfig::unlimited()),
        ("A' unlimited (control twin)", SliceConfig::unlimited()),
        ("slice 1 morsel", SliceConfig::morsels(1)),
        ("slice 4 morsels", SliceConfig::morsels(4)),
        ("slice 16 morsels", SliceConfig::morsels(16)),
        ("slice 64 morsels", SliceConfig::morsels(64)),
        ("default (32 morsels / 1 ms)", SliceConfig::default()),
    ];
    println!(
        "host: {} cores; workers={workers}; total rows {total_rows}; rounds={rounds}; load1 at start {:.2}; {}",
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        load1(),
        std::env::var("NATA_NOTE").unwrap_or_default()
    );
    for &tasks in &[workers, 4 * workers] {
        for &rows in &[4096usize, 65536, 1 << 20] {
            let shape = Shape {
                rows,
                morsels_per_task: total_rows / rows / tasks,
            };
            let want = reference(shape, tasks);
            let load_before = load1();
            // times[arm] = per-round seconds; yields[arm] = yields of the last round
            let mut times: Vec<Vec<f64>> = vec![Vec::new(); arms.len()];
            let mut yields = vec![0usize; arms.len()];
            let mut cpu = vec![0u64; arms.len()]; // CPU ticks (10 ms) summed over the measured rounds
            for round in 0..rounds + 1 {
                for k in 0..arms.len() {
                    let a = (k + round) % arms.len(); // rotate the starting arm every round
                    let c0 = cpu_ticks();
                    let (dt, y, got) = run_once(shape, tasks, workers, arms[a].1);
                    let c1 = cpu_ticks();
                    assert_eq!(got, want, "WRONG RESULT in arm {} (rows {rows}, tasks {tasks})", arms[a].0);
                    if round > 0 {
                        times[a].push(dt.as_secs_f64() * 1e3);
                        yields[a] = y;
                        cpu[a] += c1 - c0;
                    }
                }
            }
            let load_after = load1();
            println!(
                "\n### tasks={tasks} workers={workers} morsel={rows} rows ({} morsels/task, {} total); load1 before {load_before:.2}, after {load_after:.2}",
                shape.morsels_per_task,
                shape.morsels_per_task * tasks
            );
            println!("| arm | wall min ms | wall median ms | median paired wall ratio to A | paired ratio min..max | mean CPU ms/run | CPU ratio to A | yields/run |");
            println!("|---|---:|---:|---:|---:|---:|---:|---:|");
            for (i, (name, _)) in arms.iter().enumerate() {
                let mut t = times[i].clone();
                let mut ratios: Vec<f64> = times[i].iter().zip(&times[0]).map(|(x, a)| x / a).collect();
                let (rmin, rmax) = ratios.iter().fold((f64::MAX, 0f64), |(lo, hi), &r| (lo.min(r), hi.max(r)));
                println!(
                    "| {name} | {:.1} | {:.1} | {:.3} | {rmin:.3}..{rmax:.3} | {:.0} | {:.3} | {} |",
                    t.iter().cloned().fold(f64::MAX, f64::min),
                    median(&mut t),
                    median(&mut ratios),
                    cpu[i] as f64 * 10.0 / rounds as f64,
                    cpu[i] as f64 / cpu[0] as f64,
                    yields[i]
                );
            }
        }
    }
}
