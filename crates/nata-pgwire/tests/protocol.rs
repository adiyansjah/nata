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
                let config = Config {listen:address, unsafe_listen:false, max_connections:max, threads:2, max_queries:2, metrics:Some(m)};
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
        assert_eq!(tags, b"RSSSSSSSSKZ");
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
    assert_eq!(c.startup, b"RSSSSSSSSKZ");
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
    assert_eq!(error_field(&e[0].1, b'C'), Some("42703"));
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
        max_queries: 1,
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
        max_queries: 1,
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

impl Client {
    fn frame(&mut self, tag: u8, body: &[u8]) {
        self.socket
            .write_all(&codec::packet(tag, body).expect("packet"))
            .expect("send");
    }
    fn parse(&mut self, q: &str, oids: &[i32]) {
        let mut b = format!("\0{q}\0").into_bytes();
        b.extend((oids.len() as i16).to_be_bytes());
        for o in oids {
            b.extend(o.to_be_bytes());
        }
        self.frame(b'P', &b);
    }
    fn bind(&mut self, params: &[Option<&str>], format: i16, result: i16) {
        let mut b = b"\0\0\0\x01".to_vec();
        b.extend(format.to_be_bytes());
        b.extend((params.len() as i16).to_be_bytes());
        for p in params {
            match p {
                None => b.extend((-1i32).to_be_bytes()),
                Some(v) => {
                    b.extend((v.len() as i32).to_be_bytes());
                    b.extend(v.as_bytes());
                }
            }
        }
        b.extend(1i16.to_be_bytes());
        b.extend(result.to_be_bytes());
        self.frame(b'B', &b);
    }
    fn execute(&mut self, limit: i32) {
        self.frame(b'E', &[vec![0], limit.to_be_bytes().to_vec()].concat());
    }
    fn sync(&mut self) -> Vec<(u8, Vec<u8>)> {
        self.frame(b'S', b"");
        self.result()
    }
}
#[test]
fn extended_recorded_sequences_and_sync_recovery() {
    let s = Server::start(4);
    let mut c = s.client();
    let q = format!("SELECT i, $1 AS p FROM '{}' LIMIT 1", fixture());
    // JDBC shape: Parse Bind Describe(portal) Execute Sync. No duplicate RowDescription at Execute.
    c.parse(&q, &[23]);
    c.bind(&[Some("7")], 0, 0);
    c.frame(b'D', b"P\0");
    c.execute(0);
    let r = c.sync();
    assert_eq!(tags(&r), b"12TDCZ");
    assert_eq!(values(&r)[0][1], Some("7".into()));
    // Psycopg shape: Parse Describe(statement) Flush, Bind Execute Sync, repeated unnamed parse.
    c.parse(&q, &[23]);
    c.frame(b'D', b"S\0");
    c.frame(b'H', b"");
    assert_eq!(read(&mut c.socket).0, b'1');
    let (t, b) = read(&mut c.socket);
    assert_eq!(t, b't');
    assert_eq!(b, b"\0\x01\0\0\0\x17");
    assert_eq!(read(&mut c.socket).0, b'T');
    c.bind(&[None], 0, 0);
    c.execute(0);
    assert_eq!(values(&c.sync())[0][1], None);
    c.frame(b'C', b"P\0");
    c.frame(b'C', b"S\0");
    assert_eq!(tags(&c.sync()), b"33Z");
    c.parse("SET extra_float_digits = 3", &[]);
    c.frame(b'D', b"S\0");
    c.bind(&[], 0, 0);
    c.frame(b'D', b"P\0");
    c.execute(0);
    assert_eq!(tags(&c.sync()), b"1tn2nCZ");
    // Errors at Parse, Bind and Execute each suppress later work, then Sync restores normal operation.
    for stage in 0..3 {
        if stage == 0 {
            c.parse("SELECT @", &[]);
        } else {
            c.parse(&q, &[23]);
            c.bind(&[Some("7")], if stage == 1 { 1 } else { 0 }, 0);
            if stage == 2 {
                c.execute(1);
            }
        }
        c.parse(&q, &[23]);
        c.bind(&[Some("8")], 0, 0);
        c.execute(0);
        let r = c.sync();
        assert_eq!(r.iter().filter(|m| m.0 == b'E').count(), 1);
        assert!(values(&r).is_empty());
        c.parse(&q, &[23]);
        c.bind(&[Some("9")], 0, 0);
        c.frame(b'D', b"P\0");
        c.execute(0);
        assert_eq!(values(&c.sync())[0][1], Some("9".into()));
    }
    // Execution error after a successful Parse/Bind must also recover.
    let bad = format!("SELECT CAST(s AS INTEGER) FROM '{}'", fixture());
    c.parse(&bad, &[]);
    c.bind(&[], 0, 0);
    c.execute(0);
    let r = c.sync();
    assert!(r.iter().any(|m| m.0 == b'E'));
    c.parse(&q, &[23]);
    c.bind(&[Some("10")], 0, 0);
    c.execute(0);
    assert_eq!(values(&c.sync())[0][1], Some("10".into()));
    c.frame(b'P', format!("named\0{q}\0\0\x01\0\0\0\x17").as_bytes());
    let r = c.sync();
    assert_eq!(error_field(&r[0].1, b'C'), Some("0A000"));
    c.parse(&q, &[23]);
    c.bind(&[Some("1")], 0, 1);
    let r = c.sync();
    assert_eq!(error_field(&r[1].1, b'C'), Some("0A000"));
}
#[test]
fn extended_parameters_are_nodes_and_never_error_values() {
    let s = Server::start(2);
    let mut c = s.client();
    let q = format!("SELECT $1 AS p, $2 AS n FROM '{}' LIMIT 1", fixture());
    let attack = "x' FROM 'no-file'; SELECT 'secret-cell";
    c.parse(&q, &[25, 23]);
    c.bind(&[Some(attack), Some("42")], 0, 0);
    c.frame(b'D', b"P\0");
    c.execute(0);
    assert_eq!(
        values(&c.sync()),
        vec![vec![Some(attack.into()), Some("42".into())]]
    );
    c.parse(&q, &[25, 23]);
    c.bind(&[None, Some("1 + 2")], 0, 0);
    let r = c.sync();
    assert_eq!(error_field(&r[1].1, b'C'), Some("22P02"));
    c.parse(&q, &[16, 23]);
    c.bind(&[Some("TRUE"), Some("42")], 0, 0);
    c.execute(0);
    assert_eq!(
        values(&c.sync()),
        vec![vec![Some("t".into()), Some("42".into())]]
    );
    // Unspecified types resolve to text; callers use an explicit cast for numeric contexts.
    c.parse(&q, &[]);
    c.bind(&[Some("hello"), None], 0, 0);
    c.execute(0);
    assert_eq!(values(&c.sync())[0], vec![Some("hello".into()), None]);
    let q = format!("SELECT CAST($1 AS DATE) FROM '{}' LIMIT 1", fixture());
    c.parse(&q, &[25]);
    c.bind(&[Some("secret-cell")], 0, 0);
    c.execute(0);
    let r = c.sync();
    assert_eq!(
        error_field(&r.iter().find(|m| m.0 == b'E').expect("error").1, b'C'),
        Some("22P02")
    );
    assert!(
        !r.iter()
            .any(|(_, b)| b.windows(11).any(|v| v == b"secret-cell"))
    );
}
#[test]
fn session_chatter_status_and_transaction_markers() {
    let s = Server::start(2);
    let mut c = s.client();
    for q in [
        "SET extra_float_digits = 3",
        "SET application_name = 'driver'",
        "SET DateStyle TO 'ISO'",
        "SET client_encoding = 'UTF8'",
        "SET TimeZone = 'UTC'",
        "SET TIME ZONE 'UTC'",
        "SET search_path = public",
        "SET statement_timeout = 0",
        "SET standard_conforming_strings = on",
    ] {
        let r = c.query(q);
        assert!(!r.iter().any(|m| m.0 == b'E'), "{q}: {r:?}");
        assert_eq!(r.iter().find(|m| m.0 == b'C').expect("command").1, b"SET\0");
        let status = r.iter().any(|m| m.0 == b'S');
        assert_eq!(
            status,
            q.contains("application_name")
                || q.contains("DateStyle")
                || q.contains("client_encoding")
                || (q.contains("TimeZone") || q.contains("TIME ZONE"))
                || q.contains("standard_conforming_strings")
        );
    }
    for (q, v) in [
        ("SELECT 1", "1"),
        ("SHOW application_name", "driver"),
        ("SHOW TIME ZONE", "UTC"),
        ("SELECT current_setting('client_encoding')", "UTF8"),
        ("SELECT current_schema()", "public"),
        ("SELECT version()", "PostgreSQL 16.0 (Nata compatibility)"),
    ] {
        assert_eq!(values(&c.query(q)), vec![vec![Some(v.into())]]);
    }
    for q in [
        "SET statement_timeout = 100",
        "SET client_encoding = 'LATIN1'",
        "SET work_mem = '1GB'",
        "SHOW unknown",
        "SET standard_conforming_strings = off",
    ] {
        assert_eq!(error_field(&c.query(q)[0].1, b'C'), Some("0A000"));
    }
    let r = c.query("BEGIN;");
    assert_eq!(r.last().expect("ready").1, b"T");
    let r = c.query("SELECT @");
    assert_eq!(r.last().expect("ready").1, b"E");
    let r = c.query("ROLLBACK;");
    assert_eq!(r.last().expect("ready").1, b"I");
    c.query("BEGIN");
    let r = c.query("COMMIT");
    assert_eq!(r.last().expect("ready").1, b"I");
}
#[test]
fn sqlstate_table_preserves_nata_diagnostics_and_hints() {
    let s = Server::start(2);
    let mut c = s.client();
    let f = fixture();
    for (expr, code) in [
        ("@", "42601"),
        ("nope", "42703"),
        ("mystery(i)", "42883"),
        ("i = 'x'", "42804"),
        ("i / 0", "22012"),
        ("bi + 1", "22003"),
        ("CAST(s AS INTEGER)", "22P02"),
        ("i LIKE 'x'", "0A000"),
    ] {
        let r = c.query(&format!("SELECT {expr} FROM '{f}'"));
        let e = &r.iter().find(|m| m.0 == b'E').expect("error").1;
        assert_eq!(error_field(e, b'C'), Some(code), "{expr}: {e:?}");
        assert!(error_field(e, b'P').is_some());
        assert!(!e.windows(11).any(|v| v == b"secret-cell"));
        if code == "42703" {
            assert_eq!(error_field(e, b'M'), Some("unknown column \"nope\""));
        }
        if code == "22012" {
            assert!(
                error_field(e, b'M')
                    .expect("message")
                    .contains("Guard it with CASE")
            );
        }
    }
    let r = c.query("SELECT x FROM 'missing-step3.parquet'");
    assert_eq!(error_field(&r[0].1, b'C'), Some("42P01"));
    let r = c.query(&format!("SELECT ii FROM '{f}'"));
    assert!(
        error_field(&r[0].1, b'H')
            .expect("hint")
            .contains("did you mean")
    );
}
#[test]
fn stalled_reader_does_not_starve_eight_other_sessions() {
    let s = Server::start(16);
    let mut stalled = s.client();
    stalled.send_query(&format!("SELECT s FROM '{}'", large()));
    assert_eq!(read(&mut stalled.socket).0, b'T');
    wait_until(|| s.metrics.rows_produced.load(Ordering::SeqCst) > 32);
    std::thread::sleep(Duration::from_millis(250));
    // Step 2's one admission cannot finish any of these before the 1.5s deadline.
    let start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let address = s.address;
            let q = format!("SELECT si FROM '{}' LIMIT 1", fixture());
            scope.spawn(move || {
                let sock = TcpStream::connect(address).expect("connect");
                sock.set_read_timeout(Some(Duration::from_millis(1500)))
                    .expect("timeout");
                let mut c = Client::start(sock);
                assert_eq!(values(&c.query(&q)), vec![vec![Some("-32768".into())]]);
            });
        }
    });
    assert!(start.elapsed() < Duration::from_millis(1500));
    // With no cancel/disconnect from the client, the 2s unwritable deadline releases all workers.
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
}

