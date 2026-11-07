//! The reliability suites as cargo tests (the same cases `osb faults` / `osb cache` run; `osb mutants` shows each fails on a known-bad version).
use spike_object_store_baseline::{cache::CacheMutants, cachecases, faultcases, policy::Mutants};

fn assert_all(res: Vec<faultcases::CaseResult>) {
    let bad: Vec<_> = res.iter().filter(|c| !c.ok).map(|c| format!("{}: {}", c.name, c.detail)).collect();
    assert!(bad.is_empty(), "failed cases:\n{}", bad.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fault_cases_hold_on_the_real_wrapper() {
    assert_all(faultcases::all_cases(&Mutants::default()).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cache_and_scan_cases_hold() {
    let mut r = cachecases::cache_cases(&Mutants::default(), &CacheMutants::default()).await;
    r.extend(cachecases::scan_cases(&Mutants::default()).await);
    assert_all(r);
}
