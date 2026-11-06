"""The hand-written test set (a): derived from the slice card (goal, quality-target row 'Correctness', part 3) and from check/queries/scan.
Writes testset.jsonl: {"id", "q", "expect": "accept" | "refuse" | "edge", "tag"}.
accept = in the subset, every arm must parse it to the same AST; refuse = outside the subset (or invalid), every arm must refuse it;
edge = dialect edge cases with no fixed expectation (compared with DuckDB and between the arms, and listed)."""
import json, sys

F = "'f.parquet'"

# --- in the subset: expressions, each used as a select item and (for booleans) after WHERE
EXPRS = [
    # literals
    "1", "0", "42", "2147483648", "9223372036854775808", "1.5", "0.1", "123456789012345678901234567890.12", "1e400", "1E-7", "1.5e+3", ".5", "5.", "007", "0.10",
    "'x'", "''", "'it''s'", "'Straße'", "'a--b'", "'/* c */'", "NULL", "DATE '1994-01-01'", "date '2000-02-29'",
    # identifiers
    "a", "A_b", "_x", "a$b", "é", "\"select\"", "\"A b\"", "\"a\"\"b\"", "\"日本語\"", "date",
    # unary
    "-a", "- a", "- -a", "-(-a)", "-0.0", "-1", "NOT a", "NOT NOT a", "NOT (a AND b)",
    # arithmetic and precedence
    "a + b", "a - b", "a * b", "a / b", "a % b", "a + b * c", "(a + b) * c", "a - b - c", "a - (b - c)", "a / b / c", "a * b % c", "-a * b", "a - -b", "a * -b",
    "a + b + c + d", "l_extendedprice * (1 - l_discount) * (1 + l_tax)", "1 + 2 * 3 - 4 / 5 % 6",
    # comparison
    "a = b", "a <> b", "a != b", "a < b", "a <= b", "a > b", "a >= b", "a + 1 = b * 2", "(a = b) = c",
    # boolean
    "a AND b", "a OR b", "a AND b OR c", "a OR b AND c", "(a OR b) AND c", "a OR b OR c", "a AND b AND c AND d", "NOT a AND b", "NOT a = b", "a = 1 AND b = 2 OR c = 3",
    "a = 1 OR b = 2 AND c = 3", "NOT a OR NOT b", "a AND NOT b",
    # between / in / is null
    "a BETWEEN 1 AND 2", "a NOT BETWEEN 1 AND 2", "a BETWEEN b + 1 AND c * 2", "a BETWEEN 0.05 AND 0.07", "d BETWEEN DATE '1994-01-01' AND DATE '1994-12-31'",
    "a BETWEEN 1 AND 2 AND b = 3", "a = b BETWEEN 1 AND 2", "a BETWEEN 1 AND 2 = c",
    "a IN (1)", "a IN (1, 2, 3)", "a NOT IN (1, 2)", "a IN ('x', 'y')", "a IN (1, NULL)", "a IN (b, c + 1)", "a IN (DATE '2020-01-01')", "a + 1 IN (2, 3)", "a IN (1, 2) AND b IN (3)",
    "a IS NULL", "a IS NOT NULL", "a + 1 IS NULL", "NOT a IS NULL", "a IS NULL AND b IS NOT NULL", "(a = b) IS NULL", "a = b IS NULL",
    # case
    "CASE WHEN a THEN b END", "CASE WHEN a THEN b ELSE c END", "CASE WHEN a > 1 THEN 'x' WHEN a > 0 THEN 'y' ELSE 'z' END", "CASE a WHEN 1 THEN 'one' WHEN 2 THEN 'two' END",
    "CASE a + 1 WHEN 1 THEN b ELSE c END", "CASE WHEN a THEN CASE WHEN b THEN 1 ELSE 2 END ELSE 3 END", "CASE WHEN a IS NULL THEN 0 ELSE a END + 1", "case when a then b else c end",
    # cast
    "CAST(a AS BOOLEAN)", "CAST(a AS INTEGER)", "CAST(a AS INT)", "CAST(a AS BIGINT)", "CAST(a AS DOUBLE)", "CAST(a AS DOUBLE PRECISION)",
    "CAST(a AS DECIMAL)", "CAST(a AS DECIMAL(10))", "CAST(a AS DECIMAL(10,2))", "CAST(a AS NUMERIC(38,0))", "CAST(a AS VARCHAR)", "CAST(a AS DATE)", "cast(a as decimal(15, 2))",
    "a::INTEGER", "1.5::DECIMAL(5,2)", "-a::INTEGER", "a::INT::BIGINT", "a::DOUBLE PRECISION", "a::VARCHAR = 'x'", "a + b::DECIMAL(10,2)", "(a + b)::BIGINT",
    "a // b", "a // b * c", "1 + 2 // 3", "a//b", "a % b // c",
    "TRUE", "FALSE", "a = TRUE", "a <> false", "NOT TRUE", "CASE WHEN TRUE THEN 1 ELSE 2 END", "true AND false OR a",
    "CAST(CAST(a AS BIGINT) AS DOUBLE)", "CAST('1.5' AS DECIMAL(5,2))", "CAST(NULL AS INTEGER)", "CAST(a + 1 AS BIGINT) * 2", "CAST(a AS DECIMAL(15,2)) / CAST(b AS DECIMAL(15,2))",
]
BOOLS = [e for e in EXPRS if any(t in e for t in (" = ", "<", ">", "AND", "OR", "NOT", "BETWEEN", " IN ", "IS ", "!="))]

