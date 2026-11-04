//! Pipeline builders: the generic shapes (sort, top-N, window, joins in memory and partitioned, aggregation) over any source, and the
//! TPC-H style queries built from them (Q18, Q21, a Q13 variant without LIKE, join-kind counts, sort, top-N, window).
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use arrow::array::{ArrayRef, Decimal128Array, Int64Array, RecordBatch, Scalar, StringArray};
use arrow::compute::cast;
use arrow::compute::kernels::cmp;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use crate::agg::{AggFn, HashAgg};
use crate::io::ParquetSource;
use crate::join::*;
use crate::ops::{CollectSink, FilterFn, ProjectFn, SharedQueue};
use crate::proto::api::*;
use crate::proto::sched::Query;
use crate::sort::{SortKey, SortSink, SortSpec, TopNSink};
use crate::spill::Budget;
use crate::window::{Window, WindowSource};

/// Run a pipeline set; a hang (a lost wake-up or a deadlock) is reported as an error after `watchdog`.
pub fn run(pipelines: Vec<Pipeline>, threads: usize, watchdog: Duration) -> std::result::Result<(), String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Query::new(pipelines).run(threads));
    });
    match rx.recv_timeout(watchdog) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("query error: {e}")),
        Err(_) => Err(format!("no result after {watchdog:?}: hang")),
    }
}

fn pipe(name: &str, source: Arc<dyn SourceFactory>, ops: Vec<Arc<dyn OperatorFactory>>, sink: Arc<dyn GlobalSink>, deps: &[usize], tasks: usize) -> Pipeline {
    Pipeline { name: name.into(), source, ops, sink, deps: deps.to_vec(), tasks }
}

// ------------------------------------------------------------------------------------------------ generic shapes

/// ORDER BY: P0 source -> sort sink (spills), P1 merge source (one task) -> collect.
pub fn sort_query(src: Arc<dyn SourceFactory>, spec: SortSpec, budget: &Arc<Budget>, fanin: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>) {
    let sink = SortSink::new(spec, budget, fanin);
    let merged = sink.source();
    let out = CollectSink::new();
    let ps = vec![
        pipe("sort-in", src, vec![], Arc::new(sink), &[], tasks),
        pipe("sort-out", Arc::new(merged), vec![], Arc::new(Arc::clone(&out)), &[0], 1),
    ];
    (ps, out)
}

/// ORDER BY ... LIMIT n.
pub fn topn_query(src: Arc<dyn SourceFactory>, spec: SortSpec, n: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>) {
    let sink = TopNSink::new(n, spec);
    let out = CollectSink::new();
    let ps = vec![
        pipe("topn-in", src, vec![], Arc::new(Arc::clone(&sink)), &[], tasks),
        pipe("topn-out", Arc::new(Arc::clone(&sink.out)), vec![], Arc::new(Arc::clone(&out)), &[0], 1),
    ];
    (ps, out)
}

/// A window query: sort by (partition, order) then the window source.
pub fn window_query(src: Arc<dyn SourceFactory>, in_schema: &SchemaRef, win: Window, order: SortSpec, budget: &Arc<Budget>, fanin: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let spec = win.sort_spec(&order);
    let sink = SortSink::new(spec, budget, fanin);
    let sorted = sink.source();
    let win = Arc::new(win);
    let out_schema = win.out_schema(in_schema);
    let out = CollectSink::new();
    let ws = Arc::new(WindowSource { win, sorted, in_schema: in_schema.clone() });
    let ps = vec![
        pipe("win-sort", src, vec![], Arc::new(sink), &[], tasks),
        pipe("win-out", Arc::new(ws), vec![], Arc::new(Arc::clone(&out)), &[0], 1),
    ];
    (ps, out, out_schema)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    /// hash table in memory
    Mem,
    /// both sides split into this many spill partitions
    Grace(usize),
}

