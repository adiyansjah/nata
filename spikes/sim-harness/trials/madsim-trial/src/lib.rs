//! Build-or-buy trial: the SAME toy protocol (`nata_sim::protocol`, generic over `Env`) on madsim.
//! What it takes to port: an `Env` implementation over madsim's endpoint, time, randomness and task APIs (below),
//! and a driver that builds the same scenario (`nata_sim::scenario::Scenario`) out of madsim's knobs.
//! Compiles only with RUSTFLAGS="--cfg madsim"; without it the crate is empty.
#![cfg(madsim)]

use std::cell::RefCell;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use madsim::net::{Endpoint, NetSim};
use madsim::runtime::{Handle, Runtime};
use madsim::time::{self, Instant};
use nata_sim::env::*;
use nata_sim::exec::TaskCtl;
use nata_sim::frame::{Msg, encode_i64s};
use nata_sim::protocol;
use nata_sim::scenario::{Scenario, verify_result};
use nata_sim::sim::Ev;
use nata_sim::trialsupport::Shared;

pub const COORD: NodeId = 0;

fn addr_of(node: NodeId) -> SocketAddr {
    format!("10.0.0.{}:1", node + 1).parse().expect("address")
}

fn node_of(ip: IpAddr) -> NodeId {
    match ip {
        IpAddr::V4(v4) => v4.octets()[3] as NodeId - 1,
        IpAddr::V6(_) => 255,
    }
}

/// `Env` over madsim. Time, randomness and the network are madsim's; the object store is `Shared` with
/// madsim-timed latency and madsim-random failures (madsim has no object store).
#[derive(Clone)]
pub struct MEnv {
    node: NodeId,
    ep: Rc<Endpoint>,
    base: Instant,
    shared: Arc<Shared>,
}

impl MEnv {
    fn draw_store(&self) -> (u64, bool) {
        let (lo, hi) = self.shared.store_latency_ns;
        let lat = lo + madsim::rand::random::<u64>() % (hi - lo + 1);
        let fail = self.shared.store_fail_ppm > 0 && madsim::rand::random::<u64>() % 1_000_000 < self.shared.store_fail_ppm as u64;
        (lat, fail)
    }
}

impl Env for MEnv {
    fn me(&self) -> NodeId {
        self.node
    }

    fn now(&self) -> Nanos {
        self.base.elapsed().as_nanos() as Nanos
    }

    fn rand(&self) -> u64 {
        madsim::rand::random()
    }

    fn sleep(&self, d: Nanos) -> impl Future<Output = ()> {
        time::sleep(Duration::from_nanos(d))
    }

    fn send(&self, to: NodeId, bytes: Vec<u8>) {
        let ep = Rc::clone(&self.ep);
        madsim::task::spawn_local(async move {
            let _ = ep.send_to(addr_of(to), 0, &bytes).await;
        });
    }

    fn recv_timeout(&self, d: Nanos) -> impl Future<Output = Option<(NodeId, Vec<u8>)>> {
        let ep = Rc::clone(&self.ep);
        async move {
            let mut buf = vec![0u8; 65536];
            match time::timeout(Duration::from_nanos(d), ep.recv_from(0, &mut buf)).await {
                Ok(Ok((n, from))) => {
                    buf.truncate(n);
                    Some((node_of(from.ip()), buf))
                }
                _ => None,
            }
        }
    }

    fn spawn(&self, f: impl Future<Output = ()> + 'static) -> TaskCtl {
        let h = madsim::task::spawn_local(f);
        let ah = h.abort_handle();
        TaskCtl::external(move || ah.abort())
    }

    fn store_get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StoreError>> {
        let (lat, fail) = self.draw_store();
        let (sh, key) = (Arc::clone(&self.shared), key.to_string());
        async move {
            time::sleep(Duration::from_nanos(lat)).await;
            if fail { Err(StoreError::Unavailable) } else { sh.get_now(&key) }
        }
    }

    fn store_put(&self, key: &str, val: Vec<u8>) -> impl Future<Output = Result<(), StoreError>> {
        let (lat, fail) = self.draw_store();
        let (sh, key) = (Arc::clone(&self.shared), key.to_string());
        async move {
            time::sleep(Duration::from_nanos(lat)).await;
            if fail {
                return Err(StoreError::Unavailable);
            }
            sh.put_object(&key, val);
            Ok(())
        }
    }

