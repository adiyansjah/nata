//! One or more tests for every rule of `docs/dialect.md` (sections 1 to 9) and the typing rules of `docs/decimal-rules.md` that the binder applies.
//! The expected values are written from the documents (their tables and examples), not from the code. Names say which rule they hold.

mod common;
use common::*;
use nata_parquet::schema::Kind;
use nata_sql::source::MemSource;
use nata_sql::texpr::{ExprKind, Value};
use nata_sql::{BindOptions, ErrorClass, SourceProvider, plan_sql, plan_sql_with};

fn lit(expr: &str) -> (Value, String) {
    let p = ok(&format!("SELECT {expr} AS r FROM 't.parquet'"));
    let e = &p.plan.project.exprs[0].expr;
    match &e.kind {
        ExprKind::Literal(v) => (v.clone(), e.ty.to_string()),
        other => panic!("{expr}: not a literal: {other:?}"),
    }
}

#[test]
fn comparisons_require_explicit_parentheses_when_chained() {
    // Boolean operands keep the binder from masking a missing parse guard with
    // an unrelated boolean-versus-number type error.
    for expr in [
        "TRUE = FALSE = TRUE",
        "bo = TRUE = FALSE",
        "TRUE = bo <> FALSE",
    ] {
        let e = ty_err(expr);
        assert_eq!(e.class, ErrorClass::Syntax, "{expr}: {e}");
        assert!(e.to_string().contains("chained comparisons"), "{expr}: {e}");
    }
    for expr in ["(TRUE = FALSE) = TRUE", "TRUE = (bo <> FALSE)"] {
        assert_eq!(ty(expr), "BOOLEAN");
    }
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 1

#[test]
fn s1_statement_forms_and_the_explain_modes() {
    use nata_sql::ast::ExplainMode::*;
    assert_eq!(ok("SELECT a FROM 't.parquet'").mode, No);
    assert_eq!(ok("EXPLAIN SELECT a FROM 't.parquet'").mode, Plan);
    assert_eq!(
        ok("explain analyze select a from 't.parquet' where a > 1 limit 2").mode,
        Analyze
    );
    // a trailing semicolon is allowed, one statement per call
    ok("SELECT a FROM 't.parquet';");
    assert_eq!(
        err("SELECT a FROM 't.parquet'; SELECT b FROM 't.parquet'").class,
        ErrorClass::NotSupported
    );
    // EXPLAIN plans the same query
    assert_eq!(
        ok("EXPLAIN SELECT a FROM 't.parquet' WHERE a > 1 LIMIT 2")
            .plan
            .explain_text(),
        ok("SELECT a FROM 't.parquet' WHERE a > 1 LIMIT 2")
            .plan
            .explain_text()
    );
}

#[test]
fn s1_what_is_not_yet_supported_names_the_construct_at_its_position() {
    for (q, what, col) in [
        ("SELECT a FROM 't.parquet' GROUP BY ALL", "GROUP BY", 27),
        (
            "SELECT a FROM 't.parquet' HAVING count(DISTINCT a) > 1",
            "DISTINCT aggregates",
            34,
        ),
        ("SELECT a FROM 't.parquet' ORDER BY a", "ORDER BY", 27),
        (
            "SELECT a FROM 't.parquet' JOIN 'u.parquet' ON a = b",
            "joins",
            27,
        ),
        ("SELECT a FROM (SELECT a FROM 't.parquet')", "subquery", 16),
        ("WITH x AS (SELECT 1) SELECT a FROM 't.parquet'", "WITH", 1),
        ("SELECT DISTINCT a FROM 't.parquet'", "DISTINCT", 8),
        (
            "SELECT a FROM 't.parquet' UNION SELECT a FROM 't.parquet'",
            "set operations",
            27,
        ),
        (
            "SELECT a FROM 't.parquet', 'u.parquet'",
            "more than one table",
            26,
        ),
        ("SELECT a FROM t", "table names", 15),
        ("SELECT abs(a) FROM 't.parquet'", "function calls", 8),
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::NotSupported, "{q}: {e}");
        assert!(e.message.contains(what), "{q}: {e}");
        assert_eq!((e.pos.line, e.pos.col), (1, col), "{q}: {e}");
    }
}

#[test]
fn s1_all_the_files_of_a_glob_must_have_the_same_schema_and_the_first_differing_file_is_named() {
    let same = t_columns();
    let mut other = t_columns();
    other[2].1 = Ok(Kind::Int {
        bits: 32,
        signed: true,
    }); // c: BIGINT here
    let s = MemSource::new()
        .file("d/1.parquet", &same)
        .file("d/2.parquet", &same)
        .file("d/3.parquet", &other)
        .file("d/4.parquet", &other)
        .glob(
            "d/*.parquet",
            &["d/1.parquet", "d/2.parquet", "d/3.parquet", "d/4.parquet"],
        )
        .glob("d/ok-*.parquet", &["d/1.parquet", "d/2.parquet"]);
    // register item P10: this is the binder that reads every footer (BindOptions::all_footers, the behaviour before P10; `nata sql --bind-all-footers`)
    let all = BindOptions { all_footers: true };
    assert!(plan_sql_with("SELECT a FROM 'd/ok-*.parquet'", &s, all).is_ok());
    let e = plan_sql_with("SELECT a FROM 'd/*.parquet'", &s, all).expect_err("schemas differ");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(
        e.message.contains("d/3.parquet") && !e.message.contains("d/4.parquet"),
        "{e}"
    );
    assert!(
        e.message.contains("column c BIGINT") || e.message.contains("c INTEGER"),
        "{e}"
    );
    assert_eq!(
        (e.pos.line, e.pos.col),
        (1, 15),
        "the place of the path: {e}"
    );
    // a glob with no file, a missing file
    let e = plan_sql("SELECT a FROM 'nothing/*.parquet'", &s).expect_err("empty");
    assert!(e.message.contains("no file matches"), "{e}");
    let e = plan_sql("SELECT a FROM 'missing.parquet'", &MemSource::new()).expect_err("missing");
    assert_eq!(
        e.class,
        ErrorClass::Execution,
        "a missing file is a failure of the data"
    );
    assert!(e.message.contains("missing.parquet"), "{e}");
    // the file order of a glob is the order of its expansion
    let p = plan_sql("SELECT a FROM 'd/ok-*.parquet'", &s).expect("plan");
    let files: Vec<String> = p
        .plan
        .scan
        .files
        .iter()
        .map(|f| f.display().to_string())
        .collect();
    assert_eq!(files, ["d/1.parquet", "d/2.parquet"]);
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 2

#[test]
fn s2_an_unquoted_identifier_is_folded_to_lower_case() {
    let p = ok("SELECT L_OrderKey, L_ORDERKEY FROM 'lineitem.parquet'");
    assert_eq!(p.plan.scan.columns.len(), 1, "both read l_orderkey");
    assert_eq!(p.output[0].name, "l_orderkey");
}

#[test]
fn s2_a_quoted_identifier_is_exact_and_case_sensitive() {
    // t.parquet has both `a` and `A`: they are two columns
    let p = ok("SELECT a, \"A\" FROM 't.parquet'");
    assert_eq!(
        p.plan
            .scan
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "A"]
    );
    // unquoted `MixedCase` folds to `mixedcase`, which does not exist; the quoted spelling finds it
    let e = err("SELECT MixedCase FROM 't.parquet'");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(e.message.contains("mixedcase"), "{e}");
    ok("SELECT \"MixedCase\" FROM 't.parquet'");
    // and a quoted name in the wrong case does not find the column
    let e = err("SELECT \"mixedcase\" FROM 't.parquet'");
    assert_eq!(e.suggestion.as_deref(), Some("MixedCase"), "{e}");
    // names with spaces, and a quote inside a name written as two
    ok("SELECT \"with space\" FROM 't.parquet'");
    let s = MemSource::new().file(
        "q.parquet",
        &[("a\"b", Ok(Kind::Bool)), ("x", Ok(Kind::Bool))],
    );
    plan_sql("SELECT \"a\"\"b\" FROM 'q.parquet'", &s).expect("a quote in a name is doubled");
}

