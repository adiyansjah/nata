//! The reliability suites as cargo tests: every fault case, cache case and (in child processes under `ulimit -v`) every hostile-allocation case must pass
//! on the real wrapper. `src/bin/sweep.rs` (feature `mutants`) shows each of them fails on a known-bad version.

#[path = "../cases/mod.rs"]
mod cases;

use cases::{cache, fault};
use nata_objstore::policy::{CacheMutants, Mutants};

fn assert_all(res: Vec<fault::CaseResult>) {
    let bad: Vec<_> = res
        .iter()
        .filter(|c| !c.ok)
        .map(|c| format!("{}: {}", c.name, c.detail))
        .collect();
    assert!(bad.is_empty(), "failed cases:\n{}", bad.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fault_cases_hold_on_the_real_wrapper() {
    let res = fault::all_cases(&Mutants::default()).await;
    assert!(
        res.len() >= 30,
        "the 29 spike cases and the grant case are all run: {}",
        res.len()
    );
    assert_all(res);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cache_cases_hold() {
    let res = cache::cache_cases(&Mutants::default(), &CacheMutants::default()).await;
    assert_eq!(res.len(), 7);
    assert_all(res);
}

/// Hostile bodies in a child process under `ulimit -v 1536 MiB`: a missing bound kills the child instead of the test run.
#[test]
fn hostile_cases_hold_under_an_address_space_limit() {
    // the child is this crate's own test binary running one case through `NATA_OBJSTORE_HOSTILE`
    let exe = std::env::current_exe().expect("test binary");
    for name in fault::HOSTILE_CASES {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(
                "ulimit -v 1572864; exec \"$0\" --exact hostile_child --nocapture --test-threads 1",
            )
            .arg(&exe)
            .env("NATA_OBJSTORE_HOSTILE", name)
            .env("MALLOC_ARENA_MAX", "2")
            .output()
            .expect("sh");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && text.contains(&format!("CASE {name}: PASS")),
            "{name}: status {:?}\n{text}\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Runs one hostile case when the parent test asks for it (the environment variable); a no-op otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hostile_child() {
    let Ok(name) = std::env::var("NATA_OBJSTORE_HOSTILE") else {
        return;
    };
    let c = fault::hostile(&name, &Mutants::default()).await;
    println!(
        "CASE {}: {} {}",
        c.name,
        if c.ok { "PASS" } else { "FAIL" },
        c.detail
    );
    assert!(c.ok);
}
