//! Crafted tables and guarded-expression queries shared by `tests/guarded.rs` (assertions) and
//! `examples/guard_diff.rs` (the DuckDB differential).
//!
//! Every query exists three ways: a vectorized pipeline over our kernels (`run`, for each executor), a
//! row-by-row reference (`reference`, plain Rust with short-circuit guards), and SQL for DuckDB.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    dead_code,
    reason = "test support: small values, shared by two targets that each use part of it"
)]
use arrow_array::Int64Array;
use arrow_schema::ArrowError;
use nata_kernels::arrow_path::{div_compacted, div_folded, to_array};
use nata_kernels::kernels::{
    Col, EngineError, Out, Valid, constant, div, div_const_left, filter_ne, filter_range,
    merge_branches, split_ne,
};

/// A NULL slot stores a value that would fail if it were ever divided: a dividend of `i64::MIN` and a
/// divisor of 0. A kernel that lets a NULL slot raise an error is caught by these.
pub const NULL_DIVIDEND_SLOT: i64 = i64::MIN;
pub const NULL_DIVISOR_SLOT: i64 = 0;

pub struct Table {
    pub name: &'static str,
    pub rows: Vec<(Option<i64>, Option<i64>)>,
}

pub struct Cols {
    pub id: Vec<i64>,
    pub a: Vec<i64>,
    pub va: Vec<u8>,
    pub b: Vec<i64>,
    pub vb: Vec<u8>,
}