#[test]
fn s2_star_expands_to_every_column_in_file_order_and_an_unsupported_column_is_an_error_only_when_touched()
 {
    let p = ok("SELECT * FROM 'nation.parquet'");
    let names: Vec<&str> = p.output.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["n_nationkey", "n_name", "n_regionkey", "n_comment"]);
    let p = ok("SELECT a, * FROM 'nation.parquet'"
        .replace("a, *", "n_name, *")
        .as_str());
    assert_eq!(p.output.len(), 5);
    // t.parquet has a TIMESTAMP, an unsigned and a nested column: * touches them
    let e = err("SELECT * FROM 't.parquet'");
    assert_eq!(e.class, ErrorClass::NotSupported);
    assert_eq!((e.pos.line, e.pos.col), (1, 8), "{e}");
    // naming only supported columns is fine
    ok("SELECT a, s FROM 't.parquet'");
    for c in ["ts", "u", "nested"] {
        let e = err(&format!("SELECT {c} FROM 't.parquet'"));
        assert_eq!(e.class, ErrorClass::NotSupported, "{c}: {e}");
        assert!(e.message.contains(c), "{e}");
    }
}

#[test]
fn s2_an_alias_needs_as_names_an_output_column_and_cannot_be_used_in_the_same_select_or_where() {
    let p = ok("SELECT a + 1 AS Total, b AS \"Quoted Alias\" FROM 't.parquet'");
    let names: Vec<&str> = p.output.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["total", "Quoted Alias"],
        "an unquoted alias folds to lower case"
    );
    let e = err("SELECT a + 1 total FROM 't.parquet'");
    assert_eq!(e.class, ErrorClass::NotSupported);
    assert!(e.message.contains("AS"), "{e}");
    let e = err("SELECT a + 1 AS x, x * 2 AS y FROM 't.parquet'");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(e.message.contains("alias"), "{e}");
    let e = err("SELECT a + 1 AS x FROM 't.parquet' WHERE x > 2");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(e.message.contains("alias"), "{e}");
}

#[test]
fn s2_two_output_columns_may_have_the_same_name() {
    let p = ok("SELECT a, a, b AS a FROM 't.parquet'");
    let names: Vec<&str> = p.output.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a", "a", "a"]);
    assert_eq!(
        p.plan.scan.columns.len(),
        2,
        "the scan reads a and b once each"
    );
    // Nothing can REFER to an output name in this slice (no ORDER BY, no subquery, no aliases inside the SELECT), so the "ambiguous name" error
    // of the page has no case to raise yet; a source with two columns of one name is refused when the footer is read.
}

#[test]
fn s2_a_reserved_word_needs_quotes_to_be_a_name() {
    for q in [
        "SELECT end FROM 't.parquet'",
        "SELECT a AS from FROM 't.parquet'",
        "SELECT select FROM 't.parquet'",
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::Syntax, "{q}: {e}");
        assert!(e.message.contains("reserved"), "{q}: {e}");
    }
    let s = MemSource::new().file(
        "r.parquet",
        &[("end", Ok(Kind::Bool)), ("order", Ok(Kind::Bool))],
    );
    plan_sql("SELECT \"end\", \"order\" FROM 'r.parquet'", &s)
        .expect("quoted reserved words are names");
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 3

#[test]
fn s3_integer_literals_take_the_smallest_of_integer_and_bigint_then_a_decimal_up_to_38_digits() {
    for (text, ty) in [
        ("0", "INTEGER"),
        ("42", "INTEGER"),
        ("007", "INTEGER"),
        ("2147483647", "INTEGER"),
        ("2147483648", "BIGINT"),
        ("3000000000", "BIGINT"),
        ("9223372036854775807", "BIGINT"),
        ("9223372036854775808", "DECIMAL(19,0)"),
        ("99999999999999999999999999999999999999", "DECIMAL(38,0)"),
        ("-2147483648", "INTEGER"),
        ("-2147483649", "BIGINT"),
        ("-9223372036854775808", "BIGINT"),
        ("-9223372036854775809", "DECIMAL(19,0)"),
    ] {
        assert_eq!(lit(text).1, ty, "{text}");
    }
    assert_eq!(lit("3000000000").0, Value::Int(3_000_000_000));
    assert_eq!(lit("-2147483648").0, Value::Int(-2_147_483_648));
    let e = ty_err("100000000000000000000000000000000000000");
    assert_eq!(e.class, ErrorClass::Bind, "39 digits: {e}");
    assert_eq!((e.pos.line, e.pos.col), (1, 8));
}

