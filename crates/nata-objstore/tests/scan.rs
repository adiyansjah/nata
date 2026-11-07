//! End to end through the real engine: SQL over `s3://` locations returns the rows of the identical local files, and the failure modes of the scan (a
//! changed object, a stalled store, a LIMIT, a cancel) end as the card says. The store is the fake endpoint; the engine is `nata-sql` and `nata-exec`.

#[path = "../cases/mod.rs"]
mod cases;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cases::scan_support::ScanRig as Rig;
use cases::testdata::lineitem_like;
use nata_objfake::{Fault, Plan};
use nata_objstore::Config;
use nata_parquet::{ErrorKind, ScanCancel};

fn lineitems(rig: &Rig, n: usize) {
    for i in 0..n {
        rig.put(
            &format!("li/part-{i}.parquet"),
            lineitem_like(12_000, 2_000, 100 + i as u64),
        );
    }
}

#[test]
fn object_rows_equal_the_rows_of_the_identical_local_files() {
    let rig = Rig::new("equal");
    lineitems(&rig, 3);
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    let queries = [
        "SELECT * FROM '{d}/one.parquet'",
        "SELECT l_orderkey, l_shipdate, l_comment FROM '{d}/li/part-1.parquet' WHERE l_orderkey > 30 AND l_orderkey < 60",
        "SELECT l_returnflag, l_discount FROM '{d}/li/*.parquet' WHERE l_discount > 50000",
        "SELECT l_orderkey FROM '{d}/li/part-0.parquet' LIMIT 10 OFFSET 5",
        "SELECT l_orderkey FROM '{d}/li/**/part-2.parquet'",
        "SELECT count(*), sum(l_quantity), min(l_shipdate) FROM '{d}/li/*.parquet'",
        "SELECT l_returnflag, count(*) FROM '{d}/li/*.parquet' GROUP BY l_returnflag",
    ];
    // a grouped result has no promised order: compare the lines as a set
    let sorted = |s: String| {
        let mut l: Vec<String> = s.lines().map(str::to_string).collect();
        l.sort();
        l
    };
    for q in queries {
        let want = rig.local(q);
        let got = rig.remote(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        assert_eq!(sorted(got), sorted(want.clone()), "{q}");
        assert!(!want.is_empty());
    }
    assert!(rig.fake.gets() > 0 && rig.fake.heads() > 0);
    // read-only: the library's write-path body readers (delete, multipart) are unreachable because nothing but GET and HEAD is ever sent
    assert!(
        rig.fake
            .log()
            .iter()
            .all(|r| r.method == "GET" || r.method == "HEAD"),
        "a method other than GET and HEAD reached the store"
    );
}

#[test]
fn rows_are_the_same_with_the_cache_off_and_with_retries_happening() {
    let rig = Rig::with("nocache", Config::fast_tests(), false);
    lineitems(&rig, 2);
    let q = "SELECT l_orderkey, l_extendedprice FROM '{d}/li/*.parquet' WHERE l_orderkey > 100";
    let want = rig.local(q);
    assert_eq!(rig.remote(q).expect("no cache"), want);
    // every third GET fails transiently: the retries are invisible in the rows
    rig.fake.set_plan(Plan {
        seeded: Some((
            7,
            300,
            vec![
                Fault::Status {
                    code: 503,
                    retry_after: None,
                    body: 10,
                    endless: false,
                },
                Fault::Reset,
                Fault::TruncateAfter { after: 700 },
            ],
        )),
        ..Default::default()
    });
    assert_eq!(rig.remote(q).expect("with retries"), want);
}

#[test]
fn a_glob_lists_pages_and_sorts_in_byte_order() {
    let rig = Rig::new("glob");
    for name in [
        "b/2.parquet",
        "a/10.parquet",
        "a/9.parquet",
        "a/x.txt",
        "a/sub/1.parquet",
        "c.parquet",
    ] {
        rig.put(name, lineitem_like(5, 5, 3));
    }
    rig.fake.set_list_page(2);
    let found = rig
        .io
        .expand("s3://bkt/**/*.parquet", &|| false)
        .expect("expand");
    assert_eq!(
        found,
        [
            "s3://bkt/a/10.parquet",
            "s3://bkt/a/9.parquet",
            "s3://bkt/a/sub/1.parquet",
            "s3://bkt/b/2.parquet",
            "s3://bkt/c.parquet"
        ],
        "all pages, byte order"
    );
    let direct = rig
        .io
        .expand("s3://bkt/a/*.parquet", &|| false)
        .expect("expand");
    assert_eq!(direct, ["s3://bkt/a/10.parquet", "s3://bkt/a/9.parquet"]);
    assert_eq!(
        rig.io
            .expand("s3://bkt/c.parquet", &|| false)
            .expect("plain"),
        ["s3://bkt/c.parquet"],
        "a location without a glob is returned as it is"
    );
    assert!(
        rig.sql("SELECT l_orderkey FROM 's3://bkt/none/*.parquet'", None)
            .is_err(),
        "no file matches"
    );
}

#[test]
fn an_object_replaced_during_the_scan_fails_and_never_mixes_versions() {
    let rig = Rig::new("race");
    lineitems(&rig, 3);
    let swapped = lineitem_like(12_000, 2_000, 999);
    let q = "SELECT sum(l_partkey), sum(l_suppkey) FROM 's3://bkt/li/*.parquet'";
    let want = rig.local(&q.replace("s3://bkt", "{d}"));
    let mut outcomes = Vec::new();
    for swap_at in [1usize, 2, 3, 5] {
        rig.fake.put(
            "li/part-1.parquet",
            std::fs::read(rig.dir.join("li/part-1.parquet")).expect("original"),
        );
        rig.fake.set_plan(Plan {
            swap_before_get: vec![(
                swap_at,
                "li/part-1.parquet".into(),
                Arc::new(swapped.clone()),
            )],
            ..Default::default()
        });
        match rig.remote(&q.replace("s3://bkt", "{d}")) {
            Err(e) => {
                assert!(
                    matches!(e.kind, ErrorKind::Io(_)) && e.to_string().contains("changed"),
                    "swap@{swap_at}: {e}"
                );
                outcomes.push(format!("swap@{swap_at}: failed, object changed"));
            }
            Ok(text) => {
                assert_eq!(
                    text, want,
                    "swap@{swap_at}: a success must be version A's answer, never rows of two versions"
                );
                outcomes.push(format!(
                    "swap@{swap_at}: succeeded with version A (the swap came after its last read)"
                ));
            }
        }
    }
    assert!(
        outcomes.iter().any(|o| o.contains("failed")),
        "{outcomes:?}"
    );
}

#[test]
fn a_limit_does_not_wait_for_stalled_files_behind_it() {
    let mut cfg = Config::fast_tests();
    cfg.attempt_timeout = Duration::from_secs(20);
    cfg.op_deadline = Duration::from_secs(60);
    let rig = Rig::with("limit", cfg, true);
    lineitems(&rig, 4);
    // file 0 is served; every other file stalls (headers never come): the LIMIT is satisfied by file 0 alone
    rig.fake.set_plan(Plan {
        by_key: (1..4)
            .map(|i| (format!("li/part-{i}.parquet"), Fault::StallHeaders))
            .collect(),
        ..Default::default()
    });
    let t = Instant::now();
    let text = rig
        .sql(
            "SELECT l_orderkey FROM 's3://bkt/li/part-0.parquet' LIMIT 3",
            None,
        )
        .expect("limit on one file");
    assert_eq!(text.lines().count(), 4, "{text}");
    // the glob binds from file 0's footer alone (the other footers are read when the scan reaches them) and the scan opens files 1..3 AHEAD: those
    // opens stall, the limit is met by file 0, and the scan must not wait for them
    let glob = Instant::now();
    let text = rig
        .sql(
            "SELECT l_orderkey FROM 's3://bkt/li/*.parquet' LIMIT 3",
            None,
        )
        .expect("limit over a glob");
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(
        t.elapsed() < Duration::from_secs(10) && glob.elapsed() < Duration::from_secs(5),
        "the LIMIT waited for stalled reads: {:?} / {:?}",
        t.elapsed(),
        glob.elapsed()
    );
}

#[test]
fn cancel_stops_a_scan_blocked_in_a_stalled_store_within_a_second() {
    let mut cfg = Config::fast_tests();
    cfg.attempt_timeout = Duration::from_secs(30);
    cfg.op_deadline = Duration::from_secs(60);
    let rig = Rig::with("cancel", cfg, true);
    lineitems(&rig, 2);
    // the bind (the LIST, file 0's footer tail and its magic) is served; everything the scan itself asks for stalls in the body
    let mut script: Vec<Option<Fault>> = vec![None, None, None];
    script.extend((0..64).map(|_| Some(Fault::StallBody { after: 100 })));
    rig.fake.set_plan(Plan::script(script));
    let cancel = ScanCancel::new();
    let stopper = {
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            c.cancel();
        })
    };
    let t0 = Instant::now();
    let r = rig.sql(
        "SELECT sum(l_partkey) FROM 's3://bkt/li/*.parquet'",
        Some(&cancel),
    );
    let took = t0.elapsed();
    stopper.join().expect("stopper");
    let e = r.expect_err("cancelled");
    assert!(matches!(e.kind, ErrorKind::Cancelled(_)), "{e}");
    assert!(
        took < Duration::from_millis(600 + 1000),
        "quiescence after the cancel took {:?}",
        took - Duration::from_millis(600)
    );
}

