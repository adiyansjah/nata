//! Independent scalar reference: plain row loops over `Vec<Row>` with `Option` cells, a nested-loop join,
//! BTreeMap grouping, exact i128 arithmetic. It shares no code with the engine (only the plain-data
//! result type `Output`). `join_indexed` is a BTreeMap-index join used at TPC-H scale, where a nested loop
//! is impossible; a property test shows it equals the nested loop on small random tables.
use std::collections::BTreeMap;

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Int32Type, Int64Type};

use crate::out::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum V {
    I(i64),
    S(String),
}
pub type Cell = Option<V>;
pub type Row = Vec<Cell>;

fn kval(c: &Cell) -> KVal {
    match c {
        None => KVal::Null,
        Some(V::I(i)) => KVal::I(*i),
        Some(V::S(s)) => KVal::S(s.clone()),
    }
}

fn int(c: &Cell) -> Option<i128> {
    match c {
        Some(V::I(i)) => Some(*i as i128),
        _ => None,
    }
}

/// How a SUM's input is computed from a row.
#[derive(Debug, Clone, Copy)]
pub enum SumKind {
    Col(usize),
    /// `price * (100 - disc)`; NULL if either is NULL.
    PriceTimes100MinusDisc { price: usize, disc: usize },
    /// `CASE WHEN col [NOT] IN (set) THEN 1 ELSE 0 END`
    FlagIn { col: usize, set: &'static [&'static str], negate: bool },
}

fn sum_input(r: &Row, k: &SumKind) -> Option<i128> {
    match *k {
        SumKind::Col(c) => int(&r[c]),
        SumKind::PriceTimes100MinusDisc { price, disc } => Some(int(&r[price])? * (100 - int(&r[disc])?)),
        SumKind::FlagIn { col, set, negate } => {
            let inside = matches!(&r[col], Some(V::S(s)) if set.contains(&s.as_str()));
            let non_null = r[col].is_some();
            Some((non_null && (inside != negate)) as i128)
        }
    }
}

pub fn agg(rows: &[Row], keys: &[usize], sums: &[SumKind], scales: Vec<u32>, show_count: bool) -> Output {
    let mut groups: BTreeMap<Vec<KVal>, GroupOut> = BTreeMap::new();
    for r in rows {
        let k: Vec<KVal> = keys.iter().map(|&c| kval(&r[c])).collect();
        let g = groups.entry(k).or_insert_with(|| GroupOut { count: 0, sums: vec![(0, 0); sums.len()] });
        g.count += 1;
        for (j, s) in sums.iter().enumerate() {
            if let Some(v) = sum_input(r, s) {
                g.sums[j].0 += v;
                g.sums[j].1 += 1;
            }
        }
    }
    Output { rows: groups, scales, show_count }
}

/// Inner join by nested loops. Output rows: the `probe_out` columns of the probe row, then the `payload`
/// columns of the build row. A NULL in any key column never matches.
pub fn join_nested(probe: &[Row], pk: &[usize], build: &[Row], bk: &[usize], probe_out: &[usize], payload: &[usize]) -> Vec<Row> {
    let mut out = Vec::new();
    for p in probe {
        for b in build {
            let all = pk.iter().zip(bk).all(|(&i, &j)| p[i].is_some() && b[j].is_some() && p[i] == b[j]);
            if all {
                let mut r: Row = probe_out.iter().map(|&c| p[c].clone()).collect();
                r.extend(payload.iter().map(|&c| b[c].clone()));
                out.push(r);
            }
        }
    }
    out
}

