//! The compute side of a worker: the REAL operator protocol, scheduler and in-process exchange of
//! `spikes/operator-trait` and `spikes/exchange-loom` (included with #[path] from lib.rs, never copied).
//!
//! A fragment is two pipelines on one scheduler thread: scan -> filter -> [exchange] -> map -> collect.
//! It runs to completion inside one poll of a simulated worker task and takes no `Env`: from the simulation's
//! point of view CPU work is instantaneous. The thread interleavings INSIDE it are not under the simulator's
//! control (loom and shuttle cover those; see the results file).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::*;
use crate::batch::Batch;
use crate::exchange::exchange;
use crate::ops::{Collect, Filter, Generator};
use crate::sched::Query;

pub const KEEP_MOD: i64 = 5;

pub fn keep(id: i64) -> bool {
    id.rem_euclid(KEEP_MOD) != 0
}

pub fn map(id: i64) -> i64 {
    id.wrapping_mul(3).wrapping_add(7)
}

/// The independent reference: plain loops, no scheduler.
pub fn reference(ids: &[i64]) -> Vec<i64> {
    let mut v: Vec<i64> = ids.iter().copied().filter(|&i| keep(i)).map(map).collect();
    v.sort();
    v
}

struct Scale;

struct ScaleOp;

impl OperatorFactory for Scale {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(ScaleOp)
    }
}

impl Operator for ScaleOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        *out = Batch::from_cols(vec![input.cols[0].iter().map(|&v| map(v)).collect()]);
        Ok(OpResult::NeedMoreInput)
    }
}

static INLINE: AtomicBool = AtomicBool::new(false);

/// Compute with plain loops instead of the scheduler (no OS thread is spawned). The madsim trial needs it:
/// madsim forbids system threads inside a simulation. Never set in the harness proper.
pub fn set_inline(on: bool) {
    INLINE.store(on, Ordering::SeqCst);
}

/// Run the fragment on the real scheduler with `threads` worker threads. Returns the sorted output.
pub fn process(ids: &[i64], morsel_rows: usize, threads: usize) -> Vec<i64> {
    if INLINE.load(Ordering::SeqCst) {
        return reference(ids);
    }
    let ids = Arc::new(ids.to_vec());
    let morsel = morsel_rows.max(1);
    let morsels = ids.len().div_ceil(morsel);
    let src = Arc::clone(&ids);
    let gen_source = Generator::new(morsels, move |m| {
        let lo = m * morsel;
        let hi = (lo + morsel).min(src.len());
        Batch::from_cols(vec![src[lo..hi].to_vec()])
    });
    let keep_fn: Arc<dyn Fn(&[i64]) -> bool + Send + Sync> = Arc::new(|row| keep(row[0]));
    let (sink, source, _chan) = exchange(2, 1, 1);
    let collect = Collect::new();
    let producer = Pipeline {
        name: "scan".into(),
        source: Arc::new(gen_source),
        ops: vec![Arc::new(Filter(keep_fn))],
        sink,
        deps: vec![],
        tasks: 1,
    };
    let consumer = Pipeline {
        name: "map".into(),
        source,
        ops: vec![Arc::new(Scale)],
        sink: Arc::new(collect.clone()),
        deps: vec![],
        tasks: 1,
    };
    Query::new(vec![producer, consumer]).run(threads).expect("fragment ran");
    collect.rows().into_iter().map(|r| r[0]).collect()
}
