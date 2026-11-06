use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int8Array,
    Int16Array, Int32Array, Int64Array, RecordBatch, StringArray,
};
use nata_pgwire::{
    codec,
    server::{Config, Metrics, serve_listener},
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

struct Server {
    address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    metrics: Arc<Metrics>,
}
impl Server {
    fn start(max: usize) -> Self {
        let (address_tx, address_rx) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let metrics = Arc::new(Metrics::default());
        let m = Arc::clone(&metrics);
        let thread = std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime").block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listen");
                let address = listener.local_addr().expect("address"); address_tx.send(address).expect("send");
                let config = Config {listen:address, unsafe_listen:false, max_connections:max, threads:2, metrics:Some(m)};
                tokio::select! { r = serve_listener(listener,config) => panic!("server ended {r:?}"), _ = stopped => {} }
            });
        });
        Self {
            address: address_rx.recv().expect("address"),
            stop: Some(stop),
            thread: Some(thread),
            metrics,
        }
    }
    fn socket(&self) -> TcpStream {
        let s = TcpStream::connect(self.address).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        s.set_write_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        s
    }
    fn client(&self) -> Client {
        Client::start(self.socket())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop.take().expect("stop").send(());
        self.thread.take().expect("thread").join().expect("server");
    }
}
struct Client {
    socket: TcpStream,
    key: (i32, i32),
    startup: Vec<u8>,
}
fn read(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0; 5];
    s.read_exact(&mut h).expect("header");
    let n = i32::from_be_bytes(h[1..].try_into().expect("length"));
    assert!((4..100_000_000).contains(&n));
    let mut b = vec![0; n as usize - 4];
    s.read_exact(&mut b).expect("body");
    (h[0], b)
}
fn startup(s: &mut TcpStream) {
    let body = b"\0\x03\0\0user\0test\0database\0nata\0\0";
    s.write_all(&((body.len() + 4) as i32).to_be_bytes())
        .expect("length");
    s.write_all(body).expect("startup");
}
impl Client {
    fn start(mut socket: TcpStream) -> Self {
        startup(&mut socket);
        let mut tags = Vec::new();
        let mut key = (0, 0);
        loop {
            let (t, b) = read(&mut socket);
            tags.push(t);
            match t {
                b'K' => {
                    key = (
                        i32::from_be_bytes(b[..4].try_into().expect("pid")),
                        i32::from_be_bytes(b[4..].try_into().expect("secret")),
                    );
                }
                b'Z' => break,
                b'E' => panic!("startup error: {b:?}"),
                _ => {}
            }
        }
        assert_eq!(tags, b"RSSSSSSKZ");
        Self {
            socket,
            key,
            startup: tags,
        }
    }
    fn send_query(&mut self, q: &str) {
        self.socket
            .write_all(&codec::packet(b'Q', format!("{q}\0").as_bytes()).expect("packet"))
            .expect("query");
    }
    fn query(&mut self, q: &str) -> Vec<(u8, Vec<u8>)> {
        self.send_query(q);
        self.result()
    }
    fn result(&mut self) -> Vec<(u8, Vec<u8>)> {
        let mut result = Vec::new();
        loop {
            let m = read(&mut self.socket);
            let end = m.0 == b'Z';
            result.push(m);
            if end {
                return result;
            }
        }
    }
}
fn tags(m: &[(u8, Vec<u8>)]) -> Vec<u8> {
    m.iter().map(|m| m.0).collect()
}
fn values(m: &[(u8, Vec<u8>)]) -> Vec<Vec<Option<String>>> {
    m.iter()
        .filter(|m| m.0 == b'D')
        .map(|(_, b)| {
            let n = i16::from_be_bytes(b[..2].try_into().expect("columns"));
            let mut at = 2;
            (0..n)
                .map(|_| {
                    let len = i32::from_be_bytes(b[at..at + 4].try_into().expect("len"));
                    at += 4;
                    if len == -1 {
                        None
                    } else {
                        let v = String::from_utf8(b[at..at + len as usize].to_vec()).expect("text");
                        at += len as usize;
                        Some(v)
                    }
                })
                .collect()
        })
        .collect()
}
fn error_field(b: &[u8], tag: u8) -> Option<&str> {
    b.split(|b| *b == 0)
        .find(|v| v.first() == Some(&tag))
        .map(|v| std::str::from_utf8(&v[1..]).expect("field"))
}
fn wait_until(f: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < until, "deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn cancel(s: &Server, key: (i32, i32)) {
    let mut c = s.socket();
    let b = [
        16i32.to_be_bytes(),
        80877102i32.to_be_bytes(),
        key.0.to_be_bytes(),
        key.1.to_be_bytes(),
    ]
    .concat();
    c.write_all(&b).expect("cancel");
    let mut end = [0];
    assert_eq!(c.read(&mut end).expect("closed cancel"), 0);
}
fn fixture() -> String {
    static FILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("nata-pgwire-{}.parquet", std::process::id()));
        let columns: Vec<(&str, ArrayRef)> = vec![
            (
                "b",
                Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
            ),
            (
                "ti",
                Arc::new(Int8Array::from(vec![Some(-128), Some(127), None])),
            ),
            (
                "si",
                Arc::new(Int16Array::from(vec![Some(-32768), Some(32767), None])),
            ),
            (
                "i",
                Arc::new(Int32Array::from(vec![Some(i32::MIN), Some(i32::MAX), None])),
            ),
            (
                "bi",
                Arc::new(Int64Array::from(vec![Some(i64::MIN), Some(i64::MAX), None])),
            ),
            (
                "r",
                Arc::new(Float32Array::from(vec![
                    Some(-0.0),
                    Some(f32::INFINITY),
                    None,
                ])),
            ),
            (
                "d",
                Arc::new(Float64Array::from(vec![
                    Some(f64::NAN),
                    Some(f64::NEG_INFINITY),
                    None,
                ])),
            ),
            (
                "n",
                Arc::new(
                    Decimal128Array::from(vec![Some(123450), Some(-1), None])
                        .with_precision_and_scale(38, 3)
                        .expect("decimal"),
                ),
            ),
            (
                "s",
                Arc::new(StringArray::from(vec![
                    Some(""),
                    Some("secret-cell-a,b\né"),
                    None,
                ])),
            ),
            (
                "dt",
                Arc::new(Date32Array::from(vec![Some(0), Some(19782), None])),
            ),
        ];
        let batch = RecordBatch::try_from_iter(columns).expect("batch");
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&path).expect("create"),
            batch.schema(),
            None,
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
        path.to_str().expect("path").into()
    })
    .clone()
}
fn large() -> String {
    static FILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("nata-pgwire-large-{}.parquet", std::process::id()));
        let batch = RecordBatch::try_from_iter([
            (
                "id",
                Arc::new(Int64Array::from_iter_values(0..8192)) as ArrayRef,
            ),
            (
                "s",
                Arc::new(StringArray::from(vec!["x".repeat(1024); 8192])) as ArrayRef,
            ),
        ])
        .expect("batch");
        let props = parquet::file::properties::WriterProperties::builder()
            .set_max_row_group_row_count(Some(8192))
            .build();
        let mut w = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&path).expect("create"),
            batch.schema(),
            Some(props),
        )
        .expect("writer");
        for _ in 0..128 {
            w.write(&batch).expect("write");
        }
        w.close().expect("close");
        path.to_str().expect("path").into()
    })
    .clone()
}
#[test]
fn recorded_simple_error_empty_types_nulls_and_terminate() {
    let s = Server::start(4);
    let mut c = s.client();
    assert_eq!(c.startup, b"RSSSSSSKZ");
    let m = c.query(&format!("SELECT * FROM '{}'", fixture()));
    assert_eq!(tags(&m), b"TDDDCZ");
    assert_eq!(m[m.len() - 2].1, b"SELECT 3\0");
    let rows = values(&m);
    assert_eq!(
        rows[0],
        vec![
            Some("t".into()),
            Some("-128".into()),
            Some("-32768".into()),
            Some(i32::MIN.to_string()),
            Some(i64::MIN.to_string()),
            Some("-0.0".into()),
            Some("NaN".into()),
            Some("123.450".into()),
            Some("".into()),
            Some("1970-01-01".into())
        ]
    );
    assert_eq!(rows[1][5], Some("Infinity".into()));
    assert_eq!(rows[1][6], Some("-Infinity".into()));
    assert!(rows[2].iter().all(Option::is_none));
    let mut desc = &m[0].1[2..];
    for oid in [16, 25, 21, 23, 20, 700, 701, 1700, 25, 1082] {
        let end = desc.iter().position(|b| *b == 0).expect("name");
        desc = &desc[end + 1..];
        assert_eq!(
            i32::from_be_bytes(desc[6..10].try_into().expect("oid")),
            oid
        );
        desc = &desc[18..];
    }
    let q = format!("SELECT 'é',\n missing FROM '{}'", fixture());
    let e = c.query(&q);
    assert_eq!(tags(&e), b"EZ");
    assert_eq!(error_field(&e[0].1, b'C'), Some("42804"));
    assert_eq!(error_field(&e[0].1, b'P'), Some("14"));
    let e = c.query("SELECT @");
    assert_eq!(error_field(&e[0].1, b'C'), Some("42601"));
    assert_eq!(tags(&c.query(" ; \n")), b"IZ");
    let e = c.query(&format!("SELECT CAST(s AS INTEGER) FROM '{}'", fixture()));
    assert!(e.iter().any(|m| m.0 == b'E'));
    assert!(
        !e.iter()
            .any(|(_, b)| b.windows(11).any(|x| x == b"secret-cell"))
    );
    let q = format!("EXPLAIN SELECT i FROM '{}'", fixture());
    assert!(tags(&c.query(&q)).starts_with(b"TD"));
    c.socket
        .write_all(&codec::packet(b'X', b"").expect("terminate"))
        .expect("send");
    let mut end = [0];
    assert_eq!(c.socket.read(&mut end).expect("end"), 0);
}
#[test]
fn cancel_wrong_key_disconnect_backpressure_and_session_isolation() {
    let large = large();
    let s = Server::start(4);
    let mut a = s.client();
    let mut b = s.client();
    a.send_query(&format!("SELECT s FROM '{large}'"));
    assert_eq!(read(&mut a.socket).0, b'T');
    wait_until(|| s.metrics.rows_produced.load(Ordering::SeqCst) > 32);
    // Stop reading; socket + bridge fill and executor production must plateau far short of 1M rows.
    std::thread::sleep(Duration::from_millis(200));
    let first = s.metrics.rows_produced.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    let second = s.metrics.rows_produced.load(Ordering::SeqCst);
    eprintln!("backpressure produced first={first} second={second} total=1048576");
    assert_eq!(first, second, "a slow reader stops production");
    assert!(second < 1_048_576);
    cancel(&s, (a.key.0, a.key.1.wrapping_add(1)));
    // The cancel connection closes BEFORE workers necessarily exit. Give a wrong-key
    // mutant time to release admission: an immediate running-count assertion races it.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.metrics.running.load(Ordering::SeqCst), 1);
    b.send_query(&format!("SELECT si FROM '{}' LIMIT 1", fixture()));
    cancel(&s, a.key);
    let r = a.result();
    let e = r.iter().find(|m| m.0 == b'E').expect("cancel error");
    assert_eq!(error_field(&e.1, b'C'), Some("57014"));
    assert_eq!(
        error_field(&e.1, b'M'),
        Some("canceling statement due to user request")
    );
    assert_eq!(r.last().expect("ready").0, b'Z');
    assert_eq!(values(&b.result()), vec![vec![Some("-32768".into())]]);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert_eq!(
        values(&a.query(&format!("SELECT ti FROM '{}' LIMIT 1", fixture()))),
        vec![vec![Some("-128".into())]]
    );
    // Distinct concurrent sessions must retain their own row stream.
    a.send_query(&format!("SELECT si FROM '{}'", fixture()));
    b.send_query(&format!("SELECT ti FROM '{}'", fixture()));
    assert_eq!(values(&a.result())[0][0], Some("-32768".into()));
    assert_eq!(values(&b.result())[0][0], Some("-128".into()));
    a.send_query(&format!("SELECT s FROM '{large}'"));
    assert_eq!(read(&mut a.socket).0, b'T');
    drop(a);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert_eq!(
        tags(&b.query(&format!("SELECT ti FROM '{}' LIMIT 0", fixture()))),
        b"TCZ"
    );
    // Idle/stale keys do not poison the next query.
    cancel(&s, b.key);
    assert_eq!(
        tags(&b.query(&format!("SELECT ti FROM '{}' LIMIT 0", fixture()))),
        b"TCZ"
    );
}
#[test]
fn malformed_lengths_startup_ssl_and_max_connections() {
    let s = Server::start(1);
    let mut probe = s.socket();
    probe
        .write_all(&[8i32.to_be_bytes(), 80877103i32.to_be_bytes()].concat())
        .expect("ssl");
    let mut n = [0];
    probe.read_exact(&mut n).expect("N");
    assert_eq!(&n, b"N");
    drop(probe);
    let mut c = s.client();
    let mut excess = s.socket();
    startup(&mut excess);
    let (t, b) = read(&mut excess);
    assert_eq!(t, b'E');
    assert_eq!(error_field(&b, b'C'), Some("53300"));
    // Full session admission still reserves cancel connections.
    cancel(&s, c.key);
    for len in [3i32, -1, 1_114_113] {
        c.socket
            .write_all(&[vec![b'Q'], len.to_be_bytes().to_vec()].concat())
            .expect("header only");
        let (t, b) = read(&mut c.socket);
        assert_eq!(t, b'E');
        assert_eq!(error_field(&b, b'C'), Some("08P01"));
        assert_eq!(c.socket.read(&mut n).expect("closed"), 0);
        drop(c);
        wait_until(|| s.metrics.sessions.load(Ordering::SeqCst) == 0);
        c = s.client();
    }
    c.socket.write_all(b"Q\0\0\0\x08x").expect("partial");
    c.socket
        .shutdown(std::net::Shutdown::Write)
        .expect("half close");
    assert_eq!(read(&mut c.socket).0, b'E');
    drop(c);
    wait_until(|| s.metrics.sessions.load(Ordering::SeqCst) == 0);
    for len in [7, 10_001, i32::MAX] {
        let mut c = s.socket();
        c.write_all(&len.to_be_bytes()).expect("oversized startup");
        assert_eq!(read(&mut c).0, b'E');
        assert_eq!(c.read(&mut n).expect("closed"), 0);
    }
    let c = Config {
        listen: "127.0.0.1:0".parse().expect("addr"),
        unsafe_listen: false,
        max_connections: 0,
        threads: 1,
        metrics: None,
    };
    assert!(c.validate().is_err());
}
#[test]
fn reserved_socket_cap_and_startup_deadline() {
    let s = Server::start(1);
    let _c = s.client();
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut c = s.socket();
        c.write_all(&[0]).expect("partial startup");
        held.push(c);
    }
    std::thread::sleep(Duration::from_millis(100));
    let mut excess = s.socket();
    let mut end = [0];
    assert_eq!(excess.read(&mut end).expect("excess closed"), 0);
    for mut h in held {
        let (t, b) = read(&mut h);
        assert_eq!(t, b'E');
        assert_eq!(error_field(&b, b'C'), Some("08P01"));
        assert_eq!(h.read(&mut end).expect("deadline closed"), 0);
    }
}
#[test]
fn psql_when_installed() {
    let Ok(version) = std::process::Command::new("psql").arg("--version").output() else {
        eprintln!("SKIP: psql is absent");
        return;
    };
    assert!(version.status.success());
    eprintln!("{}", String::from_utf8_lossy(&version.stdout));
    let s = Server::start(2);
    let out = std::process::Command::new("psql")
        .env("PGSSLMODE", "disable")
        .env_remove("PGOPTIONS")
        .args([
            "-X",
            "-h",
            "127.0.0.1",
            "-p",
            &s.address.port().to_string(),
            "-U",
            "test",
            "-d",
            "nata",
            "-A",
            "-t",
            "-c",
            &format!("SELECT ti FROM '{}' LIMIT 1", fixture()),
        ])
        .output()
        .expect("psql");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"-128\n");
}

