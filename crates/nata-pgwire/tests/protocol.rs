include!("support/mod.rs");
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
            Some("-0".into()),
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
        catalog: Arc::new(nata_pgwire::catalog::Snapshot::default()),
        listen: "127.0.0.1:0".parse().expect("addr"),
        unsafe_listen: false,
        max_connections: 0,
        max_queries: 1,
        threads: 1,
        execution: nata_exec::ExecConfig::new(1),
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
        catalog: Arc::new(nata_pgwire::catalog::Snapshot::default()),
        listen: SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), 5432),
        unsafe_listen: false,
        max_connections: 64,
        max_queries: 1,
        threads: 1,
        execution: nata_exec::ExecConfig::new(1),
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
        assert_eq!(
            error_field(&result[1].1, b'C'),
            Some(if name == "zero" { "22000" } else { "22008" })
        );
        assert!(!result[1].1.windows(7).any(|w| w == b"private"));
        std::fs::remove_file(file).expect("remove");
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
    for stage in 0..2 {
        if stage == 0 {
            c.parse("SELECT @", &[]);
        } else {
            c.parse(&q, &[23]);
            c.bind(&[Some("7")], if stage == 1 { 2 } else { 0 }, 0);
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
    assert_eq!(tags(&r), b"1Z");
    c.parse(&q, &[23]);
    c.bind(&[Some("1")], 0, 2);
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
        "SET statement_timeout = -1",
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
    assert_eq!(error_field(&r[1].1, b'C'), Some("22P03"));
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

#[test]
fn named_lifecycle_duplicates_lookup_sync_and_budgets() {
    let s = Server::start(2);
    let mut c = s.client();
    let q = format!("SELECT si FROM '{}' LIMIT 1", fixture());
    c.parse_named("stmt", &q, &[]);
    assert_eq!(tags(&c.sync()), b"1Z");
    for _ in 0..10 {
        c.bind_named("portal", "stmt");
        c.frame(b'D', b"Pportal\0");
        c.frame(b'E', b"portal\0\0\0\0\0");
        assert_eq!(values(&c.sync()), vec![vec![Some("-32768".into())]]);
    }
    c.frame(b'D', b"Pportal\0");
    assert_code(&c.sync(), "34000");
    c.parse_named("stmt", &q, &[]);
    assert_code(&c.sync(), "42P05");
    // Duplicate failed Parse retains the old named statement.
    c.bind_named("", "stmt");
    c.execute(0);
    assert_eq!(values(&c.sync()), vec![vec![Some("-32768".into())]]);
    c.bind_named("", "absent");
    assert_code(&c.sync(), "26000");
    c.frame(b'D', b"Sabsent\0");
    assert_code(&c.sync(), "26000");
    c.frame(b'E', b"absent\0\0\0\0\0");
    assert_code(&c.sync(), "34000");
    c.query("BEGIN");
    c.bind_named("p", "stmt");
    assert_eq!(tags(&c.sync()), b"2Z");
    c.bind_named("p", "stmt");
    assert_code(&c.sync(), "42P03");
    c.query("ROLLBACK");
    c.frame(b'D', b"Pp\0");
    assert_code(&c.sync(), "34000");
    // Close missing objects is idempotent in PostgreSQL.
    c.frame(b'C', b"Sabsent\0");
    c.frame(b'C', b"Pabsent\0");
    c.frame(b'C', b"Sstmt\0");
    assert_eq!(tags(&c.sync()), b"333Z");
    c.frame(b'D', b"Sstmt\0");
    assert_code(&c.sync(), "26000");
    c.parse(&q, &[]);
    c.parse("SELECT 1", &[]);
    c.bind_named("", "");
    c.execute(0);
    assert_eq!(values(&c.sync()), vec![vec![Some("1".into())]]);
    c.frame(b'C', b"S\0");
    c.sync();
    for n in 0..256 {
        c.parse_named(&format!("s{n}"), "SELECT 1", &[]);
        assert_eq!(tags(&c.sync()), b"1Z");
    }
    c.parse_named("over", "SELECT 1", &[]);
    assert_code(&c.sync(), "54000");
    c.frame(b'C', b"Ss0\0");
    c.sync();
    c.parse_named("over", "SELECT 1", &[]);
    assert_eq!(tags(&c.sync()), b"1Z");
    drop(c);
    wait_until(|| s.metrics.sessions.load(Ordering::SeqCst) == 0 && s.metrics.objects.now() == 0);
    let mut c = s.client();
    let long = format!("{q} /*{}*/", "x".repeat(900_000));
    for n in 0..4 {
        c.parse_named(&format!("s{n}"), &long, &[]);
        assert_eq!(tags(&c.sync()), b"1Z");
    }
    c.parse_named("over", &long, &[]);
    assert_code(&c.sync(), "54000");
    // Portal budget applies even to tiny statements (conservative parameter allowance).
    c.query("BEGIN");
    for n in 0..3 {
        c.bind_named(&format!("p{n}"), "s0");
        assert_eq!(tags(&c.sync()), b"2Z");
    }
    c.bind_named("over", "s0");
    assert_code(&c.sync(), "54000");
}

#[test]
fn binary_default_driver_scalars_and_descriptions() {
    let s = Server::start(2);
    let mut c = s.client();
    let q = format!("SELECT * FROM '{}'", fixture());
    c.parse_named("types", &q, &[]);
    c.frame(b'B', b"p\0types\0\0\0\0\0\0\x01\0\x01");
    c.frame(b'D', b"Pp\0");
    c.frame(b'E', b"p\0\0\0\0\0");
    let r = c.sync();
    assert_eq!(tags(&r), b"12TDDDCZ");
    let desc = &r[2].1;
    let mut at = 2;
    for _ in 0..10 {
        at += desc[at..].iter().position(|b| *b == 0).expect("name") + 1;
        assert_eq!(&desc[at + 16..at + 18], b"\0\x01");
        at += 18;
    }
    // Independent PostgreSQL binary representations of the first fixture row.
    let expected = [
        vec![1],
        b"-128".to_vec(),
        i16::MIN.to_be_bytes().to_vec(),
        i32::MIN.to_be_bytes().to_vec(),
        i64::MIN.to_be_bytes().to_vec(),
        (-0.0f32).to_be_bytes().to_vec(),
        f64::NAN.to_be_bytes().to_vec(),
        vec![0, 2, 0, 0, 0, 0, 0, 3, 0, 123, 17, 148],
        vec![],
        (-10957i32).to_be_bytes().to_vec(),
    ];
    let data = &r[3].1;
    let mut at = 2;
    for value in expected {
        let n = i32::from_be_bytes(data[at..at + 4].try_into().expect("length")) as usize;
        at += 4;
        assert_eq!(&data[at..at + n], value);
        at += n;
    }
    assert_eq!(&r[5].1[2..], &[255u8; 40]);
    let q = format!("SELECT $1 AS p FROM '{}' LIMIT 1", fixture());
    c.parse_named("parameter", &q, &[23]);
    c.sync();
    for _ in 0..10 {
        let mut b = b"\0parameter\0\0\x01\0\x01\0\x01\0\0\0\x04".to_vec();
        b.extend(42i32.to_be_bytes());
        b.extend(b"\0\0");
        c.frame(b'B', &b);
        c.execute(0);
        assert_eq!(values(&c.sync()), vec![vec![Some("42".into())]]);
    }
    c.query("DEALLOCATE ALL");
    c.frame(b'D', b"Sparameter\0");
    assert_code(&c.sync(), "26000");
}

#[test]
fn limited_rowless_commands_allow_jdbc_rollback_recovery() {
    let s = Server::start(2);
    let mut c = s.client();
    c.query("BEGIN");
    let q = format!("SELECT si FROM '{}'", fixture());
    c.parse(&q, &[]);
    c.bind(&[], 0, 0);
    c.execute(100);
    assert_eq!(tags(&c.sync()), b"12DDDCZ");
    assert_code(&c.query("SELECT @"), "42601");
    c.parse("ROLLBACK", &[]);
    c.bind(&[], 0, 0);
    c.execute(1);
    let r = c.sync();
    assert_eq!(tags(&r), b"12CZ");
    assert_eq!(r[2].1, b"ROLLBACK\0");
    assert_eq!(r[3].1, b"I");
    assert_eq!(values(&c.query("SELECT 1")), vec![vec![Some("1".into())]]);
    c.frame(b'E', b"absent\0\0\0\0\x64");
    assert_code(&c.sync(), "34000");
}

#[test]
fn output_buffers_are_batched_byte_bounded_and_nodelay_is_set() {
    let s = Server::start(2);
    let mut c = s.client();
    assert_eq!(s.metrics.nodelay_sessions.load(Ordering::SeqCst), 1);
    let result = c.query(&format!("SELECT id FROM '{}' LIMIT 8192", large()));
    let rows = values(&result);
    assert_eq!(rows.len(), 8192);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row, &[Some(i.to_string())]);
    }
    let buffers = s.metrics.buffers_sent.load(Ordering::SeqCst);
    assert!(
        (2..=16).contains(&buffers),
        "per-row bridge restored: {buffers}"
    );
    let result = c.query(&format!("SELECT s FROM '{}' LIMIT 256", large()));
    assert_eq!(values(&result), vec![vec![Some("x".repeat(1024))]; 256]);
    let path = std::env::temp_dir().join(format!("nata-wide-row-{}.parquet", std::process::id()));
    let text = "T".repeat(200_000);
    let batch = RecordBatch::try_from_iter([(
        "wide",
        Arc::new(StringArray::from(vec![text.clone()])) as ArrayRef,
    )])
    .expect("batch");
    let mut writer = parquet::arrow::ArrowWriter::try_new(
        std::fs::File::create(&path).expect("create"),
        batch.schema(),
        None,
    )
    .expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    let q = format!("SELECT wide FROM '{}'", path.display());
    assert_eq!(values(&c.query(&q)), vec![vec![Some(text.clone())]]);
    c.parse(&q, &[]);
    c.bind(&[], 0, 1);
    c.execute(0);
    assert_eq!(values(&c.sync()), vec![vec![Some(text)]]);
    std::fs::remove_file(path).expect("remove");
    assert!(s.metrics.max_buffer_bytes.load(Ordering::SeqCst) <= 65536);
    assert!(s.metrics.max_buffer_bytes.load(Ordering::SeqCst) >= 32768);
}

