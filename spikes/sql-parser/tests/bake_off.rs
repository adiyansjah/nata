//! Tests of the bake-off itself. Each one can fail: the mutants at the end show that the comparisons catch known-bad parsers.
use spike_sql_parser::hand::{self, Mutant};
use spike_sql_parser::sqlp::{self, Dial, Opts};
use spike_sql_parser::*;
use std::panic::catch_unwind;

struct Row {
    id: String,
    q: String,
    expect: String,
    tag: String,
}

/// The test set (`corpus/testset.jsonl`, written by `corpus/testset.py`): a JSON line is {"id","q","expect","tag"} with only these escapes.
fn testset() -> Vec<Row> {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/testset.jsonl"))
            .expect("testset.jsonl");
    text.lines().map(row).collect()
}

fn row(line: &str) -> Row {
    let field = |name: &str| -> String {
        let key = format!("\"{name}\": \"");
        let s = line.find(&key).expect("field") + key.len();
        let mut out = String::new();
        let mut it = line[s..].chars();
        while let Some(c) = it.next() {
            match c {
                '"' => break,
                '\\' => match it.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('u') => {
                        let hex: String = it.by_ref().take(4).collect();
                        out.push(
                            char::from_u32(u32::from_str_radix(&hex, 16).expect("hex"))
                                .expect("char"),
                        );
                    }
                    other => panic!("escape {other:?}"),
                },
                c => out.push(c),
            }
        }
        out
    };
    Row {
        id: field("id"),
        q: field("q"),
        expect: field("expect"),
        tag: field("tag"),
    }
}

fn a(q: &str) -> Result<Query, ParseError> {
    sqlp::parse_opts(q, Opts::new(Dial::Postgres))
}

#[test]
fn accepted_queries_give_the_same_ast_in_both_arms() {
    let mut n = 0;
    for r in testset().iter().filter(|r| r.expect == "accept") {
        let h =
            hand::parse(&r.q).unwrap_or_else(|e| panic!("hand refused {} {:?}: {e}", r.id, r.q));
        let x = a(&r.q).unwrap_or_else(|e| panic!("sqlparser arm refused {} {:?}: {e}", r.id, r.q));
        assert_eq!(h.sexp(), x.sexp(), "{} {:?}", r.id, r.q);
        assert_eq!(h, x);
        n += 1;
    }
    assert!(
        n >= 80,
        "the test set has {n} in-subset queries, the card asks for at least 80"
    );
}

#[test]
fn queries_outside_the_subset_are_refused_by_both_arms() {
    // what sqlparser's AST cannot show (LIMIT ALL is dropped by the crate) is listed here on purpose: if the crate changes, this test says so
    let a_accepts = ["limit-all"];
    let mut n = 0;
    for r in testset().iter().filter(|r| r.expect == "refuse") {
        assert!(
            hand::parse(&r.q).is_err(),
            "hand accepted {} {:?}",
            r.id,
            r.q
        );
        let x = a(&r.q);
        assert_eq!(
            x.is_ok(),
            a_accepts.contains(&r.tag.as_str()),
            "{} {:?} -> {:?}",
            r.id,
            r.q,
            x.map(|q| q.sexp())
        );
        n += 1;
    }
    assert!(
        n >= 40,
        "{n} queries outside the subset, the card asks for at least 40"
    );
}

#[test]
fn the_22_tpch_queries_are_refused_with_a_position_and_parsed_by_the_crate() {
    // the text of the 22 queries is DuckDB's `tpch_queries()`; here only their shape: joins, GROUP BY, ORDER BY, subqueries and function calls
    let samples = [
        "SELECT l_returnflag, sum(l_quantity) AS s FROM lineitem WHERE l_shipdate <= CAST('1998-09-02' AS date) GROUP BY l_returnflag ORDER BY l_returnflag;",
        "SELECT s_acctbal FROM part, supplier, partsupp WHERE p_partkey = ps_partkey AND ps_supplycost = (SELECT min(ps_supplycost) FROM partsupp) ORDER BY s_acctbal DESC LIMIT 100;",
        "WITH revenue AS (SELECT l_suppkey, sum(l_extendedprice) AS total FROM lineitem GROUP BY l_suppkey) SELECT * FROM revenue",
    ];
    for q in samples {
        let h = hand::parse(q).expect_err("hand must refuse");
        assert!(h.line.is_some() && h.col.is_some(), "{h}");
        assert_eq!(h.kind, ErrorKind::NotSupported, "{h}");
        let x = a(q).expect_err("arm A must refuse");
        assert_eq!(x.kind, ErrorKind::NotSupported, "{x}");
        assert!(
            sqlp::parse_raw(q, Opts::new(Dial::Generic)).is_ok(),
            "the crate itself must parse it"
        );
    }
}