#[test]
fn a_warm_cache_reads_no_footer_again_and_counts_identity_lookups_apart() {
    let rig = Rig::new("warm");
    lineitems(&rig, 3);
    let q = "SELECT count(*) FROM 's3://bkt/li/*.parquet'";
    rig.sql(q, None).expect("cold");
    let cold = rig.fake.log();
    rig.fake.clear_log();
    rig.sql(q, None).expect("warm");
    let warm = rig.fake.log();
    let footer_gets = |l: &[nata_objfake::ReqLog]| {
        l.iter()
            .filter(|r| {
                r.method == "GET" && r.range.is_some_and(|(a, _)| a > 0) && r.key.starts_with("li/")
            })
            .count()
    };
    let heads = |l: &[nata_objfake::ReqLog]| l.iter().filter(|r| r.method == "HEAD").count();
    assert!(
        heads(&cold) >= 3 && heads(&warm) >= 3,
        "an identity lookup per open, cold and warm: {} / {}",
        heads(&cold),
        heads(&warm)
    );
    assert!(
        footer_gets(&warm) < footer_gets(&cold),
        "warm {} cold {}",
        footer_gets(&warm),
        footer_gets(&cold)
    );
}

#[test]
fn errors_name_no_endpoint_credential_or_body() {
    let rig = Rig::new("errors");
    lineitems(&rig, 1);
    rig.fake.set_plan(Plan::always(Fault::Status {
        code: 403,
        retry_after: None,
        body: 40,
        endless: false,
    }));
    let e = rig
        .sql("SELECT l_orderkey FROM 's3://bkt/li/part-0.parquet'", None)
        .expect_err("403");
    let text = format!("{e} {e:?}");
    assert!(
        !text.contains("127.0.0.1") && !text.contains("xxxx"),
        "{text}"
    );
    let e = rig
        .sql("SELECT l_orderkey FROM 's3://bkt/li/missing.parquet'", None)
        .expect_err("404");
    assert!(e.to_string().contains("does not exist"), "{e}");
}