STRUCT = [
    "SELECT * FROM 'a.parquet'", "select * from 'a.parquet'", "SeLeCt * FrOm 'a.parquet'", "SELECT a FROM 'a.parquet'", "SELECT a, b, c FROM 'a.parquet'",
    "SELECT a AS x FROM 'a.parquet'", "SELECT a AS \"Quoted Alias\", b AS \"x\"\"y\" FROM 'a.parquet'", "SELECT *, a FROM 'a.parquet'", "SELECT a, * FROM 'a.parquet'",
    "SELECT 1 FROM 'a.parquet'", "SELECT 1, 2.5, 'x', NULL, DATE '2020-01-01' FROM 'a.parquet'", "SELECT a + 1 AS a1, a * 2 AS a2, a / 2 AS a3 FROM 'a.parquet' LIMIT 3",
    "SELECT * FROM 'a.parquet' LIMIT 0", "SELECT * FROM 'a.parquet' LIMIT 10", "SELECT * FROM 'a.parquet' LIMIT 10 OFFSET 5", "SELECT * FROM 'a.parquet' LIMIT 10 OFFSET 0",
    "SELECT * FROM 'a.parquet' LIMIT 18446744073709551615", "SELECT * FROM 'a.parquet' LIMIT 5;", "SELECT * FROM 'a.parquet' WHERE a = 1;", "SELECT * FROM 'a.parquet';",
    "SELECT * FROM 'a.parquet' WHERE a > 1 LIMIT 5 OFFSET 2", "limit", "EXPLAIN SELECT * FROM 'a.parquet'", "EXPLAIN ANALYZE SELECT * FROM 'a.parquet'",
    "explain analyze select a from 'a.parquet' where a > 1 limit 2", "EXPLAIN SELECT a, b + 1 AS c FROM 'a.parquet' WHERE a = 1 LIMIT 5 OFFSET 1",
    "SELECT * FROM 'dir/*.parquet'", "SELECT * FROM 's3://bucket/x/**/*.parquet'", "SELECT * FROM '/abs/path with space.parquet'", "SELECT * FROM 'it''s.parquet'",
    "SELECT * FROM 'données/é.parquet'", "SELECT * FROM ''", "SELECT * FROM 'a.parquet' WHERE a = 'multi\nline'",
    "SELECT * FROM 'a.parquet' WHERE TRUE", "SELECT * FROM 'a.parquet' WHERE a IS NULL AND NOT FALSE",
    "SELECT\n  a,\n  b\nFROM 'a.parquet'\nWHERE a = 1\nLIMIT 2", "SELECT\ta\tFROM\t'a.parquet'", "SELECT a -- comment\nFROM 'a.parquet'", "SELECT /* c */ a FROM /* d */ 'a.parquet' /* e */ WHERE /* f */ a = 1",
    "  SELECT a FROM 'a.parquet'  ", "SELECT a FROM 'a.parquet' -- trailing comment", "SELECT a FROM 'a.parquet' /* nested /* comment */ */",
    # the examples of the card and of check/queries/scan
    "SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS net FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 LIMIT 10",
    "SELECT l_orderkey, l_linenumber, l_shipdate, l_extendedprice FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01'",
    "SELECT l_returnflag, l_linestatus, l_discount FROM 'lineitem.parquet' WHERE l_discount >= 0.055 AND l_returnflag = 'R'",
    "SELECT * FROM 'nation.parquet' WHERE n_comment IS NOT NULL AND n_regionkey <> 2",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipmode = 'AIR'",
    "SELECT n_nosuchcolumn FROM 'nation.parquet'",
]

