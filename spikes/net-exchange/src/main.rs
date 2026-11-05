//! `netx`: one node of the shuffle as a process (for the network-namespace runs), the expected result, and the in-process baseline.
//!
//!   netx node   --me I --addrs A0,A1,.. [--rows R] [--batch-rows B] [--credit-bytes C] [--dead-ms D] [--seed S] [--consumer-delay-us U]
//!               [--die-at NODE:ROUND] [--cancel-at NODE:BATCHES]
//!   netx expect --nodes N [--rows R] [--seed S]
//!   netx inproc --nodes N [--rows R] [--batch-rows B] [--credit-bytes C] [--seed S]
//!
//! `node` exit status: 0 done, 3 cancelled, 4 peer lost, 5 protocol error, 6 setup/io error, 7 simulated crash.
//! Output (one line each, key=value): `RESULT ...` on success, `ERROR ...` on failure, with `at_ms` since the node started.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::process::exit;
use std::time::{Duration, Instant};

use spike_net_exchange::flow::NetError;
use spike_net_exchange::shuffle::{NodeError, Params, expected, run_inproc, run_node};

fn parse_args(args: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let k = args[i].trim_start_matches("--").to_string();
        if i + 1 >= args.len() || args[i].len() < 3 || !args[i].starts_with("--") {
            eprintln!("bad argument '{}'", args[i]);
            exit(2);
        }
        m.insert(k, args[i + 1].clone());
        i += 2;
    }
    m
}

fn num<T: std::str::FromStr>(m: &HashMap<String, String>, k: &str, default: T) -> T {
    match m.get(k) {
        None => default,
        Some(v) => v.parse().unwrap_or_else(|_| {
            eprintln!("--{k} '{v}' is not a number");
            exit(2)
        }),
    }
}

fn pair(m: &HashMap<String, String>, k: &str) -> Option<(u32, u64)> {
    let v = m.get(k)?;
    let (a, b) = v.split_once(':').unwrap_or_else(|| {
        eprintln!("--{k} wants NODE:NUMBER");
        exit(2)
    });
    Some((a.parse().unwrap_or_else(|_| exit(2)), b.parse().unwrap_or_else(|_| exit(2))))
}

fn params(m: &HashMap<String, String>, nodes: u32) -> Params {
    let mut p = Params::new(nodes, num(m, "rows", 1_000_000u64));
    p.batch_rows = num(m, "batch-rows", 4096 * nodes as usize);
    p.seed = num(m, "seed", 42u64);
    p.cfg.credit_bytes = num(m, "credit-bytes", p.cfg.credit_bytes);
    p.cfg.dead_after = Duration::from_millis(num(m, "dead-ms", 5000u64));
    p.cfg.ping_every = Duration::from_millis(num(m, "ping-ms", 500u64));
    p.consumer_delay = Duration::from_micros(num(m, "consumer-delay-us", 0u64));
    p.connect_timeout = Duration::from_secs(num(m, "connect-timeout-s", 30u64));
    p.die_at = pair(m, "die-at");
    p.cancel_at = pair(m, "cancel-at");
    p
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let m = parse_args(&args[args.len().min(2)..]);
    match cmd {
        "node" => {
            let addrs: Vec<SocketAddr> = m
                .get("addrs")
                .unwrap_or_else(|| {
                    eprintln!("--addrs is required");
                    exit(2)
                })
                .split(',')
                .map(|a| {
                    a.parse().unwrap_or_else(|_| {
                        eprintln!("bad address '{a}'");
                        exit(2)
                    })
                })
                .collect();
            let me: u32 = num(&m, "me", u32::MAX);
            if me as usize >= addrs.len() {
                eprintln!("--me must be a valid index into --addrs");
                exit(2);
            }
            let p = params(&m, addrs.len() as u32);
            let t0 = Instant::now();
            let listener = TcpListener::bind(addrs[me as usize]).unwrap_or_else(|e| {
                println!("ERROR node={me} kind=setup at_ms=0 msg=\"bind {}: {e}\"", addrs[me as usize]);
                exit(6)
            });
            match run_node(me, listener, &addrs, &p) {
                Ok(r) => {
                    println!(
                        "RESULT node={} rows_in={} checksum={} bytes_sent={} bytes_recv={} peak_buffered={} peak_inflight={} blocked_waits={} secs={:.3} credit={}",
                        r.me, r.rows_in, r.checksum, r.bytes_sent, r.bytes_recv, r.peak_buffered, r.peak_inflight, r.blocked_waits, r.secs, p.cfg.credit_bytes
                    );
                }
                Err(e) => {
                    let (kind, code) = match &e {
                        NodeError::Net(NetError::Cancelled) => ("cancelled", 3),
                        NodeError::Net(NetError::PeerLost(_)) => ("peer-lost", 4),
                        NodeError::Net(NetError::Protocol(_)) => ("protocol", 5),
                        NodeError::Net(_) | NodeError::Setup(_) => ("setup", 6),
                        NodeError::Died => ("died", 7),
                    };
                    println!("ERROR node={me} kind={kind} at_ms={} msg=\"{}\"", t0.elapsed().as_millis(), e.to_string().replace('"', "'"));
                    exit(code);
                }
            }
        }
        "expect" => {
            let nodes: u32 = num(&m, "nodes", 0);
            if nodes < 2 {
                eprintln!("--nodes must be at least 2");
                exit(2);
            }
            let p = params(&m, nodes);
            for d in 0..nodes {
                let (rows, sum) = expected(d, &p);
                println!("EXPECT node={d} rows_in={rows} checksum={sum}");
            }
        }
        "inproc" => {
            let nodes: u32 = num(&m, "nodes", 0);
            if nodes < 2 {
                eprintln!("--nodes must be at least 2");
                exit(2);
            }
            let p = params(&m, nodes);
            let (secs, bytes) = run_inproc(&p);
            println!("INPROC nodes={nodes} rows_per_node={} bytes_moved={bytes} secs={secs:.3} mb_per_s={:.1}", p.rows_per_node, bytes as f64 / 1e6 / secs);
        }
        _ => {
            eprintln!("usage: netx node|expect|inproc [--key value ...]  (see the header of src/main.rs)");
            exit(2);
        }
    }
}
