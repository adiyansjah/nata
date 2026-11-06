//! Tests of the parse stage: the limits of dialect section 8, positions, what the crate's tree hides, fuzz.

use std::panic::catch_unwind;

use crate::ast::{ExprKind, Query};
use crate::error::{ErrorClass, SqlError};
use crate::parse::{MAX_CHAIN_TERMS, MAX_NESTING, MAX_TEXT_BYTES, parse_query};
use crate::testutil::*;

fn parse(q: &str) -> Result<Query, SqlError> {
    parse_query(q)
}

fn ok(q: &str) -> Query {
    parse(q).unwrap_or_else(|e| panic!("{q:?}: {e}"))
}

fn err(q: &str) -> SqlError {
    parse(q).expect_err(q)
}

/// Run on a thread with the default test stack (2 MB), as a test does, and report whether the closure came back.
fn on_2mb<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not overflow its stack or panic")
}

const HEAD: &str = "SELECT a FROM 'f' WHERE ";

#[test]
fn nesting_of_1000_parses_and_1001_is_a_positioned_limit_error_for_every_construct() {
    for (name, open, close, unit) in [
        ("parentheses", "(", ")", ""),
        ("CASE", "CASE WHEN ", " THEN 1 ELSE 2 END", ""),
        ("NOT", "", "", "NOT "),
        ("unary minus", "", "", "- "),
    ] {
        let build = |n: usize| match (open.is_empty(), unit.is_empty()) {
            (false, _) => format!("{HEAD}{}a{}", open.repeat(n), close.repeat(n)),
            (true, _) => format!("{HEAD}{}a", unit.repeat(n)),
        };
        // `-` and `NOT` are WHERE operands: the type is the binder's business, the parser only builds the tree
        let q = build(MAX_NESTING);
        assert!(
            on_2mb(move || parse(&q).is_ok()),
            "{name}: 1,000 levels must parse"
        );
        let q = build(MAX_NESTING + 1);
        let e = on_2mb(move || parse(&q).expect_err("limit"));
        assert!(
            e.is_limit() && e.pos.line == 1 && e.pos.col > 1,
            "{name}: {e}"
        );
    }
    // a mixture: 400 parentheses, 300 CASEs, 300 prefix operators
    let mixed = |extra: usize| {
        format!(
            "{HEAD}{}{}{}a{}{}",
            "(".repeat(400),
            "CASE WHEN ".repeat(300),
            "NOT ".repeat(300 + extra),
            " THEN 1 END".repeat(300),
            ")".repeat(400)
        )
    };
    let q = mixed(0);
    assert!(on_2mb(move || parse(&q).is_ok()));
    let q = mixed(1);
    assert!(on_2mb(move || parse(&q).expect_err("limit").is_limit()));
}

#[test]
fn a_prefix_operator_after_a_value_is_binary_and_does_not_count_as_nesting() {
    let q = format!("{HEAD}a{}", " - -1".repeat(900));
    assert!(parse(&q).is_ok());
}

#[test]
fn a_chain_of_10000_terms_parses_and_10001_is_refused_for_each_operator() {
    for op in [
        "+",
        "-",
        "*",
        "/",
        "%",
        "//",
        "AND",
        "OR",
        "=",
        "::INTEGER ::",
    ] {
        let build = |terms: usize| {
            if op.starts_with("::") {
                format!("{HEAD}a{}", "::INTEGER".repeat(terms - 1))
            } else {
                format!("{HEAD}a{}", format!(" {op} a").repeat(terms - 1))
            }
        };
        if op == "=" {
            // comparisons do not chain: refused by the grammar, but the guard bounds the work first
            let q = build(MAX_CHAIN_TERMS + 1);
            assert!(
                on_2mb(move || parse(&q).expect_err("limit").is_limit()),
                "{op}"
            );
            continue;
        }
        let q = build(MAX_CHAIN_TERMS);
        let r = on_2mb(move || parse(&q).map(|_| ()));
        assert!(r.is_ok(), "{op}: 10,000 terms must parse: {r:?}");
        let q = build(MAX_CHAIN_TERMS + 1);
        let e = on_2mb(move || parse(&q).expect_err("limit"));
        assert!(e.is_limit(), "{op}: {e}");
        assert_eq!(e.pos.line, 1);
    }
    // a chain after a comma or in another clause starts afresh
    let q = format!(
        "SELECT a{} AS x, a{} AS y FROM 'f'",
        " + a".repeat(9_000),
        " + a".repeat(9_000)
    );
    assert!(parse(&q).is_ok());
    let q = format!(
        "SELECT a FROM 'f' WHERE a{} LIMIT 1",
        " AND a".repeat(9_000)
    );
    assert!(parse(&q).is_ok());
}

