#![forbid(unsafe_code)]
mod codec;
use futures::{SinkExt, stream};
use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use pgwire::{
    api::Type,
    api::results::{DataRowEncoder, FieldFormat, FieldInfo, QueryResponse},
    error::{PgWireError, PgWireResult},
    tokio::server::PgWireMessageServerCodec,
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
type Rows = Vec<Option<String>>;
type Reg = Arc<Mutex<HashMap<(i32, i32), ScanCancel>>>;
static PRODUCED: AtomicUsize = AtomicUsize::new(0);
type Fields = Arc<Vec<FieldInfo>>;
fn pgtype(t: nata_sql::types::SqlType) -> Type {
    use nata_sql::types::SqlType::*;
    match t {
        Boolean => Type::BOOL,
        TinyInt | Varchar | Null => Type::TEXT,
        SmallInt => Type::INT2,
        Integer => Type::INT4,
        BigInt => Type::INT8,
        Float => Type::FLOAT4,
        Double => Type::FLOAT8,
        Decimal { .. } => Type::NUMERIC,
        Date => Type::DATE,
    }
}
fn fields(p: &nata_sql::Planned) -> Fields {
    Arc::new(
        p.output
            .iter()
            .map(|c| FieldInfo::new(c.name.clone(), None, None, pgtype(c.ty), FieldFormat::Text))
            .collect(),
    )
}
async fn schema(q: String) -> std::io::Result<Fields> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(
            nata_sql::plan_sql(&q, &nata_sql::FsSource)
                .map(|p| fields(&p))
                .map_err(ioerr),
        );
    });
    rx.await.map_err(ioerr)?
}
fn packet(t: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![t];
    b.extend(((body.len() + 4) as i32).to_be_bytes());
    b.extend(body);
    b
}
async fn send(s: &mut TcpStream, t: u8, b: &[u8]) -> std::io::Result<()> {
    s.write_all(&packet(t, b)).await
}
async fn error(s: &mut TcpStream, code: &str) -> std::io::Result<()> {
    send(
        s,
        b'E',
        format!("SERROR\0C{code}\0Mspike query failed\0\0").as_bytes(),
    )
    .await
}
async fn ready(s: &mut TcpStream) -> std::io::Result<()> {
    send(s, b'Z', b"I").await
}
async fn description(s: &mut TcpStream, fields: &[FieldInfo]) -> std::io::Result<()> {
    let mut b = (fields.len() as i16).to_be_bytes().to_vec();
    for f in fields {
        b.extend(f.name().as_bytes());
        b.push(0);
        b.extend(0i32.to_be_bytes());
        b.extend(0i16.to_be_bytes());
        b.extend(f.datatype().oid().to_be_bytes());
        b.extend((-1i16).to_be_bytes());
        b.extend((-1i32).to_be_bytes());
        b.extend(0i16.to_be_bytes());
    }
    send(s, b'T', &b).await
}
fn ioerr(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}
fn query(q: String, cancel: ScanCancel, unbounded: bool) -> mpsc::Receiver<Result<Rows, String>> {
    let (tx, rx) = mpsc::channel(32);
    std::thread::Builder::new()
        .name("nata-query".into())
        .stack_size(16 << 20)
        .spawn(move || {
            let run = || -> Result<(), String> {
                let p = nata_sql::plan_sql_with(
                    &q,
                    &nata_sql::FsSource,
                    nata_sql::BindOptions::default(),
                )
                .map_err(|e| e.to_string())?;
                let mut cfg = nata_exec::ExecConfig::new(2);
                cfg.window = 4;
                let mut collected = Vec::new();
                nata_exec::execute(Arc::new(p.plan), &cfg, Some(&cancel), |batch| {
                    for i in 0..batch.num_rows() {
                        let row = batch
                            .columns()
                            .iter()
                            .enumerate()
                            .map(|(col, a)| {
                                let v = nata_parquet::csv::cell_text(a.as_ref(), i)?;
                                Ok(v.map(|v| {
                                    if p.output[col].ty == nata_sql::types::SqlType::Boolean {
                                        if v == "true" { "t".into() } else { "f".into() }
                                    } else {
                                        v
                                    }
                                }))
                            })
                            .collect::<Result<Rows, ScanError>>()?;
                        PRODUCED.fetch_add(1, Ordering::Relaxed);
                        if unbounded {
                            collected.push(row)
                        } else {
                            tx.blocking_send(Ok(row)).map_err(|_| {
                                ScanError::new(ErrorKind::Cancelled("closed".into()))
                            })?;
                        }
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                for r in collected {
                    if tx.blocking_send(Ok(r)).is_err() {
                        break;
                    }
                }
                Ok(())
            };
            if let Err(e) = run() {
                let _ = tx.blocking_send(Err(e));
            }
            eprintln!("query_end produced={}", PRODUCED.load(Ordering::Relaxed));
        })
        .unwrap();
    rx
}
async fn result(
    s: &mut TcpStream,
    q: String,
    pg: bool,
    desc: bool,
    cancel: ScanCancel,
    unbounded: bool,
) -> std::io::Result<()> {
    PRODUCED.store(0, Ordering::Relaxed);
    let schema = schema(q.clone()).await?;
    let mut rx = query(q, cancel, unbounded);
    if pg {
        // Use the crate's actual Stream response and Framed sink backpressure.
        let schema2 = schema.clone();
        let rows = stream::unfold(rx, move |mut rx| {
            let schema = schema2.clone();
            async move {
                rx.recv().await.map(|r| {
                    let row: PgWireResult<_> = r
                        .map_err(|_| PgWireError::IoError(ioerr("query")))
                        .and_then(|r| {
                            let mut e = DataRowEncoder::new(schema);
                            for v in r {
                                e.encode_field(&v)?;
                            }
                            Ok(e.take_row())
                        });
                    (row, rx)
                })
            }
        });
        let client =
            pgwire::api::DefaultClient::<String>::new("127.0.0.1:1".parse().unwrap(), false);
        let mut framed = tokio_util::codec::Framed::new(s, PgWireMessageServerCodec::new(client));
        pgwire::api::query::send_query_response(
            &mut framed,
            QueryResponse::new(schema, rows),
            desc,
        )
        .await
        .map_err(ioerr)?;
        framed.flush().await?;
    } else {
        if desc {
            description(s, &schema).await?;
        }
        let mut n = 0;
        while let Some(r) = rx.recv().await {
            let r = match r {
                Ok(r) => r,
                Err(_) => {
                    error(s, "57014").await?;
                    return Ok(());
                }
            };
            let mut b = Vec::new();
            b.extend((r.len() as i16).to_be_bytes());
            for v in r {
                if let Some(v) = v {
                    b.extend((v.len() as i32).to_be_bytes());
                    b.extend(v.as_bytes());
                } else {
                    b.extend((-1i32).to_be_bytes());
                }
            }
            s.write_all(&packet(b'D', &b)).await?;
            n += 1;
        }
        send(s, b'C', format!("SELECT {n}\0").as_bytes()).await?;
    }
    Ok(())
}
async fn session(
    mut s: TcpStream,
    pg: bool,
    id: i32,
    reg: Reg,
    unbounded: bool,
) -> std::io::Result<()> {
    let key = id.wrapping_mul(7919);
    let mut startup = true;
    let mut parsed = String::new();
    loop {
        let tag = if startup {
            None
        } else {
            Some(s.read_u8().await?)
        };
        let len = s.read_i32().await?;
        if len < 4 || len as usize > codec::MAX {
            return Err(ioerr("length"));
        }
        let mut b = Vec::with_capacity(len as usize + 1);
        if let Some(t) = tag {
            b.push(t)
        }
        b.extend(len.to_be_bytes());
        let old = b.len();
        b.resize(old + len as usize - 4, 0);
        s.read_exact(&mut b[old..]).await?;
        let msg = (if pg {
            codec::pg(&b, startup)
        } else {
            codec::hand(&b, startup)
        })
        .map_err(ioerr)?;
        match msg {
            codec::Msg::Ssl => {
                s.write_all(b"N").await?;
            }
            codec::Msg::Cancel(p, k) => {
                if let Some(c) = reg.lock().unwrap().get(&(p, k)) {
                    c.cancel()
                }
                return Ok(());
            }
            codec::Msg::Startup => {
                startup = false;
                send(&mut s, b'R', &0i32.to_be_bytes()).await?;
                for (k, v) in [
                    ("server_version", "16.0"),
                    ("client_encoding", "UTF8"),
                    ("DateStyle", "ISO"),
                    ("integer_datetimes", "on"),
                    ("standard_conforming_strings", "on"),
                ] {
                    send(&mut s, b'S', format!("{k}\0{v}\0").as_bytes()).await?;
                }
                let mut b = id.to_be_bytes().to_vec();
                b.extend(key.to_be_bytes());
                send(&mut s, b'K', &b).await?;
                ready(&mut s).await?;
            }
            codec::Msg::Query(q) => {
                if q.starts_with("SET ") {
                    send(&mut s, b'C', b"SET\0").await?;
                } else {
                    let c = ScanCancel::new();
                    reg.lock().unwrap().insert((id, key), c.clone());
                    let r = result(&mut s, q, pg, true, c.clone(), unbounded).await;
                    c.cancel();
                    reg.lock().unwrap().remove(&(id, key));
                    if r.is_err() {
                        error(&mut s, "57014").await?;
                    }
                }
                ready(&mut s).await?;
            }
            codec::Msg::Parse(q) => {
                parsed = q;
                send(&mut s, b'1', b"").await?;
            }
            codec::Msg::Bind => send(&mut s, b'2', b"").await?,
            codec::Msg::Describe(t) => {
                if t == b'S' {
                    send(&mut s, b't', &0i16.to_be_bytes()).await?;
                }
                description(&mut s, &schema(parsed.clone()).await?).await?;
            }
            codec::Msg::Execute => {
                if parsed.starts_with("SET ") {
                    send(&mut s, b'C', b"SET\0").await?;
                } else {
                    let c = ScanCancel::new();
                    reg.lock().unwrap().insert((id, key), c.clone());
                    let r = result(&mut s, parsed.clone(), pg, false, c.clone(), unbounded).await;
                    c.cancel();
                    reg.lock().unwrap().remove(&(id, key));
                    if r.is_err() {
                        error(&mut s, "57014").await?;
                    }
                }
            }
            codec::Msg::Sync => ready(&mut s).await?,
            codec::Msg::Close => send(&mut s, b'3', b"").await?,
            codec::Msg::Flush => {}
            codec::Msg::End => return Ok(()),
        }
    }
}
fn fuzz() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let seed = 0x4b_20261107u64;
    println!("seed={seed}");
    std::panic::set_hook(Box::new(|_| {}));
    for startup in [false, true] {
        for pg in [false, true] {
            let mut state = seed;
            let mut panics = 0;
            let mut rejects = 0;
            let mut accepted = 0;
            for i in 0..10000 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let tag = b"QPBDECSHX"[(state as usize) % 9];
                let n = (state as usize >> 8) % 128;
                let mut b = vec![tag];
                let len = match i % 5 {
                    0 => 4,
                    1 => 2,
                    2 => i32::MAX,
                    3 => (n + 4) as i32,
                    _ => (n + 5) as i32,
                };
                b.extend(len.to_be_bytes());
                for j in 0..n {
                    b.push((state >> (j % 8 * 8)) as u8)
                }
                if i % 5 == 0 {
                    b.truncate(5)
                }
                if startup {
                    b.remove(0);
                    if b.len() >= 8 {
                        b[4..8].copy_from_slice(
                            &(if i % 2 == 0 { 196608i32 } else { 80877102i32 }).to_be_bytes(),
                        );
                    }
                }
                let r = catch_unwind(AssertUnwindSafe(|| {
                    if pg {
                        codec::pg(&b, startup)
                    } else {
                        codec::hand(&b, startup)
                    }
                }));
                match r {
                    Err(_) => panics += 1,
                    Ok(Err(_)) => rejects += 1,
                    Ok(Ok(_)) => accepted += 1,
                }
            }
            println!(
                "startup={startup} pg={pg} cases=10000 panic={panics} rejected={rejects} accepted={accepted}"
            );
            if !pg {
                assert_eq!(panics, 0);
            }
        }
    }
    let bad = b"P\0\0\0\x04";
    println!(
        "minimal_parse pg_panic={} hand_reject={}",
        catch_unwind(|| codec::pg(bad, false)).is_err(),
        codec::hand(bad, false).is_err()
    );
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args[1] == "fuzz" {
        fuzz();
        return;
    }
    let _rogue = if args.iter().any(|s| s == "pool-mutant") {
        Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("rogue-pool")
                .build()
                .unwrap(),
        )
    } else {
        None
    };
    let pg = args[1] == "pg";
    let unbounded = args.iter().any(|s| s == "unbounded");
    let listener = TcpListener::bind(format!("127.0.0.1:{}", args[2]))
        .await
        .unwrap();
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            eprintln!("progress produced={}", PRODUCED.load(Ordering::Relaxed));
        }
    });
    let reg: Reg = Arc::default();
    eprintln!("ready pid={} pg={pg}", std::process::id());
    let mut id = 100;
    loop {
        let (s, _) = listener.accept().await.unwrap();
        id += 1;
        let reg = reg.clone();
        tokio::spawn(async move {
            if let Err(e) = session(s, pg, id, reg, unbounded).await {
                eprintln!("session_end {e}");
            }
        });
    }
}
