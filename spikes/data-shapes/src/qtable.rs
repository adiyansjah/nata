//! The query table of the hash-ops engine runs: every query as ONE value that yields both the engine plan (read by
//! `bin/shapes.rs`) and the DuckDB SQL text (`sql()`), so the two cannot drift apart. Pure std: also compiled alone
//! by `sqltool.rs` so the SQL can be checked without building the engine.
pub type S = &'static str;

/// A filter on one side, in the engine (`FilterDec`, `FilterStrIn`) and in SQL.
#[derive(Clone, Copy)]
pub enum Flt {
    Between(S, i64, i64),
    In(S, &'static [S]),
}

impl Flt {
    pub fn col(&self) -> S {
        match *self {
            Flt::Between(c, ..) | Flt::In(c, _) => c,
        }
    }

    pub fn sql(&self) -> String {
        match *self {
            Flt::Between(c, lo, hi) => format!("{c} between {lo} and {hi}"),
            Flt::In(c, set) => format!("{c} in ({})", set.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ")),
        }
    }
}

/// A group key column; `date` keys are printed as days since 1970-01-01 (what the engine holds).
#[derive(Clone, Copy)]
pub struct Key {
    pub col: S,
    pub date: bool,
}

#[derive(Clone)]
pub struct Side {
    pub table: S,
    pub filters: Vec<Flt>,
}

pub enum Shape {
    Agg { t: Side, keys: Vec<Key>, sums: Vec<S> },
    /// `probe JOIN build ON pkey = bkey`; the output of the join has `pout` (probe columns) then `bpay`; `keys` and
    /// `sums` name columns of that output.
    Join { build: Side, bkey: S, bpay: Vec<S>, probe: Side, pkey: S, pout: Vec<S>, keys: Vec<Key>, sums: Vec<S> },
}

pub struct Query {
    pub name: String,
    pub tag: String,
    pub shape: Shape,
}

fn st(s: String) -> S {
    Box::leak(s.into_boxed_str())
}

fn keys(v: &[S]) -> Vec<Key> {
    v.iter().map(|&c| if let Some(d) = c.strip_prefix("d:") { Key { col: d, date: true } } else { Key { col: c, date: false } }).collect()
}

fn side(table: S, filters: &[Flt]) -> Side {
    Side { table, filters: filters.to_vec() }
}

fn agg(name: &str, tag: &str, table: S, filters: &[Flt], k: &[S], sums: &[S]) -> Query {
    Query { name: name.into(), tag: tag.into(), shape: Shape::Agg { t: side(table, filters), keys: keys(k), sums: sums.to_vec() } }
}

#[allow(clippy::too_many_arguments)]
fn join(name: &str, tag: &str, build: Side, bkey: S, bpay: &[S], probe: Side, pkey: S, pout: &[S], k: &[S], sums: &[S]) -> Query {
    Query {
        name: name.into(),
        tag: tag.into(),
        shape: Shape::Join { build, bkey, bpay: bpay.to_vec(), probe, pkey, pout: pout.to_vec(), keys: keys(k), sums: sums.to_vec() },
    }
}

/// Every query. Tags: skew, hot, str, nul<PP> (PP = 00 01 10 50 90 99 100), nulshape.
pub fn queries() -> Vec<Query> {
    let mut q = Vec::new();
    let w_mid = Flt::Between("w", -250_000, 250_000);

    // ---- (1) skewed keys: tag skew --------------------------------------------------------------------------
    q.push(agg("sk_agg_k10", "skew", "skew_fact", &[], &["k10"], &["v", "w"]));
    q.push(agg("sk_agg_k15", "skew", "skew_fact", &[], &["k15"], &["v", "w"]));
    q.push(agg("sk_agg_hot", "skew", "skew_fact", &[], &["khot"], &["v"]));
    q.push(agg("sk_agg_uni", "skew", "skew_fact", &[], &["kuni"], &["v"]));
    q.push(agg("sk_agg_mode_filt", "skew", "skew_fact", &[w_mid], &["l_shipmode"], &["v", "w"]));
    q.push(agg("sk_agg_k15_mode", "skew", "skew_fact", &[], &["k15", "l_shipmode"], &["v"]));
    let fact = side("skew_fact", &[]);
    let dim = side("skew_dim", &[]);
    for (name, key) in [("sk_join_k10", "k10"), ("sk_join_k15", "k15")] {
        q.push(join(name, "skew", dim.clone(), "dk", &["dg", "dp"], fact.clone(), key, &["v"], &["dg"], &["v", "dp"]));
    }
    // dictionary payload from the build side, hot probe key kept / dropped by a build-side filter
    q.push(join("sk_join_hot", "skew", dim.clone(), "dk", &["o_orderpriority"], fact.clone(), "khot", &["v"], &["o_orderpriority"], &["v"]));
    q.push(join("sk_join_hot_df0", "skew", side("skew_dim", &[Flt::Between("df", 0, 0)]), "dk", &["o_orderpriority"], fact.clone(), "khot", &["v"], &["o_orderpriority"], &["v"]));
    q.push(join("sk_join_hot_df1", "skew", side("skew_dim", &[Flt::Between("df", 1, 1)]), "dk", &["o_orderpriority"], fact.clone(), "khot", &["v"], &["o_orderpriority"], &["v"]));
    // the skewed probe key is also the group key after the join
    q.push(join("sk_join_k15_key", "skew", dim.clone(), "dk", &["dp"], fact.clone(), "k15", &["k15"], &["k15"], &["dp"]));
    // the FACT is the build side: the hot key is a chain of 30 percent (khot) or 29 percent (k15) of N rows
    q.push(join("sk_join_build_hot", "skew", fact.clone(), "khot", &["v"], dim.clone(), "dk", &["dg"], &["dg"], &["v"]));
    q.push(join("sk_join_build_k15", "skew", fact.clone(), "k15", &["v"], dim.clone(), "dk", &["dg"], &["dg"], &["v"]));

    // ---- (1b) hot key on the build side, chain length scaling: tag hot ---------------------------------------
    // hb_chain_small builds on the first B/4 rows of hot_build (bid < B/4: a chain of 0.3 * B/4 rows), hb_chain_big on
    // all of them. The filter constant comes from the harness through SHAPES_B (the number of build rows).
    let b = std::env::var("SHAPES_B").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(2_000_000);
    let probe = side("hot_probe", &[]);
    q.push(join("hb_chain_small", "hot", side("hot_build", &[Flt::Between("bid", 0, b / 4 - 1)]), "bk", &["bg", "bv"], probe.clone(), "pk", &["pg", "pv"], &["pg", "bg"], &["pv", "bv"]));
    q.push(join("hb_chain_big", "hot", side("hot_build", &[]), "bk", &["bg", "bv"], probe.clone(), "pk", &["pg", "pv"], &["pg", "bg"], &["pv", "bv"]));
    q.push(join("hb_hot_hot", "hot", side("hh_b", &[]), "bk2", &["bg2", "bv2"], side("hh_a", &[]), "ak", &["ag", "av"], &["ag", "bg2"], &["av", "bv2"]));

    // ---- (2) string-heavy: tag str ---------------------------------------------------------------------------
    q.push(agg("st_agg_hi", "str", "str_fact", &[], &["s_hi"], &["v"]));
    q.push(agg("st_agg_long", "str", "str_fact", &[], &["s_long"], &["v", "w"]));
    q.push(agg("st_agg_empty", "str", "str_fact", &[], &["s_empty"], &["v"]));
    q.push(agg("st_agg_utf8", "str", "str_fact", &[], &["s_utf8"], &["v"]));
    q.push(agg("st_agg_last", "str", "str_fact", &[], &["s_last"], &["v"]));
    q.push(agg("st_agg_first", "str", "str_fact", &[], &["s_first"], &["v"]));
    q.push(agg("st_agg_len", "str", "str_fact", &[], &["s_len"], &["v"]));
    q.push(agg("st_agg_instr", "str", "str_fact", &[], &["l_shipinstruct"], &["v"]));
    q.push(agg("st_agg_pair", "str", "str_fact", &[], &["s_empty", "s_utf8"], &["v"]));
    q.push(agg("st_agg_filt", "str", "str_fact", &[w_mid], &["s_hi"], &["w"]));
    q.push(agg("st_agg_instr_filt", "str", "str_fact", &[Flt::In("s_empty", &["", "e1", "e2", "e3"])], &["l_shipinstruct"], &["v"]));
    let sf = side("str_fact", &[]);
    q.push(join("st_join_long", "str", side("str_dim", &[]), "dkey", &["dpay", "dtag"], sf.clone(), "s_long", &["v"], &["dtag", "dpay"], &["v"]));
    q.push(join("st_join_hi", "str", side("str_dim2", &[]), "dkey2", &["dpay2"], sf.clone(), "s_hi", &["v"], &["dpay2"], &["v"]));
    q.push(join("st_join_instr", "str", side("str_dim3", &[]), "dkey3", &["dpay3"], sf.clone(), "l_shipinstruct", &["v"], &["dpay3"], &["v"]));
    q.push(join("st_join_utf8", "str", side("str_dim4", &[]), "dku", &["dpu"], sf.clone(), "s_utf8", &["v"], &["dpu"], &["v"]));
    q.push(join("st_join_empty_fanout", "str", side("str_small", &[]), "sk", &["sg"], sf.clone(), "s_empty", &["v"], &["sg"], &["v"]));

    // ---- (3) NULLs: tags nul<PP> and nulshape ----------------------------------------------------------------
    for pp in ["00", "01", "10", "50", "90", "99", "100"] {
        let tag = format!("nul{pp}");
        let t = st(format!("nul_p{pp}"));
        let n = |s: &str| format!("{s}_p{pp}");
        q.push(agg(&n("nl_agg_kn"), &tag, t, &[], &["kn"], &["v", "w"]));
        q.push(agg(&n("nl_agg_multi"), &tag, t, &[], &["kn", "ks", "d:kd"], &["v"]));
        q.push(agg(&n("nl_agg_ksi"), &tag, t, &[], &["ks", "ki"], &["w"]));
        q.push(agg(&n("nl_agg_filt"), &tag, t, &[w_mid], &["ki"], &["v"]));
        let f = side(t, &[]);
        q.push(join(&n("nl_join_kn"), &tag, side("nul_dimn", &[]), "dk", &["dpay"], f.clone(), "kn", &["v"], &["dpay"], &["v"]));
        q.push(join(&n("nl_join_ks"), &tag, side("nul_sdim", &[]), "dks", &["dsp"], f.clone(), "ks", &["w"], &["dsp"], &["w"]));
    }
    let ns = side("nul_shape", &[]);
    q.push(agg("ns_agg_kb_al", "nulshape", "nul_shape", &[], &["kb_al"], &["v"]));
    q.push(agg("ns_agg_kb_un", "nulshape", "nul_shape", &[], &["kb_un"], &["v"]));
    q.push(agg("ns_agg_kr", "nulshape", "nul_shape", &[], &["kr"], &["v", "vr"]));
    q.push(agg("ns_agg_ksb", "nulshape", "nul_shape", &[], &["ksb"], &["v"]));
    q.push(agg("ns_agg_g_vb", "nulshape", "nul_shape", &[], &["g"], &["vb", "vr", "vall"]));
    q.push(agg("ns_agg_kall", "nulshape", "nul_shape", &[], &["kall"], &["v", "vall"]));
    q.push(agg("ns_agg_filt_vb", "nulshape", "nul_shape", &[Flt::Between("vb", 0, 499)], &["g"], &["v"]));
    q.push(join("ns_join_kb", "nulshape", side("nul_dimn", &[]), "dk", &["dpay"], ns.clone(), "kb_al", &["v"], &["dpay"], &["v"]));
    q.push(join("ns_join_kr", "nulshape", side("nul_dimn", &[]), "dk", &["dpay"], ns.clone(), "kr", &["v"], &["dpay"], &["v"]));
    q
}

pub fn key_sql(k: &Key) -> String {
    if k.date { format!("({} - date '1970-01-01')", k.col) } else { k.col.to_string() }
}

pub fn sql(s: &Shape) -> String {
    let (keys, sums, from, wheres): (&Vec<Key>, &Vec<S>, String, Vec<String>) = match s {
        Shape::Agg { t, keys, sums } => (keys, sums, t.table.to_string(), t.filters.iter().map(Flt::sql).collect()),
        Shape::Join { build, bkey, probe, pkey, keys, sums, .. } => (
            keys,
            sums,
            format!("{} join {} on {pkey} = {bkey}", probe.table, build.table),
            probe.filters.iter().chain(&build.filters).map(Flt::sql).collect(),
        ),
    };
    let sel: Vec<String> = keys.iter().map(key_sql).chain(["count(*)".to_string()]).chain(sums.iter().map(|c| format!("sum({c})"))).collect();
    let grp: Vec<String> = keys.iter().map(|k| k.col.to_string()).collect();
    let wh = if wheres.is_empty() { String::new() } else { format!(" where {}", wheres.join(" and ")) };
    format!("select {} from {from}{wh} group by {};", sel.join(", "), grp.join(", "))
}