#[test]
fn a_query_of_1_mb_parses_and_one_byte_more_is_a_positioned_error() {
    let pad = |n: usize| {
        format!(
            "SELECT a FROM 'f' -- {}",
            "x".repeat(n - "SELECT a FROM 'f' -- ".len())
        )
    };
    assert!(parse(&pad(MAX_TEXT_BYTES)).is_ok());
    let e = err(&pad(MAX_TEXT_BYTES + 1));
    assert!(e.is_limit(), "{e}");
    assert_eq!(
        (e.pos.line, e.pos.col as usize),
        (1, MAX_TEXT_BYTES + 1),
        "the first character beyond the limit"
    );
    // shapes of about 1 MB: no stack overflow, no panic, bounded time
    let shapes = [
        format!("SELECT a FROM 'f' WHERE a IN (1{})", ",1".repeat(500_000)),
        format!("SELECT '{}' FROM 'f'", "x".repeat(1_000_000)),
        format!("SELECT a{} FROM 'f'", ", a".repeat(250_000)),
        format!("SELECT a FROM 'f' -- {}", "x".repeat(1_000_000)),
        format!("SELECT {} 1 FROM 'f'", "/* c */".repeat(140_000)),
        format!("SELECT a FROM 'f' WHERE a{}", " AND a".repeat(170_000)),
        format!(
            "SELECT {}a{} FROM 'f'",
            "(".repeat(500_000),
            ")".repeat(500_000)
        ),
    ];
    for q in shapes {
        assert!(q.len() <= MAX_TEXT_BYTES + 1_000_000);
        let t = std::time::Instant::now();
        let r = on_2mb(move || parse(&q).map(|_| ()));
        if let Err(e) = &r {
            assert!(e.pos.line >= 1 && e.pos.col >= 1);
        }
        assert!(t.elapsed().as_secs() < 30, "took {:?}", t.elapsed());
    }
}

#[test]
fn a_deep_tree_is_dropped_on_a_default_stack() {
    // 10,000 terms of one operator: the longest chain the limit allows; the caller drops the tree on its own (2 MB) stack
    let q = format!("{HEAD}a{}", " + a".repeat(MAX_CHAIN_TERMS - 1));
    on_2mb(move || {
        let tree = parse(&q).expect("parses");
        drop(tree);
    });
    let q = format!(
        "{HEAD}{}a{}",
        "(".repeat(MAX_NESTING),
        ")".repeat(MAX_NESTING)
    );
    on_2mb(move || drop(parse(&q).expect("parses")));
}

#[test]
fn constructs_the_crate_hides_are_refused() {
    let e = err("SELECT a FROM 'f' LIMIT ALL");
    assert_eq!(
        (e.class, e.pos.line, e.pos.col),
        (ErrorClass::NotSupported, 1, 25),
        "{e}"
    );
    assert!(e.message.starts_with("LIMIT ALL"), "{e}");
    // a lower-case spelling and a line break between the words
    assert_eq!(err("select a from 'f' limit\n  all").pos.line, 2);
    // ALL as a name or in a string is not LIMIT ALL
    assert!(parse("SELECT 'LIMIT ALL' AS x FROM 'f'").is_ok());
    assert!(parse("SELECT a FROM 'f' LIMIT 1 -- LIMIT ALL").is_ok());
    // an error earlier in the text wins over the hidden construct
    let e = err("SELECT sum(a) FROM 'f' LIMIT ALL");
    assert!(e.message.contains("function calls"), "{e}");
    for q in [
        "SELECT ALL a FROM 'f'",
        "SELECT a FROM 'f' LIMIT 1 OFFSET 1 ROWS",
        "SELECT a FROM 'f';;",
        ";SELECT a FROM 'f'",
        "SELECT a FROM 'f'; SELECT 1",
        "SELECT a FROM 'f' LIMIT 1 OFFSET 2 ROW",
    ] {
        let e = err(q);
        assert!(e.pos.line >= 1, "{q}: {e}");
    }
    assert!(
        parse("SELECT a FROM 'f';").is_ok(),
        "one trailing semicolon is allowed"
    );
    assert!(parse("SELECT a FROM 'f' ; -- comment").is_ok());
}

#[test]
fn positions_are_one_based_lines_and_characters() {
    let e = err("SELECT a,\n  b +\nFROM 'f'");
    assert_eq!((e.pos.line, e.pos.col), (3, 1), "{e}");
    let e = err("SELECT é, ü FROM 'f' WHERE a = = 1");
    // é and ü are one character each (two bytes each)
    assert_eq!((e.pos.line, e.pos.col), (1, 32), "{e}");
    // an error at the end of the text is at the end of the text
    for (q, line, col) in [
        ("SELECT a FROM", 1, 14),
        ("SELECT a,\nb FROM 'f' WHERE", 2, 17),
        ("SELECT (a", 1, 10),
    ] {
        let e = err(q);
        assert_eq!((e.pos.line, e.pos.col), (line, col), "{q}: {e}");
    }
    let e = err("");
    assert_eq!((e.pos.line, e.pos.col), (1, 1));
    // \r is a character of its line, only \n ends one
    let e = err("SELECT a\r\nFROM 'f' WHERE =");
    assert_eq!((e.pos.line, e.pos.col), (2, 16), "{e}");
}

