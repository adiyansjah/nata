//! Metadata-cache cases (ADR 0012) against the fake endpoint. Like `fault`: each returns a record; the real code must pass them all, every cache
//! mutant must fail at least one.

use std::sync::Arc;
use std::time::Duration;

use nata_objfake::{FakeS3, Fault, Plan};
use nata_objstore::cache::{MetaCache, MetaService, Outcome};
use nata_objstore::policy::{CacheMutants, Config, Mutants};
use nata_objstore::remote::{Kind, Remote, Target};
use nata_objstore::store::build_s3;
use object_store::path::Path;

use super::fault::{CaseResult, fake_s3, memory};
use super::testdata::lineitem_like;

pub struct Env {
    pub fake: FakeS3,
    pub remote: Arc<Remote>,
    pub cfg: Config,
    store: Arc<dyn object_store::ObjectStore>,
}

impl Env {
    pub async fn new(m: &Mutants) -> Env {
        let fake = FakeS3::start("bkt").await;
        let cfg = Config::fast_tests();
        let store = build_s3(&fake_s3(&fake), "bkt", &cfg, m).expect("store");
        let remote = Remote::new(cfg.clone(), m.clone(), 7);
        Env {
            fake,
            remote,
            cfg,
            store,
        }
    }
    pub fn target(&self, key: &str) -> Target {
        Target {
            store: self.store.clone(),
            path: Path::from(key),
        }
    }
    pub fn query(&self) -> Arc<nata_objstore::remote::Query> {
        self.remote.query(memory(self.cfg.grant_bytes))
    }
}

fn res(name: &'static str, why: Vec<String>, ok_detail: String) -> CaseResult {
    CaseResult {
        name,
        ok: why.is_empty(),
        detail: if why.is_empty() {
            ok_detail
        } else {
            format!("{ok_detail} WRONG: {}", why.join("; "))
        },
    }
}

async fn guard<F: std::future::Future<Output = CaseResult>>(
    name: &'static str,
    f: F,
) -> CaseResult {
    match tokio::time::timeout(Duration::from_secs(40), f).await {
        Ok(r) => r,
        Err(_) => CaseResult {
            name,
            ok: false,
            detail: "HANG".into(),
        },
    }
}

fn svc(cache: &Option<Arc<MetaCache>>, scope: &str) -> MetaService {
    MetaService::new(cache.clone(), "s3", scope)
}

