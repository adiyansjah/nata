mod common;
use arrow_array::{ArrayRef, Decimal128Array, Int32Array, RecordBatch};
use common::*;
use nata_exec::{ExecConfig, Observed, execute_observed};
use nata_parquet::ScanCancel;
use nata_sql::{FsSource, plan_sql};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn cfg(base: &std::path::Path, threads: usize) -> ExecConfig {
    let mut cfg = ExecConfig::new(threads);
    cfg.aggregate_memory_bytes = Some(16 << 20);
    cfg.aggregate_spill_directory = Some(base.to_path_buf());
    cfg.aggregate_force_spill = true;
    cfg
}
fn rows(text: String) -> Vec<String> {
    let mut rows: Vec<_> = text.lines().skip(1).map(str::to_owned).collect();
    rows.sort();
    rows
}
fn empty(base: &std::path::Path) {
    assert_eq!(std::fs::read_dir(base).expect("spill base").count(), 0);
}

#[test]
fn typed_sql_replay_matches_resident_for_every_key_and_state_type() {
    let d = dir("spill-typed");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    standard(&p);
    for threads in [1, 4] {
        for key in ["a", "ti", "id", "b", "price", "d", "s", "dt", "f"] {
            let sql = format!(
                "SELECT {key}, count(*), count({key}), min({key}), max({key}), sum(id), avg(price) FROM '{}' GROUP BY {key}",
                p.display()
            );
            let resident = run_with(&sql, &ExecConfig::new(threads))
                .csv
                .expect("resident");
            let spilled = run_with(&sql, &cfg(&base, threads)).csv.expect("spill");
            assert_eq!(rows(spilled), rows(resident), "{key}, {threads}");
            empty(&base);
        }
        let q = format!(
            "SELECT a, sum(ti), avg(ti), sum(id), avg(id), sum(b), avg(b), sum(price), avg(price) FROM '{}' GROUP BY a HAVING count(*) > 1",
            p.display()
        );
        assert_eq!(
            rows(run_with(&q, &cfg(&base, threads)).csv.expect("spill")),
            rows(
                run_with(&q, &ExecConfig::new(threads))
                    .csv
                    .expect("resident")
            )
        );
    }
    empty(&base);
}
#[test]
fn wide_partial_sums_round_trip_without_narrowing_before_merge() {
    let d = dir("spill-wide");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    let max = 10i128.pow(38) - 1;
    let x: ArrayRef = Arc::new(
        Decimal128Array::from(vec![Some(max), Some(max), None, Some(-max), Some(-max)])
            .with_precision_and_scale(38, 0)
            .expect("decimal"),
    );
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("x", x)]).expect("batch"),
        1,
    );
    let q = format!(
        "SELECT sum(x) AS s, avg(x) AS a, count(x) AS n FROM '{}'",
        p.display()
    );
    assert_eq!(
        run_with(&q, &cfg(&base, 4)).csv.expect("wide spill"),
        "s,a,n\n0,0,4\n"
    );
    empty(&base);
}
#[test]
fn pressure_releases_reservations_and_recursive_merge_reads_every_partition() {
    let d = dir("spill-pressure");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    let id: ArrayRef = Arc::new(Int32Array::from_iter_values(
        (0..40_000).map(|i| i % 20_000),
    ));
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("id", id)]).expect("batch"),
        1024,
    );
    let q = format!(
        "SELECT id, count(*) AS n, sum(id) AS s FROM '{}' GROUP BY id HAVING count(*) = 2",
        p.display()
    );
    let mut config = cfg(&base, 4);
    config.aggregate_force_spill = false;
    config.aggregate_memory_bytes = Some(8 << 20);
    config.aggregate_spill_disk_bytes = Some(128 << 20);
    config.aggregate_merge_partition_bytes = Some(128 << 10);
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    let mut observed = Observed::default();
    let mut count = 0;
    let report = execute_observed(plan, &config, None, &mut observed, |b| {
        count += b.num_rows();
        Ok(())
    })
    .expect("pressure");
    assert_eq!(count, 20_000);
    let m = report.aggregate.expect("aggregate diagnostics");
    assert!(
        m.spill_bytes_written > 0
            && m.spill_bytes_read > 0
            && m.partitions_spilled > 0
            && m.spill_files_opened > 0
            && m.recursion_depth > 0,
        "{m:?}"
    );
    let memory = observed.aggregate_memory.expect("memory").snapshot();
    assert_eq!((memory.held, memory.live_grants), (0, 0));
    assert!(memory.peak <= 8 << 20);
    empty(&base);
}
#[test]
fn injected_write_failure_returns_error_and_removes_every_file() {
    let d = dir("spill-write-failure");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    standard(&p);
    let q = format!("SELECT id, count(*) FROM '{}' GROUP BY id", p.display());
    let mut config = cfg(&base, 4);
    config.aggregate_spill_fail_after = Some(9);
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    let mut observed = Observed::default();
    let error = execute_observed(plan, &config, None, &mut observed, |_| Ok(()))
        .expect_err("injected ENOSPC");
    assert!(error.to_string().contains("ENOSPC"), "{error}");
    assert_eq!(
        observed.aggregate_memory.expect("memory").snapshot().held,
        0
    );
    empty(&base);
}
#[test]
fn cancel_during_spill_joins_io_and_removes_every_file() {
    let d = dir("spill-cancel");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    let id: ArrayRef = Arc::new(Int32Array::from_iter_values(0..200_000));
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("id", id)]).expect("batch"),
        4096,
    );
    let q = format!("SELECT id, count(*) FROM '{}' GROUP BY id", p.display());
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    let cancel = ScanCancel::new();
    let mut observed = Observed::default();
    std::thread::scope(|scope| {
        let watcher_cancel = cancel.clone();
        let base = &base;
        scope.spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(15) {
                let written = std::fs::read_dir(base)
                    .expect("base")
                    .flatten()
                    .any(|entry| {
                        std::fs::read_dir(entry.path())
                            .ok()
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|f| f.metadata().is_ok_and(|m| m.len() > 0))
                    });
                if written {
                    watcher_cancel.cancel();
                    return;
                }
                std::thread::yield_now();
            }
            panic!("no spill within deadline");
        });
        assert!(
            execute_observed(
                plan,
                &cfg(base, 4),
                Some(&cancel),
                &mut observed,
                |_| Ok(())
            )
            .is_err()
        );
    });
    assert_eq!(
        observed.aggregate_memory.expect("memory").snapshot().held,
        0
    );
    empty(&base);
}
#[test]
fn recursion_bound_is_enforced_under_a_process_deadline() {
    const CHILD: &str = "NATA_TEST_DEEP_SPILL_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let d = dir("spill-depth");
        let base = d.join("spill");
        std::fs::create_dir(&base).expect("base");
        let p = d.join("t.parquet");
        standard(&p);
        let q = format!("SELECT id, count(*) FROM '{}' GROUP BY id", p.display());
        let mut config = cfg(&base, 1);
        config.aggregate_spill_force_collisions = true;
        config.aggregate_merge_partition_bytes = Some(1);
        let error = run_with(&q, &config).csv.expect_err("depth bound");
        assert!(error.contains("recursion bound (8) exceeded"), "{error}");
        empty(&base);
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "recursion_bound_is_enforced_under_a_process_deadline",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .spawn()
        .expect("child");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            assert!(status.success(), "deep spill child failed");
            break;
        }
        if start.elapsed() > Duration::from_secs(15) {
            child.kill().expect("deadline kill");
            let _ = child.wait();
            panic!("recursion bound deadline exceeded");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn real_smallint_and_float32_keys_and_null_extremes_use_the_typed_spill_path() {
    use arrow_array::{Float32Array, Int16Array};
    let d = dir("spill-real-small");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    let small: ArrayRef = Arc::new(Int16Array::from(vec![
        None,
        Some(i16::MIN),
        Some(i16::MAX),
        None,
        Some(i16::MAX),
    ]));
    let real: ArrayRef = Arc::new(Float32Array::from(vec![
        Some(-0.0),
        Some(0.0),
        Some(f32::NAN),
        Some(f32::NAN),
        None,
    ]));
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("s", small), ("r", real)]).expect("batch"),
        1,
    );
    for key in ["s", "r"] {
        let q = format!(
            "SELECT {key}, count({key}), min({key}), max({key}), sum(s), avg(s) FROM '{}' GROUP BY {key}",
            p.display()
        );
        assert_eq!(
            rows(run_with(&q, &cfg(&base, 4)).csv.expect("spill")),
            rows(run_with(&q, &ExecConfig::new(4)).csv.expect("resident"))
        );
    }
    empty(&base);
}

