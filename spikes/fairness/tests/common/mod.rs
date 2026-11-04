//! Shared by the spill tests: a spilling aggregate query (see `protocol.rs::agg_case(200, 256, 20_000)`).
#![allow(dead_code)]
use std::collections::HashMap;
use std::sync::Arc;

use spike_fairness::ops::*;
use spike_fairness::*;

/// `morsels` batches of `rows` rows `(key, 1 + key % 7)`, keys cycling over `keys`.
struct Rows {
    morsels: usize,
    rows: usize,
    keys: usize,
}

struct RowsSource {
    cfg: (usize, usize, usize),
    next: usize,
}

impl SourceFactory for Rows {
    fn make(&self) -> Box<dyn Source> {
        Box::new(RowsSource {
            cfg: (self.morsels, self.rows, self.keys),
            next: 0,
        })
    }
}

impl Source for RowsSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let (morsels, rows, keys) = self.cfg;
        if self.next >= morsels {
            return Ok(SourceResult::Finished);
        }
        let base = self.next * rows;
        self.next += 1;
        let ks: Vec<i64> = (0..rows).map(|j| ((base + j) % keys) as i64).collect();
        let vs: Vec<i64> = ks.iter().map(|k| 1 + k % 7).collect();
        Ok(SourceResult::Batch(Batch::from_cols(vec![ks, vs])))
    }
}

/// Output of the aggregate collected into a map.
#[derive(Clone, Default)]
struct Out(Arc<std::sync::Mutex<Vec<Vec<i64>>>>);
struct OutLocal(Arc<std::sync::Mutex<Vec<Vec<i64>>>>);

impl GlobalSink for Out {
    fn make(&self, _t: usize) -> Box<dyn LocalSink> {
        Box::new(OutLocal(Arc::clone(&self.0)))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for OutLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.0.lock().unwrap().extend(input.rows());
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// The aggregate of `protocol.rs::agg_case(200, 256, 20_000)`: 4 scan tasks on 3 workers, 256
/// partitions, budget 200 groups per task. Returns the stats and checks the result.
pub fn run_agg(slice: SliceConfig, tasks: usize, workers: usize) -> AggStats {
    let (morsels, rows, keys) = (100usize, 500usize, 20_000usize);
    let mut expect: HashMap<i64, i64> = HashMap::new();
    for _ in 0..tasks {
        for i in 0..morsels * rows {
            let k = (i % keys) as i64;
            *expect.entry(k).or_insert(0) += 1 + k % 7;
        }
    }
    let mut expected: Vec<Vec<i64>> = expect.into_iter().map(|(k, v)| vec![k, v]).collect();
    expected.sort();
    let agg = Aggregate::new(AggConfig {
        budget_groups: 200,
        partitions: 256,
    });
    let out = Out::default();
    let scan = Pipeline {
        name: "scan-agg".into(),
        source: Arc::new(Rows {
            morsels,
            rows,
            keys,
        }),
        ops: vec![],
        sink: Arc::new(agg.clone()),
        deps: vec![],
        tasks,
    };
    let emit = Pipeline {
        name: "emit".into(),
        source: agg.result_source(),
        ops: vec![],
        sink: Arc::new(out.clone()),
        deps: vec![0],
        tasks: 2,
    };
    Query::new(vec![scan, emit])
        .with_slice(slice)
        .start(workers)
        .unwrap()
        .wait()
        .expect("aggregate query");
    let mut got = out.0.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, expected, "aggregate result differs from the reference");
    agg.stats()
}