OUT = [  # (tag, query): outside the subset; every arm must refuse
    ("aggregate", "SELECT count(*) FROM 'f.parquet'"), ("aggregate", "SELECT sum(a) AS s FROM 'f.parquet'"), ("function", "SELECT abs(a) FROM 'f.parquet'"),
    ("function", "SELECT * FROM 'f.parquet' WHERE lower(a) = 'x'"), ("function", "SELECT coalesce(a, 0) FROM 'f.parquet'"), ("function", "SELECT \"abs\"(a) FROM 'f.parquet'"),
    ("group-by", "SELECT a FROM 'f.parquet' GROUP BY a"), ("having", "SELECT a FROM 'f.parquet' GROUP BY a HAVING a > 1"), ("order-by", "SELECT a FROM 'f.parquet' ORDER BY a"),
    ("order-by", "SELECT a FROM 'f.parquet' ORDER BY a DESC LIMIT 3"),
    ("join", "SELECT a FROM 'f.parquet' JOIN 'g.parquet' ON a = b"), ("join", "SELECT a FROM 'f.parquet' LEFT JOIN 'g.parquet' ON a = b"), ("join", "SELECT a FROM 'f.parquet', 'g.parquet'"),
    ("join", "SELECT a FROM 'f.parquet' CROSS JOIN 'g.parquet'"), ("join", "SELECT a FROM 'f.parquet' NATURAL JOIN 'g.parquet'"),
    ("subquery-from", "SELECT a FROM (SELECT a FROM 'f.parquet')"), ("subquery-scalar", "SELECT (SELECT 1) FROM 'f.parquet'"), ("subquery-in", "SELECT a FROM 'f.parquet' WHERE a IN (SELECT b FROM 'g.parquet')"),
    ("exists", "SELECT a FROM 'f.parquet' WHERE EXISTS (SELECT 1 FROM 'g.parquet')"),
    ("setop", "SELECT a FROM 'f.parquet' UNION SELECT a FROM 'g.parquet'"), ("setop", "SELECT a FROM 'f.parquet' INTERSECT SELECT a FROM 'g.parquet'"), ("setop", "SELECT a FROM 'f.parquet' EXCEPT SELECT a FROM 'g.parquet'"),
    ("with", "WITH t AS (SELECT 1) SELECT * FROM 'f.parquet'"), ("values", "VALUES (1), (2)"), ("window", "SELECT row_number() OVER () FROM 'f.parquet'"),
    ("distinct", "SELECT DISTINCT a FROM 'f.parquet'"), ("distinct-on", "SELECT DISTINCT ON (a) a FROM 'f.parquet'"),
    ("like", "SELECT * FROM 'f.parquet' WHERE a LIKE 'x%'"), ("like", "SELECT * FROM 'f.parquet' WHERE a NOT LIKE 'x%'"), ("ilike", "SELECT * FROM 'f.parquet' WHERE a ILIKE 'x%'"), ("similar", "SELECT * FROM 'f.parquet' WHERE a SIMILAR TO 'x'"),
    ("concat", "SELECT a || b FROM 'f.parquet'"), ("concat", "SELECT 'a' || 'b' FROM 'f.parquet'"), ("concat-after-int-div", "SELECT a // b || c FROM 'f.parquet'"),
    ("type", "SELECT CAST(a AS TINYINT) FROM 'f.parquet'"), ("type", "SELECT CAST(a AS SMALLINT) FROM 'f.parquet'"), ("type", "SELECT a::TINYINT FROM 'f.parquet'"),
    ("select-all", "SELECT ALL a FROM 'f.parquet'"), ("offset-rows", "SELECT a FROM 'f.parquet' LIMIT 1 OFFSET 1 ROWS"), ("int-div-space", "SELECT a / / b FROM 'f.parquet'"),
    ("double-semicolon", "SELECT a FROM 'f.parquet';;"), ("leading-semicolon", ";SELECT a FROM 'f.parquet'"), ("semicolon-then-select", "SELECT a FROM 'f.parquet'; SELECT b FROM 'f.parquet';"),
    ("interval", "SELECT DATE '2020-01-01' + INTERVAL 1 DAY FROM 'f.parquet'"), ("timestamp-literal", "SELECT TIMESTAMP '2020-01-01 00:00:00' FROM 'f.parquet'"),
    ("unary-plus", "SELECT +a FROM 'f.parquet'"), ("power", "SELECT a ^ 2 FROM 'f.parquet'"), 
    ("is-distinct", "SELECT * FROM 'f.parquet' WHERE a IS DISTINCT FROM b"), ("is-true", "SELECT * FROM 'f.parquet' WHERE a IS TRUE"),
    ("table-alias", "SELECT a FROM 'f.parquet' AS t"), ("table-alias", "SELECT a FROM 'f.parquet' t"), ("table-name", "SELECT a FROM f"), ("table-name", "SELECT a FROM \"f\""),
    ("no-from", "SELECT 1"), ("no-from", "SELECT 1 + 1 AS two"), ("alias-without-as", "SELECT a b FROM 'f.parquet'"), ("alias-without-as", "SELECT a + 1 x FROM 'f.parquet'"),
    ("qualified", "SELECT t.a FROM 'f.parquet'"), ("qualified-star", "SELECT t.* FROM 'f.parquet'"), ("star-exclude", "SELECT * EXCLUDE (a) FROM 'f.parquet'"),
    ("offset-only", "SELECT a FROM 'f.parquet' OFFSET 5"), ("limit-comma", "SELECT a FROM 'f.parquet' LIMIT 5, 10"), ("offset-first", "SELECT a FROM 'f.parquet' OFFSET 1 LIMIT 2"),
    ("limit-all", "SELECT a FROM 'f.parquet' LIMIT ALL"), ("limit-expr", "SELECT a FROM 'f.parquet' LIMIT 1 + 1"), ("limit-negative", "SELECT a FROM 'f.parquet' LIMIT -1"),
    ("limit-decimal", "SELECT a FROM 'f.parquet' LIMIT 1.5"), ("limit-percent", "SELECT a FROM 'f.parquet' LIMIT 10 %"), ("limit-too-big", "SELECT a FROM 'f.parquet' LIMIT 18446744073709551616"),
    ("parameter", "SELECT * FROM 'f.parquet' WHERE a = $1"), ("parameter", "SELECT * FROM 'f.parquet' WHERE a = ?"), ("list", "SELECT [1, 2] FROM 'f.parquet'"), ("struct", "SELECT {'a': 1} FROM 'f.parquet'"),
    ("fetch", "SELECT a FROM 'f.parquet' FETCH FIRST 5 ROWS ONLY"), ("insert", "INSERT INTO t VALUES (1)"), ("create", "CREATE TABLE t (a INT)"), ("delete", "DELETE FROM 'f.parquet'"),
    ("copy", "COPY (SELECT 1) TO 'x.csv'"), ("set", "SET threads = 1"), ("two-statements", "SELECT a FROM 'f.parquet'; SELECT b FROM 'f.parquet'"),
    ("try-cast", "SELECT TRY_CAST(a AS INTEGER) FROM 'f.parquet'"), ("type", "SELECT CAST(a AS TIMESTAMP) FROM 'f.parquet'"), ("type", "SELECT CAST(a AS VARCHAR(10)) FROM 'f.parquet'"),
    ("type", "SELECT CAST(a AS INTEGER[]) FROM 'f.parquet'"), ("type", "SELECT CAST(a AS REAL) FROM 'f.parquet'"), ("string-prefix", "SELECT E'a\\n' FROM 'f.parquet'"),
    ("collate", "SELECT a COLLATE nocase FROM 'f.parquet'"), ("at-time-zone", "SELECT a AT TIME ZONE 'UTC' FROM 'f.parquet'"), ("sample", "SELECT * FROM 'f.parquet' USING SAMPLE 10%"),
    ("qualify", "SELECT a FROM 'f.parquet' QUALIFY a > 1"), ("window-clause", "SELECT a FROM 'f.parquet' WINDOW w AS ()"), ("lambda", "SELECT list_transform(a, x -> x) FROM 'f.parquet'"),
    ("json-arrow", "SELECT a ->> 'b' FROM 'f.parquet'"), ("explain-options", "EXPLAIN (ANALYZE) SELECT 1 FROM 'f.parquet'"), ("explain-verbose", "EXPLAIN VERBOSE SELECT 1 FROM 'f.parquet'"),
    ("describe", "DESCRIBE SELECT 1 FROM 'f.parquet'"), ("pragma", "PRAGMA version"), ("from-first", "FROM 'f.parquet' SELECT a"),
    ("empty", ""), ("only-select", "SELECT"), ("only-comment", "-- nothing"), ("unterminated-string", "SELECT 'abc FROM 'f.parquet'"), ("unterminated-ident", "SELECT \"abc FROM 'f.parquet'"),
    ("unterminated-comment", "SELECT a /* FROM 'f.parquet'"), ("missing-path", "SELECT a FROM"), ("missing-expr", "SELECT a FROM 'f.parquet' WHERE"), ("double-op", "SELECT a FROM 'f.parquet' WHERE a = = 1"),
    ("unbalanced", "SELECT (a FROM 'f.parquet'"), ("unbalanced", "SELECT a) FROM 'f.parquet'"), ("keyword-as-name", "SELECT select FROM 'f.parquet'"), ("keyword-as-name", "SELECT a AS from FROM 'f.parquet'"),
    ("empty-in", "SELECT * FROM 'f.parquet' WHERE a IN ()"), ("case-no-when", "SELECT CASE END FROM 'f.parquet'"), ("case-no-end", "SELECT CASE WHEN a THEN b FROM 'f.parquet'"),
    ("cast-no-type", "SELECT CAST(a AS) FROM 'f.parquet'"), ("between-no-and", "SELECT * FROM 'f.parquet' WHERE a BETWEEN 1"), ("trailing-comma", "SELECT a, FROM 'f.parquet'"),
    ("trailing-junk", "SELECT a FROM 'f.parquet' garbage"), ("trailing-junk2", "SELECT a FROM 'f.parquet' WHERE a = 1 2"), ("empty-ident", "SELECT \"\" FROM 'f.parquet'"),
]