/// A join of two sources, output into a collect sink. Right and Full add the pipeline of the unmatched build rows.
pub fn join_query(
    probe: Arc<dyn SourceFactory>, probe_schema: &SchemaRef, build: Arc<dyn SourceFactory>, build_schema: &SchemaRef,
    spec: JoinSpec, mode: Mode, budget: &Arc<Budget>, tasks: usize,
) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let out = CollectSink::new();
    match mode {
        Mode::Mem => {
            let sh = JoinShared::new(spec, build_schema, probe_schema, budget);
            let schema = sh.out_schema();
            let mut ps = vec![
                pipe("build", build, vec![], Arc::new(JoinBuild(Arc::clone(&sh))), &[], tasks),
                pipe("probe", probe, vec![Arc::new(Probe(Arc::clone(&sh)))], Arc::new(Arc::clone(&out)), &[0], tasks),
            ];
            if matches!(spec.kind, JoinKind::Right | JoinKind::Full) {
                ps.push(pipe("unmatched", Arc::new(UnmatchedBuild(sh)), vec![], Arc::new(Arc::clone(&out)), &[1], tasks));
            }
            (ps, out, schema)
        }
        Mode::Grace(n) => {
            let bs = PartitionSink::new(spec.build_key, n, budget, "gbuild");
            let ps_ = PartitionSink::new(spec.probe_key, n, budget, "gprobe");
            let pj = PartitionJoin::new(spec, &bs, &ps_, build_schema, probe_schema, budget);
            let schema = pj.out_schema();
            let ps = vec![
                pipe("part-build", build, vec![], Arc::new(bs), &[], tasks),
                pipe("part-probe", probe, vec![], Arc::new(ps_), &[], tasks),
                pipe("part-join", Arc::new(pj), vec![], Arc::new(Arc::clone(&out)), &[0, 1], tasks),
            ];
            (ps, out, schema)
        }
    }
}

pub fn queue_source(batches: &[RecordBatch]) -> Arc<SharedQueue> {
    let q = SharedQueue::new();
    q.fill(batches.iter().cloned());
    q
}

// ------------------------------------------------------------------------------------------------ TPC-H style queries

pub struct Data {
    pub dir: String,
}

impl Data {
    fn t(&self, name: &str, cols: &[&str], prefix: Option<(&str, &str)>) -> Arc<ParquetSource> {
        ParquetSource::new(&format!("{}/{name}.parquet", self.dir), cols, prefix).unwrap_or_else(|e| panic!("{e}"))
    }
}

fn dec_scalar(v: i128, scale: i8) -> Scalar<Decimal128Array> {
    Scalar::new(Decimal128Array::from(vec![v]).with_precision_and_scale(38, scale).unwrap())
}

fn str_eq(col: usize, s: &'static str) -> FilterFn {
    FilterFn(Arc::new(move |b: &RecordBatch| {
        let c = cast(b.column(col), &DataType::Utf8).unwrap();
        cmp::eq(&c, &Scalar::new(StringArray::from(vec![s]))).unwrap()
    }))
}

fn date_gt(a: usize, b_: usize) -> FilterFn {
    FilterFn(Arc::new(move |b: &RecordBatch| cmp::gt(b.column(a), b.column(b_)).unwrap()))
}

fn keep(cols: &'static [usize]) -> ProjectFn {
    ProjectFn(Arc::new(move |b: &RecordBatch| {
        let schema = Arc::new(Schema::new(cols.iter().map(|&c| b.schema().field(c).clone()).collect::<Vec<_>>()));
        RecordBatch::try_new(schema, cols.iter().map(|&c| b.column(c).clone()).collect()).unwrap()
    }))
}

/// Appends `g = 0` so that a global aggregate can be written as a one-group GROUP BY.
fn with_zero_group() -> ProjectFn {
    ProjectFn(Arc::new(|b: &RecordBatch| {
        let mut f: Vec<Field> = b.schema().fields().iter().map(|f| f.as_ref().clone()).collect();
        f.push(Field::new("g", DataType::Int64, false));
        let mut cols = b.columns().to_vec();
        cols.push(Arc::new(Int64Array::from(vec![0i64; b.num_rows()])) as ArrayRef);
        RecordBatch::try_new(Arc::new(Schema::new(f)), cols).unwrap()
    }))
}

