//! The query table of the generic-ops engine runs (its Q1 and Q6 on lineitem-shaped data sets). Pure std, see qtable.rs.
pub const PCTS: [&str; 7] = ["00", "01", "10", "50", "90", "99", "100"];

pub struct Q {
    pub name: String,
    pub tag: String,
    pub table: String,
    pub q6: bool,
    /// The engine must refuse this query loudly ("not supported yet"); a result would be wrong.
    pub refuse: bool,
}

pub fn queries() -> Vec<Q> {
    let mut v = Vec::new();
    let mut add = |set: String, tag: String, refuse_q1: bool| {
        for q6 in [false, true] {
            v.push(Q {
                name: format!("li_{}_{set}", if q6 { "q6" } else { "q1" }),
                tag: tag.clone(),
                table: format!("li_{set}"),
                q6,
                refuse: refuse_q1 && !q6,
            });
        }
    };
    for pp in PCTS {
        add(format!("p{pp}"), format!("li{pp}"), false);
    }
    add("nullkey".into(), "linullkey".into(), true);
    v
}

pub fn sql(q: &Q) -> String {
    if q.q6 {
        format!(
            "select sum(l_extendedprice * l_discount) from {} where l_shipdate >= date '1994-01-01' and l_shipdate < date '1995-01-01' and l_discount between 0.05 and 0.07 and l_quantity < 24;",
            q.table
        )
    } else {
        format!(
            "select l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice * (1 - l_discount)), count(*) from {} where l_shipdate <= date '1998-09-02' group by 1, 2;",
            q.table
        )
    }
}
