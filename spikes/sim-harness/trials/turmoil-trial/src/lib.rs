//! Build-or-buy trial: the SAME toy protocol (`nata_sim::protocol`, generic over `Env`) on turmoil.
//! turmoil simulates hosts, a UDP/TCP network (latency, loss, partitions, host crash and bounce) on top of
//! tokio's paused time. It has no object store, no named fault points, no duplicate or corrupt messages; the
//! object store is `Shared` with turmoil-timed latency. The scenario is the shared generator's, mapped as far as
//! turmoil's knobs allow.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use nata_sim::env::*;
use nata_sim::exec::TaskCtl;
use nata_sim::frame::{Msg, encode_i64s};
use nata_sim::protocol;
use nata_sim::rng::{Rng, mix};
use nata_sim::scenario::{Scenario, verify_result};
use nata_sim::sim::Ev;
use nata_sim::trialsupport::Shared;
use turmoil::net::UdpSocket;

pub const COORD: NodeId = 0;

fn host(n: NodeId) -> String {
    format!("n{n}")
}

fn node_of(addr: std::net::SocketAddr) -> NodeId {
    match addr.ip() {
        std::net::IpAddr::V4(v4) => (v4.octets()[3] as NodeId).saturating_sub(1),
        _ => 255,
    }
}

#[derive(Clone)]
pub struct TEnv {
    node: NodeId,
    sock: Rc<UdpSocket>,
    shared: Arc<Shared>,
    rng: Rc<RefCell<Rng>>,
}

impl TEnv {
    fn draw_store(&self) -> (u64, bool) {
        let (lo, hi) = self.shared.store_latency_ns;
        let mut r = self.rng.borrow_mut();
        let lat = lo + r.below(hi - lo + 1);
        (lat, r.chance_ppm(self.shared.store_fail_ppm))
    }
}

impl Env for TEnv {
    fn me(&self) -> NodeId {
        self.node
    }

    fn now(&self) -> Nanos {
        turmoil::elapsed().as_nanos() as Nanos
    }

    fn rand(&self) -> u64 {
        self.rng.borrow_mut().next()
    }

    fn sleep(&self, d: Nanos) -> impl Future<Output = ()> {
        tokio::time::sleep(Duration::from_nanos(d))
    }

    fn send(&self, to: NodeId, bytes: Vec<u8>) {
        let sock = Rc::clone(&self.sock);
        tokio::task::spawn_local(async move {
            let _ = sock.send_to(&bytes, format!("{}:1", host(to))).await;
        });
    }

    fn recv_timeout(&self, d: Nanos) -> impl Future<Output = Option<(NodeId, Vec<u8>)>> {
        let sock = Rc::clone(&self.sock);
        async move {
            let mut buf = vec![0u8; 65536];
            match tokio::time::timeout(Duration::from_nanos(d), sock.recv_from(&mut buf)).await {
                Ok(Ok((n, from))) => {
                    buf.truncate(n);
                    Some((node_of(from), buf))
                }
                _ => None,
            }
        }
    }

    fn spawn(&self, f: impl Future<Output = ()> + 'static) -> TaskCtl {
        let h = tokio::task::spawn_local(f);
        let ah = h.abort_handle();
        TaskCtl::external(move || ah.abort())
    }

    fn store_get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StoreError>> {
        let (lat, fail) = self.draw_store();
        let (sh, key) = (Arc::clone(&self.shared), key.to_string());
        async move {
            tokio::time::sleep(Duration::from_nanos(lat)).await;
            if fail { Err(StoreError::Unavailable) } else { sh.get_now(&key) }
        }
    }

    fn store_put(&self, key: &str, val: Vec<u8>) -> impl Future<Output = Result<(), StoreError>> {
        let (lat, fail) = self.draw_store();
        let (sh, key) = (Arc::clone(&self.shared), key.to_string());
        async move {
            tokio::time::sleep(Duration::from_nanos(lat)).await;
            if fail {
                return Err(StoreError::Unavailable);
            }
            sh.put_object(&key, val);
            Ok(())
        }
    }

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
    pub unmapped: &'static str,
}

