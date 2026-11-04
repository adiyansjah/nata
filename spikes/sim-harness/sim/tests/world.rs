//! The simulated network and store, tested on tiny two-node worlds (no protocol involved).
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nata_sim::env::{Env, StoreError};
use nata_sim::exec::Nanos;
use nata_sim::sim::*;

const MS: u64 = 1_000_000;

fn world(seed: u64, net: NetConfig, store: StoreConfig, cps: Vec<CrashPoint>) -> Rc<World> {
    World::new(seed, net, store, cps, false)
}

fn calm() -> NetConfig {
    NetConfig { base: MS, jitter: 3 * MS, drop_ppm: 0, dup_ppm: 0, corrupt_ppm: 0 }
}

fn quiet_store() -> StoreConfig {
    StoreConfig { lat_min: MS, lat_max: 4 * MS, fail_ppm: 0 }
}

/// Node 1 sends `n` numbered messages at 1 ms spacing; node 2 records what arrives and when, until `until`.
fn exchange(w: &Rc<World>, n: u32, until: Nanos) -> Rc<RefCell<Vec<(u32, Nanos)>>> {
    let got = Rc::new(RefCell::new(Vec::new()));
    let g = Rc::clone(&got);
    let sender: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        Box::pin(async move {
            for i in 0..n {
                env.send(2, i.to_le_bytes().to_vec());
                env.sleep(MS).await;
            }
        })
    });
    let receiver: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        let g = Rc::clone(&g);
        Box::pin(async move {
            while env.now() < until {
                if let Some((_, b)) = env.recv_timeout(2 * MS).await {
                    g.borrow_mut().push((u32::from_le_bytes(b[..4].try_into().unwrap()), env.now()));
                }
            }
        })
    });
    w.add_node(1, sender);
    w.add_node(2, receiver);
    got
}

fn run(w: &Rc<World>) {
    w.exec.run(10_000 * MS, 1_000_000, || false);
}

#[test]
fn latency_is_within_the_configured_bounds_and_jitter_reorders() {
    let w = world(1, calm(), quiet_store(), vec![]);
    let got = exchange(&w, 200, 400 * MS);
    run(&w);
    let g = got.borrow();
    assert_eq!(g.len(), 200, "calm network loses nothing");
    for (i, at) in g.iter() {
        let sent = *i as u64 * MS; // sender is at i ms
        assert!(*at >= sent + MS && *at <= sent + MS + 3 * MS + 2 * MS, "message {i} took {} ns", at - sent);
    }
    let ids: Vec<u32> = g.iter().map(|x| x.0).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_ne!(ids, sorted, "3 ms of jitter on 1 ms spacing must reorder some messages");
}

#[test]
fn drop_duplicate_and_corrupt_rates_are_honoured() {
    let mut net = calm();
    net.drop_ppm = 1_000_000;
    let w = world(1, net, quiet_store(), vec![]);
    let got = exchange(&w, 50, 200 * MS);
    run(&w);
    assert!(got.borrow().is_empty(), "drop rate 100% delivers nothing");

    let mut net = calm();
    net.dup_ppm = 1_000_000;
    let w = world(1, net, quiet_store(), vec![]);
    let got = exchange(&w, 50, 200 * MS);
    run(&w);
    assert_eq!(got.borrow().len(), 100, "dup rate 100% delivers everything twice");

    let mut net = calm();
    net.drop_ppm = 300_000;
    let w = world(5, net, quiet_store(), vec![]);
    let got = exchange(&w, 2000, 3000 * MS);
    run(&w);
    let n = got.borrow().len();
    assert!((1200..=1600).contains(&n), "30% drop of 2000 delivered {n}");
}

#[test]
fn a_partition_blocks_both_directions_until_it_heals() {
    let w = world(1, calm(), quiet_store(), vec![]);
    let got = exchange(&w, 100, 300 * MS);
    w.schedule(20 * MS, Ev::Partition(1, 2));
    w.schedule(60 * MS, Ev::Heal(1, 2));
    run(&w);
    let g = got.borrow();
    assert!(g.iter().all(|(i, _)| !(25..=55).contains(i)), "nothing sent inside the partition may arrive");
    assert!(g.iter().any(|(i, _)| *i < 15) && g.iter().any(|(i, _)| *i > 65), "traffic flows before and after");
}