#[test]
fn disk_quota_error_is_clear_and_query_cleanup_is_complete() {
    let d = dir("spill-quota");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    standard(&p);
    let q = format!("SELECT id, count(*) FROM '{}' GROUP BY id", p.display());
    let mut config = cfg(&base, 2);
    config.aggregate_spill_disk_bytes = Some(9);
    let error = run_with(&q, &config).csv.expect_err("quota");
    assert!(error.contains("spill disk quota exceeded"), "{error}");
    empty(&base);
}

#[test]
fn a_hot_string_group_coalesces_without_recursive_repartition() {
    use arrow_array::StringArray;
    let d = dir("spill-hot-string");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    let p = d.join("t.parquet");
    let text = "x".repeat(400);
    let strings: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..10_000).map(|_| text.as_str()),
    ));
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("s", strings)]).expect("batch"),
        256,
    );
    let q = format!(
        "SELECT s, count(*) AS n, min(CASE WHEN true THEN s ELSE s END) AS lo, max(s) AS hi FROM '{}' GROUP BY s",
        p.display()
    );
    let mut config = cfg(&base, 1);
    config.aggregate_merge_partition_bytes = Some(1);
    let out = run_with(&q, &config);
    assert_eq!(
        out.csv.expect("hot string"),
        format!("s,n,lo,hi\n{text},10000,{text},{text}\n")
    );
    assert_eq!(
        out.report
            .expect("report")
            .aggregate
            .expect("aggregate")
            .recursion_depth,
        0
    );
    empty(&base);
}

#[test]
fn an_earlier_argument_error_wins_over_a_later_file_schema_error() {
    let d = dir("spill-lowest-source");
    let base = d.join("spill");
    std::fs::create_dir(&base).expect("base");
    standard(&d.join("a.parquet"));
    let id: ArrayRef = Arc::new(arrow_array::Int64Array::from(vec![1]));
    write_batch(
        &d.join("b.parquet"),
        &RecordBatch::try_from_iter([("id", id)]).expect("batch"),
        1,
    );
    let q = format!("SELECT sum(10 // a) FROM '{}/*.parquet'", d.display());
    let error = run_with(&q, &cfg(&base, 4))
        .csv
        .expect_err("lowest source error");
    assert!(
        error.contains("division by zero") && error.contains("a.parquet"),
        "{error}"
    );
    empty(&base);
}
