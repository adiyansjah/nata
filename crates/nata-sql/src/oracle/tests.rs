//! The bake-off comparison, kept as tests: the real parser (`crate::parse`) against the hand-written oracle (`super::hand`).

use super::hand::{self, Mutant};
use crate::ast::{ExprKind, Query, SelectItem};
use crate::parse::parse_query;
use crate::testutil::*;

fn real(q: &str) -> Result<Query, crate::error::SqlError> {
    parse_query(q)
}

#[test]
fn accepted_queries_give_the_same_tree_in_both_parsers() {
    let mut n = 0;
    for r in testset().iter().filter(|r| r.expect == "accept") {
        let h =
            hand::parse(&r.q).unwrap_or_else(|e| panic!("oracle refused {} {:?}: {e}", r.id, r.q));
        let x =
            real(&r.q).unwrap_or_else(|e| panic!("sqlparser path refused {} {:?}: {e}", r.id, r.q));
        assert_eq!(h.sexp(), x.sexp(), "{} {:?}", r.id, r.q);
        assert_eq!(h, x, "{} {:?}", r.id, r.q);
        n += 1;
    }
    assert!(
        n >= 200,
        "the test set has {n} in-subset queries, the card asks for at least 80"
    );
}

#[test]
fn queries_outside_the_subset_are_refused_by_both_parsers_with_a_position() {
    let mut n = 0;
    for r in testset().iter().filter(|r| r.expect == "refuse") {
        let h = hand::parse(&r.q);
        assert!(h.is_err(), "oracle accepted {} {:?}", r.id, r.q);
        let x = real(&r.q);
        let e = x
            .as_ref()
            .err()
            .unwrap_or_else(|| panic!("real parser accepted {} {:?}", r.id, r.q));
        assert!(
            e.pos.line >= 1 && e.pos.col >= 1,
            "{} {:?}: no position: {e}",
            r.id,
            r.q
        );
        n += 1;
    }
    assert!(
        n >= 100,
        "{n} queries outside the subset, the card asks for at least 40"
    );
}

#[test]
fn edge_cases_have_the_same_outcome_except_the_listed_ones() {
    // dialect corner cases with no fixed expectation: the two parsers agree on accept or refuse, except where the layer is deliberately stricter
    // or the oracle is (the list is the whole disagreement: a new line here is a decision)
    let known_different: &[&str] = &[
        // sqlparser takes `a = NOT b` (a prefix NOT as the right operand of a comparison); PostgreSQL and the oracle refuse it. Harmless: one tree.
        "[not-in-prefix]",
        // a no-break space is white space for DuckDB (and the oracle) and a letter of a name for sqlparser, which then refuses the query.
        "[nonascii-whitespace]",
    ];
    let mut differ = Vec::new();
    for r in testset().iter().filter(|r| r.expect == "edge") {
        let h = hand::parse(&r.q).map(|q| q.sexp());
        let x = real(&r.q).map(|q| q.sexp());
        if h.is_ok() != x.is_ok() || (h.is_ok() && h.as_ref().ok() != x.as_ref().ok()) {
            differ.push(format!(
                "{} [{}] {:?}: oracle {:?}, real {:?}",
                r.id,
                r.tag,
                r.q,
                h.as_ref().map_err(|e| e.to_string()),
                x.as_ref().map_err(|e| e.to_string())
            ));
        }
    }
    for d in &differ {
        eprintln!("DIFFERENT: {d}");
    }
    let unexpected: Vec<&String> = differ
        .iter()
        .filter(|d| !known_different.iter().any(|k| d.contains(k)))
        .collect();
    assert!(unexpected.is_empty(), "{unexpected:#?}");
}

fn numbers(e: &crate::ast::Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Number(t) => out.push(t.clone()),
        ExprKind::Unary { expr, .. }
        | ExprKind::IsNull { expr, .. }
        | ExprKind::Cast { expr, .. } => numbers(expr, out),
        ExprKind::Binary { left, right, .. } => {
            numbers(left, out);
            numbers(right, out);
        }
        _ => {}
    }
}

