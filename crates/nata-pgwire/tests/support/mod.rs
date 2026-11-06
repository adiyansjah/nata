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
        Self::start_with(max, Arc::new(Metrics::default()))
    }
    fn start_with(max: usize, metrics: Arc<Metrics>) -> Self {
        let (address_tx, address_rx) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
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
fn assert_code(r: &[(u8, Vec<u8>)], code: &str) {
    assert_eq!(error_field(&r.iter().find(|m| m.0==b'E').expect("error").1,b'C'),Some(code));
}
impl Client {
    fn parse_named(&mut self, name: &str, q: &str, oids: &[i32]) {
        let mut b=format!("{name}\0{q}\0").into_bytes();
        b.extend((oids.len() as i16).to_be_bytes());
        for o in oids { b.extend(o.to_be_bytes()); }
        self.frame(b'P',&b);
    }
    fn bind_named(&mut self, portal: &str, statement: &str) {
        self.frame(b'B', &format!("{portal}\0{statement}\0\0\0\0\0\0\0").into_bytes());
    }
}