pub async fn cache_cases(m: &Mutants, cm: &CacheMutants) -> Vec<CaseResult> {
    let mut v = Vec::new();
    macro_rules! case {
        ($name:expr, $body:expr) => {
            v.push(guard($name, $body).await)
        };
    }

    case!("cache_off_vs_bounded_cache_repeated_footer_reads", async {
        let env = Env::new(m).await;
        let nfiles = 10usize;
        let reps = 5usize;
        for i in 0..nfiles {
            env.fake.put(
                &format!("f{i}.parquet"),
                lineitem_like(3000, 1000, i as u64),
            );
        }
        let mut why = Vec::new();
        let mut line = String::new();
        // footer requests per cold open: the tail read, plus the 4-byte magic read when the object is larger than the first read (these objects are)
        let mut per_open = 0u64;
        for (label, cache) in [
            ("no_cache", None),
            ("bounded_cache", Some(MetaCache::adr(cm.clone()))),
        ] {
            let before = env.remote.stats.snap();
            let g0 = env.fake.gets();
            let h0 = env.fake.heads();
            for _ in 0..reps {
                let q = env.query();
                let s = svc(&cache, "scope-a");
                for i in 0..nfiles {
                    let o = s
                        .open(
                            &q,
                            &env.target(&format!("f{i}.parquet")),
                            &format!("bkt/f{i}.parquet"),
                        )
                        .await
                        .expect("open");
                    if o.handle.meta.file_metadata().num_rows() != 3000 {
                        why.push("wrong metadata".to_string());
                    }
                }
            }
            let d = env.remote.stats.snap().minus(&before);
            line += &format!(
                "{label}: heads={} footer_gets={} footer_bytes={} server_gets={} server_heads={}; ",
                d.requests[Kind::Head as usize],
                d.requests[Kind::Footer as usize],
                d.wire_bytes[Kind::Footer as usize],
                env.fake.gets() - g0,
                env.fake.heads() - h0
            );
            if label == "no_cache" {
                per_open = d.requests[Kind::Footer as usize] / (nfiles * reps) as u64;
                if per_open == 0
                    || d.requests[Kind::Footer as usize] != per_open * (nfiles * reps) as u64
                {
                    why.push(format!(
                        "no-cache footer requests {} are not a whole number per open",
                        d.requests[Kind::Footer as usize]
                    ));
                }
            }
            if label == "bounded_cache" {
                if d.requests[Kind::Footer as usize] != per_open * nfiles as u64 {
                    why.push(format!(
                        "cached footer requests {} != {} (one cold open per object)",
                        d.requests[Kind::Footer as usize],
                        per_open * nfiles as u64
                    ));
                }
                if d.requests[Kind::Head as usize] != (nfiles * reps) as u64 {
                    why.push(format!(
                        "identity lookups {} != {} (one per open, even when warm)",
                        d.requests[Kind::Head as usize],
                        nfiles * reps
                    ));
                }
            }
        }
        res(
            "cache_off_vs_bounded_cache_repeated_footer_reads",
            why,
            format!("{nfiles} objects x {reps} queries: {line}"),
        )
    });

    case!("replaced_object_never_served_stale", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let s = svc(&cache, "scope-a");
        let p = env.target("t.parquet");
        let q = env.query();
        let a = s
            .open(&q, &env.target("t.parquet"), "bkt/t.parquet")
            .await
            .expect("open A");
        env.fake.put("t.parquet", lineitem_like(4500, 1500, 2)); // a new version: different rows, length, ETag
        let q2 = env.query();
        let b = s.open(&q2, &p, "bkt/t.parquet").await.expect("open B");
        let mut why = Vec::new();
        if a.handle.meta.file_metadata().num_rows() != 3000 {
            why.push("first open wrong".to_string());
        }
        if b.handle.meta.file_metadata().num_rows() != 4500 {
            why.push(format!("the replaced object was served with the OLD metadata ({} rows, the new object has 4500)", b.handle.meta.file_metadata().num_rows()));
        }
        res(
            "replaced_object_never_served_stale",
            why,
            "after a replace the next open sees the new footer".into(),
        )
    });

    case!("lru_eviction_bounded_by_bytes_and_entries", async {
        let env = Env::new(m).await;
        for i in 0..6 {
            env.fake.put(
                &format!("f{i}.parquet"),
                lineitem_like(3000, 1000, i as u64),
            );
        }
        // measure one entry's charge, then size a cache for exactly three
        let probe = MetaCache::adr(CacheMutants::default());
        let q = env.query();
        let o = svc(&Some(probe.clone()), "s")
            .open(&q, &env.target("f0.parquet"), "bkt/f0.parquet")
            .await
            .expect("ok");
        let one = o.handle.charge();
        drop(o);
        let cap = one * 3 + one / 2;
        let cache = MetaCache::new(cap, 1024, 16 << 20, cm.clone());
        let s = svc(&Some(cache.clone()), "s");
        let mut why = Vec::new();
        for i in [0, 1, 2, 0, 3, 4, 5] {
            let q = env.query();
            let _ = s
                .open(
                    &q,
                    &env.target(&format!("f{i}.parquet")),
                    &format!("bkt/f{i}.parquet"),
                )
                .await
                .expect("ok"); // handle dropped at once: nothing stays pinned
            if cache.retained_bytes() > cap {
                why.push(format!(
                    "retained {} > cap {cap} after opening f{i}",
                    cache.retained_bytes()
                ));
            }
        }
        // f0 was touched again before f3 came in; f1 and f2 are the oldest: after 3,4,5 only the three newest remain
        let evictions = cache
            .stats
            .evictions
            .load(std::sync::atomic::Ordering::SeqCst);
        if cache.entries() > 3 {
            why.push(format!(
                "{} entries for a cache sized for 3",
                cache.entries()
            ));
        }
        if evictions < 3 {
            why.push(format!("only {evictions} evictions"));
        }
        res(
            "lru_eviction_bounded_by_bytes_and_entries",
            why,
            format!(
                "cap {cap} B (3 entries of ~{one} B): retained {} B, {} entries, {evictions} evictions",
                cache.retained_bytes(),
                cache.entries()
            ),
        )
    });

    case!("pinned_entries_stay_charged_and_force_a_bypass", async {
        let env = Env::new(m).await;
        for i in 0..4 {
            env.fake.put(
                &format!("f{i}.parquet"),
                lineitem_like(3000, 1000, i as u64),
            );
        }
        let probe = MetaCache::adr(CacheMutants::default());
        let q = env.query();
        let one = svc(&Some(probe), "s")
            .open(&q, &env.target("f0.parquet"), "bkt/f0.parquet")
            .await
            .expect("ok")
            .handle
            .charge();
        let cache = MetaCache::new(one * 2 + one / 2, 1024, 16 << 20, cm.clone());
        let s = svc(&Some(cache.clone()), "s");
        let mut pinned = Vec::new();
        let mut outcomes = Vec::new();
        for i in 0..4 {
            let q = env.query();
            let o = s
                .open(
                    &q,
                    &env.target(&format!("f{i}.parquet")),
                    &format!("bkt/f{i}.parquet"),
                )
                .await
                .expect("ok");
            outcomes.push(o.outcome.expect("ok"));
            pinned.push(o.handle); // every handle stays alive: pinned
        }
        let live: u64 = pinned.iter().map(|h| h.charge()).sum();
        let mut why = Vec::new();
        if cache.retained_bytes() < live {
            why.push(format!(
                "retained {} B but live handles hold {live} B: pinned bytes are not charged",
                cache.retained_bytes()
            ));
        }
        if cache.retained_bytes() > cache.cap_bytes() {
            why.push(format!(
                "retained {} B > cap {} B",
                cache.retained_bytes(),
                cache.cap_bytes()
            ));
        }
        if !outcomes.contains(&Outcome::Bypass) {
            why.push("no entry was bypassed although every cached entry was pinned".into());
        }
        let before = cache.retained_bytes();
        drop(pinned);
        res(
            "pinned_entries_stay_charged_and_force_a_bypass",
            why,
            format!(
                "outcomes {outcomes:?}, retained {before} B <= cap {} B, then {} B after the handles dropped",
                cache.cap_bytes(),
                cache.retained_bytes()
            ),
        )
    });

    case!("over_entry_cap_is_bypassed_not_admitted", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = MetaCache::new(64 << 20, 1024, 1024, cm.clone()); // an entry cap of 1 KiB: every real footer is over it
        let s = svc(&Some(cache.clone()), "s");
        let q = env.query();
        let o = s
            .open(&q, &env.target("t.parquet"), "bkt/t.parquet")
            .await
            .expect("ok");
        let mut why = Vec::new();
        if o.handle.meta.file_metadata().num_rows() != 3000 {
            why.push("query got wrong metadata".into());
        }
        if o.outcome != Some(Outcome::Bypass) || cache.entries() != 0 || cache.retained_bytes() != 0
        {
            why.push(format!(
                "outcome {:?}, {} entries, {} B retained: an over-cap entry was admitted",
                o.outcome,
                cache.entries(),
                cache.retained_bytes()
            ));
        }
        res(
            "over_entry_cap_is_bypassed_not_admitted",
            why,
            "bypassed, served uncached".into(),
        )
    });

    case!("failed_fill_is_never_published", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let s = svc(&cache, "s");
        let p = env.target("t.parquet");
        let q = env.query();
        let id = q.head(&p).await.expect("ok");
        env.fake.set_plan(Plan::always(Fault::Status {
            code: 500,
            retry_after: None,
            body: 10,
            endless: false,
        }));
        let r = s.open(&env.query(), &p, "bkt/t.parquet").await;
        let mut why = Vec::new();
        if r.is_ok() {
            why.push("open succeeded against a failing endpoint".into());
        }
        if cache.as_ref().expect("ok").entries() != 0 {
            why.push("a failed fill left an entry".into());
        }
        env.fake.set_plan(Plan::default());
        let o = s.open(&env.query(), &p, "bkt/t.parquet").await.expect("ok");
        if o.outcome != Some(Outcome::Miss) {
            why.push(format!(
                "after the failure the next open was {:?}, expected a clean Miss",
                o.outcome
            ));
        }
        let _ = id;
        res(
            "failed_fill_is_never_published",
            why,
            "fill failed after 4 attempts, nothing cached, next open filled normally".into(),
        )
    });

    case!("credential_scope_is_part_of_the_key", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let p = env.target("t.parquet");
        let a = svc(&cache, "scope-a")
            .open(&env.query(), &p, "bkt/t.parquet")
            .await
            .expect("ok");
        let b = svc(&cache, "scope-b")
            .open(&env.query(), &p, "bkt/t.parquet")
            .await
            .expect("ok");
        let mut why = Vec::new();
        if a.outcome != Some(Outcome::Miss) || b.outcome != Some(Outcome::Miss) {
            why.push(format!("scope-b saw {:?} after scope-a filled the entry: an entry was shared across credential scopes", b.outcome));
        }
        res(
            "credential_scope_is_part_of_the_key",
            why,
            "two scopes, two entries".into(),
        )
    });
    v
}
