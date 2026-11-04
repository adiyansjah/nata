//! Guarded expressions (ADR 0003, decision 4): `WHERE x <> 0 AND 1 / x > 1` and `CASE WHEN x <> 0 THEN 1 / x`.
//! An error in a row the guard removed, or in a NULL slot, is not an error; an error in a surviving row is.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test code on small values"
)]
mod guard_cases;

use arrow_array::{Array, Int64Array};
use arrow_schema::ArrowError;
use guard_cases::{Cat, EXECS, Exec, cat_arrow, queries, reference_outcome, tables};
use nata_kernels::arrow_path::{div_compacted, div_folded, div_uncompacted, to_array};
use nata_kernels::kernels::{Valid, filter_ne};

fn table(name: &str) -> guard_cases::Table {
    tables()
        .into_iter()
        .find(|t| t.name.starts_with(name))
        .expect("table exists")
}

fn query(name: &str) -> guard_cases::Query {
    queries()
        .into_iter()
        .find(|q| q.name == name)
        .expect("query exists")
}

fn run(table_name: &str, query_name: &str, ex: Exec) -> Result<guard_cases::Rows, Cat> {
    (query(query_name).run)(&table(table_name), ex)
}

#[test]
fn every_executor_matches_the_row_by_row_reference_on_every_table_and_query() {
    let mut combos = 0;
    let (mut oks, mut errs) = (0, 0);
    for t in tables() {
        for q in queries() {
            let want = reference_outcome(&q, &t);
            match want {
                Ok(_) => oks += 1,
                Err(_) => errs += 1,
            }
            for ex in EXECS {
                assert_eq!(
                    (q.run)(&t, ex),
                    want,
                    "table {} query {} executor {ex:?}",
                    t.name,
                    q.name
                );
                combos += 1;
            }
        }
    }
    assert_eq!(combos, 3 * 9 * 3, "3 tables x 9 queries x 3 executors");
    assert!(
        oks >= 8 && errs >= 6,
        "the crafted data must reach both outcomes: {oks} ok, {errs} error"
    );
}

// Hand-derived expectations (independent of the reference above).

#[test]
fn the_guard_removes_the_zero_divisor_rows_so_there_is_no_error() {
    // t1 rows with b <> 0 and a // b > 1: row 0 (10 // 2 = 5) and row 9 (100 // 3 = 33).
    for ex in EXECS {
        assert_eq!(
            run("t1", "guarded_filter", ex),
            Ok(vec![(0, Some(5)), (9, Some(33))]),
            "{ex:?}"
        );
    }
}

#[test]
fn without_the_guard_a_surviving_zero_divisor_is_an_error() {
    for ex in EXECS {
        assert_eq!(
            run("t1", "unguarded_filter", ex),
            Err(Cat::DivZero),
            "{ex:?}"
        );
    }
}

#[test]
fn the_guard_does_not_protect_a_surviving_min_over_minus_one() {
    // t2 row 0 is (MIN, -1): b <> 0 holds, so the row survives the guard and the division overflows.
    for ex in EXECS {
        assert_eq!(
            run("t2", "guarded_filter", ex),
            Err(Cat::Overflow),
            "{ex:?}"
        );
    }
}

#[test]
fn failures_in_null_slots_are_not_errors() {
    // t3 has (NULL, 0), (5, NULL), (NULL, NULL) and (NULL, -1): every failing division sits in a NULL slot.
    // Rows 1-4 give NULL, and NULL > 1 drops them; (MIN, 1), (-9, 3) and (0, 4) do not exceed 1.
    for ex in EXECS {
        assert_eq!(
            run("t3", "unguarded_filter", ex),
            Ok(vec![(0, Some(5))]),
            "{ex:?}"
        );
    }
    // the same table through CASE: the zero divisor rows take ELSE, (NULL, -1) gives NULL, and nothing fails
    for ex in EXECS {
        let rows = run("t3", "case_when_b_ne_0", ex).expect("no valid row fails");
        assert_eq!(
            rows,
            vec![
                (0, Some(5)),
                (1, Some(0)),
                (2, Some(0)),
                (3, Some(0)),
                (4, None),
                (5, Some(i64::MIN)),
                (6, Some(-3)),
                (7, Some(0))
            ],
            "{ex:?}"
        );
    }
}

#[test]
fn case_as_selection_splitting_gives_the_else_value_to_guarded_rows() {
    // t1 CASE WHEN b <> 0 THEN a // b ELSE 0 END: zero divisors and NULL b take ELSE; NULL a with b = -1 gives NULL.
    let want = vec![
        (0, Some(5)),
        (1, Some(0)),
        (2, Some(0)),
        (3, Some(0)),
        (4, Some(-3)),
        (5, Some(-3)),
        (6, Some(0)),
        (7, Some(0)),
        (8, Some(0)),
        (9, Some(33)),
        (10, None),
        (11, Some(-1)),
    ];
    for ex in EXECS {
        assert_eq!(
            run("t1", "case_when_b_ne_0", ex),
            Ok(want.clone()),
            "{ex:?}"
        );
    }
}

#[test]
fn case_guarding_minus_one_protects_the_overflow_row_only() {
    // t2: the guard b <> -1 sends (MIN, -1) and (8, -1) and (NULL, -1) to ELSE; no valid zero divisor remains.
    let want = vec![
        (0, Some(0)),
        (1, Some(0)),
        (2, Some(i64::MIN)),
        (3, None),
        (4, Some(0)),
        (5, Some(i64::MAX / 2)),
        (6, Some(i64::MIN / 2)),
        (7, Some(-2)),
        (8, Some(0)),
    ];
    for ex in EXECS {
        assert_eq!(
            run("t2", "case_when_b_ne_minus_1", ex),
            Ok(want.clone()),
            "{ex:?}"
        );
    }
    // while the zero guard does not protect it
    for ex in EXECS {
        assert_eq!(
            run("t2", "case_when_b_ne_0", ex),
            Err(Cat::Overflow),
            "{ex:?}"
        );
    }
}