/// Card 0006 carried-in R6c/R6d: the damaged files the local scan once hung on, read from a store, end as errors in bounded time (the `at_eof` guard holds
/// on the byte-backed reader as it does on a file).
#[test]
fn a_damaged_fixture_read_from_the_store_is_an_error_not_a_hang() {
    let rig = Rig::new("damaged");
    let bytes =
        include_bytes!("../../nata-parquet/tests/fixtures/thrift_skip_past_eof.parquet").to_vec();
    rig.put("damaged/thrift_skip_past_eof.parquet", bytes);
    let local = rig.local_result("SELECT * FROM '{d}/damaged/thrift_skip_past_eof.parquet'");
    let t = Instant::now();
    let remote = rig.remote_result("SELECT * FROM '{d}/damaged/thrift_skip_past_eof.parquet'");
    assert!(
        t.elapsed() < Duration::from_secs(20),
        "it hung: {:?}",
        t.elapsed()
    );
    let (l, r) = (
        local.expect_err("local is an error"),
        remote.expect_err("remote is an error"),
    );
    assert!(
        l.contains("corrupt") || l.contains("damaged") || !l.is_empty(),
        "{l}"
    );
    assert!(!r.is_empty() && !r.contains("127.0.0.1"), "{r}");
}

/// The `gs://` path against the fake's S3-style endpoint through the GCS builder (`GOOGLE_BASE_URL`, anonymous): the same objects, the same rows, the object
/// generation as the pinned version. Native GCS with real authentication is NOT exercised here.
#[test]
fn gs_locations_read_the_same_rows_through_the_gcs_client() {
    use nata_objstore::store::GcsOptions;
    use object_store::gcp::GoogleConfigKey;
    let rig = Rig::new("gcs");
    lineitems(&rig, 2);
    rig.fake
        .put_versioned("g.parquet", lineitem_like(6000, 1000, 9), "7001");
    std::fs::write(rig.dir.join("g.parquet"), lineitem_like(6000, 1000, 9)).expect("local copy");
    let mut o = nata_objstore::NodeOptions::from_env();
    o.cfg = Config::fast_tests();
    o.gcs = GcsOptions {
        config: vec![
            (GoogleConfigKey::BaseUrl, rig.fake.addr.clone()),
            (GoogleConfigKey::SkipSignature, "true".into()),
        ],
        allow_http: true,
    };
    let handle = nata_objstore::ObjectHandle::new(o).expect("node");
    let io: Arc<dyn nata_parquet::RemoteIo> = Arc::new(handle);
    let sql = |q: &str| {
        let stop = || false;
        let routed = nata_sql::RoutedSource {
            local: &nata_sql::FsSource,
            remote: io.as_ref(),
            cancel: &stop,
        };
        let planned = nata_sql::plan_sql_with(q, &routed, nata_sql::BindOptions::default())
            .map_err(|e| e.to_string())?;
        let mut cfg = nata_exec::ExecConfig::new(3);
        cfg.remote = Some(Arc::clone(&io));
        let mut rows = 0u64;
        nata_exec::execute(Arc::new(planned.plan), &cfg, None, |b| {
            rows += b.num_rows() as u64;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        Ok::<u64, String>(rows)
    };
    let want = rig
        .local_result("SELECT l_orderkey FROM '{d}/g.parquet' WHERE l_orderkey > 20")
        .expect("local")
        .lines()
        .count() as u64
        - 1;
    assert_eq!(
        sql("SELECT l_orderkey FROM 'gs://bkt/g.parquet' WHERE l_orderkey > 20"),
        Ok(want)
    );
    // a glob lists through the GCS client (the XML API shares the S3 listing) and pins the version of a versioned object
    assert_eq!(
        sql("SELECT l_orderkey FROM 'gs://bkt/li/*.parquet'"),
        Ok(24_000)
    );
    assert!(
        rig.fake
            .log()
            .iter()
            .any(|r| r.method == "HEAD" && r.key == "g.parquet")
    );
}

#[test]
fn cold_footer_fill_is_single_flight_and_cache_is_reserved() {
    let rig = Rig::new("single-flight");
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    rig.fake.set_plan(Plan::always(Fault::Delay { ms: 80 }));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let io = rig.io.clone();
                let barrier = barrier.clone();
                s.spawn(move || {
                    barrier.wait();
                    io.begin()
                        .expect("query")
                        .open("s3://bkt/one.parquet", &|| false)
                        .expect("footer")
                })
            })
            .collect();
        for h in hs {
            let _ = h.join().expect("thread");
        }
    });
    assert_eq!(
        rig.fake.gets(),
        1,
        "one GET for eight concurrent cold reads"
    );
    let pool = rig.handle.0.node_pool.snapshot();
    assert!(pool.peak <= pool.budget);
    let exec = rig.handle.0.memory().expect("memory").snapshot();
    assert_eq!(
        exec.budget
            + rig.handle.0.cache.as_ref().expect("cache").cap_bytes() as usize
            + pool.progress,
        pool.budget
    );
}

