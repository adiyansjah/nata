//! One run of the spilling aggregate (4 scan tasks, 3 workers, 256 partitions, budget 200 groups,
//! 200,000 input rows, 20,000 keys) -> prints `wall_ms opens peak_open`. Used by
//! `mutants/run_fd_mutant.sh` to compare open-append-close per spill (the fix) with writers kept
//! open (the mutant `spill_keep_open`) in alternating runs. The result is checked against a
//! reference inside `run_agg`.
#[path = "../tests/common/mod.rs"]
mod common;

use spike_fairness::SliceConfig;

fn main() {
    let slice = match std::env::args().nth(1).as_deref() {
        Some("unlimited") => SliceConfig::unlimited(),
        _ => SliceConfig::default(),
    };
    let t = std::time::Instant::now();
    let s = common::run_agg(slice, 4, 3);
    println!(
        "{:.2} {} {}",
        t.elapsed().as_secs_f64() * 1e3,
        s.spill_file_opens,
        s.peak_open_spill_files
    );
}