#[test]
fn exact_adbc_bootstrap_and_unknown_catalog_refusal() {
    let s = Server::start(2);
    let mut c = s.client();
    let ordinary = c.query(&format!(
        "SELECT 'pg_catalog' AS s FROM '{}' LIMIT 1",
        fixture()
    ));
    assert_eq!(values(&ordinary), vec![vec![Some("pg_catalog".into())]]);
    let attrs =
        "SELECT attrelid, attname, atttypid FROM pg_catalog.pg_attribute ORDER BY attrelid, attnum";
    let types = "SELECT oid, typname, typreceive, typbasetype, typrelid, typarray FROM pg_catalog.pg_type WHERE (typreceive != 0 OR typsend != 0) AND typtype != 'r' AND typreceive::TEXT != 'array_recv'";
    let r = c.query(attrs);
    assert_eq!(tags(&r), b"TCZ");
    let r = c.query(types);
    let rows = values(&r);
    let expected = [
        (16, "bool", "boolrecv"),
        (20, "int8", "int8recv"),
        (21, "int2", "int2recv"),
        (23, "int4", "int4recv"),
        (25, "text", "textrecv"),
        (700, "float4", "float4recv"),
        (701, "float8", "float8recv"),
        (1043, "varchar", "varcharrecv"),
        (1082, "date", "date_recv"),
        (1700, "numeric", "numeric_recv"),
    ];
    assert_eq!(rows.len(), expected.len());
    for (row, (oid, name, recv)) in rows.iter().zip(expected) {
        assert_eq!(
            row,
            &[
                Some(oid.to_string()),
                Some(name.into()),
                Some(recv.into()),
                Some("0".into()),
                Some("0".into()),
                Some("0".into())
            ]
        );
    }
    // The same exact response through Parse/Describe/Bind/binary Execute.
    c.parse(types, &[]);
    c.frame(b'D', b"S\0");
    c.bind(&[], 0, 1);
    c.execute(0);
    let r = c.sync();
    assert_eq!(r.iter().filter(|m| m.0 == b'D').count(), 10);
    for q in [
        "SELECT * FROM pg_catalog.pg_type",
        &format!("{types}; SELECT 1"),
        &types.replace("'r'", "'R'"),
    ] {
        assert_code(&c.query(q), "0A000");
    }
    let r = c.query(&format!(
        "SELECT CAST(0.5 AS DECIMAL(3,3)), CAST(-0.5 AS DECIMAL(3,3)) FROM '{}' LIMIT 1",
        fixture()
    ));
    assert_eq!(
        values(&r),
        vec![vec![Some("0.500".into()), Some("-0.500".into())]]
    );
}