#[test]
fn a_selection_that_excludes_the_bad_rows_hides_the_error_and_one_that_includes_them_raises_it() {
    for ex in EXECS {
        assert!(
            run("t1", "id_between_4_and_8", ex).is_err(),
            "row 7 is (5, 0): {ex:?}"
        );
        assert!(
            run("t1", "id_between_0_and_3", ex).is_err(),
            "row 1 is (10, 0): {ex:?}"
        );
        assert_eq!(
            run("t2", "id_between_4_and_8", ex),
            Ok(vec![
                (4, None),
                (5, Some(i64::MAX / 2)),
                (6, Some(i64::MIN / 2)),
                (7, Some(-2)),
                (8, None)
            ]),
            "{ex:?}"
        );
    }
    assert_eq!(
        run("t2", "id_between_0_and_3", Exec::Own),
        Err(Cat::Overflow),
        "row 0 is (MIN, -1)"
    );
}

#[test]
fn the_second_guard_protects_the_nested_division() {
    // t1 rows with b <> 0 and a // b <> 0 and 1000 // (a // b) > 1: row 0 (5 -> 200), row 4 (-3 -> -333 no), row 9 (33 -> 30).
    for ex in EXECS {
        assert_eq!(
            run("t1", "two_guards_then_nested_division", ex),
            Ok(vec![(0, Some(200)), (9, Some(30))]),
            "{ex:?}"
        );
        // without it, row 6 is (0, 5): 0 // 5 = 0 and 1000 // 0 fails
        assert_eq!(
            run("t1", "one_guard_then_nested_division", ex),
            Err(Cat::DivZero),
            "{ex:?}"
        );
    }
}

// The arrow-rs path: why wrappers must compact (or fold the selection into the null buffer).

#[test]
fn arrow_without_compaction_errors_on_a_row_the_guard_removed() {
    let t = table("t1");
    let c = t.cols();
    let (a, b) = (to_array(c_col(&c.a, &c.va)), to_array(c_col(&c.b, &c.vb)));
    // the guard b <> 0 selects the rows without a zero divisor...
    let sel = filter_ne(&c.b, Valid::Bytes(&c.vb), 0, t.len(), None);
    // ...but arrow knows nothing about it: row 1 is (10, 0) and is valid, so the uncompacted division fails
    assert!(matches!(
        div_uncompacted(&a, &b),
        Err(ArrowError::DivideByZero)
    ));
    // compacting first removes the row, and so does folding the selection into the null buffer
    let compacted =
        div_compacted(&a, &b, Some(&sel)).expect("the guard removed every zero divisor");
    assert_eq!(compacted.len(), sel.len());
    let folded = div_folded(&a, &b, Some(&sel)).expect("the same");
    for (k, &i) in sel.iter().enumerate() {
        assert_eq!(compacted.is_valid(k), folded.is_valid(i as usize));
        if compacted.is_valid(k) {
            assert_eq!(compacted.value(k), folded.value(i as usize));
        }
    }
}

#[test]
fn arrow_without_compaction_errors_on_min_over_minus_one_in_a_removed_row() {
    let (a, b) = (
        Int64Array::from(vec![i64::MIN, 8]),
        Int64Array::from(vec![-1i64, 2]),
    );
    assert!(matches!(
        div_uncompacted(&a, &b),
        Err(ArrowError::ArithmeticOverflow(_))
    ));
    assert_eq!(
        div_compacted(&a, &b, Some(&[1]))
            .expect("row 0 removed")
            .value(0),
        4
    );
    assert_eq!(
        div_folded(&a, &b, Some(&[1]))
            .expect("row 0 removed")
            .value(1),
        4
    );
}

#[test]
fn arrow_skips_null_slots_by_itself() {
    // t3 keeps MIN / -1 and x / 0 in NULL slots. arrow's own validity handling already ignores them,
    // so the selection is the only thing it cannot see.
    let t = table("t3");
    let c = t.cols();
    let (a, b) = (to_array(c_col(&c.a, &c.va)), to_array(c_col(&c.b, &c.vb)));
    let out = div_uncompacted(&a, &b).expect("failures only in NULL slots");
    assert_eq!(
        (
            out.is_valid(1),
            out.is_valid(2),
            out.is_valid(3),
            out.is_valid(4)
        ),
        (false, false, false, false)
    );
    // and a table with a valid failure still fails
    let t1 = table("t1");
    let c1 = t1.cols();
    assert!(
        div_uncompacted(
            &to_array(c_col(&c1.a, &c1.va)),
            &to_array(c_col(&c1.b, &c1.vb))
        )
        .is_err()
    );
}

#[test]
fn the_arrow_error_kinds_map_to_the_same_categories_as_ours() {
    let (a, b) = (Int64Array::from(vec![1i64]), Int64Array::from(vec![0i64]));
    assert_eq!(
        cat_arrow(&div_uncompacted(&a, &b).expect_err("zero")),
        Cat::DivZero
    );
    let (a, b) = (
        Int64Array::from(vec![i64::MIN]),
        Int64Array::from(vec![-1i64]),
    );
    assert_eq!(
        cat_arrow(&div_uncompacted(&a, &b).expect_err("overflow")),
        Cat::Overflow
    );
}

fn c_col<'a>(v: &'a [i64], valid: &'a [u8]) -> (&'a [i64], Valid<'a>) {
    (v, Valid::Bytes(valid))
}
