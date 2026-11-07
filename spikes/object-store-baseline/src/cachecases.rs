//! Metadata-cache cases (ADR 0012) and end-to-end scan cases (version race mid-scan, policy equivalence) against the fake endpoint.
//! Like `faultcases`: each returns a record; the real code must pass them all, every mutant must fail at least one.

use std::sync::Arc;
use std::time::Duration;

use object_store::path::Path;

use crate::cache::{CacheMutants, MetaCache, MetaService, Outcome};
use crate::engine::{Exec, ExecKind, PolicyKind, QueryKind, ScanCfg, scan};
use crate::fake::{FakeS3, Fault, Plan};
use crate::faultcases::CaseResult;
use crate::policy::{Config, Mutants};
use crate::remote::{Class, Kind, Remote, S3Spec, build_s3};
use crate::testdata::lineitem_like;

pub struct Env {
    pub fake: FakeS3,
    pub remote: Arc<Remote>,
}

impl Env {
    pub async fn new(m: &Mutants) -> Env {
        let fake = FakeS3::start("bkt").await;
        let cfg = Config::fast_tests();
        let spec = S3Spec { endpoint: fake.addr.clone(), bucket: "bkt".into(), region: "us-east-1".into(), key: None, secret: None, allow_http: true };
        let store = build_s3(&spec, &cfg, m).expect("store");
        let remote = Remote::new(store, cfg, m.clone(), 7);
        Env { fake, remote }
    }
}

fn res(name: &'static str, why: Vec<String>, ok_detail: String) -> CaseResult {
    CaseResult { name, ok: why.is_empty(), detail: if why.is_empty() { ok_detail } else { format!("{ok_detail} WRONG: {}", why.join("; ")) } }
}

async fn guard<F: std::future::Future<Output = CaseResult>>(name: &'static str, f: F) -> CaseResult {
    match tokio::time::timeout(Duration::from_secs(40), f).await {
        Ok(r) => r,
        Err(_) => CaseResult { name, ok: false, detail: "HANG".into() },
    }
}