#[test]
fn cancel_inside_a_wide_row_closes_instead_of_appending_an_error() {
    let path =
        std::env::temp_dir().join(format!("nata-wide-cancel-{}.parquet", std::process::id()));
    let text = "T".repeat(4 * 1024 * 1024);
    let batch = RecordBatch::try_from_iter([(
        "wide",
        Arc::new(StringArray::from(vec![text; 8])) as ArrayRef,
    )])
    .expect("batch");
    let props = parquet::file::properties::WriterProperties::builder()
        .set_dictionary_enabled(false)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
        .set_max_row_group_row_count(Some(1))
        .build();
    let mut writer = parquet::arrow::ArrowWriter::try_new(
        std::fs::File::create(&path).expect("file"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    let s = Server::start(2);
    let mut c = s.client();
    c.send_query(&format!("SELECT wide FROM '{}'", path.display()));
    assert_eq!(read(&mut c.socket).0, b'T');
    let mut h = [0; 5];
    c.socket.read_exact(&mut h).expect("row header");
    assert_eq!(h[0], b'D');
    let length = u32::from_be_bytes(h[1..].try_into().expect("length")) as usize;
    assert!(length > 4 * 1024 * 1024);
    std::thread::sleep(Duration::from_millis(100));
    cancel(&s, c.key);
    c.socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("timeout");
    let mut body = Vec::new();
    if let Err(e) = c.socket.read_to_end(&mut body) {
        assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
    }
    assert!(body.len() < length - 4, "unexpected full result");
    assert!(
        !body.windows(5).any(|b| b == b"57014"),
        "ErrorResponse appended inside DataRow"
    );
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    std::fs::remove_file(path).expect("remove");
}

#[test]
fn copy_binary_framing_types_nulls_extended_and_refusals() {
    let s = Server::start(3);
    let mut c = s.client();
    let query = format!("SELECT * FROM '{}'", fixture());
    // Obtain independently asserted binary fields via the ordinary binary path.
    c.parse(&query, &[]);
    c.bind(&[], 0, 1);
    c.execute(0);
    let binary = c.sync();
    let mut expected = b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0".to_vec();
    for (_, row) in binary.iter().filter(|m| m.0 == b'D') {
        expected.extend(row);
    }
    expected.extend((-1i16).to_be_bytes());
    for option in ["(FORMAT binary)", "WITH (format 'binary')"] {
        let q = format!("COPY ({query}) TO STDOUT {option}");
        let simple = c.query(&q);
        assert_eq!(tags(&simple), b"HddddcCZ");
        assert_eq!(simple[0].1, [vec![1, 0, 10], [0, 1].repeat(10)].concat());
        let stream: Vec<_> = simple
            .iter()
            .filter(|m| m.0 == b'd')
            .flat_map(|m| m.1.clone())
            .collect();
        assert_eq!(stream, expected);
        assert_eq!(simple[simple.len() - 2].1, b"COPY 3\0");
        c.parse(&q, &[]);
        c.frame(b'D', b"S\0");
        c.bind(&[], 0, 0);
        c.frame(b'D', b"P\0");
        c.execute(1);
        let extended = c.sync();
        assert_eq!(tags(&extended), b"1tn2nHddddcCZ");
        let stream: Vec<_> = extended
            .iter()
            .filter(|m| m.0 == b'd')
            .flat_map(|m| m.1.clone())
            .collect();
        assert_eq!(stream, expected);
    }
    // Exact canonical negative fraction and NULL representations, not only decode equivalence.
    let q = format!(
        "COPY (SELECT n, dt FROM '{}' ) TO STDOUT (FORMAT binary)",
        fixture()
    );
    let r = c.query(&q);
    let data: Vec<_> = r
        .iter()
        .filter(|m| m.0 == b'd')
        .flat_map(|m| m.1.clone())
        .collect();
    assert!(
        data.windows(10)
            .any(|v| v == [0, 1, 255, 255, 64, 0, 0, 3, 0, 10])
    );
    assert!(
        data.windows(10)
            .any(|v| v == [0, 2, 255, 255, 255, 255, 255, 255, 255, 255])
    );
    for bad in [
        format!("COPY ({query}) TO STDOUT (FORMAT csv)"),
        format!("COPY ({query}) TO STDOUT"),
        format!("COPY ({query}) TO '/file' (FORMAT binary)"),
        "COPY t FROM STDIN (FORMAT binary)".into(),
        format!("COPY ({query}; SELECT 1) TO STDOUT (FORMAT binary)"),
        format!("COPY ({query}) TO STDOUT (FORMAT binary, HEADER true)"),
    ] {
        assert_code(&c.query(&bad), "0A000");
        c.parse(&bad, &[]);
        assert_code(&c.sync(), "0A000");
    }
    let err = c.query(&format!(
        "COPY (SELECT nope FROM '{}') TO STDOUT (FORMAT binary)",
        fixture()
    ));
    assert_code(&err, "42703");
    assert_eq!(error_field(&err[0].1, b'P'), Some("14"));
    let empty = c.query(&format!(
        "COPY (SELECT i FROM '{}' LIMIT 0) TO STDOUT (FORMAT binary)",
        fixture()
    ));
    assert_eq!(tags(&empty), b"HdcCZ");
}

#[test]
fn copy_shares_bounded_backpressure_cancel_disconnect_and_timeout() {
    let s = Server::start(4);
    let mut slow = s.client();
    let mut healthy = s.client();
    slow.send_query(&format!(
        "COPY (SELECT s FROM '{}') TO STDOUT (FORMAT binary)",
        large()
    ));
    assert_eq!(read(&mut slow.socket).0, b'H');
    wait_until(|| s.metrics.rows_produced.load(Ordering::SeqCst) > 32);
    std::thread::sleep(Duration::from_millis(300));
    let first = s.metrics.rows_produced.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(first, s.metrics.rows_produced.load(Ordering::SeqCst));
    assert!(first < 20000);
    assert_eq!(
        values(&healthy.query("SELECT 1")),
        vec![vec![Some("1".into())]]
    );
    cancel(&s, slow.key);
    assert_code(&slow.result(), "57014");
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    slow.send_query(&format!(
        "COPY (SELECT s FROM '{}') TO STDOUT (FORMAT binary)",
        large()
    ));
    assert_eq!(read(&mut slow.socket).0, b'H');
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) > 0);
    drop(slow);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert!(s.metrics.max_buffer_bytes.load(Ordering::SeqCst) <= 65536);
    let mut stalled = s.client();
    stalled.send_query(&format!(
        "COPY (SELECT s FROM '{}') TO STDOUT (FORMAT binary)",
        large()
    ));
    assert_eq!(read(&mut stalled.socket).0, b'H');
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) > 0);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert!(s.metrics.rows_produced.load(Ordering::SeqCst) < 60000);
}

