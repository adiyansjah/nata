mod common;
use arrow_array::{ArrayRef, Decimal128Array, Int32Array, RecordBatch};
use common::*;
use nata_exec::{ExecConfig, Observed, execute_observed};
use nata_sql::{FsSource, plan_sql};
use std::sync::Arc;

fn fixture(name: &str) -> String {
    let d = dir(name);
    standard(&d.join("t.parquet"));
    d.join("t.parquet").display().to_string()
}
fn sorted(csv: String) -> Vec<String> {
    let mut rows: Vec<_> = csv.lines().skip(1).map(str::to_string).collect();
    rows.sort();
    rows
}
#[test]
fn partials_merge_once_and_having_sees_the_whole_group() {
    let p = fixture("aggregate-merge");
    let q = format!(
        "SELECT id % 2 AS k, count(*) AS n, sum(id) AS s, avg(id) AS a FROM '{p}' GROUP BY id % 2 HAVING count(*) > 6"
    );
    for threads in [1, 2, 8] {
        let rows = sorted(
            run_with(&q, &ExecConfig::new(threads))
                .csv
                .expect("aggregate"),
        );
        assert_eq!(rows, ["0,10,90,9.000000", "1,10,100,10.000000"]);
    }
}
#[test]
fn nulls_empty_input_and_selected_arguments() {
    let p = fixture("aggregate-null");
    for threads in [1, 4] {
        let cfg = ExecConfig::new(threads);
        let out=run_with(&format!("SELECT count(*) AS n, count(a) AS c, sum(id) AS s, min(s) AS lo, max(id) AS hi FROM '{p}'"),&cfg).csv.expect("global");
        assert_eq!(out, "n,c,s,lo,hi\n20,17,190,Beta,19\n");
        assert_eq!(
            run_with(
                &format!("SELECT count(*) AS n, sum(id) AS s, avg(id) AS a FROM '{p}' WHERE false"),
                &cfg
            )
            .csv
            .expect("empty"),
            "n,s,a\n0,NULL,NULL\n"
        );
        assert_eq!(
            run_with(
                &format!("SELECT id, count(*) AS n FROM '{p}' WHERE false GROUP BY id"),
                &cfg
            )
            .csv
            .expect("empty"),
            "id,n\n"
        );
        let q = format!("SELECT count(*) AS n, sum(10 // id) AS s FROM '{p}' WHERE id > 0");
        assert!(run_with(&q, &cfg).csv.is_ok());
        let q = format!("SELECT sum(10 // id) AS s FROM '{p}' HAVING false LIMIT 0");
        assert!(
            run_with(&q, &cfg)
                .csv
                .expect_err("all args evaluated")
                .contains("division by zero")
        );
        let q = format!("SELECT a, count(*) AS n FROM '{p}' GROUP BY a HAVING a IS NULL");
        assert_eq!(
            sorted(run_with(&q, &cfg).csv.expect("NULL key")),
            ["NULL,3"]
        );
    }
}
#[test]
fn wide_cancellation_final_overflow_and_average_without_narrowing_sum() {
    let d = dir("aggregate-wide");
    let p = d.join("t.parquet");
    let max = 10i128.pow(38) - 1;
    let arr: ArrayRef = Arc::new(
        Decimal128Array::from(vec![max, max, -max, -max])
            .with_precision_and_scale(38, 0)
            .expect("decimal"),
    );
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("x", arr)]).expect("batch"),
        1,
    );
    for t in [1, 4] {
        assert_eq!(
            run_with(
                &format!("SELECT sum(x) AS s FROM '{}'", p.display()),
                &ExecConfig::new(t)
            )
            .csv
            .expect("cancels"),
            "s\n0\n"
        );
    }
    let arr: ArrayRef = Arc::new(
        Decimal128Array::from(vec![max, max])
            .with_precision_and_scale(38, 0)
            .expect("decimal"),
    );
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("x", arr)]).expect("batch"),
        1,
    );
    assert!(
        run_with(
            &format!(
                "SELECT sum(x) AS s FROM '{}' HAVING false LIMIT 0",
                p.display()
            ),
            &ExecConfig::new(4)
        )
        .csv
        .expect_err("overflow before HAVING")
        .contains("overflow")
    );
    assert_eq!(
        run_with(
            &format!("SELECT avg(x) AS a FROM '{}'", p.display()),
            &ExecConfig::new(4)
        )
        .csv
        .expect("AVG"),
        format!("a\n{max}\n")
    );
}
#[test]
fn error_budget_cancel_and_sink_failure_release_all_state() {
    let p = fixture("aggregate-cleanup");
    let q = format!("SELECT id, count(*) AS n FROM '{p}' GROUP BY id");
    for fail in [false, true] {
        let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
        let mut cfg = ExecConfig::new(4);
        if fail {
            cfg.aggregate_memory_bytes = Some(8192);
        }
        let mut observed = Observed::default();
        let r = execute_observed(plan, &cfg, None, &mut observed, |_| {
            Err(nata_parquet::ScanError::usage("sink gone"))
        });
        let error = r.expect_err("failure");
        if fail {
            match &error.kind {
                nata_parquet::ErrorKind::Eval {
                    message,
                    line,
                    column,
                } => {
                    assert!(*line > 0 && *column > 0, "positioned memory limit");
                    assert!(message.contains("minimum working set"));
                    assert!(message.contains("8192 bytes"));
                    assert!(message.contains("2101248 bytes"));
                }
                other => panic!("expected resource error, got {other:?}"),
            }
        }
        let e = error.to_string();
        assert!(
            e.contains(if fail { "resource error" } else { "sink gone" }),
            "{e}"
        );
        let m = observed
            .aggregate_memory
            .expect("memory service")
            .snapshot();
        assert_eq!(m.held, 0);
        assert_eq!(m.live_grants, 0);
        let s = observed.stats.expect("scan");
        assert_eq!(s.handles.now(), 0);
        assert_eq!(s.memory.now(), 0);
    }
    let mut observed = Observed::default();
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    execute_observed(plan, &ExecConfig::new(4), None, &mut observed, |_| Ok(())).expect("success");
    let memory = observed.aggregate_memory.expect("service").snapshot();
    assert_eq!(memory.held, 0);
    assert_eq!(memory.live_grants, 0);
    let cancel = nata_parquet::ScanCancel::new();
    let mut observed = Observed::default();
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    assert!(
        execute_observed(
            plan,
            &ExecConfig::new(4),
            Some(&cancel),
            &mut observed,
            |_| {
                cancel.cancel();
                Ok(())
            }
        )
        .is_err()
    );
    assert_eq!(
        observed.aggregate_memory.expect("service").snapshot().held,
        0
    );
    let cancel = nata_parquet::ScanCancel::new();
    cancel.cancel();
    let plan = Arc::new(plan_sql(&q, &FsSource).expect("plan").plan);
    assert!(nata_exec::execute(plan, &ExecConfig::new(4), Some(&cancel), |_| Ok(())).is_err());
}
#[test]
fn high_cardinality_limit_and_having_project_only_admitted_groups() {
    let d = dir("aggregate-cardinality");
    let p = d.join("t.parquet");
    let arr: ArrayRef = Arc::new(Int32Array::from_iter_values(0..10_000));
    write_batch(
        &p,
        &RecordBatch::try_from_iter([("id", arr)]).expect("batch"),
        128,
    );
    let q = format!(
        "SELECT id, count(*) AS n FROM '{}' GROUP BY id",
        p.display()
    );
    assert_eq!(
        sorted(run_with(&q, &ExecConfig::new(8)).csv.expect("many keys")).len(),
        10_000
    );
    let q = format!(
        "SELECT 1 // (count(*) - 1) AS v FROM '{}' GROUP BY id HAVING false",
        p.display()
    );
    assert_eq!(
        run_with(&q, &ExecConfig::new(4))
            .csv
            .expect("HAVING before projection"),
        "v\n"
    );
    let q = format!(
        "SELECT count(*) AS n FROM '{}' GROUP BY id LIMIT 3 OFFSET 7",
        p.display()
    );
    assert_eq!(
        sorted(run_with(&q, &ExecConfig::new(4)).csv.expect("limit")).len(),
        3
    );
}