    // madsim has no named fault points; `buggify` would be the nearest tool. Not mapped in this trial.
    fn fault_point(&self, _name: &'static str) {}

    fn track(&self, _what: &'static str) -> Tracked {
        Tracked::new(&self.shared.buffers)
    }

    fn event(&self, kind: &'static str, a: u64, b: u64) {
        self.shared.event(self.now(), self.node, kind, a, b);
    }

    fn gauge_set(&self, name: &'static str, v: i64) {
        self.shared.gauge_set(self.node, name, v);
    }

    fn violation(&self, msg: String) {
        self.shared.violation(msg);
    }
}

pub struct TrialOutcome {
    pub seed: u64,
    pub fail: Option<String>,
    pub hash: u64,
    pub events: u64,
    pub scenario_features_unmapped: &'static str,
}

/// One seed of the shared scenario generator, run on madsim. Crash points and duplicate/corrupt faults have no
/// madsim equivalent and are not applied (reported in `scenario_features_unmapped`).
pub fn run_seed(seed: u64) -> TrialOutcome {
    let sc = Scenario::from_seed(seed);
    let mut cfg = madsim::Config::default();
    cfg.net.packet_loss_rate = sc.net.drop_ppm as f64 / 1e6;
    cfg.net.send_latency = Duration::from_nanos(sc.net.base)..Duration::from_nanos(sc.net.base + sc.net.jitter + 1);
    let rt = Runtime::with_seed_and_config(seed, cfg);
    let shared = Shared::new((sc.store.lat_min, sc.store.lat_max), sc.store.fail_ppm);
    for (f, ids) in sc.files.iter().enumerate() {
        shared.put_object(&format!("input/{f}"), encode_i64s(ids));
    }
    let expected = sc.expected();
    let pcfg = sc.cfg.clone();
    let timed = sc.timed.clone();
    let sh = Arc::clone(&shared);
    let unmapped = if !sc.crash_points.is_empty() || sc.net.dup_ppm > 0 || sc.net.corrupt_ppm > 0 || sc.store.fail_ppm > 0 {
        "crash-points,duplicates,corruption"
    } else {
        ""
    };
    let finished = rt.block_on(async move {
        let handle = Handle::current();
        let base = Instant::now();
        let mut ids = Vec::new();
        for n in 0..=pcfg.workers {
            let (sh, pcfg) = (Arc::clone(&sh), pcfg.clone());
            let node = handle
                .create_node()
                .name(format!("n{n}"))
                .ip(format!("10.0.0.{}", n + 1).parse().expect("ip"))
                .init(move || {
                    let (sh, pcfg) = (Arc::clone(&sh), pcfg.clone());
                    async move {
                        let ep = Rc::new(Endpoint::bind("0.0.0.0:1").await.expect("bind"));
                        let env = MEnv { node: n, ep, base, shared: Arc::clone(&sh) };
                        if n == COORD {
                            protocol::coordinator(env, pcfg).await;
                            sh.gauge_set(COORD, "finished", 1);
                        } else {
                            protocol::worker(env, pcfg).await;
                        }
                    }
                })
                .build();
            ids.push(node.id());
        }
        // the client: a node that sends the cancel request at its time
        for (t, ev) in &timed {
            if matches!(ev, Ev::ClientCancel) {
                let t = *t;
                handle
                    .create_node()
                    .name("client")
                    .ip("10.0.0.99".parse().expect("ip"))
                    .init(move || async move {
                        let ep = Endpoint::bind("0.0.0.0:1").await.expect("bind");
                        time::sleep(Duration::from_nanos(t)).await;
                        let _ = ep.send_to(addr_of(COORD), 0, &Msg::ClientCancel.encode()).await;
                    })
                    .build();
            }
        }
        let net = NetSim::current();
        let mut now_ns = 0u64;
        let mut events: Vec<(u64, Ev)> = timed.into_iter().filter(|(_, e)| !matches!(e, Ev::ClientCancel)).collect();
        events.sort_by_key(|(t, _)| *t);
        for (t, ev) in events {
            if t > now_ns {
                time::sleep(Duration::from_nanos(t - now_ns)).await;
                now_ns = t;
            }
            match ev {
                Ev::Crash(n) => handle.kill(ids[n as usize]),
                Ev::Restart(n) => handle.restart(ids[n as usize]),
                Ev::Partition(a, b) => net.clog_link(ids[a as usize], ids[b as usize]).pipe(|_| net.clog_link(ids[b as usize], ids[a as usize])),
                Ev::Heal(a, b) => net.unclog_link(ids[a as usize], ids[b as usize]).pipe(|_| net.unclog_link(ids[b as usize], ids[a as usize])),
                Ev::ClientCancel => {}
            }
        }
        // wait for the coordinator, at most 5 virtual seconds
        for _ in 0..1000 {
            if sh.gauge(COORD, "finished") == 1 {
                return true;
            }
            time::sleep(Duration::from_millis(5)).await;
        }
        false
    });
    let fail = if !finished {
        Some("hang: the coordinator did not finish within 5 virtual seconds".to_string())
    } else if let Some(v) = shared.violations().first() {
        Some(format!("violation: {v}"))
    } else {
        verify_result(shared.gauge(COORD, "outcome"), shared.object("result/rows"), shared.object("result/manifest"), &expected)
            .map(|f| format!("{}: {}", f.kind, f.msg))
    };
    TrialOutcome { seed, fail, hash: shared.trace_hash(), events: shared.events.load(std::sync::atomic::Ordering::SeqCst), scenario_features_unmapped: unmapped }
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

#[allow(dead_code)]
fn _unused(_: Rc<RefCell<()>>) {}