pub fn run_seed(seed: u64) -> TrialOutcome {
    let sc = Scenario::from_seed(seed);
    let shared = Shared::new((sc.store.lat_min, sc.store.lat_max), sc.store.fail_ppm);
    for (f, ids) in sc.files.iter().enumerate() {
        shared.put_object(&format!("input/{f}"), encode_i64s(ids));
    }
    let mut b = turmoil::Builder::new();
    b.simulation_duration(Duration::from_secs(5))
        .min_message_latency(Duration::from_nanos(sc.net.base))
        .max_message_latency(Duration::from_nanos(sc.net.base + sc.net.jitter))
        .fail_rate(sc.net.drop_ppm as f64 / 1e6)
        .repair_rate(1.0)
        .rng_seed(seed)
        .epoch(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .enable_random_order();
    let mut sim = b.build();
    let pcfg = sc.cfg.clone();
    for n in 0..=pcfg.workers {
        let (sh, pcfg) = (Arc::clone(&shared), pcfg.clone());
        let make = move || {
            let (sh, pcfg) = (Arc::clone(&sh), pcfg.clone());
            async move {
                let sock = Rc::new(UdpSocket::bind("0.0.0.0:1").await?);
                let rng = Rc::new(RefCell::new(Rng::stream(seed, mix(n as u64, turmoil::elapsed().as_nanos() as u64))));
                let env = TEnv { node: n, sock, shared: Arc::clone(&sh), rng };
                if n == COORD {
                    protocol::coordinator(env, pcfg).await;
                    sh.gauge_set(COORD, "finished", 1);
                } else {
                    protocol::worker(env, pcfg).await;
                }
                Ok(())
            }
        };
        if n == COORD {
            sim.client(host(n), make());
        } else {
            sim.host(host(n), make);
        }
    }
    let mut events: Vec<(u64, Ev)> = Vec::new();
    for (t, ev) in &sc.timed {
        match ev {
            Ev::ClientCancel => {
                let t = *t;
                sim.client("cli", async move {
                    let sock = UdpSocket::bind("0.0.0.0:1").await?;
                    tokio::time::sleep(Duration::from_nanos(t)).await;
                    sock.send_to(&Msg::ClientCancel.encode(), format!("{}:1", host(COORD))).await?;
                    Ok(())
                });
            }
            e => events.push((*t, e.clone())),
        }
    }
    events.sort_by_key(|(t, _)| *t);
    let mut next = 0;
    let mut result: Result<(), String> = Ok(());
    loop {
        while next < events.len() && sim.elapsed().as_nanos() as u64 >= events[next].0 {
            match &events[next].1 {
                Ev::Crash(n) => sim.crash(host(*n)),
                Ev::Restart(n) => sim.bounce(host(*n)),
                Ev::Partition(a, b) => sim.partition(host(*a), host(*b)),
                Ev::Heal(a, b) => sim.repair(host(*a), host(*b)),
                Ev::ClientCancel => {}
            }
            next += 1;
        }
        match sim.step() {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => {
                result = Err(format!("simulation ended with an error: {e}"));
                break;
            }
        }
    }
    let unmapped = if !sc.crash_points.is_empty() || sc.net.dup_ppm > 0 || sc.net.corrupt_ppm > 0 {
        "crash-points,duplicates,corruption"
    } else {
        ""
    };
    let fail = match result {
        Err(m) => Some(format!("hang: {m}")),
        Ok(()) => {
            if let Some(v) = shared.violations().first() {
                Some(format!("violation: {v}"))
            } else {
                verify_result(shared.gauge(COORD, "outcome"), shared.object("result/rows"), shared.object("result/manifest"), &sc.expected())
                    .map(|f| format!("{}: {}", f.kind, f.msg))
            }
        }
    };
    TrialOutcome { seed, fail, hash: shared.trace_hash(), events: shared.events.load(std::sync::atomic::Ordering::SeqCst), unmapped }
}