#[test]
fn s3_decimal_literals_are_exact_from_their_digits() {
    for (text, value, ty) in [
        ("0.1", 1i128, "DECIMAL(1,1)"),
        ("1.5", 15, "DECIMAL(2,1)"),
        ("0.10", 10, "DECIMAL(2,2)"),
        ("0.05", 5, "DECIMAL(2,2)"),
        ("00012.50", 1250, "DECIMAL(4,2)"),
        ("1.", 1, "DECIMAL(1,0)"),
        (".5", 5, "DECIMAL(1,1)"),
        ("1e2", 100, "DECIMAL(3,0)"),
        ("1e-3", 1, "DECIMAL(3,3)"),
        ("1.5e0", 15, "DECIMAL(2,1)"),
        ("1.5e+3", 1500, "DECIMAL(4,0)"),
        ("2.5E-1", 25, "DECIMAL(2,2)"),
        (
            "123456789012345678901234567890.12",
            12_345_678_901_234_567_890_123_456_789_012,
            "DECIMAL(32,2)",
        ),
        (
            "0.000000000000000000000000000000000001",
            1,
            "DECIMAL(36,36)",
        ),
        ("-0.0", 0, "DECIMAL(1,1)"),
        ("-1.50", -150, "DECIMAL(3,2)"),
    ] {
        let (v, t) = lit(text);
        assert_eq!((v, t.as_str()), (Value::Decimal(value), ty), "{text}");
    }
    // out of range: an error, never a rounding
    for text in [
        "1e400",
        "1E-400",
        "0.0000000000000000000000000000000000000001",
        "1e39",
        "12345678901234567890123456789012345678.9",
    ] {
        let e = ty_err(text);
        assert_eq!(e.class, ErrorClass::Bind, "{text}: {e}");
    }
}

#[test]
fn s3_there_is_no_float_literal_syntax() {
    // 1.5e0 is a decimal (the listed difference from DuckDB)
    assert_eq!(ty("1.5e0"), "DECIMAL(2,1)");
    assert_eq!(ty("CAST('1.5' AS DOUBLE)"), "DOUBLE");
    assert_eq!(ty("CAST(1.5 AS DOUBLE)"), "DOUBLE");
}

#[test]
fn s3_strings_dates_booleans() {
    assert_eq!(
        lit("'it''s'"),
        (Value::Str("it's".into()), "VARCHAR".into())
    );
    assert_eq!(
        lit("'a\\nb'").0,
        Value::Str("a\\nb".into()),
        "no backslash escapes"
    );
    assert_eq!(lit("'日本語'").0, Value::Str("日本語".into()));
    assert_eq!(lit("DATE '1994-01-01'"), (Value::Date(8766), "DATE".into()));
    assert_eq!(lit("date '1970-01-01'").0, Value::Date(0));
    assert_eq!(lit("DATE '2000-02-29'").1, "DATE");
    assert_eq!(lit("TRUE"), (Value::Bool(true), "BOOLEAN".into()));
    assert_eq!(lit("false").0, Value::Bool(false));
    // an impossible date is an error at the literal
    for d in [
        "2024-02-30",
        "1994-13-01",
        "1994-1-1",
        "hello",
        "0000-01-01",
        "2023-02-29",
    ] {
        let e = ty_err(&format!("DATE '{d}'"));
        assert_eq!(e.class, ErrorClass::Bind, "{d}: {e}");
        assert_eq!((e.pos.line, e.pos.col), (1, 8), "{d}: {e}");
    }
}

#[test]
fn s3_null_takes_the_type_its_context_needs() {
    assert_eq!(ty("a + NULL"), "INTEGER");
    assert_eq!(ty("NULL + d_15_2"), "DECIMAL(16,2)");
    assert_eq!(ty("NULL = a"), "BOOLEAN");
    assert_eq!(ty("NULL AND bo"), "BOOLEAN");
    assert_eq!(ty("NOT NULL"), "BOOLEAN");
    assert_eq!(ty("CAST(NULL AS DECIMAL(10,2))"), "DECIMAL(10,2)");
    assert_eq!(ty("CASE WHEN bo THEN NULL ELSE 1 END"), "INTEGER");
    assert_eq!(ty("CASE WHEN bo THEN 'x' END"), "VARCHAR");
    assert_eq!(ty("a IN (1, NULL)"), "BOOLEAN");
    assert_eq!(ty("NULL IN (1, 2)"), "BOOLEAN");
    assert_eq!(ty("NULL BETWEEN 1 AND 2"), "BOOLEAN");
    assert_eq!(ty("a BETWEEN NULL AND 2"), "BOOLEAN");
    assert_eq!(ty("a IS NULL"), "BOOLEAN");
    ok("SELECT a FROM 't.parquet' WHERE NULL");
    // and the typed NULL is a NULL of that type
    let p = ok("SELECT a + NULL AS r FROM 't.parquet'");
    let ExprKind::Arith { right, .. } = &p.plan.project.exprs[0].expr.kind else {
        panic!("arith")
    };
    assert_eq!(
        (right.is_null_literal(), right.ty.to_string()),
        (true, "INTEGER".to_string())
    );
}