fn numbers(e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Number(t) => out.push(t.clone()),
        ExprKind::Unary { expr, .. }
        | ExprKind::IsNull { expr, .. }
        | ExprKind::Cast { expr, .. } => numbers(expr, out),
        ExprKind::Binary { left, right, .. } => {
            numbers(left, out);
            numbers(right, out);
        }
        ExprKind::Between {
            expr, low, high, ..
        } => {
            for x in [expr, low, high] {
                numbers(x, out);
            }
        }
        ExprKind::InList { expr, list, .. } => {
            numbers(expr, out);
            list.iter().for_each(|x| numbers(x, out));
        }
        ExprKind::Case {
            operand,
            whens,
            else_,
        } => {
            operand.iter().for_each(|x| numbers(x, out));
            for (c, v) in whens {
                numbers(c, out);
                numbers(v, out);
            }
            else_.iter().for_each(|x| numbers(x, out));
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

fn literal_texts(parse: &dyn Fn(&str) -> Result<Query, ParseError>) -> Vec<String> {
    let mut seen = Vec::new();
    for l in LITERALS {
        let q = parse(&format!("SELECT {l} AS r FROM 'f.parquet'")).expect("parses");
        let SelectItem::Expr { expr, .. } = &q.items[0] else {
            panic!("item")
        };
        let mut v = Vec::new();
        numbers(expr, &mut v);
        assert_eq!(v.len(), 1);
        seen.push(v.remove(0));
    }
    seen
}

#[test]
fn numeric_literals_keep_their_exact_text_in_both_arms() {
    let expect: Vec<String> = LITERALS
        .iter()
        .map(|l| l.trim_start_matches('-').to_string())
        .collect();
    assert_eq!(literal_texts(&|q| hand::parse(q)), expect);
    assert_eq!(literal_texts(&|q| a(q)), expect);
    assert_eq!(
        literal_texts(&|q| sqlp::parse_opts(q, Opts::new(Dial::Generic))),
        expect
    );
}

#[test]
fn the_sign_of_a_literal_is_a_unary_minus_above_the_unsigned_text() {
    let q = hand::parse("SELECT -0.0 AS r FROM 'f'").expect("parses");
    assert_eq!(q.sexp(), "(query [(neg (num 0.0)) as r] from \"f\")");
    assert_eq!(
        a("SELECT -0.0 AS r FROM 'f'").expect("parses").sexp(),
        q.sexp()
    );
}

#[test]
fn precedence_and_associativity_against_fixed_answers() {
    // answers written by hand from the SQL rules (and checked against DuckDB's parser in the results file), independent of the other arm
    let cases = [
        ("a OR b AND c", "(or (id a) (and (id b) (id c)))"),
        ("a AND b OR c", "(or (and (id a) (id b)) (id c))"),
        ("NOT a AND b", "(and (not (id a)) (id b))"),
        ("NOT a = b", "(not (= (id a) (id b)))"),
        ("NOT NOT a", "(not (not (id a)))"),
        ("a - b - c", "(- (- (id a) (id b)) (id c))"),
        ("a - (b - c)", "(- (id a) (- (id b) (id c)))"),
        ("a / b * c", "(* (/ (id a) (id b)) (id c))"),
        ("a + b * c", "(+ (id a) (* (id b) (id c)))"),
        ("-a * b", "(* (neg (id a)) (id b))"),
        ("a = b + 1", "(= (id a) (+ (id b) (num 1)))"),
        ("a + 1 = b", "(= (+ (id a) (num 1)) (id b))"),
        (
            "a = b BETWEEN 1 AND 2",
            "(= (id a) (between (id b) (num 1) (num 2)))",
        ),
        (
            "a BETWEEN 1 AND 2 AND b",
            "(and (between (id a) (num 1) (num 2)) (id b))",
        ),
        ("a IS NULL AND b", "(and (is-null (id a)) (id b))"),
        ("NOT a IS NULL", "(not (is-null (id a)))"),
        ("a = b IS NULL", "(is-null (= (id a) (id b)))"),
        (
            "a IN (1, 2) OR b",
            "(or (in (id a) (num 1) (num 2)) (id b))",
        ),
    ];
    for (e, want) in cases {
        let q = hand::parse(&format!("SELECT * FROM 'f' WHERE {e}"))
            .unwrap_or_else(|x| panic!("{e}: {x}"));
        assert_eq!(q.where_.expect("where").sexp(), want, "hand: {e}");
        let q = a(&format!("SELECT * FROM 'f' WHERE {e}")).unwrap_or_else(|x| panic!("{e}: {x}"));
        assert_eq!(
            q.where_.expect("where").sexp(),
            want,
            "sqlparser (postgres): {e}"
        );
    }
    // the generic dialect of the crate gets one of them wrong (its BETWEEN and comparison share a level): recorded, not hidden
    let g = sqlp::parse_opts(
        "SELECT * FROM 'f' WHERE a = b BETWEEN 1 AND 2",
        Opts::new(Dial::Generic),
    )
    .expect("parses");
    assert_eq!(
        g.where_.expect("where").sexp(),
        "(between (= (id a) (id b)) (num 1) (num 2))"
    );
}

#[test]
fn errors_name_line_and_column() {
    let q = "SELECT a,\n  b +\nFROM 'f.parquet'";
    let e = hand::parse(q).expect_err("a missing operand");
    assert_eq!((e.line, e.col), (Some(3), Some(1)), "{e}");
    let e = hand::parse("SELECT a FROM 'f' WHERE a = = 1").expect_err("two operators");
    assert_eq!((e.line, e.col), (Some(1), Some(29)), "{e}");
    let e = hand::parse("SELECT sum(a) FROM 'f'").expect_err("a function");
    assert_eq!(
        (e.kind, e.line, e.col),
        (ErrorKind::NotSupported, Some(1), Some(8)),
        "{e}"
    );
    assert!(
        e.to_string()
            .starts_with("not supported yet: function calls"),
        "{e}"
    );
    // an error at the end of the text is at the end of the text, in both arms (sqlparser itself gives no position there)
    let e = hand::parse("SELECT a FROM").expect_err("eof");
    assert_eq!((e.line, e.col), (Some(1), Some(14)));
    let e = a("SELECT a FROM").expect_err("eof");
    assert_eq!((e.line, e.col), (Some(1), Some(14)));
    let raw = sqlp::parse_opts(
        "SELECT a FROM",
        Opts {
            eof_position_fix: false,
            ..Opts::new(Dial::Generic)
        },
    )
    .expect_err("eof");
    assert_eq!(
        (raw.line, raw.col),
        (None, None),
        "sqlparser alone gives no position at the end of the input"
    );
}

/// A small deterministic mangler (the Python one makes the 10,000 of the results file; this one keeps a version in the test suite).
fn mangled(base: &[Row], n: usize) -> Vec<String> {
    let mut x: u64 = 20261106;
    let mut next = move |m: usize| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((x >> 33) as usize) % m
    };
    let alphabet: Vec<char> = "'\"(),;*+-/%=<>!.:|$?[]{}@#&^~ _\n\t09azAZ\u{e9}\u{65e5}\u{1f600}"
        .chars()
        .collect();
    (0..n)
        .map(|_| {
            let mut c: Vec<char> = base[next(base.len())].q.chars().collect();
            let k = next(c.len());
            match next(3) {
                0 => {
                    c.remove(k);
                }
                1 if k + 1 < c.len() => c.swap(k, k + 1),
                _ => c.insert(k, alphabet[next(alphabet.len())]),
            }
            c.into_iter().collect()
        })
        .collect()
}

#[test]
fn mangled_queries_never_panic_and_every_error_has_a_position() {
    let all = testset();
    let base: Vec<Row> = all.into_iter().filter(|r| r.expect == "accept").collect();
    let qs = mangled(&base, 5000);
    let (mut refused, mut accepted) = (0, 0);
    for q in &qs {
        for (name, f) in [
            ("hand", hand::parse as fn(&str) -> Result<Query, ParseError>),
            ("sqlparser", a),
        ] {
            let r = catch_unwind(|| f(q)).unwrap_or_else(|_| panic!("{name} panicked on {q:?}"));
            match r {
                Ok(_) => accepted += 1,
                Err(e) if name == "hand" => {
                    refused += 1;
                    assert!(
                        e.line.is_some() && e.col.is_some(),
                        "hand: no position for {q:?}: {e}"
                    );
                }
                Err(_) => refused += 1,
            }
        }
    }
    assert!(
        refused > 2000 && accepted > 1000,
        "the mangler should give both kinds: {refused} refused, {accepted} accepted"
    );
}

fn on_stack<T: Send + 'static>(kb: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(kb * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not panic")
}

