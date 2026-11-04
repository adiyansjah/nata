//! The TPC-H based queries, as engine plans. SQL text next to each is in `sql/<name>.sql` and is what was
//! run against DuckDB.
use std::sync::Arc;

use crate::agg::{HashAgg, merge_all};
use crate::engine::*;
use crate::join::{JoinBuild, Probe, build_table};
use crate::kernels::R;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Query {
    AggOrderkey,
    AggPartkey,
    AggModeDate,
    AggComment,
    Q3,
    Q12,
    J3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Table {
    Lineitem,
    Orders,
    Customer,
}

impl Table {
    pub fn file(self) -> &'static str {
        match self {
            Table::Lineitem => "lineitem.parquet",
            Table::Orders => "orders.parquet",
            Table::Customer => "customer.parquet",
        }
    }
}

pub const CUTOFF: i32 = 10471; // 1998-09-02
pub const MAR15: i32 = 9204; // 1995-03-15
pub const Y1994: (i32, i32) = (8766, 9130); // 1994-01-01 ..= 1994-12-31

pub const ALL: [Query; 7] = [Query::AggOrderkey, Query::AggPartkey, Query::AggModeDate, Query::AggComment, Query::Q3, Query::Q12, Query::J3];

impl Query {
    pub fn name(self) -> &'static str {
        match self {
            Query::AggOrderkey => "agg_orderkey",
            Query::AggPartkey => "agg_partkey",
            Query::AggModeDate => "agg_modedate",
            Query::AggComment => "agg_comment",
            Query::Q3 => "q3",
            Query::Q12 => "q12",
            Query::J3 => "j3",
        }
    }

    pub fn parse(s: &str) -> Option<Query> {
        ALL.iter().copied().find(|q| q.name() == s)
    }

    /// Decimal scale of each SUM in the result.
    pub fn scales(self) -> Vec<u32> {
        match self {
            Query::AggOrderkey | Query::AggPartkey => vec![2, 4],
            Query::AggModeDate => vec![2, 2],
            Query::AggComment => vec![2],
            Query::Q3 => vec![4],
            Query::Q12 => vec![0, 0],
            Query::J3 => vec![2],
        }
    }

    pub fn show_count(self) -> bool {
        !matches!(self, Query::Q3 | Query::Q12)
    }

    pub fn columns(self, t: Table) -> &'static [&'static str] {
        use Table::*;
        match (self, t) {
            (Query::AggOrderkey, Lineitem) => &["l_orderkey", "l_extendedprice", "l_discount", "l_shipdate"],
            (Query::AggPartkey, Lineitem) => &["l_partkey", "l_quantity", "l_extendedprice", "l_discount", "l_shipdate"],
            (Query::AggModeDate, Lineitem) => &["l_shipmode", "l_shipdate", "l_discount", "l_quantity", "l_extendedprice"],
            (Query::AggComment, Orders) => &["o_comment", "o_totalprice", "o_orderdate"],
            (Query::Q3, Customer) => &["c_custkey", "c_mktsegment"],
            (Query::Q3, Orders) => &["o_orderkey", "o_custkey", "o_orderdate", "o_shippriority"],
            (Query::Q3, Lineitem) => &["l_orderkey", "l_extendedprice", "l_discount", "l_shipdate"],
            (Query::Q12, Lineitem) => &["l_orderkey", "l_shipmode", "l_commitdate", "l_receiptdate", "l_shipdate"],
            (Query::Q12, Orders) => &["o_orderkey", "o_orderpriority"],
            (Query::J3, Lineitem) => &["l_orderkey", "l_shipmode", "l_extendedprice"],
            (Query::J3, Orders) => &["o_orderkey", "o_orderpriority"],
            _ => &[],
        }
    }
}

pub type Input<'a> = (usize, Visit<'a>);

pub struct Inputs<'a> {
    pub lineitem: Input<'a>,
    pub orders: Input<'a>,
    pub customer: Input<'a>,
}

fn agg_pipeline(src: Input, threads: usize, mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync), keys: Vec<&'static str>, sums: Vec<Ref>) -> R<Vec<HashAgg>> {
    let locals = run_pipeline(src.0, src.1, threads, mk_ops, &|| HashAgg::new(keys.clone(), sums.clone()))?;
    merge_all(locals, threads)
}

fn revenue_steps() -> Vec<Step> {
    vec![Step::SubConstLeft { c: 100, a: Ref::Col("l_discount") }, Step::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Tmp(0) }]
}

fn build_side(src: Input, threads: usize, mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync), keys: Vec<&'static str>, payload: Vec<&'static str>) -> R<Arc<crate::join::JoinTable>> {
    let sinks = run_pipeline(src.0, src.1, threads, mk_ops, &|| JoinBuild::new(keys.clone(), payload.clone()))?;
    Ok(Arc::new(build_table(sinks, threads)?))
}