#[test]
fn unsafe_listen_requires_explicit_flag() {
    let mut c = Config {
        listen: SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), 5432),
        unsafe_listen: false,
        max_connections: 64,
        threads: 1,
        metrics: None,
    };
    assert!(c.validate().is_err());
    c.unsafe_listen = true;
    assert!(c.validate().is_ok());
}

#[test]
fn exceptional_date_domains_and_zero_bytes_are_explicit_conversion_errors() {
    let server = Server::start(2);
    let mut client = server.client();
    for (name, column) in [
        (
            "bc",
            Arc::new(Date32Array::from(vec![i32::MIN])) as ArrayRef,
        ),
        (
            "future",
            Arc::new(Date32Array::from(vec![i32::MAX])) as ArrayRef,
        ),
        (
            "zero",
            Arc::new(StringArray::from(vec!["cell\0private"])) as ArrayRef,
        ),
    ] {
        let file =
            std::env::temp_dir().join(format!("nata-pgwire-{name}-{}.parquet", std::process::id()));
        let batch = RecordBatch::try_from_iter([("v", column)]).expect("batch");
        let mut writer = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&file).expect("create"),
            batch.schema(),
            None,
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
        let result = client.query(&format!("SELECT v FROM '{}'", file.display()));
        assert_eq!(tags(&result), b"TEZ");
        assert_eq!(error_field(&result[1].1, b'C'), Some("22000"));
        assert!(!result[1].1.windows(7).any(|w| w == b"private"));
        std::fs::remove_file(file).expect("remove");
    }
}