/// The expected result of `kind` over `files`, computed by plain Arrow code from the whole Parquet bytes (independent of the fetch path).
pub fn oracle(files: &[Vec<u8>], kind: &QueryKind) -> String {
    use arrow::array::{Array, AsArray};
    use arrow::datatypes::Int64Type;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let (mut rows, mut nonnull) = (0u64, 0u64);
    let mut sums = [0i128; 4];
    let (mut sc, mut ss) = (0u64, 0i128);
    for f in files {
        let b = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(f.clone())).unwrap().build().unwrap();
        for batch in b {
            let batch = batch.unwrap();
            rows += batch.num_rows() as u64;
            for c in batch.columns() {
                nonnull += (c.len() - c.null_count()) as u64;
            }
            let col = |n: &str| batch.column(batch.schema().index_of(n).unwrap()).as_primitive::<Int64Type>().clone();
            for (i, n) in ["l_orderkey", "l_partkey", "l_suppkey", "l_linenumber"].iter().enumerate() {
                sums[i] += col(n).iter().flatten().map(|v| v as i128).sum::<i128>();
            }
            if let QueryKind::Selective { lo, hi } = kind {
                let (k, p) = (col("l_orderkey"), col("l_partkey"));
                for i in 0..k.len() {
                    if (*lo..=*hi).contains(&k.value(i)) {
                        sc += 1;
                        ss += p.value(i) as i128;
                    }
                }
            }
        }
    }
    match kind {
        QueryKind::Full => format!("{rows}|{nonnull}"),
        QueryKind::Proj => format!("{}|{}|{}|{}", sums[0], sums[1], sums[2], sums[3]),
        QueryKind::Selective { .. } => if sc == 0 { "0|NULL".into() } else { format!("{sc}|{ss}") },
        QueryKind::Meta => format!("{rows}"),
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
            env.fake.put(&format!("f{i}.parquet"), lineitem_like(3000, 1000, i as u64));
        }
        let mut why = Vec::new();
        let mut line = String::new();
        for (label, cache) in [("no_cache", None), ("bounded_cache", Some(MetaCache::adr(cm.clone())))] {
            let before = env.remote.stats.snap();
            let g0 = env.fake.gets();
            let h0 = env.fake.heads();
            for _ in 0..reps {
                let q = env.remote.query();
                let s = svc(&cache, "scope-a");
                for i in 0..nfiles {
                    let o = s.open(&q, &Path::from(format!("f{i}.parquet"))).await.expect("open");
                    if o.handle.meta.file_metadata().num_rows() != 3000 {
                        why.push("wrong metadata".to_string());
                    }
                }
            }
            let d = env.remote.stats.snap().minus(&before);
            line += &format!("{label}: heads={} footer_gets={} footer_bytes={} server_gets={} server_heads={}; ", d.requests[Kind::Head as usize], d.requests[Kind::Footer as usize], d.wire_bytes[Kind::Footer as usize], env.fake.gets() - g0, env.fake.heads() - h0);
            if label == "no_cache" && d.requests[Kind::Footer as usize] != (nfiles * reps) as u64 {
                why.push(format!("no-cache footer GETs {} != {}", d.requests[Kind::Footer as usize], nfiles * reps));
            }
            if label == "bounded_cache" {
                if d.requests[Kind::Footer as usize] != nfiles as u64 {
                    why.push(format!("cached footer GETs {} != {} (one per object)", d.requests[Kind::Footer as usize], nfiles));
                }
                if d.requests[Kind::Head as usize] != (nfiles * reps) as u64 {
                    why.push(format!("identity lookups {} != {} (one per open, even when warm)", d.requests[Kind::Head as usize], nfiles * reps));
                }
            }
        }
        res("cache_off_vs_bounded_cache_repeated_footer_reads", why, format!("{nfiles} objects x {reps} queries: {line}"))
    });

    case!("replaced_object_never_served_stale", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let s = svc(&cache, "scope-a");
        let p = Path::from("t.parquet");
        let q = env.remote.query();
        let a = s.open(&q, &p).await.expect("open A");
        env.fake.put("t.parquet", lineitem_like(4500, 1500, 2)); // a new version: different rows, length, ETag
        let q2 = env.remote.query();
        let b = s.open(&q2, &p).await.expect("open B");
        let mut why = Vec::new();
        if a.handle.meta.file_metadata().num_rows() != 3000 {
            why.push("first open wrong".to_string());
        }
        if b.handle.meta.file_metadata().num_rows() != 4500 {
            why.push(format!("the replaced object was served with the OLD metadata ({} rows, the new object has 4500)", b.handle.meta.file_metadata().num_rows()));
        }
        res("replaced_object_never_served_stale", why, "after a replace the next open sees the new footer".into())
    });

    case!("lru_eviction_bounded_by_bytes_and_entries", async {
        let env = Env::new(m).await;
        for i in 0..6 {
            env.fake.put(&format!("f{i}.parquet"), lineitem_like(3000, 1000, i as u64));
        }
        // measure one entry's charge, then size a cache for exactly three
        let probe = MetaCache::adr(CacheMutants::default());
        let q = env.remote.query();
        let o = svc(&Some(probe.clone()), "s").open(&q, &Path::from("f0.parquet")).await.unwrap();
        let one = o.handle.charge();
        drop(o);
        let cap = one * 3 + one / 2;
        let cache = MetaCache::new(cap, 1024, 16 << 20, cm.clone());
        let s = svc(&Some(cache.clone()), "s");
        let mut why = Vec::new();
        for i in [0, 1, 2, 0, 3, 4, 5] {
            let q = env.remote.query();
            let _ = s.open(&q, &Path::from(format!("f{i}.parquet"))).await.unwrap(); // handle dropped at once: nothing stays pinned
            if cache.retained_bytes() > cap {
                why.push(format!("retained {} > cap {cap} after opening f{i}", cache.retained_bytes()));
            }
        }
        // f0 was touched again before f3 came in; f1 and f2 are the oldest: after 3,4,5 only the three newest remain
        let evictions = cache.stats.evictions.load(std::sync::atomic::Ordering::SeqCst);
        if cache.entries() > 3 {
            why.push(format!("{} entries for a cache sized for 3", cache.entries()));
        }
        if evictions < 3 {
            why.push(format!("only {evictions} evictions"));
        }
        res("lru_eviction_bounded_by_bytes_and_entries", why, format!("cap {cap} B (3 entries of ~{one} B): retained {} B, {} entries, {evictions} evictions", cache.retained_bytes(), cache.entries()))
    });

    case!("pinned_entries_stay_charged_and_force_a_bypass", async {
        let env = Env::new(m).await;
        for i in 0..4 {
            env.fake.put(&format!("f{i}.parquet"), lineitem_like(3000, 1000, i as u64));
        }
        let probe = MetaCache::adr(CacheMutants::default());
        let q = env.remote.query();
        let one = svc(&Some(probe), "s").open(&q, &Path::from("f0.parquet")).await.unwrap().handle.charge();
        let cache = MetaCache::new(one * 2 + one / 2, 1024, 16 << 20, cm.clone());
        let s = svc(&Some(cache.clone()), "s");
        let mut pinned = Vec::new();
        let mut outcomes = Vec::new();
        for i in 0..4 {
            let q = env.remote.query();
            let o = s.open(&q, &Path::from(format!("f{i}.parquet"))).await.unwrap();
            outcomes.push(o.outcome.unwrap());
            pinned.push(o.handle); // every handle stays alive: pinned
        }
        let live: u64 = pinned.iter().map(|h| h.charge()).sum();
        let mut why = Vec::new();
        if cache.retained_bytes() < live {
            why.push(format!("retained {} B but live handles hold {live} B: pinned bytes are not charged", cache.retained_bytes()));
        }
        if cache.retained_bytes() > cache.cap_bytes() {
            why.push(format!("retained {} B > cap {} B", cache.retained_bytes(), cache.cap_bytes()));
        }
        if !outcomes.contains(&Outcome::Bypass) {
            why.push("no entry was bypassed although every cached entry was pinned".into());
        }
        let before = cache.retained_bytes();
        drop(pinned);
        res("pinned_entries_stay_charged_and_force_a_bypass", why, format!("outcomes {outcomes:?}, retained {before} B <= cap {} B, then {} B after the handles dropped", cache.cap_bytes(), cache.retained_bytes()))
    });

    case!("over_entry_cap_is_bypassed_not_admitted", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = MetaCache::new(64 << 20, 1024, 1024, cm.clone()); // an entry cap of 1 KiB: every real footer is over it
        let s = svc(&Some(cache.clone()), "s");
        let q = env.remote.query();
        let o = s.open(&q, &Path::from("t.parquet")).await.unwrap();
        let mut why = Vec::new();
        if o.handle.meta.file_metadata().num_rows() != 3000 {
            why.push("query got wrong metadata".into());
        }
        if o.outcome != Some(Outcome::Bypass) || cache.entries() != 0 || cache.retained_bytes() != 0 {
            why.push(format!("outcome {:?}, {} entries, {} B retained: an over-cap entry was admitted", o.outcome, cache.entries(), cache.retained_bytes()));
        }
        res("over_entry_cap_is_bypassed_not_admitted", why, "bypassed, served uncached".into())
    });

    case!("failed_fill_is_never_published", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let s = svc(&cache, "s");
        let p = Path::from("t.parquet");
        let q = env.remote.query();
        let id = q.head(&p).await.unwrap();
        env.fake.set_plan(Plan::always(Fault::Status { code: 500, retry_after: None, body: 10, endless: false }));
        let r = s.open(&env.remote.query(), &p).await;
        let mut why = Vec::new();
        if r.is_ok() {
            why.push("open succeeded against a failing endpoint".into());
        }
        if cache.as_ref().unwrap().entries() != 0 {
            why.push("a failed fill left an entry".into());
        }
        env.fake.set_plan(Plan::default());
        let o = s.open(&env.remote.query(), &p).await.unwrap();
        if o.outcome != Some(Outcome::Miss) {
            why.push(format!("after the failure the next open was {:?}, expected a clean Miss", o.outcome));
        }
        let _ = id;
        res("failed_fill_is_never_published", why, "fill failed after 4 attempts, nothing cached, next open filled normally".into())
    });

    case!("credential_scope_is_part_of_the_key", async {
        let env = Env::new(m).await;
        env.fake.put("t.parquet", lineitem_like(3000, 1000, 1));
        let cache = Some(MetaCache::adr(cm.clone()));
        let p = Path::from("t.parquet");
        let a = svc(&cache, "scope-a").open(&env.remote.query(), &p).await.unwrap();
        let b = svc(&cache, "scope-b").open(&env.remote.query(), &p).await.unwrap();
        let mut why = Vec::new();
        if a.outcome != Some(Outcome::Miss) || b.outcome != Some(Outcome::Miss) {
            why.push(format!("scope-b saw {:?} after scope-a filled the entry: an entry was shared across credential scopes", b.outcome));
        }
        res("credential_scope_is_part_of_the_key", why, "two scopes, two entries".into())
    });
    v
}