/// TPC-H Q18 (large volume customers): semi join on an aggregate, two inner joins, an aggregate, a top-100.
pub fn q18(d: &Data, budget: &Arc<Budget>, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let li = d.t("lineitem", &["l_orderkey", "l_quantity"], None);
    let a0 = HashAgg::new(&li.schema(), &[0], &[AggFn::Sum(1)], &["sum_qty"]);
    let big = FilterFn(Arc::new(|b: &RecordBatch| cmp::gt(b.column(1), &dec_scalar(30000, 2)).unwrap()));
    let orders = d.t("orders", &["o_orderkey", "o_custkey", "o_totalprice", "o_orderdate"], None);
    let semi = JoinShared::new(JoinSpec { kind: JoinKind::Semi, probe_key: 0, build_key: 0, residual_ne: None }, &a0.out_schema(), &orders.schema(), budget);
    let cust = d.t("customer", &["c_custkey", "c_name"], None);
    // orders (after the semi join) is the BUILD side of the lineitem join
    let oj = JoinShared::new(JoinSpec { kind: JoinKind::Inner, probe_key: 0, build_key: 0, residual_ne: None }, &semi.out_schema(), &li.schema(), budget);
    let cj = JoinShared::new(JoinSpec { kind: JoinKind::Inner, probe_key: 3, build_key: 0, residual_ne: None }, &cust.schema(), &oj.out_schema(), budget);
    let li2 = d.t("lineitem", &["l_orderkey", "l_quantity"], None);
    let a4 = HashAgg::new(&cj.out_schema(), &[7, 6, 2, 5, 4], &[AggFn::Sum(1)], &["sum_qty"]);
    let topn = TopNSink::new(100, SortSpec { keys: vec![SortKey { col: 4, desc: true, nulls_first: false }, SortKey { col: 3, desc: false, nulls_first: false }] });
    let out = CollectSink::new();
    let out_schema = a4.out_schema();
    let ps = vec![
        pipe("q18-agg", Arc::new(Arc::clone(&li)), vec![], Arc::new(Arc::clone(&a0)), &[], tasks),
        pipe("q18-big", Arc::new(Arc::clone(&a0.out)), vec![Arc::new(big)], Arc::new(JoinBuild(Arc::clone(&semi))), &[0], tasks),
        pipe("q18-orders", Arc::new(Arc::clone(&orders)), vec![Arc::new(Probe(Arc::clone(&semi)))], Arc::new(JoinBuild(Arc::clone(&oj))), &[1], tasks),
        pipe("q18-cust", Arc::new(Arc::clone(&cust)), vec![], Arc::new(JoinBuild(Arc::clone(&cj))), &[], tasks),
        pipe("q18-li", Arc::new(li2), vec![Arc::new(Probe(Arc::clone(&oj))), Arc::new(Probe(Arc::clone(&cj)))], Arc::new(Arc::clone(&a4)), &[2, 3], tasks),
        pipe("q18-top", Arc::new(Arc::clone(&a4.out)), vec![], Arc::new(Arc::clone(&topn)), &[4], tasks),
        pipe("q18-out", Arc::new(Arc::clone(&topn.out)), vec![], Arc::new(Arc::clone(&out)), &[5], 1),
    ];
    (ps, out, out_schema)
}