/// The same join through a BTreeMap index on the build side.
pub fn join_indexed(probe: &[Row], pk: &[usize], build: &[Row], bk: &[usize], probe_out: &[usize], payload: &[usize]) -> Vec<Row> {
    let mut idx: BTreeMap<Vec<V>, Vec<usize>> = BTreeMap::new();
    for (i, b) in build.iter().enumerate() {
        let key: Option<Vec<V>> = bk.iter().map(|&j| b[j].clone()).collect();
        if let Some(k) = key {
            idx.entry(k).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for p in probe {
        let key: Option<Vec<V>> = pk.iter().map(|&j| p[j].clone()).collect();
        let Some(key) = key else { continue };
        for &bi in idx.get(&key).map(Vec::as_slice).unwrap_or(&[]) {
            let mut r: Row = probe_out.iter().map(|&c| p[c].clone()).collect();
            r.extend(payload.iter().map(|&c| build[bi][c].clone()));
            out.push(r);
        }
    }
    out
}

/// Arrow batches to rows, by plain per-type accessors. Decimal64, Int64, Int32 and Date32 become `I`.
pub fn rows_of(batches: &[RecordBatch], cols: &[&str]) -> Vec<Row> {
    let mut rows = Vec::new();
    for rb in batches {
        let arrs: Vec<_> = cols.iter().map(|c| rb.column(rb.schema().index_of(c).expect("column")).clone()).collect();
        for i in 0..rb.num_rows() {
            rows.push(
                arrs.iter()
                    .map(|a| {
                        if a.is_null(i) {
                            return None;
                        }
                        Some(match a.data_type() {
                            DataType::Int64 => V::I(a.as_primitive::<Int64Type>().value(i)),
                            DataType::Int32 => V::I(a.as_primitive::<Int32Type>().value(i) as i64),
                            DataType::Date32 => V::I(a.as_primitive::<Date32Type>().value(i) as i64),
                            DataType::Decimal64(..) => V::I(a.as_primitive::<Decimal64Type>().value(i)),
                            DataType::Utf8 => V::S(a.as_string::<i32>().value(i).to_string()),
                            DataType::Dictionary(..) => {
                                let d = a.as_dictionary::<Int32Type>();
                                V::S(d.values().as_string::<i32>().value(d.keys().value(i) as usize).to_string())
                            }
                            other => panic!("unexpected type {other:?}"),
                        })
                    })
                    .collect(),
            );
        }
    }
    rows
}

pub fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort();
    rows
}

// ---------------------------------------------------------------- the TPC-H queries, row by row

use arrow::array::RecordBatch as Rb;

use crate::queries::{CUTOFF, MAR15, Query, Y1994};

pub struct RefTables {
    pub lineitem: Vec<Rb>,
    pub orders: Vec<Rb>,
    pub customer: Vec<Rb>,
}

fn ival(c: &Cell) -> Option<i64> {
    match c {
        Some(V::I(i)) => Some(*i),
        _ => None,
    }
}

fn sval(c: &Cell) -> Option<&str> {
    match c {
        Some(V::S(s)) => Some(s),
        _ => None,
    }
}

/// The query evaluated by plain row loops and BTreeMap grouping.
pub fn query(q: Query, t: &RefTables) -> Output {
    let (scales, show) = (q.scales(), q.show_count());
    match q {
        Query::AggOrderkey => {
            let rows: Vec<Row> = rows_of(&t.lineitem, &["l_orderkey", "l_extendedprice", "l_discount", "l_shipdate"])
                .into_iter()
                .filter(|r| ival(&r[3]).is_some_and(|d| d <= CUTOFF as i64))
                .collect();
            agg(&rows, &[0], &[SumKind::Col(1), SumKind::PriceTimes100MinusDisc { price: 1, disc: 2 }], scales, show)
        }
        Query::AggPartkey => {
            let rows: Vec<Row> = rows_of(&t.lineitem, &["l_partkey", "l_quantity", "l_extendedprice", "l_discount", "l_shipdate"])
                .into_iter()
                .filter(|r| ival(&r[4]).is_some_and(|d| d <= CUTOFF as i64))
                .collect();
            agg(&rows, &[0], &[SumKind::Col(1), SumKind::PriceTimes100MinusDisc { price: 2, disc: 3 }], scales, show)
        }
        Query::AggModeDate => {
            let rows: Vec<Row> = rows_of(&t.lineitem, &["l_shipmode", "l_shipdate", "l_discount", "l_quantity", "l_extendedprice"])
                .into_iter()
                .filter(|r| ival(&r[2]).is_some_and(|d| (2..=8).contains(&d)))
                .collect();
            agg(&rows, &[0, 1], &[SumKind::Col(3), SumKind::Col(4)], scales, show)
        }
        Query::AggComment => {
            let rows: Vec<Row> = rows_of(&t.orders, &["o_comment", "o_totalprice", "o_orderdate"])
                .into_iter()
                .filter(|r| ival(&r[2]).is_some_and(|d| d <= CUTOFF as i64))
                .collect();
            agg(&rows, &[0], &[SumKind::Col(1)], scales, show)
        }
        Query::Q3 => {
            let cust: Vec<Row> = rows_of(&t.customer, &["c_custkey", "c_mktsegment"]).into_iter().filter(|r| sval(&r[1]) == Some("BUILDING")).collect();
            let ord: Vec<Row> = rows_of(&t.orders, &["o_orderkey", "o_custkey", "o_orderdate", "o_shippriority"])
                .into_iter()
                .filter(|r| ival(&r[2]).is_some_and(|d| d < MAR15 as i64))
                .collect();
            let li: Vec<Row> = rows_of(&t.lineitem, &["l_orderkey", "l_extendedprice", "l_discount", "l_shipdate"])
                .into_iter()
                .filter(|r| ival(&r[3]).is_some_and(|d| d > MAR15 as i64))
                .collect();
            let j1 = join_indexed(&ord, &[1], &cust, &[0], &[0, 2, 3], &[]); // [okey, odate, prio]
            let j2 = join_indexed(&li, &[0], &j1, &[0], &[0, 1, 2], &[1, 2]); // [lkey, price, disc, odate, prio]
            agg(&j2, &[0, 3, 4], &[SumKind::PriceTimes100MinusDisc { price: 1, disc: 2 }], scales, show)
        }
        Query::Q12 => {
            let li: Vec<Row> = rows_of(&t.lineitem, &["l_orderkey", "l_shipmode", "l_commitdate", "l_receiptdate", "l_shipdate"])
                .into_iter()
                .filter(|r| {
                    let (c, rc, s) = (ival(&r[2]), ival(&r[3]), ival(&r[4]));
                    matches!(sval(&r[1]), Some("MAIL") | Some("SHIP"))
                        && matches!((c, rc), (Some(c), Some(rc)) if c < rc)
                        && matches!((s, c), (Some(s), Some(c)) if s < c)
                        && rc.is_some_and(|d| (Y1994.0 as i64..=Y1994.1 as i64).contains(&d))
                })
                .collect();
            let ord = rows_of(&t.orders, &["o_orderkey", "o_orderpriority"]);
            let j = join_indexed(&ord, &[0], &li, &[0], &[1], &[1]); // [priority, shipmode]
            const HI: &[&str] = &["1-URGENT", "2-HIGH"];
            agg(&j, &[1], &[SumKind::FlagIn { col: 0, set: HI, negate: false }, SumKind::FlagIn { col: 0, set: HI, negate: true }], scales, show)
        }
        Query::J3 => {
            let li = rows_of(&t.lineitem, &["l_orderkey", "l_shipmode", "l_extendedprice"]);
            let ord = rows_of(&t.orders, &["o_orderkey", "o_orderpriority"]);
            let j = join_indexed(&ord, &[0], &li, &[0], &[1], &[1, 2]); // [priority, shipmode, price]
            agg(&j, &[0, 1], &[SumKind::Col(2)], scales, show)
        }
    }
}
