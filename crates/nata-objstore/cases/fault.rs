//! The reliability cases of the wrapper against the hostile fake endpoint. Each case states its expected outcome (a correct result, or a
//! bounded error of a given class with a given number of attempts), runs under a hard harness limit (a hang is a failure, not a stuck
//! test), and returns a record. `all_cases(mutants)` is run on the real wrapper (all must pass) and once per mutant (at least one must
//! fail: CAUGHT). Cases that could allocate without bound run only in a child process under `ulimit -v` (see `hostile`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use nata_objfake::{FakeS3, Fault, Plan};
use nata_objstore::policy::{Config, Mutants};
use nata_objstore::remote::{Class, Identity, Kind, Query, ReadError, Remote, Target};
use nata_objstore::store::{S3Options, build_s3};
use nata_objstore::util::Rng;
use object_store::aws::AmazonS3ConfigKey;
use object_store::path::Path;

#[derive(Clone, Debug)]
pub struct CaseResult {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

pub const OBJ_LEN: usize = 1 << 20;
const HARNESS_LIMIT: Duration = Duration::from_secs(14);

pub fn object_bytes(seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    (0..OBJ_LEN).map(|_| r.next_u64() as u8).collect()
}

pub struct Rig {
    pub fake: FakeS3,
    pub remote: Arc<Remote>,
    pub q: Arc<Query>,
    pub target: Target,
    pub data: Vec<u8>,
    pub cfg: Config,
}

/// The options that point an S3 store at the fake: plain http on loopback, anonymous (the fake does not verify signatures).
pub fn fake_s3(fake: &FakeS3) -> S3Options {
    S3Options {
        config: vec![
            (AmazonS3ConfigKey::Endpoint, fake.addr.clone()),
            (AmazonS3ConfigKey::Region, "us-east-1".into()),
            (
                AmazonS3ConfigKey::Client(object_store::ClientConfigKey::AllowHttp),
                "true".into(),
            ),
            (AmazonS3ConfigKey::SkipSignature, "true".into()),
        ],
        ..Default::default()
    }
}

/// A query memory service of `budget` bytes (ADR 0009: one operator, a small progress reserve).
pub fn memory(budget: usize) -> Arc<nata_sched::memory::MemoryManager> {
    let progress = (1usize << 20).min(budget / 8).max(1);
    nata_sched::memory::MemoryManager::new(budget, progress, 1).expect("memory service")
}

impl Rig {
    pub async fn new(cfg: Config, m: Mutants) -> Rig {
        let fake = FakeS3::start("bkt").await;
        let data = object_bytes(1);
        fake.put("o.bin", data.clone());
        let store = build_s3(&fake_s3(&fake), "bkt", &cfg, &m).expect("store");
        let budget = cfg.grant_bytes;
        let remote = Remote::new(cfg.clone(), m, 42);
        let q = remote.query(memory(budget));
        Rig {
            fake,
            remote,
            q,
            target: Target {
                store,
                path: Path::from("o.bin"),
            },
            data,
            cfg,
        }
    }
    pub async fn id(&self) -> Identity {
        let id = self.q.head(&self.target).await.expect("head");
        self.fake.clear_log();
        id
    }
    pub async fn read(
        &self,
        id: &Identity,
        r: std::ops::Range<u64>,
    ) -> Result<bytes::Bytes, ReadError> {
        self.q
            .fetch_range(&self.target, id, r, Kind::Data)
            .await
            .map(|f| f.data)
    }
}

fn want(r: &Rig, rg: std::ops::Range<u64>) -> Vec<u8> {
    r.data[rg.start as usize..rg.end as usize].to_vec()
}

const R: std::ops::Range<u64> = 100_000..300_000;

fn status(code: u16, ra: Option<u64>) -> Fault {
    Fault::Status {
        code,
        retry_after: ra,
        body: 200,
        endless: false,
    }
}

struct Expect {
    ok_bytes: bool,
    class: Option<Class>,
    /// A second acceptable class (the client crate reports some malformed ranges as a generic response error).
    alt: Option<Class>,
    attempts: (u32, u32),
    max_ms: u128,
    min_ms: u128,
}

fn check(
    name: &'static str,
    res: Result<bytes::Bytes, ReadError>,
    truth: &[u8],
    e: Expect,
    t: Instant,
) -> CaseResult {
    let ms = t.elapsed().as_millis();
    let mut why = Vec::new();
    let attempts;
    let outcome;
    match (&res, e.ok_bytes) {
        (Ok(b), true) => {
            attempts = 0;
            outcome = "ok".to_string();
            if b.as_ref() != truth {
                why.push(format!(
                    "returned {} bytes that are not the object's bytes",
                    b.len()
                ));
            }
        }
        (Ok(b), false) => {
            attempts = 0;
            outcome = format!("ok({} bytes)", b.len());
            why.push("expected an error, got a success".to_string());
        }
        (Err(er), expect_ok) => {
            attempts = er.attempts;
            outcome = format!("{:?}", er.class);
            if expect_ok {
                why.push(format!("expected success, got {:?}", er.class));
            } else if let Some(c) = e.class
                && er.class != c
                && Some(er.class) != e.alt
            {
                why.push(format!("class {:?}, expected {:?}", er.class, c));
            }
        }
    }
    if let Err(er) = &res
        && (er.attempts < e.attempts.0 || er.attempts > e.attempts.1)
    {
        why.push(format!(
            "attempts {} not in {}..={}",
            er.attempts, e.attempts.0, e.attempts.1
        ));
    }
    if ms > e.max_ms {
        why.push(format!("took {ms} ms > {} ms", e.max_ms));
    }
    if ms < e.min_ms {
        why.push(format!(
            "took {ms} ms < {} ms (backoff or Retry-After not honored)",
            e.min_ms
        ));
    }
    CaseResult {
        name,
        ok: why.is_empty(),
        detail: format!(
            "outcome={outcome} attempts={attempts} elapsed_ms={ms}{}",
            if why.is_empty() {
                String::new()
            } else {
                format!(" WRONG: {}", why.join("; "))
            }
        ),
    }
}

async fn guarded<F: std::future::Future<Output = CaseResult>>(
    name: &'static str,
    f: F,
) -> CaseResult {
    match tokio::time::timeout(HARNESS_LIMIT, f).await {
        Ok(r) => r,
        Err(_) => CaseResult {
            name,
            ok: false,
            detail: format!(
                "HANG: still running after {} s (the deadline did not bound it)",
                HARNESS_LIMIT.as_secs()
            ),
        },
    }
}

fn fast() -> Config {
    Config::fast_tests()
}

/// Every in-process case. `m` is the mutant under test (default = the real wrapper).
pub async fn all_cases(m: &Mutants) -> Vec<CaseResult> {
    let mut v = Vec::new();
    macro_rules! case {
        ($name:expr, $body:expr) => {
            v.push(guarded($name, $body).await)
        };
    }
    let e = |ok, class, a: (u32, u32), min_ms, max_ms| Expect {
        ok_bytes: ok,
        class,
        alt: None,
        attempts: a,
        min_ms,
        max_ms,
    };
    let e_alt = |class, alt, a: (u32, u32)| Expect {
        ok_bytes: false,
        class: Some(class),
        alt: Some(alt),
        attempts: a,
        min_ms: 0,
        max_ms: 3000,
    };

    case!("throttle_503_slowdown_then_ok", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::script(vec![
            Some(status(503, None)),
            Some(status(503, None)),
            None,
        ]));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let mut c = check(
            "throttle_503_slowdown_then_ok",
            res,
            &want(&r, R),
            e(true, None, (0, 99), 30, 3000),
            t,
        );
        if r.fake.gets() != 3 {
            c.ok = false;
            c.detail += &format!(" WRONG: server saw {} GETs, expected 3", r.fake.gets());
        }
        c
    });
    case!("throttle_429_retry_after_honored", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::script(vec![Some(status(429, Some(1))), None]));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "throttle_429_retry_after_honored",
            res,
            &want(&r, R),
            e(true, None, (0, 9), 1000, 4000),
            t,
        )
    });
    case!(
        "retry_after_beyond_deadline_fails_without_another_attempt",
        async {
            let r = Rig::new(fast(), m.clone()).await;
            let id = r.id().await;
            r.fake.set_plan(Plan::always(status(429, Some(3600))));
            let t = Instant::now();
            let res = r.read(&id, R).await;
            let mut c = check(
                "retry_after_beyond_deadline_fails_without_another_attempt",
                res,
                &[],
                e(false, Some(Class::Throttled), (1, 1), 0, 3000),
                t,
            );
            if r.fake.gets() != 1 {
                c.ok = false;
                c.detail += &format!(" WRONG: server saw {} GETs, expected 1", r.fake.gets());
            }
            c
        }
    );
    case!("persistent_500_exactly_four_attempts", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(500, None)));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let mut c = check(
            "persistent_500_exactly_four_attempts",
            res,
            &[],
            e(false, Some(Class::Server5xx), (4, 4), 0, 5000),
            t,
        );
        if r.fake.gets() != 4 {
            c.ok = false;
            c.detail += &format!(" WRONG: server saw {} GETs, expected 4", r.fake.gets());
        }
        c
    });
    case!("backoff_gaps_follow_equal_jitter", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(500, None)));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let log = r.fake.log();
        let mut why = Vec::new();
        if res.is_ok() {
            why.push("expected an error".to_string());
        }
        // nominal 20, 40, 80 ms: each gap between attempts k and k+1 is in [nominal/2, nominal] (+ scheduling slack)
        for (k, w) in log.windows(2).enumerate() {
            let gap = (w[1].at - w[0].at).as_millis() as u64;
            let nominal = (20u64 << k).min(200);
            if gap + 3 < nominal / 2 || gap > nominal + 150 {
                why.push(format!(
                    "gap {k} = {gap} ms outside [{}, {}+slack]",
                    nominal / 2,
                    nominal
                ));
            }
        }
        CaseResult {
            name: "backoff_gaps_follow_equal_jitter",
            ok: why.is_empty(),
            detail: format!(
                "gaps_ms={:?} elapsed_ms={} {}",
                log.windows(2)
                    .map(|w| (w[1].at - w[0].at).as_millis())
                    .collect::<Vec<_>>(),
                t.elapsed().as_millis(),
                if why.is_empty() {
                    String::new()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    case!("stalled_headers_timeout_then_ok", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::script(vec![Some(Fault::StallHeaders), None]));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "stalled_headers_timeout_then_ok",
            res,
            &want(&r, R),
            e(true, None, (0, 9), 800, 4000),
            t,
        )
    });
    case!(
        "stalled_body_persistent_bounded_by_attempt_timeout",
        async {
            let r = Rig::new(fast(), m.clone()).await;
            let id = r.id().await;
            r.fake
                .set_plan(Plan::always(Fault::StallBody { after: 1000 }));
            let t = Instant::now();
            let res = r.read(&id, R).await;
            check(
                "stalled_body_persistent_bounded_by_attempt_timeout",
                res,
                &[],
                e(false, Some(Class::Timeout), (4, 4), 3000, 6000),
                t,
            )
        }
    );
    case!("truncated_body_retried_no_duplicate_bytes", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::script(vec![
            Some(Fault::TruncateAfter { after: 5000 }),
            None,
        ]));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "truncated_body_retried_no_duplicate_bytes",
            res,
            &want(&r, R),
            e(true, None, (0, 9), 0, 4000),
            t,
        )
    });
    case!(
        "chunked_range_response_without_content_length_rejected",
        async {
            // the client crate requires a Content-Length on an object response: a chunked, cut-off range response is malformed, not transient
            let r = Rig::new(fast(), m.clone()).await;
            let id = r.id().await;
            r.fake.set_plan(Plan::script(vec![
                Some(Fault::TruncateChunked { after: 3000 }),
                None,
            ]));
            let t = Instant::now();
            let res = r.read(&id, R).await;
            check(
                "chunked_range_response_without_content_length_rejected",
                res,
                &[],
                e_alt(Class::BadResponse, Class::Truncated, (1, 4)),
                t,
            )
        }
    );
    case!("truncated_persistent_four_attempts", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::always(Fault::TruncateAfter { after: 100 }));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "truncated_persistent_four_attempts",
            res,
            &[],
            e_alt(Class::Truncated, Class::Transport, (4, 4)),
            t,
        )
    });
    case!("connection_reset_then_ok", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::script(vec![
            Some(Fault::Reset),
            Some(Fault::Reset),
            None,
        ]));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "connection_reset_then_ok",
            res,
            &want(&r, R),
            e(true, None, (0, 9), 0, 4000),
            t,
        )
    });
    case!("content_range_claims_more_than_requested", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::RangeClaim {
            start_delta: 0,
            end_delta: 1000,
        }));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let mut c = check(
            "content_range_claims_more_than_requested",
            res,
            &[],
            e_alt(Class::BadRange, Class::BadResponse, (1, 1)),
            t,
        );
        if r.fake.gets() != 1 {
            c.ok = false;
            c.detail += &format!(
                " WRONG: server saw {} GETs, a malformed range must not be retried",
                r.fake.gets()
            );
        }
        c
    });
    case!("content_range_shifted_start", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::RangeClaim {
            start_delta: -100,
            end_delta: -100,
        }));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "content_range_shifted_start",
            res,
            &[],
            e_alt(Class::BadRange, Class::BadResponse, (1, 1)),
            t,
        )
    });
    case!("range_ignored_full_object_200", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::IgnoreRange));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let mut c = check(
            "range_ignored_full_object_200",
            res,
            &[],
            e(false, None, (1, 1), 0, 2000),
            t,
        );
        if r.fake.gets() != 1 {
            c.ok = false;
            c.detail += " WRONG: retried";
        }
        c
    });
    case!("content_length_longer_than_range", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::always(Fault::LengthMismatch { delta: 10 }));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "content_length_longer_than_range",
            res,
            &[],
            e(false, Some(Class::BadLength), (1, 1), 0, 2000),
            t,
        )
    });
    case!("content_length_shorter_than_range", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::always(Fault::LengthMismatch { delta: -10 }));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "content_length_shorter_than_range",
            res,
            &[],
            e(false, Some(Class::BadLength), (1, 1), 0, 2000),
            t,
        )
    });
    case!("version_change_between_two_range_reads", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        let mut b = r.data.clone();
        b.reverse(); // same length, different bytes, a new ETag
        r.fake.set_plan(Plan {
            swap_before_get: vec![(1, "o.bin".into(), Arc::new(b))],
            ..Default::default()
        });
        let t = Instant::now();
        let first = r.read(&id, 0..50_000).await;
        let second = r.read(&id, 50_000..100_000).await;
        let mut why = Vec::new();
        match &first {
            Ok(x) if x.as_ref() == &r.data[0..50_000] => {}
            _ => why.push("the first read is not version A's bytes".to_string()),
        }
        match &second {
            Err(er) if er.class == Class::IdentityChanged && er.attempts == 1 => {}
            Err(er) => why.push(format!("second read failed as {:?} after {} attempts, expected IdentityChanged after 1", er.class, er.attempts)),
            Ok(x) => why.push(format!("second read SUCCEEDED with {} bytes of another version: rows from two versions would mix", x.len())),
        }
        CaseResult {
            name: "version_change_between_two_range_reads",
            ok: why.is_empty(),
            detail: format!(
                "elapsed_ms={} {}",
                t.elapsed().as_millis(),
                if why.is_empty() {
                    "second read failed IdentityChanged (412)".into()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    case!("version_change_endpoint_ignores_if_match", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        let mut b = r.data.clone();
        b.reverse();
        r.fake.set_plan(Plan {
            swap_before_get: vec![(0, "o.bin".into(), Arc::new(b))],
            sticky: Some(Fault::IgnoreIfMatch),
            ..Default::default()
        });
        let t = Instant::now();
        let res = r.read(&id, 0..50_000).await;
        check(
            "version_change_endpoint_ignores_if_match",
            res,
            &[],
            e(false, Some(Class::IdentityChanged), (1, 1), 0, 2000),
            t,
        )
    });
    case!("size_change_endpoint_ignores_if_match", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        let mut b = r.data.clone();
        b.extend_from_slice(&[7u8; 4096]);
        r.fake.set_plan(Plan {
            swap_before_get: vec![(0, "o.bin".into(), Arc::new(b))],
            sticky: Some(Fault::IgnoreIfMatch),
            ..Default::default()
        });
        let t = Instant::now();
        let res = r.read(&id, 0..50_000).await;
        check(
            "size_change_endpoint_ignores_if_match",
            res,
            &[],
            e(false, Some(Class::IdentityChanged), (1, 1), 0, 2000),
            t,
        )
    });
    case!("auth_403_is_permanent", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(403, None)));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "auth_403_is_permanent",
            res,
            &[],
            e(false, Some(Class::Auth), (1, 1), 0, 2000),
            t,
        )
    });
    case!("missing_object_404_is_permanent", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(404, None)));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "missing_object_404_is_permanent",
            res,
            &[],
            e(false, Some(Class::NotFound), (1, 1), 0, 2000),
            t,
        )
    });
    case!(
        "range_beyond_pinned_length_rejected_before_any_request",
        async {
            let r = Rig::new(fast(), m.clone()).await;
            let id = r.id().await;
            let t = Instant::now();
            let res = r
                .read(&id, (OBJ_LEN as u64 - 10)..(OBJ_LEN as u64 + 10))
                .await;
            let mut c = check(
                "range_beyond_pinned_length_rejected_before_any_request",
                res,
                &[],
                e(false, Some(Class::BadRange), (0, 0), 0, 500),
                t,
            );
            if r.fake.gets() != 0 {
                c.ok = false;
                c.detail += " WRONG: a request was sent";
            }
            c
        }
    );
    case!("grant_too_small_is_a_bounded_error", async {
        let r = Rig::new(
            Config {
                grant_bytes: 64 << 10,
                ..fast()
            },
            m.clone(),
        )
        .await;
        let id = r.id().await;
        let t = Instant::now();
        let res = r.read(&id, R).await;
        check(
            "grant_too_small_is_a_bounded_error",
            res,
            &[],
            e(false, Some(Class::GrantTooSmall), (0, 0), 0, 500),
            t,
        )
    });
    case!("cancel_during_stalled_body", async {
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::always(Fault::StallBody { after: 100 }));
        let q = r.q.clone();
        let p = r.target.clone();
        let id2 = id.clone();
        let h =
            tokio::spawn(
                async move { q.fetch_range(&p, &id2, R, Kind::Data).await.map(|f| f.data) },
            );
        tokio::time::sleep(Duration::from_millis(150)).await;
        let t = Instant::now();
        r.q.cancel.cancel();
        let res = h.await.expect("task");
        let mut why = Vec::new();
        match res {
            Err(er) if er.class == Class::Cancelled => {}
            other => why.push(format!(
                "expected Cancelled, got {:?}",
                other.map(|b| b.len())
            )),
        }
        let q_ms = t.elapsed().as_millis();
        if q_ms > 1000 {
            why.push(format!("quiescence {q_ms} ms > 1000 ms"));
        }
        if r.remote.stats.inflight.cur() != 0 {
            why.push(format!(
                "{} attempts still counted in flight",
                r.remote.stats.inflight.cur()
            ));
        }
        CaseResult {
            name: "cancel_during_stalled_body",
            ok: why.is_empty(),
            detail: format!(
                "quiescence_ms={q_ms} {}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    case!("cancel_during_backoff_sleep", async {
        let r = Rig::new(
            Config {
                backoff_initial: Duration::from_secs(2),
                backoff_max: Duration::from_secs(4),
                ..fast()
            },
            m.clone(),
        )
        .await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(503, None)));
        let q = r.q.clone();
        let p = r.target.clone();
        let id2 = id.clone();
        let h =
            tokio::spawn(
                async move { q.fetch_range(&p, &id2, R, Kind::Data).await.map(|f| f.data) },
            );
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t = Instant::now();
        r.q.cancel.cancel();
        let res = h.await.expect("task");
        let q_ms = t.elapsed().as_millis();
        let ok = matches!(&res, Err(er) if er.class == Class::Cancelled) && q_ms < 1000;
        CaseResult {
            name: "cancel_during_backoff_sleep",
            ok,
            detail: format!(
                "quiescence_ms={q_ms} result={:?}",
                res.map(|b| b.len()).map_err(|e| e.class)
            ),
        }
    });
    case!("cancel_in_permit_wait_other_query_continues", async {
        let r = Rig::new(
            Config {
                query_inflight: 1,
                ..fast()
            },
            m.clone(),
        )
        .await;
        let id = r.id().await;
        r.fake
            .set_plan(Plan::script(vec![Some(Fault::Delay { ms: 700 })]));
        // query A holds its single permit for ~700 ms; a second logical read of the SAME query waits for the permit and is cancelled by
        // its own token (a separate query), while the first continues
        let qa = r.q.clone();
        let qb = r.remote.query(memory(r.cfg.grant_bytes));
        let (p, p2) = (r.target.clone(), r.target.clone());
        let (id_a, id_b) = (id.clone(), id.clone());
        let ha = tokio::spawn(async move {
            qa.fetch_range(&p, &id_a, R, Kind::Data)
                .await
                .map(|f| f.data)
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let qb2 = qb.clone();
        let hb = tokio::spawn(async move {
            qb2.fetch_range(&p2, &id_b, 0..1000, Kind::Data)
                .await
                .map(|f| f.data)
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let t = Instant::now();
        qb.cancel.cancel();
        let b = hb.await.expect("task");
        let a = ha.await.expect("task");
        let ok_a = matches!(&a, Ok(x) if x.as_ref() == &r.data[R.start as usize..R.end as usize]);
        // B was waiting on the node/query permit only if A held it; with separate query pools B had its own permit: it either finished
        // or was cancelled, never hung
        let ok = ok_a
            && t.elapsed().as_millis() < 1500
            && (b.is_ok() || matches!(&b, Err(e) if e.class == Class::Cancelled));
        CaseResult {
            name: "cancel_in_permit_wait_other_query_continues",
            ok,
            detail: format!(
                "a_ok={ok_a} b={:?} elapsed_ms={}",
                b.map(|x| x.len()).map_err(|e| e.class),
                t.elapsed().as_millis()
            ),
        }
    });
    case!("operation_deadline_bounds_a_long_retry_chain", async {
        let cfg = Config {
            max_attempts: 1000,
            backoff_initial: Duration::from_millis(50),
            backoff_max: Duration::from_millis(100),
            op_deadline: Duration::from_millis(1200),
            ..fast()
        };
        let r = Rig::new(cfg, m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(status(503, None)));
        let t = Instant::now();
        let res = r.read(&id, R).await;
        let ms = t.elapsed().as_millis();
        let ok = matches!(&res, Err(e) if e.class == Class::DeadlineExceeded) && ms < 2500;
        CaseResult {
            name: "operation_deadline_bounds_a_long_retry_chain",
            ok,
            detail: format!(
                "elapsed_ms={ms} result={:?} (deadline 1200 ms)",
                res.map(|b| b.len()).map_err(|e| (e.class, e.attempts))
            ),
        }
    });
    case!("inflight_caps_per_query_and_per_node", async {
        let mut why = Vec::new();
        // (a) one query, 60 concurrent reads against a 200 ms endpoint: at most 16 attempts active
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::Delay { ms: 200 }));
        let hs: Vec<_> = (0..60u64)
            .map(|i| {
                let (q, p, idc) = (r.q.clone(), r.target.clone(), id.clone());
                tokio::spawn(async move {
                    q.fetch_range(&p, &idc, i * 1000..i * 1000 + 500, Kind::Data)
                        .await
                        .map(|f| f.data)
                })
            })
            .collect();
        for h in hs {
            if h.await.expect("task").is_err() {
                why.push("a read failed".to_string());
            }
        }
        let peak_q = r.remote.stats.inflight.peak();
        if !(8..=16).contains(&peak_q) {
            why.push(format!(
                "one query: peak {peak_q} active attempts, expected 8..=16"
            ));
        }
        // (b) three queries, 30 reads each: the node cap of 32 binds, not 3 x 16
        let r = Rig::new(fast(), m.clone()).await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::Delay { ms: 200 }));
        let mut hs = Vec::new();
        for _ in 0..3 {
            let q = r.remote.query(memory(r.cfg.grant_bytes));
            for i in 0..30u64 {
                let (q, p, idc) = (q.clone(), r.target.clone(), id.clone());
                hs.push(tokio::spawn(async move {
                    q.fetch_range(&p, &idc, i * 1000..i * 1000 + 500, Kind::Data)
                        .await
                        .map(|f| f.data)
                }));
            }
        }
        for h in hs {
            if h.await.expect("task").is_err() {
                why.push("a read failed".to_string());
            }
        }
        let peak_n = r.remote.stats.inflight.peak();
        if !(17..=32).contains(&peak_n) {
            why.push(format!(
                "three queries: peak {peak_n} active attempts, expected 17..=32 (node cap 32)"
            ));
        }
        if r.remote.stats.inflight.cur() != 0 {
            why.push("attempts still counted in flight after the reads".to_string());
        }
        CaseResult {
            name: "inflight_caps_per_query_and_per_node",
            ok: why.is_empty(),
            detail: format!(
                "peak active attempts: one query {peak_q} (cap 16); three queries {peak_n} (cap 32) {}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    case!("grant_caps_the_bytes_in_flight", async {
        // a query memory budget that fits two 100 KB responses (plus the progress reserve of one eighth): eight concurrent reads must run two at a time
        let budget = (2 * 100_000 + 10_000) * 8 / 7 + 8;
        let r = Rig::new(
            Config {
                grant_bytes: budget,
                ..fast()
            },
            m.clone(),
        )
        .await;
        let id = r.id().await;
        r.fake.set_plan(Plan::always(Fault::Delay { ms: 150 }));
        let hs: Vec<_> = (0..8u64)
            .map(|i| {
                let (q, p, idc) = (r.q.clone(), r.target.clone(), id.clone());
                tokio::spawn(async move {
                    q.fetch_range(&p, &idc, i * 100_000..(i + 1) * 100_000, Kind::Data)
                        .await
                        .map(|f| f.data.len())
                })
            })
            .collect();
        let mut why = Vec::new();
        for h in hs {
            if h.await.expect("task").is_err() {
                why.push("a read failed".to_string());
            }
        }
        let peak = r.remote.stats.inflight.peak();
        if peak > 2 {
            why.push(format!("peak {peak} concurrent attempts, the budget fits 2 responses: bytes were not reserved before the requests"));
        }
        let held = r.q.memory().snapshot();
        if held.held != 0 || held.live_grants != 0 {
            why.push(format!(
                "{} bytes / {} grants still held after the reads",
                held.held, held.live_grants
            ));
        }
        CaseResult {
            name: "grant_caps_the_bytes_in_flight",
            ok: why.is_empty(),
            detail: format!(
                "peak concurrent attempts {peak} (budget fits 2) {}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    case!("seeded_chaos_every_read_exact_or_bounded_error", async {
        let seed = 20261107u64;
        let faults = vec![
            status(503, None),
            status(500, None),
            status(429, None),
            Fault::Reset,
            Fault::TruncateAfter { after: 2000 },
            Fault::TruncateChunked { after: 1000 },
            Fault::Delay { ms: 30 },
        ];
        let mut runs: Vec<Vec<String>> = Vec::new();
        let mut why = Vec::new();
        let (mut okn, mut errn) = (0, 0);
        for round in 0..2 {
            let r = Rig::new(fast(), m.clone()).await;
            let id = r.id().await;
            r.fake.set_plan(Plan {
                seeded: Some((seed, 350, faults.clone())),
                ..Default::default()
            });
            let mut hs = Vec::new();
            for i in 0..40u64 {
                let (q, p, idc, data) = (r.q.clone(), r.target.clone(), id.clone(), r.data.clone());
                hs.push(tokio::spawn(async move {
                    let a = (i * 20_000) % (OBJ_LEN as u64 - 30_000);
                    let res = q
                        .fetch_range(&p, &idc, a..a + 25_000, Kind::Data)
                        .await
                        .map(|f| f.data);
                    (a, res, data)
                }));
            }
            for h in hs {
                let (a, res, data) = h.await.expect("task");
                match res {
                    Ok(b) => {
                        okn += 1;
                        if b.as_ref() != &data[a as usize..a as usize + 25_000] {
                            why.push(format!("read at {a} returned wrong bytes"));
                        }
                    }
                    Err(e) => {
                        errn += 1;
                        if e.attempts > 4 || e.attempts == 0 {
                            why.push(format!("error with {} attempts", e.attempts));
                        }
                    }
                }
            }
            let mut log = r.fake.log();
            log.sort_by_key(|l| l.ordinal);
            runs.push(
                log.iter()
                    .filter(|l| l.method == "GET")
                    .map(|l| format!("{}:{}", l.ordinal, l.fault))
                    .collect(),
            );
            let _ = round;
        }
        // replay: the fault chosen for ordinal n is a function of (seed, n); the common prefix of both runs must be identical
        let n = runs[0].len().min(runs[1].len());
        if runs[0][..n] != runs[1][..n] {
            why.push("the fault schedule differs between two runs of the same seed".to_string());
        }
        CaseResult {
            name: "seeded_chaos_every_read_exact_or_bounded_error",
            ok: why.is_empty(),
            detail: format!(
                "seed={seed} reads_ok={okn} reads_failed_bounded={errn} gets_run1={} gets_run2={} {}",
                runs[0].len(),
                runs[1].len(),
                if why.is_empty() {
                    String::new()
                } else {
                    format!("WRONG: {}", why.join("; "))
                }
            ),
        }
    });
    v
}

/// Cases that can allocate: run in a CHILD process under `ulimit -v`. `name` selects one; prints `CASE <name>: PASS|FAIL detail` and exits 0/1.
pub async fn hostile(name: &str, m: &Mutants) -> CaseResult {
    use nata_objfake::{FakeImds, ImdsCreds, ImdsFault};
    let cfg = Config {
        attempt_timeout: Duration::from_secs(3),
        op_deadline: Duration::from_secs(10),
        ..fast()
    };
    let r = Rig::new(cfg.clone(), m.clone()).await;
    let id = r.id().await;
    let t = Instant::now();
    match name {
        "endless_error_body" => {
            r.fake.set_plan(Plan::always(Fault::Status {
                code: 503,
                retry_after: None,
                body: 0,
                endless: true,
            }));
            let res = r.read(&id, R).await;
            let ms = t.elapsed().as_millis();
            let ok = matches!(&res, Err(e) if e.class == Class::Server5xx && e.attempts == 4)
                && ms < 9000;
            CaseResult {
                name: "endless_error_body",
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|b| b.len()).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        "endless_range_body" => {
            r.fake
                .set_plan(Plan::always(Fault::Endless { claim: 1 << 40 }));
            let res = r.read(&id, R).await;
            let ms = t.elapsed().as_millis();
            let ok = matches!(&res, Err(e) if e.class == Class::BadLength && e.attempts == 1)
                && ms < 9000;
            CaseResult {
                name: "endless_range_body",
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|b| b.len()).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        "huge_header" => {
            r.fake
                .set_plan(Plan::always(Fault::HugeHeader { bytes: 200 << 20 }));
            let res = r.read(&id, R).await;
            let ms = t.elapsed().as_millis();
            let ok = res.is_err() && ms < 9000;
            CaseResult {
                name: "huge_header",
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|b| b.len()).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        // the library reads these SUCCESS bodies whole (LIST XML, credential JSON): the transport must bound them before the library allocates
        "endless_list_body" => {
            r.fake.set_plan(Plan {
                list: Some(Fault::EndlessSuccess { claim: 1 << 40 }),
                ..Default::default()
            });
            let res = r.q.list(&r.target.store, &Path::from("")).await;
            let ms = t.elapsed().as_millis();
            let ok = res.is_err() && ms < 9000;
            CaseResult {
                name: "endless_list_body",
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|k| k.len()).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        "oversized_list_document" => {
            // a well-formed LIST page larger than the bound (the bound is 8 MiB; this is 9 MiB of keys)
            let mut xml = String::from("<ListBucketResult><IsTruncated>false</IsTruncated>");
            while xml.len() < (9 << 20) {
                xml += "<Contents><Key>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</Key><Size>1</Size><ETag>x</ETag><LastModified>2015-10-21T07:28:00.000Z</LastModified></Contents>";
            }
            xml += "</ListBucketResult>";
            r.fake.set_plan(Plan {
                list: Some(Fault::Document {
                    body: xml.into_bytes(),
                }),
                ..Default::default()
            });
            let res = r.q.list(&r.target.store, &Path::from("")).await;
            let ms = t.elapsed().as_millis();
            let ok = res.is_err() && ms < 9000;
            CaseResult {
                name: "oversized_list_document",
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|k| k.len()).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        "endless_imds_token"
        | "endless_imds_role_list"
        | "endless_imds_credentials"
        | "oversized_imds_credentials" => {
            let imds = FakeImds::start(ImdsCreds {
                key_id: "AKIDFAKEIMDS".into(),
                secret: "fakesecret".into(),
                token: None,
            })
            .await;
            imds.set_fault(Some(match name {
                "endless_imds_token" => ImdsFault::EndlessToken,
                "endless_imds_role_list" => ImdsFault::EndlessRoleList,
                "endless_imds_credentials" => ImdsFault::EndlessCredentials,
                _ => ImdsFault::Credentials(vec![b' '; 9 << 20]),
            }));
            let mut s3 = fake_s3(&r.fake);
            s3.config
                .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
            s3.config
                .push((AmazonS3ConfigKey::MetadataEndpoint, imds.addr.clone()));
            let store = build_s3(&s3, "bkt", &cfg, m).expect("store");
            let target = Target {
                store,
                path: Path::from("o.bin"),
            };
            let res = r.q.head(&target).await;
            let ms = t.elapsed().as_millis();
            let ok = res.is_err() && ms < 9500;
            let name: &'static str = HOSTILE_CASES
                .iter()
                .find(|n| **n == name)
                .copied()
                .unwrap_or("unknown");
            CaseResult {
                name,
                ok,
                detail: format!(
                    "elapsed_ms={ms} result={:?}",
                    res.map(|i| i.size).map_err(|e| (e.class, e.attempts))
                ),
            }
        }
        other => CaseResult {
            name: "unknown",
            ok: false,
            detail: format!("no such hostile case {other}"),
        },
    }
}

pub const HOSTILE_CASES: [&str; 9] = [
    "endless_error_body",
    "endless_range_body",
    "huge_header",
    "endless_list_body",
    "oversized_list_document",
    "endless_imds_token",
    "endless_imds_role_list",
    "endless_imds_credentials",
    "oversized_imds_credentials",
];