#[test]
fn deep_nesting_parses_up_to_1000_and_beyond_that_is_an_error_not_a_stack_overflow() {
    let head = "SELECT a FROM 'f' WHERE ";
    // 8 MB is the main thread; the hand arm also runs in 2 MB (the default of a spawned thread) for parentheses
    for (n, ok) in [(1000usize, true), (5000, false), (100_000, false)] {
        let q = format!("{head}{}a{}", "(".repeat(n), ")".repeat(n));
        let q2 = q.clone();
        let r = on_stack(8192, move || hand::parse(&q2).map(|_| ()));
        assert_eq!(r.is_ok(), ok, "hand, {n} parentheses");
        if !ok {
            assert_eq!(r.expect_err("limit").kind, ErrorKind::Limit);
        }
        let r = on_stack(8192, move || a(&q).map(|_| ()));
        assert_eq!(r.is_ok(), ok, "sqlparser, {n} parentheses");
    }
    let q = format!("{head}a{}", " AND a".repeat(1000));
    assert!(hand::parse(&q).is_ok());
    assert!(a(&q).is_ok());
    let q = format!("{head}a{}", " AND a".repeat(250_000));
    for r in [
        on_stack(8192, {
            let q = q.clone();
            move || hand::parse(&q).map(|_| ())
        }),
        on_stack(8192, move || a(&q).map(|_| ())),
    ] {
        assert_eq!(r.expect_err("limit").kind, ErrorKind::Limit);
    }
    let q = format!("{head}a{}", " + a".repeat(250_000));
    assert_eq!(hand::parse(&q).expect_err("limit").kind, ErrorKind::Limit);
    // the hand arm in a 2 MB thread
    let q = format!("{head}{}a{}", "(".repeat(1000), ")".repeat(1000));
    assert!(on_stack(2048, move || hand::parse(&q).is_ok()));
}

