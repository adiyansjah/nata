//! What does the protocol cost? Two numbers, min of 7 runs, x86-64 VM (noisy):
//! (a) per batch through source -> filter -> sink with no blocking (includes building the batch);
//! (b) per batch through a capacity-1 exchange, where nearly every batch parks and wakes a task.
use std::sync::Arc;
use std::time::Instant;

use spike_operator_trait::ops::*;
use spike_operator_trait::*;

fn pipeline(
    name: &str,
    source: Arc<dyn SourceFactory>,
    ops: Vec<Arc<dyn OperatorFactory>>,
    sink: Arc<dyn GlobalSink>,
    tasks: usize,
) -> Pipeline {
    Pipeline {
        name: name.into(),
        source,
        ops,
        sink,
        deps: vec![],
        tasks,
    }
}

fn one_row(m: usize) -> Batch {
    Batch::from_cols(vec![vec![m as i64], vec![1]])
}

fn best(mut f: impl FnMut() -> f64) -> f64 {
    (0..7).map(|_| f()).fold(f64::MAX, f64::min)
}

fn main() {
    let n = 200_000usize;
    for threads in [1usize, 2] {
        let steady = best(|| {
            let source_gen = Generator::new(n, one_row);
            let keep_all: Arc<dyn Fn(&[i64]) -> bool + Send + Sync> = Arc::new(|_| true);
            let t = Instant::now();
            let p = pipeline(
                "steady",
                Arc::new(source_gen),
                vec![Arc::new(Filter(keep_all))],
                Arc::new(Collect::new()),
                1,
            );
            Query::new(vec![p]).run(threads).expect("run");
            t.elapsed().as_secs_f64() * 1e9 / n as f64
        });
        let parked = best(|| {
            let source_gen = Generator::new(n, one_row);
            let (sink, source, chan) = exchange(1, 1, 1);
            let t = Instant::now();
            let producer = pipeline("producer", Arc::new(source_gen), vec![], sink, 1);
            let consumer = pipeline("consumer", source, vec![], Arc::new(Collect::new()), 1);
            Query::new(vec![producer, consumer])
                .run(threads)
                .expect("run");
            assert_eq!(chan.pushed.load(std::sync::atomic::Ordering::SeqCst), n);
            t.elapsed().as_secs_f64() * 1e9 / n as f64
        });
        println!(
            "threads={threads}: steady {steady:.0} ns/batch; through a capacity-1 exchange {parked:.0} ns/batch"
        );
    }
}