#[test]
fn not_supported_errors_name_the_construct_at_its_position() {
    for (q, what, line, col) in [
        ("SELECT sum(a) FROM 'f'", "function calls", 1, 8),
        ("SELECT a FROM 'f' ORDER BY a", "ORDER BY", 1, 19),
        ("SELECT a FROM 'f' GROUP BY a", "GROUP BY", 1, 19),
        ("SELECT a FROM t", "table names", 1, 15),
        ("SELECT a b FROM 'f'", "an alias without AS", 1, 10),
        ("SELECT * FROM 'f' WHERE a LIKE 'x'", "LIKE", 1, 25),
        ("SELECT a || b FROM 'f'", "the operator ||", 1, 8),
        ("SELECT +a FROM 'f'", "the unary operator", 1, 8),
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::NotSupported, "{q}: {e}");
        assert!(e.message.contains(what), "{q}: {e}");
        assert_eq!((e.pos.line, e.pos.col), (line, col), "{q}: {e}");
        assert!(e.to_string().starts_with("not supported yet: "));
    }
}

#[test]
fn the_expression_positions_are_the_first_character_of_the_construct() {
    let q = ok(
        "SELECT  -a, NOT b, CAST(c AS INTEGER), d::BIGINT, DATE '2020-01-01', CASE WHEN a THEN b END, (a + b) * c FROM 'f'",
    );
    let pos: Vec<(u32, u32)> = q
        .items
        .iter()
        .map(|i| match i {
            crate::ast::SelectItem::Expr { expr, .. } => (expr.pos().line, expr.pos().col),
            crate::ast::SelectItem::Star(s) => (s.0.line, s.0.col),
        })
        .collect();
    assert_eq!(
        pos,
        [(1, 9), (1, 13), (1, 20), (1, 40), (1, 51), (1, 70), (1, 94)]
    );
}

#[test]
fn integer_division_is_read_as_its_own_operator_and_a_slash_pair_with_a_space_is_not() {
    let q = ok("SELECT a // b, a / b, a//b, (a) // (b) FROM 'f'").sexp();
    assert_eq!(
        q,
        "(query [(// (id a) (id b))] [(/ (id a) (id b))] [(// (id a) (id b))] [(// (id a) (id b))] from \"f\")"
    );
    let e = err("SELECT a / /b FROM 'f'");
    assert_eq!((e.class, e.pos.col), (ErrorClass::Syntax, 12), "{e}");
}

#[test]
fn mangled_queries_never_panic_and_every_error_has_a_position() {
    let seed: u64 = std::env::var("NATA_SQL_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_261_106);
    let n: u64 = std::env::var("NATA_SQL_FUZZ_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let base = accepted_queries();
    let (mut refused, mut accepted, mut at_end) = (0u64, 0u64, 0u64);
    for i in 0..n {
        let q = mangled(&base, seed, i);
        let r = catch_unwind(|| parse(&q)).unwrap_or_else(|_| {
            panic!("PANIC: seed {seed} query {i}: {q:?} (replay: NATA_SQL_FUZZ_SEED={seed})")
        });
        match r {
            Ok(_) => accepted += 1,
            Err(e) => {
                refused += 1;
                let end = crate::error::Pos::end_of(&q);
                assert!(
                    e.pos.line >= 1
                        && e.pos.col >= 1
                        && e.pos.line <= end.line.max(1)
                        && !e.message.starts_with("internal error"),
                    "seed {seed} query {i}: {q:?} -> {e}"
                );
                if e.pos == end {
                    at_end += 1;
                }
            }
        }
    }
    eprintln!(
        "mangled queries: seed {seed}, {n} cases: {accepted} parse, {refused} refused with a position ({at_end} at the end of the text)"
    );
    if n >= 10_000 {
        assert!(
            refused > 2000 && accepted > 1000,
            "the mangler should give both kinds: {refused} refused, {accepted} accepted"
        );
    }
}

#[test]
fn numbers_with_separators_or_suffixes_are_refused_at_the_literal() {
    for q in [
        "SELECT 1_000 FROM 'f'",
        "SELECT 1e FROM 'f'",
        "SELECT 0x1F FROM 'f'",
        "SELECT 1.2.3 FROM 'f'",
    ] {
        let e = err(q);
        assert!(e.pos.col >= 8, "{q}: {e}");
    }
}

#[test]
fn a_tree_node_is_a_number_with_its_text() {
    let q = ok("SELECT 0.10, 1e2, .5, 5. FROM 'f'");
    let texts: Vec<String> = q
        .items
        .iter()
        .filter_map(|i| match i {
            crate::ast::SelectItem::Expr { expr, .. } => match &expr.kind {
                ExprKind::Number(t) => Some(t.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["0.10", "1e2", ".5", "5."]);
}