#[test]
fn extended_all_types_match_simple_and_parameter_metadata_is_stable() {
    let s = Server::start(2);
    let mut c = s.client();
    let q = format!("SELECT * FROM '{}'", fixture());
    let simple = c.query(&q);
    c.parse(&q, &[]);
    c.bind(&[], 0, 0);
    c.frame(b'D', b"P\0");
    c.execute(0);
    let extended = c.sync();
    assert_eq!(values(&simple), values(&extended));
    assert_eq!(simple[0], extended[2]);
    // Numeric OID has no precision/scale in Parse. Require an explicit CAST if a bound value changes the result type.
    let q = format!("SELECT $1 AS n FROM '{}' LIMIT 1", fixture());
    c.parse(&q, &[1700]);
    c.bind(&[Some("1.25")], 0, 0);
    c.execute(0);
    let r = c.sync();
    assert_eq!(
        error_field(&r.iter().find(|m| m.0 == b'E').expect("error").1, b'C'),
        Some("0A000")
    );
    let q = format!(
        "SELECT CAST($1 AS DECIMAL(10,2)) AS n FROM '{}' LIMIT $2",
        fixture()
    );
    c.parse(&q, &[1700, 23]);
    c.bind(&[Some("1.25"), Some("1")], 0, 0);
    c.execute(0);
    assert_eq!(values(&c.sync()), vec![vec![Some("1.25".into())]]);
}