impl Table {
    pub fn cols(&self) -> Cols {
        Cols {
            id: (0..self.rows.len() as i64).collect(),
            a: self
                .rows
                .iter()
                .map(|r| r.0.unwrap_or(NULL_DIVIDEND_SLOT))
                .collect(),
            va: self.rows.iter().map(|r| r.0.is_some() as u8).collect(),
            b: self
                .rows
                .iter()
                .map(|r| r.1.unwrap_or(NULL_DIVISOR_SLOT))
                .collect(),
            vb: self.rows.iter().map(|r| r.1.is_some() as u8).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

const MIN: i64 = i64::MIN;
const MAX: i64 = i64::MAX;

/// The three crafted tables. T1 has valid zero divisors (and no `MIN / -1` among valid rows), T2 has a valid
/// `MIN / -1` (and no valid zero divisor), T3 has failing divisions only in NULL slots.
pub fn tables() -> Vec<Table> {
    let s = Some;
    vec![
        Table {
            name: "t1_zero_divisors",
            rows: vec![
                (s(10), s(2)),
                (s(10), s(0)),
                (None, s(0)),
                (s(7), None),
                (s(-7), s(2)),
                (s(7), s(-2)),
                (s(0), s(5)),
                (s(5), s(0)),
                (None, None),
                (s(100), s(3)),
                (None, s(-1)),
                (s(1), s(-1)),
            ],
        },
        Table {
            name: "t2_min_over_minus_one",
            rows: vec![
                (s(MIN), s(-1)),
                (s(8), s(-1)),
                (s(MIN), s(1)),
                (None, s(0)),
                (s(5), None),
                (s(MAX), s(2)),
                (s(MIN), s(2)),
                (s(-6), s(3)),
                (None, s(-1)),
            ],
        },
        Table {
            name: "t3_failures_only_in_null_slots",
            rows: vec![
                (s(10), s(2)),
                (None, s(0)),
                (s(5), None),
                (None, None),
                (None, s(-1)),
                (s(MIN), s(1)),
                (s(-9), s(3)),
                (s(0), s(4)),
            ],
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    DivZero,
    Overflow,
}

impl Cat {
    pub fn word(self) -> &'static str {
        match self {
            Cat::DivZero => "division_by_zero",
            Cat::Overflow => "overflow",
        }
    }
}

/// (id, value) per output row; `None` is a NULL value.
pub type Rows = Vec<(i64, Option<i64>)>;
pub type Outcome = Result<Rows, Cat>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exec {
    /// Our own selection- and validity-aware kernels.
    Own,
    /// `div` through arrow-rs, after compacting the inputs to the selection.
    ArrowCompacted,
    /// `div` through arrow-rs, after folding the selection into the null buffers.
    ArrowFolded,
}

pub const EXECS: [Exec; 3] = [Exec::Own, Exec::ArrowCompacted, Exec::ArrowFolded];

fn cat_engine(e: &EngineError) -> Cat {
    if e.0.contains("division by zero") {
        Cat::DivZero
    } else if e.0.contains("overflow") {
        Cat::Overflow
    } else {
        panic!("unexpected engine error: {e:?}")
    }
}

pub fn cat_arrow(e: &ArrowError) -> Cat {
    match e {
        ArrowError::DivideByZero => Cat::DivZero,
        ArrowError::ArithmeticOverflow(_) => Cat::Overflow,
        other => panic!("unexpected arrow error: {other:?}"),
    }
}

fn out_from_array(arr: &Int64Array) -> Out {
    use arrow_array::Array;
    Out {
        v: arr.values().to_vec(),
        valid: arr
            .nulls()
            .map(|n| (0..arr.len()).map(|i| n.is_valid(i) as u8).collect()),
    }
}

/// `a / b` over the selection, through the chosen executor. Output is full length.
fn div_via(
    ex: Exec,
    a: Col<i64>,
    b: Col<i64>,
    rows: usize,
    sel: Option<&[u32]>,
) -> Result<Out, Cat> {
    match ex {
        Exec::Own => div(a, b, rows, sel).map_err(|e| cat_engine(&e)),
        Exec::ArrowCompacted => {
            let compact =
                div_compacted(&to_array(a), &to_array(b), sel).map_err(|e| cat_arrow(&e))?;
            Ok(scatter(&compact, rows, sel))
        }
        Exec::ArrowFolded => {
            let folded = div_folded(&to_array(a), &to_array(b), sel).map_err(|e| cat_arrow(&e))?;
            Ok(out_from_array(&folded))
        }
    }
}

/// `c / a` over the selection, through the chosen executor.
fn div_const_left_via(
    ex: Exec,
    c: i64,
    a: Col<i64>,
    rows: usize,
    sel: Option<&[u32]>,
) -> Result<Out, Cat> {
    match ex {
        Exec::Own => div_const_left(c, a, rows, sel).map_err(|e| cat_engine(&e)),
        _ => {
            let cs = vec![c; rows];
            div_via(ex, (&cs, Valid::All), a, rows, sel)
        }
    }
}

/// Puts a compacted array (one slot per selected row) back at the selected positions of a full-length column.
fn scatter(compact: &Int64Array, rows: usize, sel: Option<&[u32]>) -> Out {
    use arrow_array::Array;
    let mut v = vec![0i64; rows];
    let mut valid = vec![0u8; rows];
    let positions: Vec<usize> = sel.map_or((0..rows).collect(), |s| {
        s.iter().map(|&i| i as usize).collect()
    });
    for (k, &i) in positions.iter().enumerate() {
        v[i] = compact.value(k);
        valid[i] = compact.is_valid(k) as u8;
    }
    Out {
        v,
        valid: Some(valid),
    }
}

fn emit(ids: &[i64], sel: Option<&[u32]>, out: &Out) -> Rows {
    let rows = ids.len();
    let positions: Vec<usize> = sel.map_or((0..rows).collect(), |s| {
        s.iter().map(|&i| i as usize).collect()
    });
    positions
        .into_iter()
        .map(|i| {
            (
                ids[i],
                out.valid
                    .as_ref()
                    .is_none_or(|v| v[i] != 0)
                    .then_some(out.v[i]),
            )
        })
        .collect()
}

/// Row-level reference: `Ok(None)` = row filtered out, `Ok(Some(v))` = row output with value `v`.
pub type RefRow = fn(id: i64, a: Option<i64>, b: Option<i64>) -> Result<Option<Option<i64>>, Cat>;

pub struct Query {
    pub name: &'static str,
    /// SQL over a table `t(id BIGINT, a BIGINT, b BIGINT)`.
    pub sql: &'static str,
    pub reference: RefRow,
    pub run: fn(&Table, Exec) -> Result<Rows, Cat>,
}

/// Checked integer division in SQL semantics for a row whose inputs are valid.
fn sdiv(a: i64, b: i64) -> Result<i64, Cat> {
    if b == 0 {
        Err(Cat::DivZero)
    } else if a == MIN && b == -1 {
        Err(Cat::Overflow)
    } else {
        Ok(a / b)
    }
}

/// `x <> k` as a guard: NULL means "not true", so the row is dropped.
fn ne(x: Option<i64>, k: i64) -> bool {
    x.is_some_and(|x| x != k)
}

/// `a // b` with NULL propagation; the division only happens when both are present.
fn div_null(a: Option<i64>, b: Option<i64>) -> Result<Option<i64>, Cat> {
    match (a, b) {
        (Some(a), Some(b)) => sdiv(a, b).map(Some),
        _ => Ok(None),
    }
}

/// `v > 1` where NULL is not true.
fn gt1(v: Option<i64>) -> bool {
    v.is_some_and(|v| v > 1)
}

pub fn queries() -> Vec<Query> {
    vec![
        Query {
            name: "guarded_filter",
            sql: "SELECT id, a // b FROM t WHERE b <> 0 AND a // b > 1 ORDER BY id",
            reference: |_, a, b| {
                if !ne(b, 0) {
                    return Ok(None);
                }
                let q = div_null(a, b)?;
                Ok(gt1(q).then_some(q))
            },
            run: |t, ex| {
                let c = t.cols();
                let n = t.len();
                let sel1 = filter_ne(&c.b, Valid::Bytes(&c.vb), 0, n, None);
                let q = div_via(
                    ex,
                    (&c.a, Valid::Bytes(&c.va)),
                    (&c.b, Valid::Bytes(&c.vb)),
                    n,
                    Some(&sel1),
                )?;
                let sel2 = filter_range(&q.v, q.col().1, 2, MAX, n, Some(&sel1));
                Ok(emit(&c.id, Some(&sel2), &q))
            },
        },
        Query {
            name: "unguarded_filter",
            sql: "SELECT id, a // b FROM t WHERE a // b > 1 ORDER BY id",
            reference: |_, a, b| {
                let q = div_null(a, b)?;
                Ok(gt1(q).then_some(q))
            },
            run: |t, ex| {
                let c = t.cols();
                let n = t.len();
                let q = div_via(
                    ex,
                    (&c.a, Valid::Bytes(&c.va)),
                    (&c.b, Valid::Bytes(&c.vb)),
                    n,
                    None,
                )?;
                let sel = filter_range(&q.v, q.col().1, 2, MAX, n, None);
                Ok(emit(&c.id, Some(&sel), &q))
            },
        },
        Query {
            name: "case_when_b_ne_0",
            sql: "SELECT id, CASE WHEN b <> 0 THEN a // b ELSE 0 END FROM t ORDER BY id",
            reference: |_, a, b| {
                if ne(b, 0) {
                    div_null(a, b).map(Some)
                } else {
                    Ok(Some(Some(0)))
                }
            },
            run: |t, ex| case_div(t, ex, 0),
        },
        Query {
            name: "case_when_b_ne_minus_1",
            sql: "SELECT id, CASE WHEN b <> -1 THEN a // b ELSE 0 END FROM t ORDER BY id",
            reference: |_, a, b| {
                if ne(b, -1) {
                    div_null(a, b).map(Some)
                } else {
                    Ok(Some(Some(0)))
                }
            },
            run: |t, ex| case_div(t, ex, -1),
        },
        Query {
            name: "case_when_a_ne_0_const_dividend",
            sql: "SELECT id, CASE WHEN a <> 0 THEN 100 // b ELSE -1 END FROM t ORDER BY id",
            reference: |_, a, b| {
                if ne(a, 0) {
                    match b {
                        Some(b) => sdiv(100, b).map(|q| Some(Some(q))),
                        None => Ok(Some(None)),
                    }
                } else {
                    Ok(Some(Some(-1)))
                }
            },
            run: |t, ex| {
                let c = t.cols();
                let n = t.len();
                let (yes, no) = split_ne(&c.a, Valid::Bytes(&c.va), 0, n, None);
                let then = div_const_left_via(ex, 100, (&c.b, Valid::Bytes(&c.vb)), n, Some(&yes))?;
                let els = constant(-1, n);
                let merged = merge_branches(n, &[(&yes, &then), (&no, &els)]);
                Ok(emit(&c.id, None, &merged))
            },
        },
        Query {
            name: "id_between_4_and_8",
            sql: "SELECT id, a // b FROM t WHERE id BETWEEN 4 AND 8 ORDER BY id",
            reference: |id, a, b| {
                if (4..=8).contains(&id) {
                    div_null(a, b).map(Some)
                } else {
                    Ok(None)
                }
            },
            run: |t, ex| {
                let c = t.cols();
                let n = t.len();
                let sel = filter_range(&c.id, Valid::All, 4, 8, n, None);
                let q = div_via(
                    ex,
                    (&c.a, Valid::Bytes(&c.va)),
                    (&c.b, Valid::Bytes(&c.vb)),
                    n,
                    Some(&sel),
                )?;
                Ok(emit(&c.id, Some(&sel), &q))
            },
        },
        Query {
            name: "id_between_0_and_3",
            sql: "SELECT id, a // b FROM t WHERE id BETWEEN 0 AND 3 ORDER BY id",
            reference: |id, a, b| {
                if (0..=3).contains(&id) {
                    div_null(a, b).map(Some)
                } else {
                    Ok(None)
                }
            },
            run: |t, ex| {
                let c = t.cols();
                let n = t.len();
                let sel = filter_range(&c.id, Valid::All, 0, 3, n, None);
                let q = div_via(
                    ex,
                    (&c.a, Valid::Bytes(&c.va)),
                    (&c.b, Valid::Bytes(&c.vb)),
                    n,
                    Some(&sel),
                )?;
                Ok(emit(&c.id, Some(&sel), &q))
            },
        },
        Query {
            name: "two_guards_then_nested_division",
            sql: "SELECT id, 1000 // (a // b) FROM t WHERE b <> 0 AND a // b <> 0 AND 1000 // (a // b) > 1 ORDER BY id",
            reference: |_, a, b| {
                if !ne(b, 0) {
                    return Ok(None);
                }
                let q = div_null(a, b)?;
                if !ne(q, 0) {
                    return Ok(None);
                }
                let r = div_null(Some(1000), q)?;
                Ok(gt1(r).then_some(r))
            },
            run: |t, ex| nested(t, ex, true),
        },
        Query {
            name: "one_guard_then_nested_division",
            sql: "SELECT id, 1000 // (a // b) FROM t WHERE b <> 0 AND 1000 // (a // b) > 1 ORDER BY id",
            reference: |_, a, b| {
                if !ne(b, 0) {
                    return Ok(None);
                }
                let q = div_null(a, b)?;
                let r = div_null(Some(1000), q)?;
                Ok(gt1(r).then_some(r))
            },
            run: |t, ex| nested(t, ex, false),
        },
    ]
}

/// `CASE WHEN b <> k THEN a / b ELSE 0 END` as selection splitting.
fn case_div(t: &Table, ex: Exec, k: i64) -> Result<Rows, Cat> {
    let c = t.cols();
    let n = t.len();
    let (yes, no) = split_ne(&c.b, Valid::Bytes(&c.vb), k, n, None);
    let then = div_via(
        ex,
        (&c.a, Valid::Bytes(&c.va)),
        (&c.b, Valid::Bytes(&c.vb)),
        n,
        Some(&yes),
    )?;
    let els = constant(0, n);
    let merged = merge_branches(n, &[(&yes, &then), (&no, &els)]);
    Ok(emit(&c.id, None, &merged))
}

/// `WHERE b <> 0 [AND a / b <> 0] AND 1000 / (a / b) > 1`, projecting `1000 / (a / b)`.
fn nested(t: &Table, ex: Exec, second_guard: bool) -> Result<Rows, Cat> {
    let c = t.cols();
    let n = t.len();
    let sel1 = filter_ne(&c.b, Valid::Bytes(&c.vb), 0, n, None);
    let q = div_via(
        ex,
        (&c.a, Valid::Bytes(&c.va)),
        (&c.b, Valid::Bytes(&c.vb)),
        n,
        Some(&sel1),
    )?;
    let sel2 = if second_guard {
        filter_ne(&q.v, q.col().1, 0, n, Some(&sel1))
    } else {
        sel1
    };
    let r = div_const_left_via(ex, 1000, q.col(), n, Some(&sel2))?;
    let sel3 = filter_range(&r.v, r.col().1, 2, MAX, n, Some(&sel2));
    Ok(emit(&c.id, Some(&sel3), &r))
}

/// The row-by-row reference outcome of a query over a table.
/// Panics if the table makes two different error categories possible, because then the result is ambiguous.
pub fn reference_outcome(q: &Query, t: &Table) -> Outcome {
    let mut rows = Vec::new();
    let mut errs: Vec<Cat> = Vec::new();
    for (i, &(a, b)) in t.rows.iter().enumerate() {
        match (q.reference)(i as i64, a, b) {
            Ok(Some(v)) => rows.push((i as i64, v)),
            Ok(None) => {}
            Err(e) => {
                if !errs.contains(&e) {
                    errs.push(e);
                }
            }
        }
    }
    assert!(
        errs.len() <= 1,
        "table {} query {}: both error kinds are reachable, the expected category is ambiguous",
        t.name,
        q.name
    );
    match errs.first() {
        Some(&e) => Err(e),
        None => Ok(rows),
    }
}