pub async fn scan_cases(m: &Mutants) -> Vec<CaseResult> {
    let mut v = Vec::new();
    macro_rules! case {
        ($name:expr, $body:expr) => {
            v.push(guard($name, $body).await)
        };
    }
    let files: Vec<Vec<u8>> = (0..3).map(|i| lineitem_like(12_000, 2_000, 100 + i)).collect();
    let paths: Vec<Path> = (0..3).map(|i| Path::from(format!("li/part-{i}.parquet"))).collect();

    case!("scan_matches_the_oracle_all_policies_and_executors", async {
        let env = Env::new(m).await;
        for (i, f) in files.iter().enumerate() {
            env.fake.put(&format!("li/part-{i}.parquet"), f.clone());
        }
        let mut why = Vec::new();
        let mut n = 0;
        let mut waste = String::new();
        for kind in [QueryKind::Full, QueryKind::Proj, QueryKind::Selective { lo: 30, hi: 60 }, QueryKind::Meta] {
            let want = oracle(&files, &kind);
            for (pn, policy) in [("unmerged", PolicyKind::Unmerged), ("merged", PolicyKind::Merged { rg_per_fetch: 1 }), ("merged-x4", PolicyKind::Merged { rg_per_fetch: 4 })] {
                for ek in [ExecKind::Split, ExecKind::TokioCpu] {
                    let exec = Exec::new(ek, 3);
                    let q = env.remote.query();
                    let sc = ScanCfg { policy: policy.clone(), exec: exec.clone(), files_ahead: 2, units_ahead: 6 };
                    let s = MetaService::new(None, "s3", "s");
                    match scan(&q, &s, &paths, &kind, &sc).await {
                        Ok(o) => {
                            n += 1;
                            if o.text != want {
                                why.push(format!("{} {pn} {ek:?}: got {} want {want}", kind.name(), o.text));
                            }
                            if kind == QueryKind::Full && ek == ExecKind::Split {
                                waste += &format!("{pn}: needed {} fetched {} ({}%); ", o.needed_bytes, o.fetched_bytes, (o.fetched_bytes * 100).checked_div(o.needed_bytes).unwrap_or(0));
                            }
                        }
                        Err(e) => why.push(format!("{} {pn} {ek:?}: {e}", kind.name())),
                    }
                    exec.shutdown();
                }
            }
        }
        res("scan_matches_the_oracle_all_policies_and_executors", why, format!("{n} scans equal the oracle; Full bytes: {waste}"))
    });

    case!("version_change_mid_scan_fails_never_mixes_versions", async {
        let env = Env::new(m).await;
        for (i, f) in files.iter().enumerate() {
            env.fake.put(&format!("li/part-{i}.parquet"), f.clone());
        }
        // the SAME length (same row groups), other rows: a scan that mixed versions would produce a number matching neither
        let swapped = lineitem_like(12_000, 2_000, 999);
        let want_a = oracle(&files, &QueryKind::Proj);
        // swap the second file before its 3rd, 4th and 6th GET (its footer is the first, so its identity is pinned and its footer read before the swap)
        let mut why = Vec::new();
        let mut outcomes = Vec::new();
        for swap_at in [2usize, 3, 5] {
            env.fake.put("li/part-1.parquet", files[1].clone());
            env.fake.set_plan(Plan { swap_before_get: vec![(swap_at, "li/part-1.parquet".into(), Arc::new(swapped.clone()))], ..Default::default() });
            let exec = Exec::new(ExecKind::Split, 2);
            let q = env.remote.query();
            let sc = ScanCfg { policy: PolicyKind::Merged { rg_per_fetch: 1 }, exec: exec.clone(), files_ahead: 3, units_ahead: 4 };
            let r = scan(&q, &MetaService::new(None, "s3", "s"), &paths, &QueryKind::Proj, &sc).await;
            exec.shutdown();
            match r {
                Err(e) if e.class == Class::IdentityChanged => outcomes.push(format!("swap@{swap_at}: failed IdentityChanged")),
                Err(e) => outcomes.push(format!("swap@{swap_at}: failed {:?}", e.class)),
                Ok(o) if o.text == want_a => why.push(format!("swap@{swap_at}: the scan succeeded with version A's result although the object changed under it (a read must have been skipped)")),
                Ok(o) => why.push(format!("swap@{swap_at}: SUCCEEDED with {} which is not version A's {want_a}: rows of two versions mixed", o.text)),
            }
        }
        res("version_change_mid_scan_fails_never_mixes_versions", why, outcomes.join("; "))
    });
    v
}