EDGE = [  # dialect edge cases: no fixed expectation
    ("chained-comparison", "SELECT * FROM 'f.parquet' WHERE a = b = c"), ("chained-comparison", "SELECT * FROM 'f.parquet' WHERE a < b < c"), ("chained-between", "SELECT * FROM 'f.parquet' WHERE a BETWEEN 1 AND 2 BETWEEN 3 AND 4"),
    ("chained-in", "SELECT * FROM 'f.parquet' WHERE a IN (1) IN (2)"), ("between-and-precedence", "SELECT * FROM 'f.parquet' WHERE a BETWEEN 1 AND 2 AND b"), ("not-between-not", "SELECT * FROM 'f.parquet' WHERE NOT a NOT BETWEEN 1 AND 2"),
    ("number-trailing-letter", "SELECT 1a FROM 'f.parquet'"), ("number-trailing-e", "SELECT 1e FROM 'f.parquet'"), ("hex", "SELECT 0x1F FROM 'f.parquet'"), ("underscore-number", "SELECT 1_000 FROM 'f.parquet'"),
    ("dot-number", "SELECT 1.e2, 1.2.3 FROM 'f.parquet'"), ("hash-ident", "SELECT a#b FROM 'f.parquet'"), ("at-ident", "SELECT @a FROM 'f.parquet'"), ("emoji-ident", "SELECT 😀 FROM 'f.parquet'"),
    ("adjacent-strings", "SELECT 'a' 'b' FROM 'f.parquet'"), ("backtick-ident", "SELECT `a` FROM 'f.parquet'"), ("bracket-ident", "SELECT [a] FROM 'f.parquet'"), ("single-quote-alias", "SELECT a AS 'x' FROM 'f.parquet'"),
    ("double-quote-path", "SELECT * FROM \"f.parquet\""), ("is-null-eq", "SELECT * FROM 'f.parquet' WHERE a IS NULL = b"), ("not-in-prefix", "SELECT * FROM 'f.parquet' WHERE a = NOT b"),
    ("minus-minus", "SELECT a--1 FROM 'f.parquet'"), ("neg-neg-space", "SELECT a - - 1 FROM 'f.parquet'"), ("semicolon-only", ";"),
    ("nonascii-whitespace", "SELECT a FROM 'f.parquet'"), ("cr-lf", "SELECT a\r\nFROM 'f.parquet'"), ("nul-byte-free-control", "SELECT\u000ba FROM 'f.parquet'"),
    ("unicode-quotes", "SELECT ‘a’ FROM 'f.parquet'"), ("unicode-minus", "SELECT 1 − 1 FROM 'f.parquet'"), ("limit-zero-offset", "SELECT a FROM 'f.parquet' LIMIT 0 OFFSET 0"),
    ("limit-leading-zero", "SELECT a FROM 'f.parquet' LIMIT 007"), ("limit-exponent", "SELECT a FROM 'f.parquet' LIMIT 1e3"), ("date-bad-shape", "SELECT DATE '1994-1-1' FROM 'f.parquet'"),
    ("date-invalid-day", "SELECT DATE '1994-13-45' FROM 'f.parquet'"), ("date-not-a-date", "SELECT DATE 'hello' FROM 'f.parquet'"), ("date-as-name", "SELECT date AS date FROM 'f.parquet'"),
    ("as-keyword-lower", "SELECT a as b from 'f.parquet' where a between 1 and 2 limit 1 offset 1"), ("deep-parens", "SELECT ((((((((((a)))))))))) FROM 'f.parquet'"),
]


def build():
    rows, n = [], 0
    def add(q, expect, tag):
        nonlocal n
        n += 1
        rows.append({"id": f"t{n:04d}", "q": q, "expect": expect, "tag": tag})
    for e in STRUCT:
        if e == "limit":
            continue
        add(e, "accept", "structure")
    for e in EXPRS:
        add(f"SELECT {e} AS r FROM {F}", "accept", "select-item")
    for e in BOOLS:
        add(f"SELECT * FROM {F} WHERE {e}", "accept", "where")
    for tag, q in OUT:
        add(q, "refuse", tag)
    for tag, q in EDGE:
        add(q, "edge", tag)
    return rows


if __name__ == "__main__":
    rows = build()
    with open(sys.argv[1] if len(sys.argv) > 1 else "testset.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    c = {}
    for r in rows:
        c[r["expect"]] = c.get(r["expect"], 0) + 1
    print(c)