#[test]
fn postgres_float_text_boundaries_and_legacy_setting_refusal() {
    let s = Server::start(2);
    let mut c = s.client();
    for (ty, x, expected) in [
        ("DOUBLE", "1.0", "1"),
        ("DOUBLE", "1e15", "1e+15"),
        ("DOUBLE", "1e23", "9.999999999999999e+22"),
        ("DOUBLE", "1e14", "100000000000000"),
        ("DOUBLE", "1e-4", "0.0001"),
        ("DOUBLE", "1e-5", "1e-05"),
    ] {
        let q = format!("SELECT CAST({x} AS {ty}) FROM '{}' LIMIT 1", fixture());
        assert_eq!(
            values(&c.query(&q)),
            vec![vec![Some(expected.into())]],
            "{q}"
        );
    }
    let path = std::env::temp_dir().join(format!("nata-float-{}.parquet", std::process::id()));
    let batch = RecordBatch::try_from_iter([(
        "v",
        Arc::new(Float32Array::from(vec![
            1e6,
            1e5,
            1e-4,
            1e-5,
            -0.0,
            f32::from_bits(1),
            f32::MAX,
        ])) as ArrayRef,
    )])
    .expect("batch");
    let mut writer = parquet::arrow::ArrowWriter::try_new(
        std::fs::File::create(&path).expect("file"),
        batch.schema(),
        None,
    )
    .expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    assert_eq!(
        values(&c.query(&format!("SELECT v FROM '{}'", path.display()))),
        [
            "1e+06",
            "100000",
            "0.0001",
            "1e-05",
            "-0",
            "1e-45",
            "3.4028235e+38"
        ]
        .map(|s| vec![Some(s.into())])
    );
    std::fs::remove_file(path).expect("remove");
    for n in [-15, -1, 0] {
        assert_code(&c.query(&format!("SET extra_float_digits = {n}")), "0A000");
    }
    for n in [1, 2, 3] {
        assert_eq!(
            tags(&c.query(&format!("SET extra_float_digits = {n}"))),
            b"CZ"
        );
    }
}
#[test]
fn exceptional_dates_text_binary_copy_and_range_errors() {
    let s = Server::start(4);
    let mut c = s.client();
    let dir = std::env::temp_dir().join(format!("nata-date-domain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    for (days, text) in [
        (-735_160, "0044-03-15 BC"),
        (-719_528, "0001-01-01 BC"),
        (2_932_897, "10000-01-01"),
        (-2_440_588, "4714-11-24 BC"),
        (2_145_042_905, "5874897-12-31"),
    ] {
        let path = dir.join("date.parquet");
        let b = RecordBatch::try_from_iter(vec![(
            "d",
            Arc::new(Date32Array::from(vec![Some(days), None])) as ArrayRef,
        )])
        .expect("batch");
        let mut w = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&path).expect("file"),
            b.schema(),
            None,
        )
        .expect("writer");
        w.write(&b).expect("write");
        w.close().expect("close");
        let q = format!("SELECT d FROM '{}'", path.display());
        let m = c.query(&q);
        assert_eq!(values(&m), vec![vec![Some(text.into())], vec![None]]);
        let m = c.query(&format!("COPY ({q}) TO STDOUT (FORMAT binary)"));
        assert!(!m.iter().any(|(t, _)| *t == b'E'));
        let data: Vec<u8> = m
            .iter()
            .filter(|(t, _)| *t == b'd')
            .flat_map(|(_, b)| b.iter().copied())
            .collect();
        let mut want = b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0".to_vec();
        want.extend(1i16.to_be_bytes());
        want.extend(4i32.to_be_bytes());
        want.extend((days - 10_957).to_be_bytes());
        want.extend(1i16.to_be_bytes());
        want.extend((-1i32).to_be_bytes());
        want.extend((-1i16).to_be_bytes());
        assert_eq!(data, want);
    }
    for days in [i32::MIN, -i32::MAX, -2_440_589, 2_145_042_906, i32::MAX] {
        let path = dir.join("bad.parquet");
        let b = RecordBatch::try_from_iter(vec![(
            "d",
            Arc::new(Date32Array::from(vec![days])) as ArrayRef,
        )])
        .expect("batch");
        let mut w = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&path).expect("file"),
            b.schema(),
            None,
        )
        .expect("writer");
        w.write(&b).expect("write");
        w.close().expect("close");
        let q = format!("SELECT d FROM '{}'", path.display());
        for q in [q.clone(), format!("COPY ({q}) TO STDOUT (FORMAT binary)")] {
            let m = c.query(&q);
            let e = m.iter().find(|(t, _)| *t == b'E').expect("error");
            assert_eq!(error_field(&e.1, b'C'), Some("22008"));
            assert!(!m.iter().any(|(t, _)| *t == b'D'));
        }
    }
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn aggregates_named_prepare_text_binary_copy_and_positioned_errors() {
    let s = Server::start(4);
    let mut c = s.client();
    let q = format!(
        "SELECT sum(i), sum(bi), sum(n), avg(i), count(*), avg(n) FROM '{}'",
        fixture()
    );
    let expected = vec![vec![
        Some("-1".into()),
        Some("-1".into()),
        Some("123.449".into()),
        Some("-0.500000".into()),
        Some("3".into()),
        Some("61.725".into()),
    ]];
    let r = c.query(&q);
    assert_eq!(values(&r), expected, "{r:?}");
    let mut desc = &r[0].1[2..];
    for (oid, typmod) in [
        (20, -1),
        (1700, (38 << 16) + 4),
        (1700, ((38 << 16) | 3) + 4),
        (1700, ((16 << 16) | 6) + 4),
        (20, -1),
        (1700, ((38 << 16) | 3) + 4),
    ] {
        let end = desc.iter().position(|b| *b == 0).expect("name");
        desc = &desc[end + 1..];
        assert_eq!(
            i32::from_be_bytes(desc[6..10].try_into().expect("oid")),
            oid
        );
        assert_eq!(
            i32::from_be_bytes(desc[12..16].try_into().expect("typmod")),
            typmod
        );
        desc = &desc[18..];
    }
    c.parse_named("aggregate", &q, &[]);
    assert_eq!(tags(&c.sync()), b"1Z");
    for binary in [false, true, false] {
        let mut body = b"\0aggregate\0\0\0\0\0".to_vec();
        body.extend(1i16.to_be_bytes());
        body.extend(i16::from(binary).to_be_bytes());
        c.frame(b'B', &body);
        c.execute(0);
        let r = c.sync();
        assert_eq!(tags(&r), b"2DCZ");
        if binary {
            let data = &r[1].1;
            assert_eq!(&data[2..6], &8i32.to_be_bytes());
            assert_eq!(&data[6..14], &(-1i64).to_be_bytes());
            // Numeric -1: ndigits=1, weight=0, negative, dscale=0, digit=1.
            assert_eq!(&data[18..28], &[0, 1, 0, 0, 0x40, 0, 0, 0, 0, 1]);
        } else {
            assert_eq!(values(&r), expected, "{r:?}");
        }
    }
    let parameterized = format!(
        "SELECT sum(CAST(i AS BIGINT)+$1),avg($1) FROM '{}' HAVING count(*)>$2",
        fixture()
    );
    c.parse_named("aggregate-parameters", &parameterized, &[23, 23]);
    assert_eq!(tags(&c.sync()), b"1Z");
    let mut body = b"\0aggregate-parameters\0".to_vec();
    body.extend(0i16.to_be_bytes());
    body.extend(2i16.to_be_bytes());
    for v in [b"7", b"2"] {
        body.extend(1i32.to_be_bytes());
        body.extend(v);
    }
    body.extend(0i16.to_be_bytes());
    c.frame(b'B', &body);
    c.execute(0);
    assert_eq!(
        values(&c.sync()),
        vec![vec![Some("13".into()), Some("7.000000".into())]]
    );
    let copy = c.query(&format!("COPY ({q}) TO STDOUT (FORMAT binary)"));
    assert_eq!(tags(&copy), b"HddcCZ");
    assert!(copy[1].1.starts_with(b"PGCOPY\n\xff\r\n\0"));
    let empty = c.query(&format!(
        "SELECT i, count(*) FROM '{}' WHERE false GROUP BY i",
        fixture()
    ));
    assert_eq!(tags(&empty), b"TCZ");
    for select in [
        "count(DISTINCT i)",
        "sum(sum(i))",
        "sum(i) FILTER (WHERE true)",
    ] {
        let r = c.query(&format!("SELECT {select} FROM '{}'", fixture()));
        assert_code(&r, "0A000");
        assert!(error_field(&r[0].1, b'P').is_some());
    }
    let r = c.query(&format!(
        "SELECT sum(CAST(99999999999999999999999999999999999999 AS DECIMAL(38,0))) FROM '{}'",
        fixture()
    ));
    assert_code(&r, "22003");
}

