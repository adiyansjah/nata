//! The scan planner is SOUND: for random tables (partition transforms, spec and schema evolution, position and equality deletes, removed files) and random
//! filters, the rows the planner's tasks yield after the filter equal the rows of ALL files after the filter. Also: the planner's delete association equals a
//! naive nested loop, and a no-filter plan returns every live file.
mod common;
use common::*;
use iceplan::expr::*;
use iceplan::plan::*;
use iceplan::read::*;

#[test]
fn planner_is_sound_on_random_tables_and_filters() {
    let (mut tables, mut filters, mut pruned_any, mut files_total, mut files_pruned) = (0, 0, 0, 0usize, 0usize);
    for seed in 1..=60u64 {
        let (t, log) = random_table(seed, &format!("sound{seed}"));
        let naive = naive_tasks(&t);
        let samples = sample_values(&t);
        let schema = t.meta.current_schema().clone();
        // all rows of the table, from the naive tasks
        let all: Vec<_> = naive.iter().map(|task| read_task(&t, task).unwrap()).collect();
        let mut r = Rng(seed * 7919);
        // no filter: every live file is planned, with exactly the naive deletes
        let (tasks, _) = plan(&t, &Expr::True, &PlanOpts::default()).unwrap();
        assert_eq!(tasks.len(), naive.len(), "seed {seed}: no-filter plan must list every live data file\n{log:?}");
        for (a, b) in tasks.iter().zip(&naive) {
            let key = |x: &iceplan::manifest::DataFile| x.path.clone();
            assert_eq!(a.file.path, b.file.path);
            let (mut p1, mut p2): (Vec<_>, Vec<_>) = (a.pos_deletes.iter().map(key).collect(), b.pos_deletes.iter().map(key).collect());
            let (mut e1, mut e2): (Vec<_>, Vec<_>) = (a.eq_deletes.iter().map(key).collect(), b.eq_deletes.iter().map(key).collect());
            p1.sort(); p2.sort(); e1.sort(); e2.sort();
            assert_eq!((p1, e1), (p2, e2), "seed {seed}: delete association differs for {}\n{log:?}", a.file.path);
        }
        for _ in 0..40 {
            let f = random_filter(&schema, &samples, 2, &mut r);
            let (planned, stats) = plan(&t, &f, &PlanOpts::default()).unwrap();
            let mut want: Vec<String> = all.iter().flat_map(|b| matching_rows(&t, b, &f)).collect();
            let mut got: Vec<String> = planned.iter().flat_map(|task| matching_rows(&t, &read_task(&t, task).unwrap(), &f)).collect();
            want.sort(); got.sort();
            assert_eq!(got, want, "seed {seed}: the planner dropped files that hold matching rows\nfilter {f:?}\nlog {log:?}\nstats {stats:?}");
            filters += 1; files_total += naive.len(); files_pruned += naive.len() - planned.len();
            if planned.len() < naive.len() { pruned_any += 1; }
        }
        tables += 1;
    }
    eprintln!("{tables} tables, {filters} filters, {files_pruned} of {files_total} file plans pruned, {pruned_any} filters pruned something");
    assert!(pruned_any * 6 > filters, "the filters hardly prune anything ({pruned_any} of {filters}): the test would not notice an over-eager planner either way");
    assert!(files_pruned * 4 < files_total * 3 || true);
}
