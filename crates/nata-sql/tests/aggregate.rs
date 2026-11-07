mod common;
use common::*;
use nata_sql::{ErrorClass, types::SqlType};
#[test]
fn result_types_and_phases() {
    let _constants =
        ok("SELECT true, 'one', 1.5, count(*) FROM 't.parquet' GROUP BY true, 'one', 1.5");
    let _case = ok("SELECT A % 2 FROM 't.parquet' GROUP BY a % 2 HAVING A % 2 > 0");
    let p = ok(
        "SELECT sum(ti),sum(si),sum(a),sum(c),sum(d_15_2),avg(a),avg(c),avg(d_15_2),min(s),max(f),count(*),count(a) FROM 't.parquet'",
    );
    let types: Vec<_> = p.output.iter().map(|c| c.ty).collect();
    use SqlType::*;
    assert_eq!(
        types,
        vec![
            BigInt,
            BigInt,
            BigInt,
            Decimal {
                precision: 38,
                scale: 0
            },
            Decimal {
                precision: 38,
                scale: 2
            },
            Decimal {
                precision: 16,
                scale: 6
            },
            Decimal {
                precision: 25,
                scale: 6
            },
            Decimal {
                precision: 21,
                scale: 8
            },
            Varchar,
            Float,
            BigInt,
            BigInt
        ]
    );
    let p = ok("SELECT a % 2, sum(c) FROM 't.parquet' GROUP BY a % 2 HAVING count(*) > 2");
    let text = p.plan.explain_text();
    assert!(text.contains("phases=partial -> hash partition -> final (unordered)"));
    assert!(text.contains("Having"));
}
#[test]
fn positioned_unsupported_and_ungrouped_references() {
    for (q, part) in [
        (
            "SELECT count(DISTINCT a) FROM 't.parquet'",
            "DISTINCT aggregates",
        ),
        (
            "SELECT count(*) FILTER (WHERE a > 0) FROM 't.parquet'",
            "aggregate FILTER",
        ),
        ("SELECT sum(sum(a)) FROM 't.parquet'", "nested aggregates"),
        (
            "SELECT a FROM 't.parquet' WHERE count(*) > 1",
            "aggregates in WHERE",
        ),
        ("SELECT sum(d) FROM 't.parquet'", "sum(DOUBLE)"),
        ("SELECT a FROM 't.parquet' GROUP BY 1", "GROUP BY ordinals"),
        (
            "SELECT a % 2 AS k FROM 't.parquet' GROUP BY k",
            "GROUP BY aliases",
        ),
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::NotSupported, "{q}: {e}");
        assert!(e.message.contains(part), "{e}");
        assert!(e.pos.col > 0);
    }
    for q in [
        "SELECT a,sum(b) FROM 't.parquet'",
        "SELECT a FROM 't.parquet' GROUP BY b",
        "SELECT count(*) FROM 't.parquet' HAVING a > 0",
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::Bind);
        assert!(e.message.contains("ungrouped column"));
        assert!(e.pos.col > 0);
    }
}