#[test]
fn a_query_of_one_megabyte_parses_in_bounded_time() {
    let shapes = [
        format!("SELECT a FROM 'f' WHERE a IN (1{})", ",1".repeat(500_000)),
        format!("SELECT '{}' FROM 'f'", "x".repeat(1_000_000)),
        format!("SELECT a{} FROM 'f'", ", a".repeat(350_000)),
        format!("SELECT a FROM 'f' -- {}", "x".repeat(1_000_000)),
    ];
    for q in shapes {
        assert!(q.len() >= 1_000_000);
        let t = std::time::Instant::now();
        assert!(hand::parse(&q).is_ok());
        assert!(a(&q).is_ok());
        assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
    }
}

// ---------------------------------------------------------------- the checks must be able to fail

fn differs_from_sqlparser(m: Mutant, set: &[String]) -> usize {
    set.iter()
        .filter(|q| hand::parse_with(q, m).ok().map(|x| x.sexp()) != a(q).ok().map(|x| x.sexp()))
        .count()
}

#[test]
fn known_bad_parsers_are_caught_by_the_comparison_with_sqlparser() {
    let set: Vec<String> = testset()
        .into_iter()
        .filter(|r| r.expect == "accept")
        .map(|r| r.q)
        .collect();
    assert_eq!(
        differs_from_sqlparser(Mutant::None, &set),
        0,
        "the real parser is the control"
    );
    for (m, why) in [
        (Mutant::OrBeforeAnd, "OR binds tighter than AND"),
        (Mutant::LoseNot, "a NOT is dropped"),
        (Mutant::NumberViaF64, "a decimal literal goes through f64"),
    ] {
        assert!(
            differs_from_sqlparser(m, &set) > 0,
            "the comparison missed the mutant where {why}"
        );
    }
}

#[test]
fn the_literal_check_catches_a_number_that_went_through_f64() {
    let bad = literal_texts(&|q| hand::parse_with(q, Mutant::NumberViaF64));
    let expect: Vec<String> = LITERALS
        .iter()
        .map(|l| l.trim_start_matches('-').to_string())
        .collect();
    let changed = bad.iter().zip(&expect).filter(|(b, e)| b != e).count();
    assert!(
        changed >= 10,
        "only {changed} of {} literals changed through f64",
        LITERALS.len()
    );
    assert!(
        bad.contains(&"inf".to_string()),
        "1e400 must overflow to inf in the mutant"
    );
}

#[test]
fn the_precedence_answers_catch_the_or_and_not_mutants() {
    let q = hand::parse_with("SELECT * FROM 'f' WHERE a OR b AND c", Mutant::OrBeforeAnd)
        .expect("parses");
    assert_ne!(
        q.where_.expect("where").sexp(),
        "(or (id a) (and (id b) (id c)))"
    );
    let q = hand::parse_with("SELECT * FROM 'f' WHERE NOT a", Mutant::LoseNot).expect("parses");
    assert_eq!(
        q.where_.expect("where").sexp(),
        "(id a)",
        "the mutant really drops the NOT"
    );
}