#[test]
fn deep_aggregate_having_and_projection_use_sized_stacks() {
    let p = fixture("aggregate-deep");
    let mut value = "count(*)".to_string();
    let mut condition = "count(*) > 0".to_string();
    for _ in 0..999 {
        value = format!("CASE WHEN true THEN {value} ELSE 0 END");
        condition = format!("CASE WHEN true THEN {condition} ELSE false END");
    }
    let q = format!("SELECT {value} AS n FROM '{p}' HAVING {condition}");
    assert_eq!(
        run_with(&q, &ExecConfig::new(2))
            .csv
            .expect("deep aggregate"),
        "n\n20\n"
    );
}

#[test]
fn output_batches_gather_having_survivors_and_apply_offset_limit() {
    use arrow_array::cast::AsArray;
    let d = dir("aggregate-output-batches");
    let p = d.join("t.parquet");
    write_batch(
        &p,
        &RecordBatch::try_from_iter([(
            "id",
            Arc::new(Int32Array::from_iter_values(0..1024)) as ArrayRef,
        )])
        .expect("batch"),
        128,
    );
    for force in [false, true] {
        for limited in [false, true] {
            let suffix = if limited { " LIMIT 91 OFFSET 37" } else { "" };
            let q = format!(
                "SELECT id, 1024 // (id + 1) AS v FROM '{}' GROUP BY id HAVING id % 3 = 0{suffix}",
                p.display()
            );
            let planned = plan_sql(&q, &FsSource).expect("plan");
            let mut cfg = ExecConfig::new(4);
            cfg.aggregate_force_spill = force;
            cfg.aggregate_memory_bytes = Some(64 << 20);
            let mut rows = Vec::new();
            let mut calls = 0;
            let report = nata_exec::execute(Arc::new(planned.plan), &cfg, None, |b| {
                calls += 1;
                assert!((1..=32).contains(&b.num_rows()), "bounded construction");
                let keys = b.column(0).as_primitive::<arrow_array::types::Int32Type>();
                let values = b.column(1).as_primitive::<arrow_array::types::Int32Type>();
                for i in 0..b.num_rows() {
                    let key = keys.value(i);
                    assert_eq!(key % 3, 0);
                    assert_eq!(values.value(i), 1024 / (key + 1));
                    rows.push(key);
                }
                Ok(())
            })
            .expect("execute");
            let expected = if limited { 91 } else { 342 };
            assert_eq!(rows.len(), expected);
            assert!(calls < expected / 2, "sink must receive batches: {calls}");
            rows.sort_unstable();
            rows.dedup();
            assert_eq!(rows.len(), expected, "each group once");
            if !limited {
                assert_eq!(rows, (0..1024).step_by(3).collect::<Vec<_>>());
            }
            let stats = report.aggregate.expect("stats");
            assert_eq!(stats.sink_calls, calls as u64);
            assert!(stats.output_nanos > 0);
        }
    }
}

