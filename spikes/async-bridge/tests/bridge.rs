//! Functional tests of the real bridge (tokio producers -> bounded channel -> snapshotted scheduler).
use std::time::Duration;

use spike_async_bridge::harness::*;

fn sc(
    pool: usize,
    channels: usize,
    producers: usize,
    cap: usize,
    chunks: usize,
    lat: Latency,
    kind: Kind,
) -> Scenario {
    Scenario {
        pool,
        channels,
        producers,
        cap,
        chunks,
        latency: lat,
        kind,
        seed: 7,
    }
}

fn expect_ok(s: &Scenario) -> Outcome {
    match run_timeboxed(s, Duration::from_secs(30)) {
        Verdict::Done(Ok(o)) => o,
        Verdict::Done(Err(e)) => panic!("{s:?}: {e}"),
        Verdict::Hang => panic!("{s:?}: hang"),
    }
}

#[test]
fn single_worker_single_channel_delivers_everything() {
    let o = expect_ok(&sc(
        1,
        1,
        1,
        1,
        100,
        Latency::Sleep(Duration::from_micros(100)),
        Kind::Normal,
    ));
    assert_eq!((o.delivered, o.planned), (100, 100));
}

/// The consumer task is scheduled first on the only CPU thread and finds the channel empty: it must
/// park (free the thread) and be re-queued by a send that comes from a tokio thread.
#[test]
fn empty_channel_parks_and_tokio_send_requeues() {
    let o = expect_ok(&sc(
        1,
        8,
        3,
        1,
        40,
        Latency::Random { max_us: 300 },
        Kind::Normal,
    ));
    assert_eq!(o.delivered, 8 * 3 * 40);
}

#[test]
fn producers_closing_early_end_the_consumer() {
    for seed in 0..20 {
        let mut s = sc(2, 3, 3, 2, 30, Latency::Yield, Kind::EarlyClose);
        s.seed = seed;
        expect_ok(&s);
    }
}

#[test]
fn cancel_frees_blocked_producers() {
    // Capacity 1, producers far ahead of the consumer, then cancel: producers must see the closed channel.
    for seed in 0..30 {
        let mut s = sc(1, 2, 3, 1, 200, Latency::None, Kind::Cancel);
        s.seed = seed;
        expect_ok(&s);
    }
}

#[test]
fn matrix_prefix_runs_clean() {
    for i in 0..150 {
        let s = config(i, 99);
        expect_ok(&s);
    }
}