/// TPC-H Q21 (suppliers who kept orders waiting): EXISTS and NOT EXISTS as semi and anti joins with a `<>` residual.
pub fn q21(d: &Data, budget: &Arc<Budget>, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let ji = |kind, pk, bk, resid| JoinSpec { kind, probe_key: pk, build_key: bk, residual_ne: resid };
    let nation = d.t("nation", &["n_nationkey", "n_name"], None);
    let nj = JoinShared::new(ji(JoinKind::Inner, 2, 0, None), &nation.schema(), &d.t("supplier", &["s_suppkey", "s_name", "s_nationkey"], None).schema(), budget);
    let supplier = d.t("supplier", &["s_suppkey", "s_name", "s_nationkey"], None);
    let sj_schema = nj.out_schema(); // supplier ++ nation: s_suppkey, s_name, s_nationkey, n_nationkey, n_name
    let l1 = d.t("lineitem", &["l_orderkey", "l_suppkey", "l_receiptdate", "l_commitdate"], None);
    let sj = JoinShared::new(ji(JoinKind::Inner, 1, 0, None), &sj_schema, &l1.schema(), budget);
    let orders = d.t("orders", &["o_orderkey", "o_orderstatus"], None);
    let oj = JoinShared::new(ji(JoinKind::Inner, 0, 0, None), &orders.schema(), &sj.out_schema(), budget);
    let l2 = d.t("lineitem", &["l_orderkey", "l_suppkey"], Some(("l_", "l2_")));
    let semi = JoinShared::new(ji(JoinKind::Semi, 0, 0, Some((1, 1))), &l2.schema(), &oj.out_schema(), budget);
    let l3 = d.t("lineitem", &["l_orderkey", "l_suppkey", "l_receiptdate", "l_commitdate"], Some(("l_", "l3_")));
    let l3_schema = Arc::new(Schema::new(vec![l3.schema().field(0).clone(), l3.schema().field(1).clone()]));
    let anti = JoinShared::new(ji(JoinKind::Anti, 0, 0, Some((1, 1))), &l3_schema, &semi.out_schema(), budget);
    let s_name_idx = 5usize; // l1 (4 cols), then s_suppkey, s_name
    let agg_in = Arc::new(Schema::new(vec![anti.out_schema().field(s_name_idx).clone()]));
    let agg = HashAgg::new(&agg_in, &[0], &[AggFn::CountStar], &["numwait"]);
    let topn = TopNSink::new(100, SortSpec { keys: vec![SortKey { col: 1, desc: true, nulls_first: false }, SortKey { col: 0, desc: false, nulls_first: false }] });
    let out = CollectSink::new();
    let out_schema = agg.out_schema();
    let ps = vec![
        pipe("q21-nation", Arc::new(Arc::clone(&nation)), vec![Arc::new(str_eq(1, "SAUDI ARABIA"))], Arc::new(JoinBuild(Arc::clone(&nj))), &[], tasks),
        pipe("q21-supplier", Arc::new(Arc::clone(&supplier)), vec![Arc::new(Probe(Arc::clone(&nj)))], Arc::new(JoinBuild(Arc::clone(&sj))), &[0], tasks),
        pipe("q21-orders", Arc::new(Arc::clone(&orders)), vec![Arc::new(str_eq(1, "F"))], Arc::new(JoinBuild(Arc::clone(&oj))), &[], tasks),
        pipe("q21-l2", Arc::new(Arc::clone(&l2)), vec![], Arc::new(JoinBuild(Arc::clone(&semi))), &[], tasks),
        pipe("q21-l3", Arc::new(Arc::clone(&l3)), vec![Arc::new(date_gt(2, 3)), Arc::new(keep(&[0, 1]))], Arc::new(JoinBuild(Arc::clone(&anti))), &[], tasks),
        pipe(
            "q21-l1",
            Arc::new(Arc::clone(&l1)),
            vec![
                Arc::new(date_gt(2, 3)),
                Arc::new(Probe(Arc::clone(&sj))),
                Arc::new(Probe(Arc::clone(&oj))),
                Arc::new(Probe(Arc::clone(&semi))),
                Arc::new(Probe(Arc::clone(&anti))),
                Arc::new(keep(&[5])),
            ],
            Arc::new(Arc::clone(&agg)),
            &[1, 2, 3, 4],
            tasks,
        ),
        pipe("q21-top", Arc::new(Arc::clone(&agg.out)), vec![], Arc::new(Arc::clone(&topn)), &[5], tasks),
        pipe("q21-out", Arc::new(Arc::clone(&topn.out)), vec![], Arc::new(Arc::clone(&out)), &[6], 1),
    ];
    (ps, out, out_schema)
}