#[test]
fn dictionary_values_change_across_row_groups_and_files() {
    use arrow_array::{Int64Array, StringArray};
    let d = dir("aggregate-dictionary-values");
    for (file, reversed) in [("a.parquet", false), ("b.parquet", true)] {
        let keys: Vec<Option<&str>> = (0..128)
            .map(|i| match (i + usize::from(reversed)) % 4 {
                0 => Some("雪"),
                1 => Some(""),
                2 => None,
                _ => Some("other"),
            })
            .collect();
        let other: Vec<_> = (0..128)
            .map(|i| if i % 2 == 0 { "A" } else { "B" })
            .collect();
        let b = RecordBatch::try_from_iter([
            ("k", Arc::new(StringArray::from(keys)) as ArrayRef),
            ("l", Arc::new(StringArray::from(other)) as ArrayRef),
            ("v", Arc::new(Int64Array::from(vec![1; 128])) as ArrayRef),
        ])
        .expect("dictionary fixture");
        write_batch(&d.join(file), &b, 32);
    }
    let q = format!(
        "SELECT k,l,sum(v) AS s,count(*) AS n FROM '{}/*.parquet' GROUP BY k,l",
        d.display()
    );
    let expected = [
        "\"雪\",A,32,32",
        "\"雪\",B,32,32",
        ",A,32,32",
        ",B,32,32",
        "NULL,A,32,32",
        "NULL,B,32,32",
        "other,A,32,32",
        "other,B,32,32",
    ];
    for threads in [1, 4] {
        for spill in [false, true] {
            let mut cfg = ExecConfig::new(threads);
            cfg.aggregate_force_spill = spill;
            for collisions in [false, true] {
                cfg.aggregate_spill_force_collisions = collisions;
                assert_eq!(
                    sorted(run_with(&q, &cfg).csv.expect("dictionary fixture")),
                    expected
                );
            }
        }
    }
}