#[test]
fn s3_a_null_whose_type_nothing_gives_is_not_supported_yet() {
    for q in [
        "SELECT NULL FROM 't.parquet'",
        "SELECT NULL AS x FROM 't.parquet'",
        "SELECT NULL IS NULL FROM 't.parquet'",
        "SELECT NULL = NULL FROM 't.parquet'",
        "SELECT NULL + NULL FROM 't.parquet'",
        "SELECT -NULL FROM 't.parquet'",
        "SELECT CASE WHEN bo THEN NULL END FROM 't.parquet'",
        "SELECT NULL IN (NULL) FROM 't.parquet'",
    ] {
        let e = err(q);
        assert_eq!(e.class, ErrorClass::NotSupported, "{q}: {e}");
        assert!(e.message.contains("NULL"), "{q}: {e}");
    }
    assert_eq!(err("SELECT NULL FROM 't.parquet'").pos.col, 8);
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 4

#[test]
fn s4_precedence_from_tightest_to_loosest() {
    for (e, want) in [
        ("-a * b", "((-a) * b)"),
        ("a + b * a", "(a + (b * a))"),
        ("(a + b) * a", "((a + b) * a)"),
        ("a - b - a", "((a - b) - a)"),
        ("a * b % a", "((a * b) % a)"),
        ("a // b * a", "((a // b) * a)"),
        ("a % b // a", "((a % b) // a)"),
        ("a + b = a * b", "((a + b) = (a * b))"),
        ("bo = (a = b)", "(bo = (a = b))"),
        ("NOT bo AND bo", "((NOT bo) AND bo)"),
        ("NOT a = b", "(NOT (a = b))"),
        ("bo OR bo AND bo", "(bo OR (bo AND bo))"),
        ("bo AND bo OR bo", "((bo AND bo) OR bo)"),
        ("(bo OR bo) AND bo", "((bo OR bo) AND bo)"),
        ("bo AND bo AND bo AND bo", "(bo AND bo AND bo AND bo)"),
        (
            "a = 1 OR a = 2 AND b = 3",
            "((a = 1) OR ((a = 2) AND (b = 3)))",
        ),
        ("a BETWEEN 1 AND 2 AND bo", "((a BETWEEN 1 AND 2) AND bo)"),
        ("bo = a BETWEEN 1 AND 2", "(bo = (a BETWEEN 1 AND 2))"),
        (
            "a IS NULL AND b IS NOT NULL",
            "((a IS NULL) AND (b IS NOT NULL))",
        ),
        ("NOT a IS NULL", "(NOT (a IS NULL))"),
        ("a = b IS NULL", "((a = b) IS NULL)"),
        ("a IN (1, 2) OR bo", "((a IN (1, 2)) OR bo)"),
        ("-a::BIGINT", "(-CAST(a AS BIGINT))"),
    ] {
        // the query uses a and b as INTEGER, c as BIGINT; widening casts of the binder are part of the text
        let got = show(e);
        let norm = |s: &str| s.replace(" AS r", "");
        assert_eq!(norm(&got), want, "{e}");
    }
}

#[test]
fn s4_unary_minus_binds_tighter_than_the_cast_operators_of_multiplication() {
    assert_eq!(show("-d_10_2 * d_10_2"), "((-d_10_2) * d_10_2)");
}

#[test]
fn s4_integer_arithmetic_stays_integer_in_the_wider_type() {
    for (e, want) in [
        ("a * b", "INTEGER"),
        ("a + b", "INTEGER"),
        ("a - b", "INTEGER"),
        ("a % b", "INTEGER"),
        ("a // b", "INTEGER"),
        ("ti + si", "SMALLINT"),
        ("ti + a", "INTEGER"),
        ("a + c", "BIGINT"),
        ("ti * c", "BIGINT"),
        ("c // a", "BIGINT"),
        ("-ti", "TINYINT"),
        ("-c", "BIGINT"),
        ("1 + 2", "INTEGER"),
        ("1 + 3000000000", "BIGINT"),
        ("7 // 2", "INTEGER"),
    ] {
        assert_eq!(ty(e), want, "{e}");
    }
}

#[test]
fn s4_only_division_of_integers_is_a_decimal() {
    assert_eq!(
        ty("7 / 2"),
        "DECIMAL(16,6)",
        "decimal-rules: `7 / 2` is 3.500000 of type DECIMAL(16,6)"
    );
    assert_eq!(ty("a / b"), "DECIMAL(16,6)");
    assert_eq!(ty("c / c"), "DECIMAL(25,6)");
    assert_eq!(ty("ti / ti"), "DECIMAL(9,6)");
    assert_eq!(ty("a / d_15_2"), "DECIMAL(18,6)");
}

#[test]
fn s4_decimal_result_types_follow_the_tables_of_decimal_rules() {
    // (left, right, +, *, /, %) from the example table of decimal-rules section 3; "err" is a bind error, "ns" is `not supported yet`
    let rows: [(&str, &str, &str, &str, &str, &str); 14] = [
        (
            "d_15_2",
            "e_15_2",
            "DECIMAL(16,2)",
            "DECIMAL(18,4)",
            "DECIMAL(23,8)",
            "DECIMAL(15,2)",
        ),
        (
            "d_15_2",
            "d_5_3",
            "DECIMAL(17,3)",
            "DECIMAL(18,5)",
            "DECIMAL(24,8)",
            "DECIMAL(5,3)",
        ),
        (
            "d_18_2",
            "d_18_2",
            "DECIMAL(19,2)",
            "DECIMAL(18,4)",
            "DECIMAL(26,8)",
            "DECIMAL(18,2)",
        ),
        (
            "d_38_2",
            "d_38_2",
            "DECIMAL(38,2)",
            "DECIMAL(38,4)",
            "err",
            "DECIMAL(38,2)",
        ),
        (
            "d_10_2",
            "d_10_2",
            "DECIMAL(11,2)",
            "DECIMAL(18,4)",
            "DECIMAL(18,8)",
            "DECIMAL(10,2)",
        ),
        (
            "d_10_9",
            "d_10_9",
            "DECIMAL(11,9)",
            "DECIMAL(20,18)",
            "DECIMAL(25,15)",
            "DECIMAL(10,9)",
        ),
        (
            "d_15_2",
            "d_20_2",
            "DECIMAL(21,2)",
            "DECIMAL(35,4)",
            "DECIMAL(23,8)",
            "DECIMAL(15,2)",
        ),
        (
            "d_20_20",
            "d_20_20",
            "DECIMAL(21,20)",
            "err",
            "err",
            "DECIMAL(20,20)",
        ),
        (
            "d_10_0",
            "d_10_0",
            "DECIMAL(11,0)",
            "DECIMAL(18,0)",
            "DECIMAL(16,6)",
            "DECIMAL(10,0)",
        ),
        (
            "d_19_0",
            "d_19_0",
            "DECIMAL(20,0)",
            "DECIMAL(38,0)",
            "DECIMAL(25,6)",
            "DECIMAL(19,0)",
        ),
        (
            "d_38_0",
            "d_38_0",
            "DECIMAL(38,0)",
            "DECIMAL(38,0)",
            "err",
            "DECIMAL(38,0)",
        ),
        (
            "d_10_8",
            "d_10_8",
            "DECIMAL(11,8)",
            "DECIMAL(18,16)",
            "DECIMAL(24,14)",
            "DECIMAL(10,8)",
        ),
        (
            "d_38_38",
            "d_1_0",
            "DECIMAL(38,38)",
            "DECIMAL(38,38)",
            "DECIMAL(38,38)",
            "ns",
        ),
        (
            "d_38_0",
            "d_38_38",
            "DECIMAL(38,38)",
            "DECIMAL(38,38)",
            "err",
            "ns",
        ),
    ];
    for (l, r, add, mul, div, rem) in rows {
        for (op, want) in [("+", add), ("-", add), ("*", mul), ("/", div), ("%", rem)] {
            let q = format!("{l} {op} {r}");
            match want {
                "err" => {
                    let e = ty_err(&q);
                    assert_eq!(e.class, ErrorClass::Bind, "{q}: {e}");
                    assert!(e.message.contains("above the maximum of 38"), "{q}: {e}");
                }
                "ns" => {
                    let e = ty_err(&q);
                    assert_eq!(e.class, ErrorClass::NotSupported, "{q}: {e}");
                }
                t => assert_eq!(ty(&q), t, "{q}"),
            }
        }
    }
}

#[test]
fn s4_an_integer_meets_a_decimal_as_an_exact_decimal() {
    assert_eq!(ty("a + d_15_2"), "DECIMAL(16,2)");
    assert_eq!(ty("a * d_15_2"), "DECIMAL(18,2)");
    assert_eq!(ty("c * d_15_2"), "DECIMAL(34,2)");
    assert_eq!(ty("1 + 2.5"), "DECIMAL(12,1)");
    assert_eq!(ty("-d_15_2"), "DECIMAL(15,2)");
    // the lineitem expression of the card: price * (1 - discount) * (1 + tax) is DECIMAL(18,6) and stays 64-bit (decimal-rules section 3 note)
    let p =
        ok("SELECT l_extendedprice * (1 - l_discount) * (1 + l_tax) AS r FROM 'lineitem.parquet'");
    assert_eq!(p.output[0].ty.to_string(), "DECIMAL(18,6)");
}

#[test]
fn s4_decimals_with_floats_and_dates_in_arithmetic() {
    let e = ty_err("d_15_2 + d");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(
        e.message.contains("cannot add DECIMAL(15,2) and DOUBLE")
            && e.message.contains("not mixed implicitly"),
        "{e}"
    );
    for q in ["d_5_3 * d", "d / d_10_2", "d_15_2 % f"] {
        assert_eq!(ty_err(q).class, ErrorClass::Bind, "{q}");
    }
    // floats with integers and with floats: the page does not say
    for q in ["d + 1", "d + d", "f * f", "-d", "a + d"] {
        assert_eq!(ty_err(q).class, ErrorClass::NotSupported, "{q}");
    }
    // DuckDB difference (section 9): date + integer
    let e = ty_err("DATE '1994-01-01' + 1");
    assert_eq!(e.class, ErrorClass::NotSupported);
    assert!(e.message.contains("intervals"), "{e}");
    assert_eq!(ty_err("dt - dt").class, ErrorClass::NotSupported);
    // strings and booleans are not numbers
    for q in ["s + 1", "bo * 2", "-s", "s || s"] {
        assert!(
            matches!(ty_err(q).class, ErrorClass::Bind | ErrorClass::NotSupported),
            "{q}"
        );
    }
    assert_eq!(ty_err("s + 1").class, ErrorClass::Bind);
    assert_eq!(ty_err("// 1").class, ErrorClass::Syntax);
    let e = ty_err("d_10_2 // a");
    assert_eq!(e.class, ErrorClass::NotSupported);
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 5

#[test]
fn s5_both_sides_of_a_comparison_have_one_type_or_mix_integers_and_decimals() {
    for q in [
        "a = b",
        "a = c",
        "ti < a",
        "a <> 3000000000",
        "d_15_2 > a",
        "a >= d_15_2",
        "d_15_2 = d_5_3",
        "d_38_2 <= d_10_9",
        "s = 'x'",
        "s < s",
        "dt >= DATE '1994-01-01'",
        "bo = TRUE",
        "bo < bo",
        "d > d",
        "f = f",
        "d_15_2 BETWEEN 1 AND 2.5",
        "1 < 2",
        "'a' = 'b'",
    ] {
        assert_eq!(ty(q), "BOOLEAN", "{q}");
    }
}

#[test]
fn s5_no_implicit_conversion_between_strings_numbers_and_dates() {
    // `1 = '1'` is a bind error naming the two types (DuckDB: true)
    let e = ty_err("1 = '1'");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(
        e.message.contains("INTEGER") && e.message.contains("VARCHAR"),
        "{e}"
    );
    assert_eq!((e.pos.line, e.pos.col), (1, 8));
    for q in [
        "a = 'x'",
        "s = 1",
        "dt = '1994-01-01'",
        "dt = 8766",
        "s > DATE '1994-01-01'",
        "bo = 1",
        "bo = 'true'",
        "d_15_2 = 'x'",
        "s IN (1, 2)",
        "a IN ('1')",
        "a BETWEEN '1' AND '2'",
        "CASE a WHEN '1' THEN 1 END",
    ] {
        let e = ty_err(q);
        assert_eq!(e.class, ErrorClass::Bind, "{q}: {e}");
    }
    let e = err("SELECT a FROM 't.parquet' WHERE s = 1");
    assert!(
        e.message.contains("VARCHAR") && e.message.contains("INTEGER"),
        "{e}"
    );
}

#[test]
fn s5_floats_are_not_compared_with_decimals_and_the_integer_case_is_not_specified() {
    let e = ty_err("d = d_15_2");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(e.message.contains("not mixed implicitly"), "{e}");
    assert_eq!(ty_err("f > d_5_3").class, ErrorClass::Bind);
    assert_eq!(ty_err("d > 1").class, ErrorClass::NotSupported);
    // FLOAT and DOUBLE are two types
    assert_eq!(ty_err("f = d").class, ErrorClass::Bind);
    // the way out: a cast on one side
    assert_eq!(ty("d = CAST(1.5 AS DOUBLE)"), "BOOLEAN");
    assert_eq!(ty("CAST(d_15_2 AS DOUBLE) > d"), "BOOLEAN");
}

#[test]
fn s5_between_is_two_comparisons_with_the_type_rules_of_both() {
    assert_eq!(show("a BETWEEN 1 AND 3"), "(a BETWEEN 1 AND 3)");
    assert_eq!(show("a NOT BETWEEN 1 AND 3"), "(a NOT BETWEEN 1 AND 3)");
    assert_eq!(
        show("d_15_2 BETWEEN 0.05 AND 7"),
        "(d_15_2 BETWEEN 0.05 AND 7)"
    );
    assert_eq!(ty_err("a BETWEEN 1 AND 'x'").class, ErrorClass::Bind);
    let p = ok("SELECT a BETWEEN 1 AND 2 AS r FROM 't.parquet'");
    assert!(matches!(
        p.plan.project.exprs[0].expr.kind,
        ExprKind::Between { negated: false, .. }
    ));
    // SYMMETRIC does not exist
    assert!(matches!(
        err("SELECT a BETWEEN SYMMETRIC 1 AND 2 FROM 't.parquet'").class,
        ErrorClass::Syntax | ErrorClass::NotSupported
    ));
}

#[test]
fn s5_in_takes_literals_of_a_comparable_type() {
    assert_eq!(show("a IN (1, 2, 3)"), "(a IN (1, 2, 3))");
    assert_eq!(show("a NOT IN (1, 2)"), "(a NOT IN (1, 2))");
    assert_eq!(show("s IN ('x', 'y')"), "(s IN ('x', 'y'))");
    assert_eq!(show("c IN (1, 3000000000)"), "(c IN (1, 3000000000))");
    assert_eq!(show("d_15_2 IN (1, 2.5, 100)"), "(d_15_2 IN (1, 2.5, 100))");
    assert_eq!(
        show("dt IN (DATE '2020-01-01', DATE '2021-01-01')"),
        "(dt IN (DATE '2020-01-01', DATE '2021-01-01'))"
    );
    assert_eq!(show("a IN (-1, -2)"), "(a IN (-1, -2))");
    // not a literal: the page says the list holds literals
    let e = ty_err("a IN (b, 2)");
    assert_eq!(e.class, ErrorClass::NotSupported);
    assert!(e.message.contains("literal"), "{e}");
    assert_eq!(ty_err("a IN (1 + 1)").class, ErrorClass::NotSupported);
    assert_eq!(ty_err("a IN ('x')").class, ErrorClass::Bind);
    assert_eq!(
        ty_err("a IN (1, DATE '2020-01-01')").class,
        ErrorClass::Bind
    );
}

#[test]
fn s5_the_logic_operators_need_booleans() {
    for q in ["a AND bo", "bo OR a", "NOT a", "a AND b"] {
        let e = ty_err(q);
        assert_eq!(e.class, ErrorClass::Bind, "{q}: {e}");
        assert!(e.message.contains("BOOLEAN"), "{q}: {e}");
    }
    let e = err("SELECT a FROM 't.parquet' WHERE a");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(e.message.contains("WHERE needs a BOOLEAN"), "{e}");
    assert_eq!((e.pos.line, e.pos.col), (1, 33));
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 6

#[test]
fn s6_case_results_have_one_type_or_widen_integers_and_decimals() {
    for (e, want) in [
        ("CASE WHEN bo THEN 1 ELSE 2 END", "INTEGER"),
        ("CASE WHEN bo THEN a ELSE c END", "BIGINT"),
        ("CASE WHEN bo THEN ti ELSE si END", "SMALLINT"),
        ("CASE WHEN bo THEN 'x' ELSE s END", "VARCHAR"),
        ("CASE WHEN bo THEN dt ELSE DATE '2020-01-01' END", "DATE"),
        ("CASE WHEN bo THEN d ELSE d END", "DOUBLE"),
        ("CASE WHEN bo THEN 1 ELSE 2.5 END", "DECIMAL(11,1)"),
        ("CASE WHEN bo THEN d_15_2 ELSE a END", "DECIMAL(15,2)"),
        ("CASE WHEN bo THEN d_10_2 ELSE d_5_3 END", "DECIMAL(11,3)"),
        ("CASE WHEN bo THEN d_20_2 ELSE 0.001 END", "DECIMAL(21,3)"),
        ("CASE a WHEN 1 THEN 'one' WHEN 2 THEN 'two' END", "VARCHAR"),
        ("CASE a + 1 WHEN 1 THEN b ELSE c END", "BIGINT"),
    ] {
        assert_eq!(ty(e), want, "{e}");
    }
    // no ELSE is ELSE NULL
    let p = ok("SELECT CASE WHEN bo THEN 1 END AS r FROM 't.parquet'");
    let ExprKind::Case {
        else_,
        whens,
        operand,
    } = &p.plan.project.exprs[0].expr.kind
    else {
        panic!("case")
    };
    assert!(else_.is_null_literal() && operand.is_none() && whens.len() == 1);
    assert_eq!(else_.ty.to_string(), "INTEGER");
}

#[test]
fn s6_case_results_that_cannot_be_one_type_are_bind_errors() {
    for (q, frag) in [
        ("CASE WHEN bo THEN 1 ELSE 'x' END", "INTEGER and VARCHAR"),
        ("CASE WHEN bo THEN dt ELSE 1 END", "DATE"),
        (
            "CASE WHEN bo THEN d ELSE d_15_2 END",
            "not mixed implicitly",
        ),
        (
            "CASE WHEN bo THEN d_38_0 ELSE d_38_38 END",
            "no common decimal type within 38 digits",
        ),
        (
            "CASE WHEN bo THEN d_20_20 ELSE d_38_2 END",
            "no common decimal type within 38 digits",
        ),
    ] {
        let e = ty_err(q);
        assert_eq!(e.class, ErrorClass::Bind, "{q}: {e}");
        assert!(e.message.contains(frag), "{q}: {e}");
    }
    assert_eq!(ty_err("CASE WHEN a THEN 1 END").class, ErrorClass::Bind);
}

#[test]
fn s6_cast_targets_and_the_x_colon_colon_type_form() {
    for (e, want) in [
        ("CAST(a AS INTEGER)", "INTEGER"),
        ("CAST(a AS INT)", "INTEGER"),
        ("CAST(a AS BIGINT)", "BIGINT"),
        ("CAST(a AS DOUBLE)", "DOUBLE"),
        ("CAST(a AS DOUBLE PRECISION)", "DOUBLE"),
        ("CAST(a AS DECIMAL(10,2))", "DECIMAL(10,2)"),
        ("CAST(a AS NUMERIC(38,0))", "DECIMAL(38,0)"),
        ("CAST(a AS VARCHAR)", "VARCHAR"),
        ("CAST(s AS DATE)", "DATE"),
        ("CAST(s AS DECIMAL(5,2))", "DECIMAL(5,2)"),
        ("CAST(d AS DECIMAL(5,2))", "DECIMAL(5,2)"),
        ("CAST(d_15_2 AS BIGINT)", "BIGINT"),
        ("CAST(d_15_2 AS DOUBLE)", "DOUBLE"),
        ("CAST(dt AS VARCHAR)", "VARCHAR"),
        ("CAST('x' AS BOOLEAN)", "BOOLEAN"),
        ("a::BIGINT", "BIGINT"),
        ("1.5::DECIMAL(5,2)", "DECIMAL(5,2)"),
        ("a::INT::BIGINT", "BIGINT"),
    ] {
        assert_eq!(ty(e), want, "{e}");
    }
    // `x::type` is the same as CAST
    assert_eq!(
        exprs("SELECT a::BIGINT AS r, (d_15_2 + 1)::DOUBLE AS q FROM 't.parquet'"),
        exprs("SELECT CAST(a AS BIGINT) AS r, CAST(d_15_2 + 1 AS DOUBLE) AS q FROM 't.parquet'")
    );
}

#[test]
fn s6_casts_the_page_does_not_allow() {
    for (q, class) in [
        ("CAST(a AS TINYINT)", ErrorClass::NotSupported),
        ("CAST(a AS SMALLINT)", ErrorClass::NotSupported),
        ("CAST(a AS REAL)", ErrorClass::NotSupported),
        ("TRY_CAST(a AS INTEGER)", ErrorClass::NotSupported),
        ("CAST(a AS DECIMAL(40,2))", ErrorClass::NotSupported),
        ("CAST(a AS DECIMAL(5,6))", ErrorClass::Bind),
        ("CAST(a AS DECIMAL(0,0))", ErrorClass::Bind),
        ("CAST(dt AS INTEGER)", ErrorClass::Bind),
        ("CAST(a AS DATE)", ErrorClass::Bind),
        ("CAST(bo AS DATE)", ErrorClass::Bind),
        ("CAST(bo AS INTEGER)", ErrorClass::NotSupported),
        ("CAST(d AS VARCHAR)", ErrorClass::NotSupported),
        ("CAST(a AS TIMESTAMP)", ErrorClass::NotSupported),
    ] {
        let e = ty_err(q);
        assert_eq!(e.class, class, "{q}: {e}");
    }
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 7

#[test]
fn s7_limit_and_offset_take_non_negative_integer_literals() {
    use nata_sql::plan::Limit;
    assert_eq!(
        ok("SELECT a FROM 't.parquet' LIMIT 10").plan.limit,
        Some(Limit { n: 10, offset: 0 })
    );
    assert_eq!(
        ok("SELECT a FROM 't.parquet' LIMIT 10 OFFSET 5").plan.limit,
        Some(Limit { n: 10, offset: 5 })
    );
    assert_eq!(
        ok("SELECT a FROM 't.parquet' LIMIT 0").plan.limit,
        Some(Limit { n: 0, offset: 0 })
    );
    assert_eq!(ok("SELECT a FROM 't.parquet'").plan.limit, None);
    assert_eq!(
        ok("SELECT a FROM 't.parquet' LIMIT 18446744073709551615")
            .plan
            .limit,
        Some(Limit {
            n: u64::MAX,
            offset: 0
        })
    );
    for (q, class, frag) in [
        (
            "SELECT a FROM 't.parquet' LIMIT -1",
            ErrorClass::Bind,
            "negative",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT 1 OFFSET -2",
            ErrorClass::Bind,
            "negative",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT 1.5",
            ErrorClass::Bind,
            "whole number",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT 18446744073709551616",
            ErrorClass::Bind,
            "larger",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT 1 + 1",
            ErrorClass::NotSupported,
            "whole number",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT a",
            ErrorClass::NotSupported,
            "whole number",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT '5'",
            ErrorClass::NotSupported,
            "whole number",
        ),
        (
            "SELECT a FROM 't.parquet' OFFSET 5",
            ErrorClass::NotSupported,
            "OFFSET without LIMIT",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT ALL",
            ErrorClass::NotSupported,
            "LIMIT ALL",
        ),
        (
            "SELECT a FROM 't.parquet' LIMIT 5, 10",
            ErrorClass::Syntax,
            "found: ,",
        ),
    ] {
        let e = err(q);
        assert_eq!(e.class, class, "{q}: {e}");
        assert!(e.message.contains(frag), "{q}: {e}");
    }
    assert_eq!(err("SELECT a FROM 't.parquet' LIMIT -1").pos.col, 34);
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 8

#[test]
fn s8_every_error_has_a_class_a_message_and_a_one_based_position() {
    let cases: [(&str, ErrorClass, u32, u32); 6] = [
        ("SELEC a FROM 't.parquet'", ErrorClass::Syntax, 1, 1),
        (
            "SELECT a,\n  b +\nFROM 't.parquet'",
            ErrorClass::Syntax,
            3,
            1,
        ),
        ("SELECT zz FROM 't.parquet'", ErrorClass::Bind, 1, 8),
        (
            "SELECT a FROM 't.parquet'\nWHERE s = 1",
            ErrorClass::Bind,
            2,
            7,
        ),
        (
            "SELECT abs(a) FROM 't.parquet'",
            ErrorClass::NotSupported,
            1,
            8,
        ),
        (
            "SELECT é, a FROM 't.parquet' WHERE zz",
            ErrorClass::Bind,
            1,
            8,
        ),
    ];
    for (q, c, line, col) in cases {
        let e = err(q);
        assert_eq!((e.class, e.pos.line, e.pos.col), (c, line, col), "{q}: {e}");
        let shown = e.to_string();
        assert!(
            shown.contains(&format!("line {line}, column {col}")),
            "{shown}"
        );
        match c {
            ErrorClass::NotSupported => {
                assert!(shown.starts_with("not supported yet: "), "{shown}")
            }
            ErrorClass::Syntax => assert!(shown.starts_with("syntax error: "), "{shown}"),
            ErrorClass::Bind => assert!(shown.starts_with("bind error: "), "{shown}"),
            ErrorClass::Execution => unreachable!(),
        }
    }
    // the column counts characters: é is one
    assert_eq!(err("SELECT 'é', zz FROM 't.parquet'").pos.col, 13);
    // an error at the end of the input is at the end
    let e = err("SELECT a FROM 't.parquet' WHERE");
    assert_eq!((e.pos.line, e.pos.col), (1, 32));
}

#[test]
fn s8_an_unknown_column_suggests_the_closest_name_within_an_edit_distance_of_2() {
    for (typo, want) in [
        ("l_orderky", Some("l_orderkey")),   // one deleted
        ("l_orderkeyy", Some("l_orderkey")), // one inserted
        ("l_ordrkey", Some("l_orderkey")),   // one deleted
        ("l_orderkye", Some("l_orderkey")),  // a swap is two edits
        ("l_oderky", Some("l_orderkey")),    // two deleted
        ("l_odrky", None),                   // three
        ("lineitem", None),
        ("l_shipdat", Some("l_shipdate")),
        ("L_SHIPDATE", Some("l_shipdate")), // unquoted folds: exact
    ] {
        let sql = format!("SELECT {typo} FROM 'lineitem.parquet'");
        if typo == "L_SHIPDATE" {
            ok(&sql);
            continue;
        }
        let e = err(&sql);
        assert_eq!(e.class, ErrorClass::Bind, "{typo}: {e}");
        assert_eq!(e.suggestion.as_deref(), want, "{typo}: {e}");
        if let Some(w) = want {
            assert!(
                e.to_string().contains(&format!("did you mean `{w}`?")),
                "{e}"
            );
        } else {
            assert!(!e.to_string().contains("did you mean"), "{e}");
        }
        assert_eq!((e.pos.line, e.pos.col), (1, 8));
    }
    // the closest wins
    let e = err("SELECT l_shipdat FROM 'lineitem.parquet'");
    assert_eq!(e.suggestion.as_deref(), Some("l_shipdate"));
    // in WHERE too
    let e = err("SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdte > DATE '1994-01-01'");
    assert_eq!(e.suggestion.as_deref(), Some("l_shipdate"));
}

#[test]
fn s8_the_limits_are_positioned_errors_before_parsing() {
    // detail of each limit is in the parse tests; here the class and that the plan call reports them
    let q = format!(
        "SELECT a FROM 't.parquet' WHERE {}a{}",
        "(".repeat(1001),
        ")".repeat(1001)
    );
    let e = plan(&q).expect_err("too deep");
    assert!(e.is_limit() && e.class == ErrorClass::Syntax, "{e}");
    let q = format!(
        "SELECT a FROM 't.parquet' WHERE bo{}",
        " AND bo".repeat(10_000)
    );
    assert!(plan(&q).expect_err("too long a chain").is_limit());
    let q = format!("SELECT a FROM 't.parquet' -- {}", "x".repeat(1_048_576));
    assert!(plan(&q).expect_err("too long a text").is_limit());
}

// ------------------------------------------------------------------------------------------------------------------------------------- section 9

#[test]
fn s9_the_differences_from_duckdb_each_have_a_test() {
    // 1. quoted identifiers are exact: "A" finds column A, not a (DuckDB: case-insensitive)
    let p = ok("SELECT \"A\" FROM 't.parquet'");
    assert_eq!(p.plan.scan.columns[0].name, "A");
    let s = MemSource::new().file("only_a.parquet", &[("a", Ok(Kind::Bool))]);
    let e = plan_sql("SELECT \"A\" FROM 'only_a.parquet'", &s).expect_err("exact");
    assert_eq!(e.class, ErrorClass::Bind);
    assert_eq!(e.suggestion.as_deref(), Some("a"));
    // 2. a string compared with a number is a bind error (DuckDB: true)
    assert_eq!(ty_err("1 = '1'").class, ErrorClass::Bind);
    // 3. date + integer is not supported yet (DuckDB: the next day)
    assert_eq!(
        ty_err("DATE '1994-01-01' + 1").class,
        ErrorClass::NotSupported
    );
    // 4. no float literal syntax: 1.5e0 is an exact decimal (DuckDB: DOUBLE)
    assert_eq!(ty("1.5e0"), "DECIMAL(2,1)");
    // and the numeric rows of decimal-rules section 9: / is exact, decimal with DOUBLE is an error
    assert_eq!(ty("a / b"), "DECIMAL(16,6)");
    assert_eq!(ty_err("d_15_2 + d").class, ErrorClass::Bind);
    assert_eq!(ty("1e2"), "DECIMAL(3,0)");
}

/// Counts the footers the binder asks for.
struct Counting<'a> {
    inner: &'a MemSource,
    asked: std::sync::Mutex<Vec<std::path::PathBuf>>,
}

impl SourceProvider for Counting<'_> {
    fn expand(&self, pattern: &str) -> Result<Vec<std::path::PathBuf>, String> {
        self.inner.expand(pattern)
    }

    fn leaves(
        &self,
        file: &std::path::Path,
    ) -> Result<Vec<nata_parquet::schema::Leaf>, nata_parquet::ScanError> {
        self.asked.lock().expect("lock").push(file.to_path_buf());
        self.inner.leaves(file)
    }
}

#[test]
fn s1_the_binder_reads_the_footer_of_the_first_file_only_and_the_other_files_are_not_looked_at() {
    // register item P10: 1,000 files must not mean 1,000 footer reads before the query starts; one of the later files does not even exist here
    let same = t_columns();
    let names: Vec<String> = (0..1000).map(|i| format!("d/{i:04}.parquet")).collect();
    let mut s = MemSource::new().file(&names[0], &same);
    for n in &names[1..10] {
        s = s.file(n, &same);
    }
    // files 10.. are not known to the source at all: asking for their footers would be an error
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let s = s.glob("d/*.parquet", &refs);
    let c = Counting {
        inner: &s,
        asked: std::sync::Mutex::new(vec![]),
    };
    let p = plan_sql("SELECT a FROM 'd/*.parquet'", &c).expect("binds on the first footer alone");
    assert_eq!(
        c.asked.lock().expect("lock").as_slice(),
        [std::path::PathBuf::from("d/0000.parquet")],
        "one footer"
    );
    assert_eq!(p.plan.scan.files.len(), 1000, "every file is still scanned");
    // the old binder asked for all of them, and failed on the unknown ones
    let c = Counting {
        inner: &s,
        asked: std::sync::Mutex::new(vec![]),
    };
    let e = plan_sql_with(
        "SELECT a FROM 'd/*.parquet'",
        &c,
        BindOptions { all_footers: true },
    )
    .expect_err("file 10 is unknown");
    assert!(e.message.contains("d/0010.parquet"), "{e}");
    assert_eq!(c.asked.lock().expect("lock").len(), 11);
}

#[test]
fn s1_a_glob_whose_later_file_has_another_schema_binds_and_the_scan_finds_the_file() {
    // P10: the bind-time error of the schema check moved to the scan (tests/real_files.rs runs it on real files); here only that the binder no longer sees it
    let same = t_columns();
    let mut other = t_columns();
    other[2].1 = Ok(Kind::Int {
        bits: 32,
        signed: true,
    });
    let s = MemSource::new()
        .file("d/1.parquet", &same)
        .file("d/2.parquet", &other)
        .glob("d/*.parquet", &["d/1.parquet", "d/2.parquet"]);
    assert!(plan_sql("SELECT a FROM 'd/*.parquet'", &s).is_ok());
    assert!(
        plan_sql_with(
            "SELECT a FROM 'd/*.parquet'",
            &s,
            BindOptions { all_footers: true }
        )
        .is_err()
    );
}

#[test]
fn s8b_nested_negations_preserve_the_negative_literals_type() {
    for (min, ty) in [("9223372036854775808", "BIGINT"), ("2147483648", "INTEGER")] {
        for expr in [
            format!("- -{min}"),
            format!("-(-{min})"),
            format!("-(-(-{min}))"),
        ] {
            let p = ok(&format!("EXPLAIN SELECT {expr} AS x FROM 't.parquet'"));
            let e = &p.plan.project.exprs[0].expr;
            assert_eq!(e.ty.to_string(), ty, "{expr}");
            assert!(matches!(e.kind, ExprKind::Neg(_)), "{expr}: {e:?}");
            assert!(p.plan.explain_text().contains(&format!("AS x: {ty}")));
        }
    }
}