#[test]
fn startup_chatter_and_binary_payload_refusals() {
    let s = Server::start(2);
    let mut socket = s.socket();
    let mut b = 196608i32.to_be_bytes().to_vec();
    b.extend(b"user\0test\0database\0nata\0application_name\0startup driver\0TimeZone\0UTC\0\0");
    socket
        .write_all(&codec::packet(0, &b).expect("packet")[1..])
        .expect("startup");
    let mut statuses = Vec::new();
    loop {
        let (t, b) = read(&mut socket);
        assert_ne!(t, b'E');
        if t == b'S' {
            statuses.push(b);
        }
        if t == b'Z' {
            break;
        }
    }
    assert!(statuses.contains(&b"application_name\0startup driver\0".to_vec()));
    assert!(statuses.contains(&b"TimeZone\0UTC\0".to_vec()));
    let mut c = s.client();
    let q = format!("SELECT $1 AS p FROM '{}' LIMIT 1", fixture());
    c.parse(&q, &[23]);
    // Invalid UTF8 binary data is still a supported frame with an unsupported format.
    let b = b"\0\0\0\x01\0\x01\0\x01\0\0\0\x01\xff\0\0";
    c.frame(b'B', b);
    let r = c.sync();
    assert_eq!(error_field(&r[1].1, b'C'), Some("0A000"));
    for (v, code) in [("1.5", "22P02"), ("2147483648", "22003")] {
        c.parse(&q, &[23]);
        c.bind(&[Some(v)], 0, 0);
        let r = c.sync();
        assert_eq!(error_field(&r[1].1, b'C'), Some(code));
    }
    let long = "x".repeat(600_000);
    let q = format!("SELECT $1 AS a, $1 AS b FROM '{}' LIMIT 1", fixture());
    c.parse(&q, &[25]);
    c.bind(&[Some(&long)], 0, 0);
    let r = c.sync();
    assert_eq!(error_field(&r[1].1, b'C'), Some("0A000"));
}
