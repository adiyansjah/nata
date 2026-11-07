#![allow(dead_code)]
include!("support/mod.rs");
use std::sync::atomic::AtomicBool;
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;
struct Panics {
    count: Arc<std::sync::atomic::AtomicUsize>,
    previous: Option<Arc<PanicHook>>,
}
impl Panics {
    fn new() -> Self {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let previous = Arc::new(std::panic::take_hook());
        let hook = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            c.fetch_add(1, Ordering::SeqCst);
            hook(info);
        }));
        Self {
            count,
            previous: Some(previous),
        }
    }
    fn check(&self) {
        assert_eq!(self.count.load(Ordering::SeqCst), 0, "background panic");
    }
}
impl Drop for Panics {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let previous = self.previous.take().expect("hook");
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn count(dir: &str) -> usize {
    std::fs::read_dir(dir).expect("proc directory").count()
}
fn rss() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .expect("status")
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .expect("RSS")
        .split_whitespace()
        .nth(1)
        .expect("KiB")
        .parse()
        .expect("RSS number")
}
fn released(s: &Server, sessions: usize) {
    wait_until(|| {
        s.metrics.registered.load(Ordering::SeqCst) == sessions
            && s.metrics.sessions.load(Ordering::SeqCst) == sessions
            && s.metrics.running.load(Ordering::SeqCst) == 0
            && s.metrics.admitted.now() == 0
            && s.metrics.objects.now() == 0
    });
    for stats in s.metrics.scans.lock().expect("scans").iter() {
        assert_eq!(stats.handles.now(), 0, "open file");
        assert_eq!(stats.footers.now(), 0, "footer");
        assert_eq!(stats.memory.now(), 0, "scan buffer");
    }
}
#[derive(Debug)]
struct SlowOpen;
impl nata_parquet::Opener for SlowOpen {
    fn open(
        &self,
        path: &std::path::Path,
        why: nata_parquet::OpenFor,
    ) -> std::io::Result<nata_parquet::OpenFile> {
        std::thread::sleep(Duration::from_millis(1));
        nata_parquet::Opener::open(&nata_parquet::FsOpener, path, why)
            .map(|f| f.slow_reads(Duration::from_micros(100)))
    }
}
struct Contention {
    stop: Arc<AtomicBool>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
impl Contention {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let workers = (0..knob("NATA_PGWIRE_CONTENTION", 0))
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        for _ in 0..1024 {
                            std::hint::black_box(17u64.wrapping_mul(7919));
                        }
                    }
                })
            })
            .collect();
        Self { stop, workers }
    }
}
impl Drop for Contention {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for w in self.workers.drain(..) {
            w.join().expect("competitor");
        }
    }
}
fn kill_client(s: &Server, q: &str, phase: u64) {
    use std::io::BufRead;
    let script = r#"import socket,struct,sys,time
s=socket.create_connection(('127.0.0.1',int(sys.argv[1])))
def frame():
 h=s.recv(5)
 while len(h)<5:h+=s.recv(5-len(h))
 n=struct.unpack('!i',h[1:])[0]-4
 while n:
  b=s.recv(n);n-=len(b)
 return h[0]
phase=int(sys.argv[3])
if phase==0:
 s.sendall(b'\0\0\0');print('ready',flush=True);time.sleep(600)
b=b'\0\3\0\0user\0test\0\0';s.sendall(struct.pack('!i',len(b)+4)+b)
while frame()!=90:pass
if phase==1:
 s.sendall(b'P\0\0\0\x20name\0SELECT');print('ready',flush=True);time.sleep(600)
if phase==2:
 b=b's\0'+sys.argv[2].encode()+b'\0\0\0';s.sendall(b'P'+struct.pack('!i',len(b)+4)+b+b'H\0\0\0\4');assert frame()==49;print('ready',flush=True);time.sleep(600)
q=sys.argv[2].encode()+b'\0';s.sendall(b'Q'+struct.pack('!i',len(q)+4)+q)
if phase!=4:assert frame()==84
print('ready',flush=True)
time.sleep(600)
"#;
    let mut child = std::process::Command::new("python3")
        .args([
            "-c",
            script,
            &s.address.port().to_string(),
            q,
            &phase.to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("child");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("child ready");
    assert_eq!(line.trim(), "ready");
    child.kill().expect("SIGKILL");
    child.wait().expect("reap");
}
// Stop healthy clients before scope joins, including when a deliberately bad
// implementation makes an assertion panic inside the scenario loop.
struct StopHealthy(Arc<AtomicBool>);
impl Drop for StopHealthy {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
fn bounded_cancel_result(c: &mut Client) -> Vec<(u8, Vec<u8>)> {
    let mut result = Vec::new();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            result.len() < 10_000 && Instant::now() < until,
            "cancel failed to stop result"
        );
        let message = read(&mut c.socket);
        let end = message.0 == b'Z';
        result.push(message);
        if end {
            return result;
        }
    }
}
#[test]
fn seeded_disconnect_cancel_kill_under_contention() {
    let _serial = SERIAL.lock().expect("serial");
    let panics = Panics::new();
    let n = knob("NATA_PGWIRE_SCENARIOS", 28);
    let mut seed = knob("NATA_PGWIRE_SEED", 0x4b20261107) as u64;
    let start = Instant::now();
    eprintln!("RELIABILITY seed={seed} scenarios={n}");
    let aggregate = knob("NATA_PGWIRE_AGGREGATE", 0) != 0;
    let q = if aggregate {
        format!("SELECT id,count(*) FROM '{}' GROUP BY id", large())
    } else {
        format!("SELECT s FROM '{}'", large())
    };
    let small = format!("SELECT si FROM '{}' LIMIT 1", fixture());
    let m = Arc::new(Metrics {
        opener: Some(Arc::new(SlowOpen)),
        ..Metrics::default()
    });
    let s = if aggregate {
        let mut execution = nata_exec::ExecConfig::new(1);
        execution.aggregate_memory_bytes = Some(128 << 20);
        execution.aggregate_force_spill = true;
        Server::start_execution(64, m, execution)
    } else {
        Server::start_with(64, m)
    };
    // Warm allocator arenas and all query paths before taking a baseline.
    let mut warm = s.client();
    warm.send_query(&q);
    assert_eq!(read(&mut warm.socket).0, b'T');
    drop(warm);
    panics.check();
    released(&s, 0);
    let mut healthy = Vec::new();
    for _ in 0..4 {
        let mut c = s.client();
        assert_eq!(values(&c.query(&small)), vec![vec![Some("-32768".into())]]);
        healthy.push(c);
    }
    let baseline = (count("/proc/self/fd"), count("/proc/self/task"), rss());
    let mut peak = baseline.2;
    let contention = Contention::start();
    let stop = Arc::new(AtomicBool::new(false));
    let good = std::thread::scope(|scope| {
        let _stop_healthy = StopHealthy(stop.clone());
        let good: Vec<_> = healthy
            .into_iter()
            .map(|mut c| {
                let stop = stop.clone();
                scope.spawn(move || {
                    let mut rows = 0;
                    while !stop.load(Ordering::Relaxed) {
                        assert_eq!(values(&c.query("SELECT 1")), vec![vec![Some("1".into())]]);
                        rows += 1;
                        if rows % 100 == 0 {
                            assert_eq!(
                                values(
                                    &c.query(&format!("SELECT si FROM '{}' LIMIT 1", fixture()))
                                ),
                                vec![vec![Some("-32768".into())]]
                            );
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    rows
                })
            })
            .collect();
        let mut kinds = [0usize; 7];
        for i in 0..n {
            let kind = (random(&mut seed) % 7) as usize;
            kinds[kind] += 1;
            let delay = Duration::from_micros(random(&mut seed) % 2000);
            match kind {
                0 => {
                    let mut c = s.socket();
                    c.write_all(&[0, 0, 0]).expect("partial startup");
                    std::thread::sleep(delay);
                    drop(c);
                }
                1 => {
                    let mut c = s.client();
                    c.socket
                        .write_all(b"P\0\0\0\x20name\0SELECT")
                        .expect("partial Parse");
                    std::thread::sleep(delay);
                    drop(c);
                }
                2 => {
                    let mut c = s.client();
                    c.send_query(&q);
                    assert_eq!(read(&mut c.socket).0, b'T');
                    std::thread::sleep(delay);
                    drop(c);
                }
                3 => {
                    let mut c = s.client();
                    c.parse_named("s", &small, &[]);
                    c.bind_named("p", "s");
                    c.frame(b'H', b"");
                    assert_eq!(read(&mut c.socket).0, b'1');
                    std::thread::sleep(delay);
                    drop(c);
                }
                4 => {
                    let mut c = s.client();
                    c.send_query(&q);
                    assert_eq!(read(&mut c.socket).0, b'T');
                    std::thread::sleep(delay);
                    cancel(&s, c.key);
                    assert_code(&bounded_cancel_result(&mut c), "57014");
                }
                5 => kill_client(&s, &q, random(&mut seed) % 4),
                _ => {
                    let mut a = s.client();
                    let mut b = s.client();
                    a.send_query(&q);
                    b.send_query(&q);
                    assert_eq!(read(&mut a.socket).0, b'T');
                    assert_eq!(read(&mut b.socket).0, b'T');
                    if random(&mut seed).is_multiple_of(3) {
                        kill_client(&s, &small, 4);
                    }
                    let mut c = s.client();
                    c.send_query(&small);
                    std::thread::sleep(delay + Duration::from_millis(2));
                    if random(&mut seed) & 1 == 0 {
                        cancel(&s, c.key);
                        assert_code(&bounded_cancel_result(&mut c), "57014");
                    }
                    drop(c);
                    drop(a);
                    drop(b);
                }
            }
            panics.check();
            released(&s, 4);
            peak = peak.max(rss());
            if (i + 1) % 250 == 0 {
                eprintln!("RELIABILITY completed={} peak_rss_kib={peak}", i + 1);
            }
        }
        stop.store(true, Ordering::Relaxed);
        let completed: usize = good
            .into_iter()
            .map(|t| t.join().expect("healthy session"))
            .sum();
        eprintln!("RELIABILITY kinds={kinds:?} healthy_queries={completed}");
        completed
    });
    drop(contention);
    assert!(good > 0);
    panics.check();
    released(&s, 0);
    // Healthy client sockets account for eight fds in the baseline (both ends).
    wait_until(|| count("/proc/self/task") == baseline.1);
    assert_eq!(count("/proc/self/fd"), baseline.0 - 8);
    let final_rss = rss();
    assert!(
        final_rss <= baseline.2 + 64 * 1024,
        "RSS retention {baseline:?} final={final_rss}"
    );
    assert!(peak <= baseline.2 + 256 * 1024, "peak RSS {peak}");
    eprintln!(
        "RELIABILITY PASS baseline_fd_threads_rss={baseline:?} final_fd_threads_rss={:?} peak_rss_kib={peak} elapsed={:?}",
        (count("/proc/self/fd"), count("/proc/self/task"), final_rss),
        start.elapsed()
    );
}

#[test]
fn seeded_live_protocol_fuzz() {
    let _serial = SERIAL.lock().expect("serial");
    let panics = Panics::new();
    let n = knob("NATA_PGWIRE_LIVE_CASES", 100);
    let mut seed = knob("NATA_PGWIRE_SEED", 0x4b20261107) as u64;
    eprintln!("LIVE_FUZZ seed={seed} cases={n}");
    let start = Instant::now();
    let s = Server::start(64);
    let mut healthy = s.client();
    assert_eq!(
        values(&healthy.query("SELECT 1")),
        vec![vec![Some("1".into())]]
    );
    let baseline = (count("/proc/self/fd"), count("/proc/self/task"), rss());
    let mut kinds = [0usize; 12];
    for i in 0..n {
        let kind = (random(&mut seed) % 12) as usize;
        kinds[kind] += 1;
        let mut c = s.socket();
        if kind == 0 {
            c.write_all(&[7i32.to_be_bytes().as_slice(), b"bad"].concat())
                .expect("startup malformed");
        } else {
            let mut client = Client::start(c);
            let bytes = match kind {
                1 => codec::packet(b'E', b"\0\0\0\0\0").expect("Execute without Bind"),
                2 => codec::packet(b'B', b"\0missing\0\0\0\0\0\0\0").expect("unknown statement"),
                3 => {
                    client.frame(b'C', b"Pabsent\0");
                    assert_eq!(read(&mut client.socket).0, b'3');
                    codec::packet(b'D', b"Pabsent\0").expect("unknown portal")
                }
                4 => {
                    for _ in 0..32 {
                        client.frame(b'S', b"");
                        assert_eq!(read(&mut client.socket).0, b'Z');
                    }
                    codec::packet(b'E', b"absent\0\0\0\0\0").expect("Sync storms")
                }
                5 => codec::packet(b'B', b"\0\0\0\0\x7f\xff").expect("huge parameter count"),
                6 => codec::packet(b'B', b"\0\0\0\0\0\x01\x7f\xff\xff\xff\0\0")
                    .expect("huge parameter length"),
                7 => {
                    let tags = b"PBDEC";
                    codec::packet(tags[(random(&mut seed) % 5) as usize], b"").expect("short body")
                }
                8 => vec![b'Q', 0x7f, 0xff, 0xff, 0xff],
                9 => {
                    let mut b = codec::packet(b'P', b"s\0SELECT 1\0\0\0").expect("Parse");
                    b.truncate(1 + (random(&mut seed) as usize % (b.len() - 1)));
                    b
                }
                10 => {
                    let mut b = vec![0; (random(&mut seed) % 128) as usize];
                    for v in &mut b {
                        *v = random(&mut seed) as u8;
                    }
                    codec::packet(b'?', &b).expect("unknown tag")
                }
                _ => {
                    client.frame(b'P', b"s\0SELECT 1\0\0\0");
                    assert_eq!(read(&mut client.socket).0, b'1');
                    codec::packet(b'P', b"s\0SELECT 1\0\0\0").expect("duplicate")
                }
            };
            client.socket.write_all(&bytes).expect("malformed frame");
            c = client.socket;
        }
        if let Err(e) = c.shutdown(std::net::Shutdown::Write) {
            assert!(
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotConnected | std::io::ErrorKind::ConnectionReset
                ),
                "EOF truncation: {e}"
            );
        }
        let mut bytes = Vec::new();
        if let Err(e) = c.read_to_end(&mut bytes) {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset,
                "deadline/closed"
            );
        }
        // A closed connection satisfies the fatal/truncated input contract; semantic
        // misuse must send ErrorResponse before EOF (Sync not needed to close).
        if matches!(kind, 1..=6 | 11) {
            assert_eq!(bytes.first(), Some(&b'E'), "case {i} kind {kind}");
        }
        drop(c);
        panics.check();
        released(&s, 1);
        if i % 100 == 0 {
            assert_eq!(
                values(&healthy.query("SELECT 1")),
                vec![vec![Some("1".into())]]
            );
        }
    }
    assert_eq!(count("/proc/self/fd"), baseline.0);
    assert_eq!(count("/proc/self/task"), baseline.1);
    assert!(rss() <= baseline.2 + 32 * 1024);
    eprintln!(
        "LIVE_FUZZ PASS kinds={kinds:?} elapsed={:?} rss_baseline_final_kib={:?}",
        start.elapsed(),
        (baseline.2, rss())
    );
}

#[test]
fn sixty_four_sessions_and_reserved_sockets() {
    let _serial = SERIAL.lock().expect("serial");
    let panics = Panics::new();
    let s = Server::start(64);
    let mut clients: Vec<_> = (0..64).map(|_| s.client()).collect();
    let mut extra = s.socket();
    startup(&mut extra);
    let (t, b) = read(&mut extra);
    assert_eq!(t, b'E');
    assert_eq!(error_field(&b, b'C'), Some("53300"));
    drop(extra);
    let mut reserved = Vec::new();
    for _ in 0..8 {
        let mut c = s.socket();
        c.write_all(&[0]).expect("partial startup");
        reserved.push(c);
    }
    std::thread::sleep(Duration::from_millis(30));
    let mut excess = s.socket();
    let mut end = [0];
    assert_eq!(excess.read(&mut end).expect("socket cap"), 0);
    drop(excess);
    drop(reserved);
    let q = format!("SELECT si FROM '{}' LIMIT 1", fixture());
    std::thread::scope(|scope| {
        for mut c in clients.drain(..) {
            let q = &q;
            scope.spawn(move || {
                assert_eq!(values(&c.query(q)), vec![vec![Some("-32768".into())]]);
            });
        }
    });
    panics.check();
    released(&s, 0);
    eprintln!("BOUND PASS 64 concurrent correct queries; 64+8 socket cap; 65th startup=53300");
}

#[test]
fn one_byte_per_millisecond_reader_is_bounded() {
    let _serial = SERIAL.lock().expect("serial");
    let panics = Panics::new();
    let q = format!("SELECT s FROM '{}'", large());
    let s = Server::start(64);
    let mut c = s.client();
    let baseline = rss();
    let mut peak = baseline;
    let before = s.metrics.rows_produced.load(Ordering::SeqCst);
    c.send_query(&q);
    assert_eq!(read(&mut c.socket).0, b'T');
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let reader = std::thread::spawn(move || {
        let mut bytes = 0;
        let mut byte = [0];
        while !done.load(Ordering::Relaxed) {
            match c.socket.read(&mut byte) {
                Ok(1) => bytes += 1,
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(e) => panic!("slow reader {e}"),
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        bytes
    });
    let mut healthy = s.client();
    assert_eq!(
        values(&healthy.query(&format!("SELECT si FROM '{}' LIMIT 1", fixture()))),
        vec![vec![Some("-32768".into())]]
    );
    let deadline = Instant::now() + Duration::from_secs(6);
    while s.metrics.running.load(Ordering::SeqCst) > 0 {
        peak = peak.max(rss());
        assert!(
            Instant::now() < deadline,
            "slow reader did not release query"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.store(true, Ordering::Relaxed);
    let bytes = reader.join().expect("reader");
    drop(healthy);
    panics.check();
    released(&s, 0);
    let produced = s.metrics.rows_produced.load(Ordering::SeqCst) - before;
    assert!(bytes > 0);
    assert!(produced < 20_000, "buffered {produced} rows of 1,048,576");
    assert!(
        peak <= baseline + 128 * 1024,
        "RSS baseline={baseline} peak={peak}"
    );
    eprintln!(
        "SLOW_READER PASS result_rows=1048576 width=1024 bytes_read={bytes} produced={produced} rss_baseline_peak_kib={baseline},{peak} bound_delta_kib=131072"
    );
}