pub fn run(q: Query, inp: &Inputs, threads: usize) -> R<Vec<HashAgg>> {
    match q {
        Query::AggOrderkey => agg_pipeline(
            inp.lineitem,
            threads,
            &|| vec![Box::new(FilterDate { col: "l_shipdate", lo: i32::MIN, hi: CUTOFF }), Box::new(Project(revenue_steps()))],
            vec!["l_orderkey"],
            vec![Ref::Col("l_extendedprice"), Ref::Tmp(1)],
        ),
        Query::AggPartkey => agg_pipeline(
            inp.lineitem,
            threads,
            &|| vec![Box::new(FilterDate { col: "l_shipdate", lo: i32::MIN, hi: CUTOFF }), Box::new(Project(revenue_steps()))],
            vec!["l_partkey"],
            vec![Ref::Col("l_quantity"), Ref::Tmp(1)],
        ),
        Query::AggModeDate => agg_pipeline(
            inp.lineitem,
            threads,
            &|| vec![Box::new(FilterDec { col: "l_discount", lo: 2, hi: 8 })],
            vec!["l_shipmode", "l_shipdate"],
            vec![Ref::Col("l_quantity"), Ref::Col("l_extendedprice")],
        ),
        Query::AggComment => agg_pipeline(
            inp.orders,
            threads,
            &|| vec![Box::new(FilterDate { col: "o_orderdate", lo: i32::MIN, hi: CUTOFF })],
            vec!["o_comment"],
            vec![Ref::Col("o_totalprice")],
        ),
        Query::Q3 => {
            // customer (BUILDING) -> build; orders probe it, filtered by date -> build; lineitem probes that -> aggregate.
            let t1 = build_side(
                inp.customer,
                threads,
                &|| vec![Box::new(FilterStrIn { col: "c_mktsegment", set: &["BUILDING"] })],
                vec!["c_custkey"],
                vec![],
            )?;
            let t2 = {
                let sinks = run_pipeline(
                    inp.orders.0,
                    inp.orders.1,
                    threads,
                    &|| -> Vec<Box<dyn Op>> {
                        vec![
                            Box::new(FilterDate { col: "o_orderdate", lo: i32::MIN, hi: MAR15 - 1 }),
                            Box::new(Probe::new(t1.clone(), vec!["o_custkey"], vec!["o_orderkey", "o_orderdate", "o_shippriority"])),
                        ]
                    },
                    &|| JoinBuild::new(vec!["o_orderkey"], vec!["o_orderdate", "o_shippriority"]),
                )?;
                Arc::new(build_table(sinks, threads)?)
            };
            let locals = run_pipeline(
                inp.lineitem.0,
                inp.lineitem.1,
                threads,
                &|| -> Vec<Box<dyn Op>> {
                    vec![
                        Box::new(FilterDate { col: "l_shipdate", lo: MAR15 + 1, hi: i32::MAX }),
                        Box::new(Probe::new(t2.clone(), vec!["l_orderkey"], vec!["l_orderkey", "l_extendedprice", "l_discount"])),
                        Box::new(Project(revenue_steps())),
                    ]
                },
                &|| HashAgg::new(vec!["l_orderkey", "o_orderdate", "o_shippriority"], vec![Ref::Tmp(1)]),
            )?;
            merge_all(locals, threads)
        }
        Query::Q12 => {
            // Build the (small) filtered lineitem side with duplicate order keys; probe with all of orders.
            let t = build_side(
                inp.lineitem,
                threads,
                &|| -> Vec<Box<dyn Op>> {
                    vec![
                        Box::new(FilterStrIn { col: "l_shipmode", set: &["MAIL", "SHIP"] }),
                        Box::new(FilterDateLt { a: "l_commitdate", b: "l_receiptdate" }),
                        Box::new(FilterDateLt { a: "l_shipdate", b: "l_commitdate" }),
                        Box::new(FilterDate { col: "l_receiptdate", lo: Y1994.0, hi: Y1994.1 }),
                    ]
                },
                vec!["l_orderkey"],
                vec!["l_shipmode"],
            )?;
            let locals = run_pipeline(
                inp.orders.0,
                inp.orders.1,
                threads,
                &|| -> Vec<Box<dyn Op>> {
                    vec![
                        Box::new(Probe::new(t.clone(), vec!["o_orderkey"], vec!["o_orderpriority"])),
                        Box::new(Project(vec![
                            Step::FlagIn { col: "o_orderpriority", set: &["1-URGENT", "2-HIGH"], negate: false },
                            Step::FlagIn { col: "o_orderpriority", set: &["1-URGENT", "2-HIGH"], negate: true },
                        ])),
                    ]
                },
                &|| HashAgg::new(vec!["l_shipmode"], vec![Ref::Tmp(0), Ref::Tmp(1)]),
            )?;
            merge_all(locals, threads)
        }
        Query::J3 => {
            // Fan-out stress: all of lineitem (about 4 rows per order key) is the build side.
            let t = build_side(inp.lineitem, threads, &|| vec![], vec!["l_orderkey"], vec!["l_shipmode", "l_extendedprice"])?;
            let locals = run_pipeline(
                inp.orders.0,
                inp.orders.1,
                threads,
                &|| -> Vec<Box<dyn Op>> { vec![Box::new(Probe::new(t.clone(), vec!["o_orderkey"], vec!["o_orderpriority"]))] },
                &|| HashAgg::new(vec!["o_orderpriority", "l_shipmode"], vec![Ref::Col("l_extendedprice")]),
            )?;
            merge_all(locals, threads)
        }
    }
}
