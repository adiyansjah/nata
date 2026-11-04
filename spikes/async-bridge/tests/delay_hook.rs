//! Same scenarios with the delay hooks on, as `tests/shutdown.rs` of the operator-trait spike does:
//! the hooks widen the windows (after a lock is released and before the waker fires; after a waker
//! is stored and before `Blocked` is returned; a worker holding the queue lock before it waits).
//! The delays are process-wide, so this test has its own binary.
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use spike_async_bridge::TEST_WORKER_DELAY_US;
use spike_async_bridge::chan::HOOK_DELAY_US;
use spike_async_bridge::harness::*;

#[test]
fn matrix_with_widened_race_windows() {
    HOOK_DELAY_US.store(200, SeqCst);
    TEST_WORKER_DELAY_US.store(200, SeqCst);
    let mut cancel_cut = 0;
    for i in 0..60 {
        let s = config(i, 5);
        match run_timeboxed(&s, Duration::from_secs(30)) {
            Verdict::Done(Ok(o)) => cancel_cut += usize::from(o.delivered < o.planned),
            Verdict::Done(Err(e)) => panic!("{s:?}: {e}"),
            Verdict::Hang => panic!("{s:?}: hang"),
        }
    }
    assert!(cancel_cut > 0, "the matrix never cut a run off mid-flight");
}