#[test]
fn two_spilling_queries_share_pool_cancel_and_release() {
    let dir = std::env::temp_dir().join(format!("nata-server-spill-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut e = nata_exec::ExecConfig::new(1);
    let pool = 64 << 20;
    e.aggregate_memory_bytes = Some(pool);
    e.aggregate_spill_directory = Some(dir.clone());
    e.aggregate_force_spill = true;
    let s = Server::start_execution(4, Arc::new(Metrics::default()), e);
    let mut a = s.client();
    let mut b = s.client();
    let q = format!("SELECT id,count(*) FROM '{}' GROUP BY id", large());
    a.send_query(&q);
    b.send_query(&q);
    // Description precedes accumulation. Wait for BOTH admitted spill owners.
    assert_eq!(read(&mut a.socket).0, b'T');
    assert_eq!(read(&mut b.socket).0, b'T');
    wait_until(|| {
        std::fs::read_dir(&dir)
            .expect("base")
            .filter_map(Result::ok)
            .filter(|e| {
                e.path().is_dir()
                    && std::fs::read_dir(e.path())
                        .expect("query")
                        .filter_map(Result::ok)
                        .any(|f| {
                            f.path().extension().is_some_and(|s| s == "ipc")
                                && f.metadata().is_ok_and(|m| m.len() > 0)
                        })
            })
            .count()
            == 2
    });
    assert_eq!(s.metrics.admitted.now(), 2);
    let cancelled_at = Instant::now();
    cancel(&s, a.key);
    cancel(&s, b.key);
    assert_code(&a.result(), "57014");
    assert_code(&b.result(), "57014");
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "spilling cancellation must join promptly"
    );
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    let samples = s.metrics.aggregates.lock().expect("samples");
    assert_eq!(samples.len(), 2);
    assert!(
        samples
            .iter()
            .all(|&(budget, peak, held, cancelled)| budget == pool / 2
                && peak <= budget
                && held == 0
                && cancelled)
    );
    assert!(samples.iter().map(|s| s.1).sum::<usize>() <= pool);
    drop(samples);
    let small = format!("SELECT i,count(*),sum(i) FROM '{}' GROUP BY i", fixture());
    a.send_query(&small);
    b.send_query(&small);
    let mut left = values(&a.result());
    let mut right = values(&b.result());
    left.sort();
    right.sort();
    assert_eq!(left, right);
    assert_eq!(left.len(), 3);
    assert_eq!(std::fs::read_dir(&dir).expect("clean").count(), 0);
    assert_eq!(
        values(&a.query(&format!("SELECT count(*) FROM '{}'", fixture()))),
        vec![vec![Some("3".into())]]
    );
    drop(s);
    std::fs::remove_dir(dir).expect("removed");
}

#[test]
fn aggregate_cancel_before_output_and_injected_spill_error_recover() {
    let s = Server::start(4);
    let mut c = s.client();
    c.send_query(&format!(
        "SELECT id,count(*) FROM '{}' GROUP BY id",
        large()
    ));
    assert_eq!(read(&mut c.socket).0, b'T');
    cancel(&s, c.key);
    assert_code(&c.result(), "57014");
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert_eq!(
        values(&c.query(&format!(
            "SELECT count(*),sum(i),avg(i) FROM '{}' WHERE false",
            fixture()
        ))),
        vec![vec![Some("0".into()), None, None]]
    );
    let dir = std::env::temp_dir().join(format!("nata-server-fault-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("base");
    let mut e = nata_exec::ExecConfig::new(1);
    e.aggregate_force_spill = true;
    e.aggregate_spill_fail_after = Some(9);
    e.aggregate_spill_directory = Some(dir.clone());
    let s = Server::start_execution(4, Arc::new(Metrics::default()), e);
    let mut c = s.client();
    let r = c.query(&format!("SELECT sum(i) FROM '{}'", fixture()));
    assert!(r.iter().any(|m| m.0 == b'E'));
    assert!(
        !r.iter().any(|m| m.0 == b'C'),
        "failed spill must not report success"
    );
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert_eq!(std::fs::read_dir(&dir).expect("clean").count(), 0);
    drop(s);
    std::fs::remove_dir(dir).expect("removed");
}

#[test]
fn aggregate_output_backpressure_disconnect_and_cancel_join() {
    let path = std::env::temp_dir().join(format!(
        "nata-aggregate-output-{}.parquet",
        std::process::id()
    ));
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
    let mut writer = parquet::arrow::ArrowWriter::try_new(
        std::fs::File::create(&path).expect("create"),
        batch.schema(),
        None,
    )
    .expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    let s = Server::start(4);
    let mut c = s.client();
    let q = format!("SELECT id,max(s) FROM '{}' GROUP BY id", path.display());
    c.send_query(&q);
    assert_eq!(read(&mut c.socket).0, b'T');
    wait_until(|| s.metrics.rows_produced.load(Ordering::SeqCst) > 100);
    std::thread::sleep(Duration::from_millis(200));
    let before = s.metrics.rows_produced.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.metrics.rows_produced.load(Ordering::SeqCst), before);
    assert!(
        before < 8192,
        "aggregate output must stop at a slow consumer"
    );
    cancel(&s, c.key);
    drop(c);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    let mut c = s.client();
    c.send_query(&q);
    assert_eq!(read(&mut c.socket).0, b'T');
    drop(c);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert!(
        s.metrics
            .aggregates
            .lock()
            .expect("samples")
            .iter()
            .all(|s| s.2 == 0)
    );
    std::fs::remove_file(path).expect("remove");
}

fn fetched(c: &mut Client, limit: i32) -> Vec<(u8, Vec<u8>)> {
    c.execute(limit);
    c.frame(b'H', b"");
    let mut r = Vec::new();
    loop {
        let m = read(&mut c.socket);
        let end = matches!(m.0, b's' | b'C' | b'E');
        r.push(m);
        if end {
            return r;
        }
    }
}
fn park(c: &mut Client) {
    c.query("BEGIN");
    c.parse(&format!("SELECT i FROM '{}'", fixture()), &[]);
    c.bind(&[], 0, 0);
    let r = fetched(c, 1);
    assert_eq!(tags(&r), b"12Ds");
}
#[test]
fn skeleton_fetch_exact_end_cancel_suspended_and_timeout_scope() {
    let s = Server::start(4);
    for n in 0..=3 {
        let mut c = s.client();
        c.query("BEGIN");
        c.parse(&format!("SELECT i FROM '{}' LIMIT {n}", fixture()), &[]);
        c.bind(&[], 0, 0);
        let r = fetched(&mut c, 2);
        if n >= 2 {
            assert_eq!(tags(&r), b"12DDs");
            cancel(&s, c.key); // PostgreSQL rule: suspended means no executing statement.
            let r = fetched(&mut c, 2);
            assert_eq!(
                tags(&r),
                if n == 2 { b"C".as_slice() } else { b"DC" },
                "{r:?}"
            );
            assert_eq!(
                r.last().expect("complete").1,
                if n == 2 {
                    b"SELECT 0\0".as_slice()
                } else {
                    b"SELECT 1\0"
                }
            );
            assert_eq!(fetched(&mut c, 2), vec![(b'C', b"SELECT 0\0".to_vec())]);
        } else {
            assert_eq!(
                r.last().expect("complete").1,
                format!("SELECT {n}\0").as_bytes()
            );
        }
        c.sync();
        c.query("ROLLBACK");
    }
    let mut a = s.client();
    let mut b = s.client();
    park(&mut a);
    park(&mut b); // Holds both admissions, not a separate parked pool.
    let mut c = s.client();
    assert_eq!(tags(&c.query("SET statement_timeout = '20ms'")), b"CZ");
    let start = Instant::now();
    assert_code(&c.query(&format!("SELECT i FROM '{}'", fixture())), "57014");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(values(&c.query("SELECT 1")), vec![vec![Some("1".into())]]);
    drop(b);
    drop(a);
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    c.query("SET statement_timeout = 1000");
    park(&mut c);
    std::thread::sleep(Duration::from_millis(1100));
    cancel(&s, c.key);
    assert_eq!(tags(&fetched(&mut c, 0)), b"DDC"); // Fresh Execute deadline; idle time excluded.
    c.sync();
    c.query("ROLLBACK");
    c.query("BEGIN");
    assert_code(&c.query("SELECT @"), "42601");
    assert_code(&c.query("SELECT 1"), "25P02");
    assert_eq!(tags(&c.query("ROLLBACK")), b"CZ");
    for q in ["DECLARE c CURSOR FOR SELECT 1", "FETCH 1 FROM c", "CLOSE c"] {
        assert_code(&c.query(q), "0A000");
    }
}
#[test]
fn skeleton_portal_expiry_and_close_release_admission() {
    let s = Server::start(4);
    let mut c = s.client();
    park(&mut c);
    c.sync(); // Inside BEGIN retains the portal.
    std::thread::sleep(Duration::from_millis(5200));
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    assert_code(&fetched(&mut c, 1), "34000");
    c.sync();
    c.query("ROLLBACK");
    park(&mut c);
    c.frame(b'C', b"P\0");
    c.sync();
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
}
#[test]
fn skeleton_snapshot_named_scan_and_metadata() {
    let path = std::env::temp_dir().join(format!("nata-registry-{}", std::process::id()));
    std::fs::write(
        &path,
        format!(
            "nata\tpublic\tExample\t{}\nnata\tother\tExample\t{}\n",
            fixture(),
            fixture()
        ),
    )
    .expect("snapshot fixture");
    let snapshot = nata_pgwire::catalog::Snapshot::load(&path).expect("snapshot fixture");
    let a = nata_sql::plan_sql("SELECT * FROM nata.public.\"Example\"", &snapshot)
        .expect("snapshot fixture");
    let b = nata_sql::plan_sql(&format!("SELECT * FROM '{}'", fixture()), &snapshot)
        .expect("snapshot fixture");
    assert_eq!(a.plan, b.plan);
    assert_ne!(snapshot.tables[0].oid, snapshot.tables[1].oid);
    assert_ne!(snapshot.tables[0].schema_oid, snapshot.tables[1].schema_oid);
    assert!(nata_sql::plan_sql("SELECT * FROM nata.public.example", &snapshot).is_err());
    std::fs::remove_file(path).expect("snapshot fixture");
    let s = Server::start(4);
    let mut c = s.client();
    for (q, rows) in [
        ("SELECT table_name FROM information_schema.tables", 0),
        (
            "SELECT catalog_name FROM information_schema.information_schema_catalog_name",
            0,
        ),
    ] {
        let r = c.query(q);
        assert_eq!(values(&r).len(), rows);
        assert!(!tags(&r).contains(&b'E'));
    }
    // Unsupported catalog projections cannot masquerade as empty catalogs.
    for q in [
        "SELECT * FROM information_schema.columns",
        "SELECT * FROM information_schema.tables",
        "SELECT relpartbound FROM pg_catalog.pg_class",
        "SELECT * FROM pg_catalog.pg_policy",
        "SELECT * FROM information_schema.columns WHERE table_name = 'absent'",
    ] {
        let r = c.query(q);
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty());
    }
    let r = c.query("SELECT oid, typname, typreceive, typbasetype, typrelid, typarray FROM pg_catalog.pg_type WHERE (typreceive != 0 OR typsend != 0) AND typtype != 'r' AND typreceive::TEXT != 'array_recv'");
    assert!(
        values(&r)
            .iter()
            .any(|r| r[0].as_deref() == Some("23") && r[1].as_deref() == Some("int4"))
    );
}

#[test]
fn skeleton_all_36_inventory_additions_refuse_before_rows() {
    let s = Server::start(4);
    let mut c = s.client();
    {
        let r = c.query("SELECT array_agg()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "ADBC function:array_agg");
    }
    {
        let r = c.query("SELECT pg_my_temp_schema()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "ADBC function:pg_my_temp_schema");
    }
    {
        let r = c.query("SELECT unnest()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "ADBC function:unnest");
    }
    {
        let r = c.query("SELECT array_agg()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgcli function:array_agg");
    }
    {
        let r = c.query("SELECT generate_subscripts()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgcli function:generate_subscripts");
    }
    {
        let r = c.query("SELECT unnest()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgcli function:unnest");
    }
    {
        let r = c.query("SELECT deptype,objid FROM pg_catalog.pg_depend");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgcli pg_catalog.pg_depend");
    }
    {
        let r = c.query("SELECT proargdefaults FROM pg_catalog.pg_proc");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgcli pg_catalog.pg_proc");
    }
    {
        let r = c.query("SELECT array_upper()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:array_upper");
    }
    {
        let r = c.query("SELECT current_catalog()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:current_catalog");
    }
    {
        let r = c.query("SELECT generate_series()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:generate_series");
    }
    {
        let r = c.query("SELECT information_schema._pg_expandarray()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "pgjdbc function:information_schema._pg_expandarray"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.btrim()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:pg_catalog.btrim");
    }
    {
        let r = c.query("SELECT pg_catalog.substring()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "pgjdbc function:pg_catalog.substring"
        );
    }
    {
        let r = c.query("SELECT pg_get_function_result()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "pgjdbc function:pg_get_function_result"
        );
    }
    {
        let r = c.query("SELECT replace()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:replace");
    }
    {
        let r = c.query("SELECT row_number()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc function:row_number");
    }
    {
        let r = c.query("SELECT indisclustered,indoption,indpred FROM pg_catalog.pg_index");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "pgjdbc pg_catalog.pg_index");
    }
    {
        let r = c.query("SELECT pg_catalog.array_to_string()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.array_to_string"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.array_upper()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.array_upper"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.generate_series()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.generate_series"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.pg_get_function_arguments()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.pg_get_function_arguments"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.pg_get_function_result()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.pg_get_function_result"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.pg_get_statisticsobjdef_columns()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.pg_get_statisticsobjdef_columns"
        );
    }
    {
        let r = c.query("SELECT pg_catalog.pg_relation_is_publishable()");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql function:pg_catalog.pg_relation_is_publishable"
        );
    }
    {
        let r = c.query("SELECT string_agg()");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql function:string_agg");
    }
    {
        let r = c.query("SELECT relpartbound FROM pg_catalog.pg_class");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_class");
    }
    {
        let r = c.query("SELECT datacl,daticulocale,datlocprovider FROM pg_catalog.pg_database");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_database");
    }
    {
        let r = c.query("SELECT inhdetachpending FROM pg_catalog.pg_inherits");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_inherits");
    }
    {
        let r = c.query("SELECT polcmd,polname,polpermissive,polqual,polrelid,polroles,polwithcheck FROM pg_catalog.pg_policy");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_policy");
    }
    {
        let r = c.query("SELECT oid,puballtables,pubname FROM pg_catalog.pg_publication");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_publication");
    }
    {
        let r = c.query("SELECT pnnspid,pnpubid FROM pg_catalog.pg_publication_namespace");
        assert_code(&r, "0A000");
        assert!(
            values(&r).is_empty(),
            "psql pg_catalog.pg_publication_namespace"
        );
    }
    {
        let r = c.query("SELECT prattrs,prpubid,prqual,prrelid FROM pg_catalog.pg_publication_rel");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_publication_rel");
    }
    {
        let r = c.query("SELECT oid,stxkind,stxname,stxnamespace,stxrelid,stxstattarget FROM pg_catalog.pg_statistic_ext");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psql pg_catalog.pg_statistic_ext");
    }
    {
        let r = c.query("SELECT character_set_catalog,character_set_name,character_set_schema,collation_catalog,collation_name,collation_schema,dtd_identifier,identity_cycle,identity_generation,identity_increment,identity_maximum,identity_minimum,identity_start,interval_precision,interval_type,is_self_referencing,maximum_cardinality,scope_catalog,scope_name,scope_schema FROM information_schema.columns");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psycopg information_schema.columns");
    }
    {
        let r = c.query("SELECT commit_action,is_typed,reference_generation,self_referencing_column_name,user_defined_type_catalog,user_defined_type_name,user_defined_type_schema FROM information_schema.tables");
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty(), "psycopg information_schema.tables");
    }
}

#[test]
fn bounded_explain_refuses_instead_of_ignoring_the_row_limit() {
    let s = Server::start(4);
    let mut c = s.client();
    for q in [
        format!("EXPLAIN SELECT i FROM '{}'", fixture()),
        format!("EXPLAIN ANALYZE SELECT i FROM '{}'", fixture()),
    ] {
        c.query("BEGIN");
        c.parse(&q, &[]);
        c.bind(&[], 0, 0);
        c.execute(1);
        let r = c.sync();
        assert_code(&r, "0A000");
        assert!(values(&r).is_empty());
        c.query("ROLLBACK");
    }
}

#[test]
fn bounded_aggregate_keeps_admitted_spill_configuration() {
    let dir = std::env::temp_dir().join(format!("nata-portal-spill-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("base");
    let pool = 64 << 20;
    let mut execution = nata_exec::ExecConfig::new(1);
    execution.aggregate_memory_bytes = Some(pool);
    execution.aggregate_force_spill = true;
    execution.aggregate_spill_directory = Some(dir.clone());
    let s = Server::start_execution(4, Arc::new(Metrics::default()), execution);
    let mut c = s.client();
    c.query("BEGIN");
    c.parse(
        &format!("SELECT i,count(*) FROM '{}' GROUP BY i", fixture()),
        &[],
    );
    c.bind(&[], 0, 0);
    let first = fetched(&mut c, 1);
    assert_eq!(tags(&first), b"12Ds");
    cancel(&s, c.key);
    let rest = fetched(&mut c, 0);
    assert_eq!(tags(&rest), b"DDC");
    assert_eq!(values(&first).len() + values(&rest).len(), 3);
    c.sync();
    c.query("ROLLBACK");
    wait_until(|| s.metrics.running.load(Ordering::SeqCst) == 0);
    let samples = s.metrics.aggregates.lock().expect("samples");
    assert_eq!(samples.len(), 1);
    assert!(samples.iter().all(|&(budget, peak, held, cancelled)| {
        budget == pool / 2 && peak > 0 && peak <= budget && held == 0 && !cancelled
    }));
    assert_eq!(std::fs::read_dir(&dir).expect("clean").count(), 0);
    drop(samples);
    drop(s);
    std::fs::remove_dir(dir).expect("removed");
}