#[test]
fn binder_hands_footer_to_scan_even_with_cache_off() {
    let rig = Rig::with("handoff", Config::fast_tests(), false);
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    let sql = "SELECT l_orderkey FROM '{d}/one.parquet'";
    assert_eq!(rig.remote(sql).expect("remote"), rig.local(sql));
    assert_eq!(rig.fake.gets(), 2, "one footer GET plus one data GET");
}

/// Indicative delayed-loopback experiment, never a frozen real-store verdict.
#[test]
#[ignore = "manual paired throughput evidence"]
fn window_not_workers_carries_io_concurrency() {
    use nata_parquet::{ParallelConfig, ScanOptions, ScanPlan, run_parallel};
    let rig = Rig::new("window");
    rig.put("groups.parquet", lineitem_like(32_000, 1_000, 3));
    rig.fake.set_plan(Plan::always(Fault::Delay { ms: 50 }));
    for round in 0..3 {
        for (threads, window) in [(1, 1), (1, 16), (4, 16), (1, 1)] {
            let plan = Arc::new(
                ScanPlan::open_remote(
                    &["s3://bkt/groups.parquet".into()],
                    &ScanOptions {
                        columns: Some(vec!["l_orderkey".into()]),
                        ..Default::default()
                    },
                    Arc::new(nata_parquet::FsOpener),
                    None,
                    None,
                    None,
                    Some(&rig.io),
                )
                .expect("plan"),
            );
            let mut cfg = ParallelConfig::new(threads);
            cfg.window = window;
            let start = Instant::now();
            let mut rows = 0;
            let report = run_parallel(plan, cfg, None, |b| {
                rows += b.num_rows();
                Ok(())
            })
            .expect("scan");
            assert_eq!(rows, 32_000);
            assert_eq!(report.threads, threads);
            assert_eq!(report.window, window);
            println!(
                "WINDOW round={round} threads={threads} window={window} ms={:.2} rows={rows}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

#[test]
fn cancelling_one_shared_fill_waiter_preserves_the_other() {
    let rig = Rig::new("fill-cancel");
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    rig.fake.set_plan(Plan::always(Fault::Delay { ms: 150 }));
    let first = rig.io.begin().expect("query");
    let second = rig.io.begin().expect("query");
    let barrier = Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|s| {
        let start = barrier.clone();
        let q = first.clone();
        let a = s.spawn(move || {
            start.wait();
            q.open("s3://bkt/one.parquet", &|| false)
        });
        let start = barrier.clone();
        let b = s.spawn(move || {
            start.wait();
            second.open("s3://bkt/one.parquet", &|| false)
        });
        barrier.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        while (rig.fake.gets() == 0
            || rig.handle.0.cache.as_ref().expect("cache").fill_waiters() < 2)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(rig.fake.gets(), 1);
        assert_eq!(
            rig.handle.0.cache.as_ref().expect("cache").fill_waiters(),
            2
        );
        first.cancel();
        assert!(a.join().expect("first thread").is_err());
        assert!(b.join().expect("second thread").is_ok());
    });
    assert_eq!(rig.fake.gets(), 1);
}

#[test]
fn last_fill_waiter_cancel_releases_stalled_response_and_does_not_publish() {
    let rig = Rig::new("last-fill-cancel");
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    rig.fake
        .set_plan(Plan::always(Fault::StallBody { after: 1 }));
    let query = rig.io.begin().expect("query");
    std::thread::scope(|s| {
        let q = query.clone();
        let reader = s.spawn(move || q.open("s3://bkt/one.parquet", &|| false));
        let deadline = Instant::now() + Duration::from_secs(1);
        while rig.fake.gets() == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(rig.fake.gets(), 1);
        query.cancel();
        assert!(reader.join().expect("reader").is_err());
    });
    let memory = rig.handle.0.memory().expect("memory");
    let deadline = Instant::now() + Duration::from_secs(1);
    while memory.snapshot().held != 0 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(memory.snapshot().held, 0);
    assert_eq!(rig.handle.0.cache.as_ref().expect("cache").entries(), 0);
}

#[test]
fn concurrent_binders_keep_their_own_identity_and_cancel_context() {
    let rig = Rig::with("scope-isolation", Config::fast_tests(), false);
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    let a = rig.io.scoped().expect("scope A");
    let b = rig.io.scoped().expect("scope B");
    let _old = a
        .footer("s3://bkt/one.parquet", &|| false)
        .expect("old footer");
    rig.put("one.parquet", lineitem_like(2, 1, 8));
    let _new = b
        .footer("s3://bkt/one.parquet", &|| false)
        .expect("new footer");
    let qa = a.begin().expect("query A");
    assert!(
        qa.open("s3://bkt/one.parquet", &|| false).is_err(),
        "A must reject replacement after its bind"
    );
    qa.cancel();
    let qb = b.begin().expect("query B");
    let file = qb
        .open("s3://bkt/one.parquet", &|| false)
        .expect("B's pinned footer");
    assert_eq!(
        rig.fake.gets(),
        2,
        "one footer GET per version, no duplicate during handoff"
    );
    let range = 0..4;
    let data = qb
        .fetch(&file, std::slice::from_ref(&range), &|| false)
        .expect("B not cancelled by A");
    assert_eq!(data.pieces[0].1.as_ref(), b"PAR1");
}

#[test]
fn zero_cache_allowance_is_the_uncached_control() {
    let mut cfg = Config::fast_tests();
    cfg.cache_bytes = 0;
    let rig = Rig::with("zero-cache", cfg, true);
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    let sql = "SELECT l_orderkey FROM '{d}/one.parquet'";
    assert_eq!(rig.remote(sql).expect("remote"), rig.local(sql));
    assert!(rig.handle.0.cache.is_none());
    assert_eq!(rig.fake.gets(), 2);
}

#[test]
fn grant_wait_and_network_share_one_logical_read_deadline() {
    let mut cfg = Config::fast_tests();
    cfg.op_deadline = Duration::from_millis(800);
    cfg.attempt_timeout = Duration::from_millis(1500);
    let rig = Rig::with("grant-deadline", cfg, true);
    rig.put("one.parquet", lineitem_like(1, 1, 7));
    let query = rig.handle.0.query().expect("query");
    let file = nata_parquet::RemoteQuery::open(query.as_ref(), "s3://bkt/one.parquet", &|| false)
        .expect("footer");
    let memory = query.q.memory().clone();
    let snapshot = memory.snapshot();
    let held = match nata_sched::memory::MemoryManager::request(
        &memory,
        0,
        snapshot.budget - snapshot.progress,
    )
    .expect("hold capacity")
    {
        nata_sched::memory::Reservation::Granted(g) => g,
        _ => panic!("initial capacity"),
    };
    rig.fake.set_plan(Plan::always(Fault::Delay { ms: 500 }));
    let before = rig.fake.gets();
    let release = rig.rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(held);
    });
    let start = Instant::now();
    let range = 0..4;
    let error = nata_parquet::RemoteQuery::fetch(
        query.as_ref(),
        &file,
        std::slice::from_ref(&range),
        &|| false,
    )
    .err()
    .expect("combined read exceeds 800 ms");
    assert!(error.message.contains("deadline"), "{error:?}");
    assert!(start.elapsed() < Duration::from_millis(1300));
    rig.rt.block_on(release).expect("release");
    assert_eq!(
        rig.fake.gets(),
        before + 1,
        "grant wait ended and the network attempt started"
    );
    assert_eq!(memory.snapshot().held, 0);
}

#[test]
fn a_permanent_footer_failure_does_not_wait_for_the_other_stalled_range() {
    let rig = Rig::new("footer-fail-fast");
    let bytes = lineitem_like(12_000, 1_000, 7);
    assert!(bytes.len() > 64 << 10);
    rig.put("many.parquet", bytes);
    rig.fake.set_plan(Plan {
        script: vec![Some(Fault::Status {
            code: 403,
            retry_after: None,
            body: 10,
            endless: false,
        })],
        sticky: Some(Fault::StallBody { after: 1 }),
        ..Default::default()
    });
    let start = Instant::now();
    let error = rig
        .io
        .begin()
        .expect("query")
        .open("s3://bkt/many.parquet", &|| false)
        .expect_err("permanent error");
    assert!(error.message.contains("denied"), "{error:?}");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "a permanent failure waited for its sibling"
    );
    assert!(
        rig.fake.gets() <= 2,
        "no sibling retries after the permanent failure"
    );
}