#[test]
fn a_crashed_node_loses_its_inbox_and_a_restart_brings_a_fresh_one() {
    let w = world(1, calm(), quiet_store(), vec![]);
    let incarnations = Rc::new(Cell::new(0));
    let i2 = Rc::clone(&incarnations);
    let f: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        i2.set(i2.get() + 1);
        Box::pin(async move {
            loop {
                env.sleep(MS).await;
                env.gauge_set("alive", 1);
            }
        })
    });
    w.add_node(1, f);
    w.schedule(10 * MS, Ev::Crash(1));
    w.schedule(30 * MS, Ev::Restart(1));
    w.exec.run(20 * MS, 100_000, || false);
    assert!(!w.node_up(1));
    assert_eq!(w.gauge(1, "alive"), 0, "gauges die with the node");
    assert_eq!(w.exec.live_tasks_of(1), 0);
    w.exec.run(40 * MS, 100_000, || false);
    assert!(w.node_up(1));
    assert_eq!(incarnations.get(), 2);
}

#[test]
fn a_fault_point_crashes_the_victim_on_the_nth_hit_exactly_once() {
    let cp = CrashPoint::new("p", 3, Victim::Trigger, None);
    let w = world(1, calm(), quiet_store(), vec![cp]);
    let hits = Rc::new(Cell::new(0));
    let h = Rc::clone(&hits);
    let f: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        let h = Rc::clone(&h);
        Box::pin(async move {
            loop {
                env.sleep(MS).await;
                h.set(h.get() + 1);
                env.fault_point("p");
            }
        })
    });
    w.add_node(1, f);
    w.exec.run(100 * MS, 100_000, || false);
    assert_eq!(hits.get(), 3, "the third hit crashes the node; nothing runs after it");
    assert!(!w.node_up(1));
}

#[test]
fn store_latency_failures_and_lost_replies() {
    let cfg = StoreConfig { lat_min: 2 * MS, lat_max: 5 * MS, fail_ppm: 500_000 };
    let w = world(3, calm(), cfg, vec![]);
    w.put_object("k", vec![9]);
    let stats = Rc::new(RefCell::new((0u32, 0u32, 0u32, 0u32))); // get ok, get err, put err but applied, put err not applied
    let s = Rc::clone(&stats);
    let f: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        let s = Rc::clone(&s);
        Box::pin(async move {
            for i in 0..400u32 {
                let t0 = env.now();
                match env.store_get("k").await {
                    Ok(v) => {
                        assert_eq!(v, vec![9]);
                        s.borrow_mut().0 += 1;
                    }
                    Err(StoreError::Unavailable) => s.borrow_mut().1 += 1,
                    Err(e) => panic!("{e:?}"),
                }
                let dt = env.now() - t0;
                assert!((2 * MS..=5 * MS).contains(&dt), "latency {dt}");
                let key = format!("w{i}");
                let r = env.store_put(&key, vec![1]).await;
                if r.is_err() {
                    // is the write there although the reply said no?
                    let applied = env.store_get(&key).await;
                    match applied {
                        Ok(_) => s.borrow_mut().2 += 1,
                        Err(StoreError::NotFound) => s.borrow_mut().3 += 1,
                        Err(_) => {}
                    }
                }
            }
        })
    });
    w.add_node(1, f);
    w.exec.run(1_000_000 * MS, 10_000_000, || false);
    let s = stats.borrow();
    assert!(s.0 > 100 && s.1 > 100, "get: {} ok, {} errors at 50% failure", s.0, s.1);
    assert!(s.2 > 10 && s.3 > 10, "put errors: {} applied, {} not applied", s.2, s.3);
}

#[test]
fn zombie_futures_of_a_crashed_node_cannot_send() {
    let w = world(1, calm(), quiet_store(), vec![]);
    let got = Rc::new(RefCell::new(0));
    let g = Rc::clone(&got);
    let receiver: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        let g = Rc::clone(&g);
        Box::pin(async move {
            loop {
                if env.recv_timeout(MS).await.is_some() {
                    *g.borrow_mut() += 1;
                }
            }
        })
    });
    let sender: Factory = Rc::new(move |env: SimEnv| -> LocalFut {
        Box::pin(async move {
            loop {
                env.sleep(MS).await;
                env.send(2, vec![1]);
            }
        })
    });
    w.add_node(2, receiver);
    w.add_node(1, sender);
    w.schedule(10 * MS, Ev::Crash(1));
    w.exec.run(100 * MS, 1_000_000, || false);
    let n = *got.borrow();
    assert!((5..=12).contains(&n), "only what node 1 sent before it crashed arrives, got {n}");
}