/// Join kinds over customer (probe) and orders (build), aggregated to counts so the output is tiny:
/// `select count(*), count(c_custkey), count(o_orderkey) from customer <kind> join orders on c_custkey = o_custkey`
/// (semi and anti: `count(*)` of customer rows with / without an order).
pub fn join_counts(d: &Data, kind: JoinKind, mode: Mode, budget: &Arc<Budget>, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let cust = d.t("customer", &["c_custkey"], None);
    let orders = d.t("orders", &["o_orderkey", "o_custkey"], None);
    let spec = JoinSpec { kind, probe_key: 0, build_key: 1, residual_ne: None };
    let (mut ps, _, jschema) = join_query(Arc::new(Arc::clone(&cust)), &cust.schema(), Arc::new(Arc::clone(&orders)), &orders.schema(), spec, mode, budget, tasks);
    // replace the collect sink of the join pipelines by the aggregate: every pipeline that fed `out` now feeds a queue
    let q = SharedQueue::new();
    let qs = Arc::new(Arc::clone(&q));
    let last_join_pipes: Vec<usize> = (0..ps.len()).filter(|&i| ps[i].name == "probe" || ps[i].name == "unmatched" || ps[i].name == "part-join").collect();
    for &i in &last_join_pipes {
        ps[i].sink = Arc::new(QueueSink(Arc::clone(&q)));
    }
    let agg_in = Arc::new(Schema::new(jschema.fields().iter().map(|f| f.as_ref().clone()).chain([Field::new("g", DataType::Int64, false)]).collect::<Vec<_>>()));
    let aggs: Vec<AggFn> = match kind {
        JoinKind::Semi | JoinKind::Anti => vec![AggFn::CountStar],
        _ => vec![AggFn::CountStar, AggFn::Count(0), AggFn::Count(1)],
    };
    let names: &[&str] = if aggs.len() == 1 { &["n"] } else { &["n", "c", "o"] };
    let agg = HashAgg::new(&agg_in, &[agg_in.fields().len() - 1], &aggs, names);
    let out = CollectSink::new();
    let n = ps.len();
    ps.push(pipe("counts", qs as _, vec![Arc::new(with_zero_group())], Arc::new(Arc::clone(&agg)), &last_join_pipes, tasks));
    ps.push(pipe("counts-out", Arc::new(Arc::clone(&agg.out)), vec![], Arc::new(Arc::clone(&out)), &[n], 1));
    (ps, out, agg.out_schema())
}

/// Several pipelines feed one downstream pipeline through a queue (a stand-in for a UNION or a multi-input sink: the protocol's `finalize`
/// runs once per pipeline, so the downstream pipeline cannot be a shared sink of both).
pub struct QueueSink(pub Arc<SharedQueue>);

struct QueueLocal(Arc<SharedQueue>);

