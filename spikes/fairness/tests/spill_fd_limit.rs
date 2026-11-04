//! The spilling aggregate with the file-descriptor limit of a fresh Debian VM (1024), set inside the
//! test process with setrlimit so the result does not depend on the shell's `ulimit`. Own test
//! binary, because the limit is process-wide.
#![cfg(target_os = "linux")]
mod common;

use common::run_agg;
use spike_fairness::*;

fn lower_fd_limit(to: u64) {
    #[repr(C)]
    struct Rlimit {
        cur: u64,
        max: u64,
    }
    unsafe extern "C" {
        fn getrlimit(resource: i32, rlim: *mut Rlimit) -> i32;
        fn setrlimit(resource: i32, rlim: *const Rlimit) -> i32;
    }
    const RLIMIT_NOFILE: i32 = 7; // Linux
    let mut r = Rlimit { cur: 0, max: 0 };
    unsafe {
        assert_eq!(getrlimit(RLIMIT_NOFILE, &mut r), 0);
        if r.cur > to {
            r.cur = to;
            assert_eq!(setrlimit(RLIMIT_NOFILE, &r), 0, "setrlimit");
        }
    }
}

/// The query of `protocol.rs::finalize_spills_*` with the descriptor limit of a fresh Debian VM,
/// under the 1-morsel slice that interleaves all tasks the most.
#[test]
fn spilling_aggregate_under_1024_fd_limit() {
    lower_fd_limit(1024);
    let s = run_agg(SliceConfig::morsels(1), 4, 3);
    println!("under fd limit 1024: {s:?}");
}

#[test]
fn spilling_aggregate_under_1024_fd_limit_default_slice() {
    lower_fd_limit(1024);
    let s = run_agg(SliceConfig::default(), 4, 3);
    println!("under fd limit 1024, default slice: {s:?}");
}

/// The same bug without any slicing: four workers each running a task to completion hold
/// 4 x 256 = 1024 spill files. The slice made it appear with three workers; it was there for four.
#[test]
fn spilling_aggregate_under_1024_fd_limit_unlimited_slice_four_workers() {
    lower_fd_limit(1024);
    let s = run_agg(SliceConfig::unlimited(), 4, 4);
    println!("under fd limit 1024, unlimited slice, 4 workers: {s:?}");
}