const LITERALS: [&str; 16] = [
    "0.1",
    "0.10",
    "-0.0",
    "123456789012345678901234567890.12",
    "1e400",
    "1E-400",
    "1.5e+3",
    ".5",
    "5.",
    "00012",
    "99999999999999999999999999999999999999",
    "0.000000000000000000000000000000000001",
    "4.9e-324",
    "1.7976931348623157e308",
    "9007199254740993",
    "18446744073709551616",
];

fn literal_texts(parse: &dyn Fn(&str) -> Option<Query>) -> Vec<String> {
    LITERALS
        .iter()
        .map(|l| {
            let q = parse(&format!("SELECT {l} AS r FROM 'f.parquet'")).expect("parses");
            let SelectItem::Expr { expr, .. } = &q.items[0] else {
                panic!("item")
            };
            let mut v = Vec::new();
            numbers(expr, &mut v);
            assert_eq!(v.len(), 1);
            v.remove(0)
        })
        .collect()
}

#[test]
fn numeric_literals_keep_their_exact_text_in_both_parsers_and_a_float_round_trip_fails_the_check() {
    let expect: Vec<String> = LITERALS
        .iter()
        .map(|l| l.trim_start_matches('-').to_string())
        .collect();
    assert_eq!(literal_texts(&|q| hand::parse(q).ok()), expect);
    assert_eq!(literal_texts(&|q| real(q).ok()), expect);
    let bad = literal_texts(&|q| hand::parse_with(q, Mutant::NumberViaF64).ok());
    let changed = bad.iter().zip(&expect).filter(|(b, e)| b != e).count();
    assert!(
        changed >= 10,
        "only {changed} of 16 literals changed through f64: the check cannot fail"
    );
}

#[test]
fn the_comparison_catches_known_bad_oracles() {
    let set = accepted_queries();
    let differs = |m: Mutant| {
        set.iter()
            .filter(|q| {
                hand::parse_with(q, m).ok().map(|x| x.sexp()) != real(q).ok().map(|x| x.sexp())
            })
            .count()
    };
    assert_eq!(differs(Mutant::None), 0, "the real oracle is the control");
    for (m, why) in [
        (Mutant::OrBeforeAnd, "OR binds tighter than AND"),
        (Mutant::LoseNot, "a NOT is dropped"),
        (Mutant::NumberViaF64, "a decimal literal goes through f64"),
    ] {
        assert!(
            differs(m) > 0,
            "the comparison missed the mutant where {why}"
        );
    }
}

#[test]
fn generated_expressions_give_the_same_tree_in_both_parsers_and_in_both_renderings() {
    let mut rng = Rng(20_261_106);
    let (mut n, mut agree) = (0, 0);
    for i in 0..3000 {
        let e = gen_expr(&mut rng, 4);
        let where_ = i % 2 == 0;
        let (full, min) = (query_of(&e, false, where_), query_of(&e, true, where_));
        let h = hand::parse(&full);
        let x = real(&full);
        let hm = hand::parse(&min);
        let xm = real(&min);
        n += 1;
        // the generator may write something outside the subset (a comparison of comparisons); the two parsers must still agree
        assert_eq!(
            h.is_ok(),
            x.is_ok(),
            "seed 20261106 query {i}: {full:?}\n oracle {h:?}\n real {x:?}"
        );
        assert_eq!(hm.is_ok(), xm.is_ok(), "seed 20261106 query {i}: {min:?}");
        if let (Ok(h), Ok(x), Ok(hm), Ok(xm)) = (h, x, hm, xm) {
            assert_eq!(h.sexp(), x.sexp(), "seed 20261106 query {i}: {full:?}");
            assert_eq!(hm.sexp(), xm.sexp(), "seed 20261106 query {i}: {min:?}");
            assert_eq!(
                x.sexp(),
                xm.sexp(),
                "the two renderings of one tree, query {i}: {min:?}"
            );
            agree += 1;
        }
    }
    assert!(
        agree * 10 > n * 7,
        "only {agree} of {n} generated queries parse"
    );
}
