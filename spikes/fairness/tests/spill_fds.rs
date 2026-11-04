//! Open spill files under time slicing. A clean VM (file-descriptor limit 1024) found that the
//! aggregate's per-task spill writers, kept open until `combine`, multiply when tasks interleave.
//! This test counts the files open at the same moment, with and without slicing.
mod common;

use common::run_agg;
use spike_fairness::*;

/// Prints the peak; the assertion is the bound that must hold whatever the slice: at most one file
/// open per running worker.
#[test]
fn peak_open_spill_files_by_slice() {
    let mut rows = Vec::new();
    for (name, slice) in [
        ("unlimited", SliceConfig::unlimited()),
        ("default", SliceConfig::default()),
        ("1 morsel", SliceConfig::morsels(1)),
    ] {
        let s = run_agg(slice, 4, 3);
        println!(
            "PEAK slice={name:<10} tasks=4 workers=3 partitions=256: peak open spill files {:>5}, opens {:>7}, spill files on disk {}",
            s.peak_open_spill_files, s.spill_file_opens, s.spill_files
        );
        rows.push((name, s));
    }
    for (name, s) in rows {
        assert!(
            s.peak_open_spill_files <= 3,
            "slice {name}: {} spill files open at once (more than the 3 workers)",
            s.peak_open_spill_files
        );
    }
}