impl GlobalSink for QueueSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(QueueLocal(Arc::clone(&self.0)))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for QueueLocal {
    fn push(&mut self, input: &mut crate::batch::Batch, _w: &Waker) -> Result<SinkResult> {
        if !input.is_empty() {
            self.0.fill([input.rb().clone()]);
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// A Q13 variant (no LIKE): customers by number of orders, with a LEFT join, two aggregates and a sort.
/// `select c_count, count(*) custdist from (select c_custkey, count(o_orderkey) c_count from customer left join orders on c_custkey = o_custkey group by c_custkey) group by c_count order by custdist desc, c_count desc`
pub fn q13_nolike(d: &Data, mode: Mode, budget: &Arc<Budget>, fanin: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let cust = d.t("customer", &["c_custkey"], None);
    let orders = d.t("orders", &["o_orderkey", "o_custkey"], None);
    let spec = JoinSpec { kind: JoinKind::Left, probe_key: 0, build_key: 1, residual_ne: None };
    let (mut ps, _, jschema) = join_query(Arc::new(Arc::clone(&cust)), &cust.schema(), Arc::new(Arc::clone(&orders)), &orders.schema(), spec, mode, budget, tasks);
    let j = ps.iter().position(|p| p.name == "probe" || p.name == "part-join").unwrap();
    let a1 = HashAgg::new(&jschema, &[0], &[AggFn::Count(1)], &["c_count"]); // jschema: c_custkey, o_orderkey, o_custkey
    ps[j].sink = Arc::new(Arc::clone(&a1));
    let a2 = HashAgg::new(&a1.out_schema(), &[1], &[AggFn::CountStar], &["custdist"]); // (c_custkey, c_count) -> group by c_count
    let sort = SortSink::new(SortSpec { keys: vec![SortKey { col: 1, desc: true, nulls_first: false }, SortKey { col: 0, desc: true, nulls_first: false }] }, budget, fanin);
    let merged = sort.source();
    let out = CollectSink::new();
    let n = ps.len();
    ps.push(pipe("q13-agg2", Arc::new(Arc::clone(&a1.out)), vec![], Arc::new(Arc::clone(&a2)), &[j], tasks));
    ps.push(pipe("q13-sort", Arc::new(Arc::clone(&a2.out)), vec![], Arc::new(sort), &[n], tasks));
    ps.push(pipe("q13-out", Arc::new(merged), vec![], Arc::new(Arc::clone(&out)), &[n + 1], 1));
    (ps, out, a2.out_schema())
}

/// `select o_orderkey, o_custkey, o_totalprice, o_orderstatus, <window functions> from orders`
/// `peers = true`: partition by o_custkey order by o_orderstatus (many ties): rank, dense_rank, sum over the partition.
/// `peers = false`: order by o_totalprice desc, o_orderkey (no ties): row_number, lag, sum over the partition.
pub fn window_orders(d: &Data, peers: bool, budget: &Arc<Budget>, fanin: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let orders = d.t("orders", &["o_orderkey", "o_custkey", "o_totalprice", "o_orderstatus"], None);
    let (order, fns): (SortSpec, Vec<(crate::window::WinFn, &'static str)>) = if peers {
        (
            SortSpec::asc(&[3]),
            vec![(crate::window::WinFn::Rank, "rk"), (crate::window::WinFn::DenseRank, "dr"), (crate::window::WinFn::SumPartition(2), "tot")],
        )
    } else {
        (
            SortSpec { keys: vec![SortKey { col: 2, desc: true, nulls_first: false }, SortKey { col: 0, desc: false, nulls_first: false }] },
            vec![(crate::window::WinFn::RowNumber, "rn"), (crate::window::WinFn::Lag(2), "prev"), (crate::window::WinFn::SumPartition(2), "tot")],
        )
    };
    let order_cols: Vec<usize> = order.keys.iter().map(|k| k.col).collect();
    let win = Window { partition: vec![1], order: order_cols, fns };
    window_query(Arc::new(Arc::clone(&orders)), &orders.schema(), win, order, budget, fanin, tasks)
}

pub fn lineitem_sort(d: &Data, budget: &Arc<Budget>, fanin: usize, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let li = d.t("lineitem", &["l_orderkey", "l_linenumber", "l_shipdate", "l_extendedprice", "l_comment"], None);
    let spec = SortSpec { keys: vec![SortKey { col: 2, desc: true, nulls_first: true }, SortKey { col: 0, desc: false, nulls_first: false }, SortKey { col: 1, desc: false, nulls_first: false }] };
    let (ps, out) = sort_query(Arc::new(Arc::clone(&li)), spec, budget, fanin, tasks);
    (ps, out, li.schema())
}

pub fn lineitem_topn(d: &Data, tasks: usize) -> (Vec<Pipeline>, Arc<CollectSink>, SchemaRef) {
    let li = d.t("lineitem", &["l_orderkey", "l_linenumber", "l_extendedprice"], None);
    let spec = SortSpec { keys: vec![SortKey { col: 2, desc: true, nulls_first: false }, SortKey { col: 0, desc: false, nulls_first: false }, SortKey { col: 1, desc: false, nulls_first: false }] };
    let (ps, out) = topn_query(Arc::new(Arc::clone(&li)), spec, 1000, tasks);
    (ps, out, li.schema())
}

