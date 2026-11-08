"""Tests of the slice 4 subset fuzzer (`run-subset`, `replay-subset`, `prove-subset`): determinism of a seed, a clean control, a PLANTED failure that is found,
shrunk, printed with its seed and replayed from the printed numbers, the queries stay inside the subset. Needs `duckdb` on the PATH. `python3 -I tests/test_subset.py`."""
import os, random, re, subprocess, sys, tempfile, unittest
HERE = os.path.dirname(os.path.abspath(__file__)); ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
from sqlfuzz import subset, subset_cli
from sqlfuzz.render import *

NO = frozenset()


def fuzz(*args):
    env = dict(os.environ); env.pop("AI_AGENT", None); env.pop("CLAUDECODE", None)
    return subprocess.run([sys.executable, "-I", os.path.join(ROOT, "fuzz.py"), *args], capture_output=True, text=True, env=env, cwd=ROOT)


class Generator(unittest.TestCase):
    def test_a_seed_replays_exactly(self):
        a, b = subset_cli.build(7, 3, 40, 25), subset_cli.build(7, 3, 40, 25)
        self.assertEqual(subset.setup_sql(a[0]), subset.setup_sql(b[0]))
        self.assertEqual([q.sql(NO) for q in a[1]], [q.sql(NO) for q in b[1]])
        self.assertNotEqual([q.sql(NO) for q in a[1]], [q.sql(NO) for q in subset_cli.build(7, 4, 40, 25)[1]])

    def test_the_queries_stay_inside_the_subset(self):
        banned = [" JOIN ", " ORDER BY ", " OVER (", " UNION ", " EXISTS ", "SELECT DISTINCT", " LIKE ", " DISTINCT FROM ", "TIMESTAMP", "COALESCE", " / "]
        for batch in range(30):
            for q in subset_cli.build(1, batch, 40, 25)[1]:
                sql = q.sql(NO)
                self.assertTrue(sql.startswith("SELECT ") and " FROM t" in sql, sql)
                for b in banned: self.assertNotIn(b, sql, sql)
                self.assertNotRegex(sql, r"\b\d+\.\d+e\d+", "no float literal syntax")

    def test_the_queries_cover_what_the_subset_has(self):
        text = " ".join(q.sql(NO) for b in range(40) for q in subset_cli.build(2, b, 40, 25)[1])
        for k in ["CASE WHEN ", "CAST(", "BETWEEN ", " IN (", "IS NULL", "IS NOT NULL", "NOT ", " AND ", " OR ", " % ", " // ", "LIMIT ", "OFFSET ", "WHERE ", "GROUP BY ", "HAVING ", "count(", "sum(", "avg(", "min(", "max("]:
            self.assertIn(k, text)


class DifferenceClasses(unittest.TestCase):
    def test_minimum_negation_class_requires_the_exact_operation_type_and_position(self):
        for value, typ in [("2147483648", "INTEGER"), ("9223372036854775808", "BIGINT")]:
            sql = f"SELECT (- -{value}) AS c0 FROM t1"
            why = f"target raised an error but the oracle answered: execution error: integer overflow in a unary minus: the result does not fit {typ}. Cast an operand (line 1, column 8)"
            self.assertTrue(subset_cli.classify(why, sql=sql).startswith("listed: negating"))
            for w, q in [(why.replace("column 8", "column 9"), sql),
                         (why.replace(typ, "SMALLINT"), sql),
                         (why.replace("a unary minus", "a multiplication"), sql),
                         (why, sql.replace(value, "5")), (why, sql.replace(value, value + ".0")),
                         (why, sql.replace(value, value + " + 1")),
                         (why.replace("(line 1, column 8)", ""), sql)]:
                self.assertFalse(subset_cli.classify(w, sql=q).startswith("listed:"), (w, q))
        # The new post-fix C difference: the literal is in WHERE, not necessarily a projection.
        _, queries = subset_cli.build(20261107, 2, 40, 25)
        sql = queries[13].sql(NO)
        path = "/tmp/fuzz/t2.parquet"
        submitted = sql.replace("FROM t2", f"FROM '{path}'")
        col = submitted.index("(- -9223372036854775808)") + 1
        why = f"target raised an error: error: {path}: row group 0: execution error: integer overflow in a unary minus: the result does not fit BIGINT. Cast an operand (line 1, column {col})"
        self.assertTrue(subset_cli.classify(why, sql=sql).startswith("listed: negating"))

    def test_shrunk_minimum_negation_anywhere_requires_the_exact_error(self):
        from dataclasses import replace
        _, queries = subset_cli.build(22014202119, 82, 40, 25)
        base = replace(queries[2], where=None, limit=None)
        for typ, minimum in [("INTEGER", "-2147483648"), ("BIGINT", "-9223372036854775808")]:
            why = f"target raised an error: integer overflow in a unary minus: the result does not fit {typ}."
            neg = Neg(typ, Lit(typ, minimum))
            for expr in (neg, Bin("BOOLEAN", "<>", neg, Col("SMALLINT", "sm")),
                         Neg(typ, Neg(typ, neg)),
                         Case(typ, ((Lit("BOOLEAN", "true"), neg),), Lit(typ, "0"))):
                query = replace(base, items=(Item(expr, "c1"),))
                self.assertTrue(subset_cli.shrunk_class(why, query.sql(NO), query).startswith("listed: negating"))
            query = replace(base, items=(Item(neg, "c1"),))
            for error in (why.replace("a unary minus", "a multiplication"),
                          why.replace(typ, "SMALLINT"), "target raised an error: decimal overflow in `*`"):
                self.assertTrue(subset_cli.shrunk_class(error, query.sql(NO), query).startswith("BUG?"))
            for expr in (Neg(typ, Col(typ, "b")), Neg(typ, Lit(typ, "-5")),
                         Bin(typ, "+", neg, Neg(typ, Col(typ, "b")))):
                query = replace(base, items=(Item(expr, "c1"),))
                self.assertTrue(subset_cli.shrunk_class(why, query.sql(NO), query).startswith("BUG?"))

    def test_decimal_remainder_product_requires_shape_type_and_overflow_witness(self):
        from dataclasses import replace
        tables, queries = subset_cli.build(22014202119, 2406, 40, 25)
        from decimal import Decimal, localcontext
        with localcontext() as ctx:
            ctx.prec = 80
            remainder = Decimal(tables["t1"][0][8]) % Decimal("99999999.99")
            self.assertEqual(remainder, Decimal("-69135780.36345"))
            self.assertEqual(remainder * Decimal(tables["t1"][0][7]), Decimal("-6913578035653642.1963655"))
        query = replace(queries[6], items=queries[6].items[:1])
        why = "target raised an error: decimal overflow in `*`: the result does not fit DECIMAL(18,7)."
        classify = lambda q=query, t=tables, w=why: subset_cli.shrunk_class(w, q.sql(NO), q, t)
        self.assertTrue(classify().startswith("listed: the tight decimal"))
        for t in ({"t1": tables["t1"][1:]}, {"t1": []}):
            self.assertTrue(classify(t=t).startswith("BUG?"))
        for w in (why.replace("18,7", "25,7"), why.replace("`*`", "`+`")):
            self.assertTrue(classify(w=w).startswith("BUG?"))
        for q in (replace(query, where=Lit("BOOLEAN", "true")), replace(query, limit=1),
                  replace(query, items=(Item(Case("BOOLEAN", ((Lit("BOOLEAN", "false"), query.items[0].e),), Lit("BOOLEAN", "true")), "c0"),)),
                  replace(query, items=(Item(Bin("DECIMAL(10,2)", "*", Col("DECIMAL(10,2)", "d"), Col("DECIMAL(10,2)", "d")), "c0"),))):
            self.assertTrue(classify(q=q).startswith("BUG?"))
        # A '%' in SQL cannot excuse an unrelated decimal overflow.
        self.assertTrue(subset_cli.shrunk_class(why, "SELECT (d * d), (a % 3) FROM t1").startswith("BUG?"))

    def test_decimal_integer_remainder_product_requires_the_exact_reaching_witness(self):
        from dataclasses import replace
        from decimal import Decimal, localcontext
        tables, queries = subset_cli.build(20261108, 1868, 40, 25)
        rem = Bin("DECIMAL(25,5)", "%", Lit("DECIMAL(25,5)", "-12345678901234567890.12345"), Col("INTEGER", "a"))
        product = Bin("DECIMAL(25,5)", "*", rem, Col("DECIMAL(10,2)", "d"))
        query = replace(queries[39], items=(Item(product, "c0"),), where=None)
        witness = list(tables["t2"][0])
        witness[1], witness[7] = "-2147483648", "12345.67"
        tables = {"t2": [witness]}
        with localcontext() as ctx:
            ctx.prec = 80
            r = Decimal(rem.l.text) % Decimal(witness[1])
            self.assertEqual(r, Decimal("-1797196498.12345"))
            self.assertEqual(r * Decimal(witness[7]), Decimal("-22187594890987.7329615"))
        why = "target raised an error: decimal overflow in `*`: the result does not fit DECIMAL(18,7)."
        classify = lambda q=query, t=tables, w=why: subset_cli.shrunk_class(w, q.sql(NO), q, t)
        self.assertTrue(classify().startswith("listed: tight decimal % INTEGER"))
        for a, d in [("NULL", "12345.67"), ("0", "12345.67"), ("-2147483648", "NULL"), ("-2147483648", "0.01")]:
            row = list(witness); row[1], row[7] = a, d
            self.assertTrue(classify(t={"t2": [row]}).startswith("BUG?"))
        self.assertTrue(classify(t={"t2": []}).startswith("BUG?"))
        zero = list(witness); zero[1] = "0"
        self.assertTrue(classify(t={"t2": [witness, zero]}).startswith("BUG?"))
        for q in (replace(query, where=Lit("BOOLEAN", "false")), replace(query, limit=0), replace(query, offset=1),
                  replace(query, items=(Item(Case(product.t, ((Lit("BOOLEAN", "false"), product),), Lit(product.t, "0")), "c0"),)),
                  replace(query, items=(Item(replace(product, r=Col("DECIMAL(25,5)", "w")), "c0"),)),
                  replace(query, items=(Item(replace(product, l=replace(rem, r=Col("BIGINT", "b"))), "c0"),))):
            self.assertTrue(classify(q=q).startswith("BUG?"))
        for w in (why.replace("18,7", "25,7"), why.replace("`*`", "`%`")):
            self.assertTrue(classify(w=w).startswith("BUG?"))

    def test_between_null_upper_cast_requires_a_reaching_null_row_and_the_exact_error(self):
        from dataclasses import replace
        tables, queries = subset_cli.build(20261107, 2202, 40, 25)
        original = queries[31]
        case = original.items[0].e
        between = case.whens[0][0]
        # The replay's shrunk shape, retaining its actual failing row (id 12).
        case = replace(case, whens=((replace(between, x=Col("INTEGER", "a")), Col("REAL", "r")),
                                   (Lit("BOOLEAN", "true"), Col("REAL", "r"))), els=Col("REAL", "r"))
        query = replace(original, items=(Item(case, "c0"),), where=None)
        row = tables["t1"][-1]
        tables = {"t1": [row], "t2": []}
        why = "target raised an error but the oracle answered: execution error: a BIGINT value does not fit INTEGER in this conversion"
        classify = lambda q, t=tables, w=why: subset_cli.shrunk_class(w, q.sql(NO), q, t)
        self.assertTrue(classify(query).startswith("listed: BETWEEN"))
        # No data, a FALSE or TRUE lower comparison, a fitting/NULL upper cast, another
        # operation's error, an earlier CASE branch, WHERE or LIMIT must never be excused.
        self.assertTrue(subset_cli.shrunk_class(why, query.sql(NO)).startswith("BUG?"))
        for a, b in [("0", "-4294967296"), ("2147483647", "-4294967296"), ("NULL", "1"), ("NULL", "NULL")]:
            r = row[:]; r[1], r[2] = a, b
            self.assertTrue(classify(query, {"t1": [r]}).startswith("BUG?"), (a, b))
        true_lower = row[:]; true_lower[1] = "2147483647"
        self.assertTrue(classify(query, {"t1": [row, true_lower]}).startswith("BUG?"))
        self.assertTrue(classify(query, {"t1": []}).startswith("BUG?"))
        self.assertTrue(classify(query, w="target raised an error: integer overflow").startswith("BUG?"))
        for q in [replace(query, where=Lit("BOOLEAN", "true")), replace(query, limit=1),
                  replace(query, items=(Item(replace(case, whens=((Lit("BOOLEAN", "true"), Col("REAL", "r")),) + case.whens), "c0"),))]:
            self.assertTrue(classify(q).startswith("BUG?"))


class Runs(unittest.TestCase):
    def test_the_control_finds_nothing(self):
        # DuckDB as the target of itself, the faithful rendering: no difference
        with tempfile.TemporaryDirectory() as out:
            r = fuzz("run-subset", "--seed", "3", "--cases", "400", "--jobs", "2", "--out", out)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertRegex(r.stdout, r"differences-to-classify 0")

    def test_a_planted_failure_is_found_and_replays_from_its_printed_seed(self):
        with tempfile.TemporaryDirectory() as out:
            r = fuzz("run-subset", "--seed", "11", "--cases", "600", "--jobs", "2", "--fault", "limit-off-by-one", "--out", out, "--shrink", "3")
            self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
            m = re.search(r"BUG\? seed=(\d+) batch=(\d+) query=(\d+): (.*)", r.stdout)
            self.assertIsNotNone(m, r.stdout)
            seed, batch, query, why = m.groups()
            self.assertEqual(seed, "11")
            self.assertTrue(any(f.startswith("subset-seed11-") for f in os.listdir(out)), "the shrunk reproducer is written")
            # the same failure from the printed numbers alone
            rr = fuzz("replay-subset", "--seed", seed, "--batch", batch, "--query", query, "--fault", "limit-off-by-one")
            self.assertEqual(rr.returncode, 0, rr.stdout + rr.stderr)
            self.assertIn(f"query {query}:", rr.stdout)
            self.assertNotIn("-> same", rr.stdout)
            self.assertIn("row count", rr.stdout)
            # and without the fault the replay is clean
            ok = fuzz("replay-subset", "--seed", seed, "--batch", batch, "--query", query)
            self.assertIn("-> same", ok.stdout)

    def test_the_known_bad_engines_the_subset_can_express_are_caught(self):
        if not os.environ.get("NATA_FUZZ_PROVE"): self.skipTest("set NATA_FUZZ_PROVE=1 (about two minutes)")
        r = fuzz("prove-subset", "--seed", "1", "--cases", "8000", "--control", "2000", "--jobs", "4")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("9 of 9 subset faults caught; control clean", r.stdout)



class AggregateContract(unittest.TestCase):
    def test_grouped_multisets_preserve_duplicates(self):
        from sqlfuzz.engine import Result
        from sqlfuzz.compare import compare
        a = Result(True, ['x'], ['INTEGER'], [[1],[1],[2]])
        b = Result(True, ['x'], ['INTEGER'], [[2],[1],[1]])
        self.assertIsNone(compare(a,b,False))
        b.rows[1] = [2]
        self.assertIsNotNone(compare(a,b,False))

    def test_integer_sum_metadata_classes(self):
        from sqlfuzz.subset_cli import aggregate_type_class
        self.assertIsNotNone(aggregate_type_class('sum','INTEGER','BIGINT','HUGEINT'))
        self.assertIsNotNone(aggregate_type_class('sum','BIGINT','DECIMAL(38,0)','HUGEINT'))
        self.assertIsNone(aggregate_type_class('sum','BIGINT','BIGINT','HUGEINT'))
        self.assertIsNone(aggregate_type_class('min','INTEGER','BIGINT','HUGEINT'))

    def test_exact_avg_class_requires_independent_answer(self):
        from sqlfuzz.subset_cli import exact_aggregate_oracle
        from sqlfuzz.render import Agg, Col, Item
        from sqlfuzz.engine import Result
        q = subset.SubsetSelect((Item(Agg('INTEGER','avg',Col('INTEGER','a')),'v0'),), subset.File('t1'))
        rows = [['1','1'] + ['NULL']*10, ['2','0'] + ['NULL']*10, ['3','0'] + ['NULL']*10]
        r = Result(True,['v0'],['DOUBLE'],[[1/3]])
        self.assertTrue(exact_aggregate_oracle(q,{'t1':rows},r))
        self.assertEqual(r.rows,[['0.333333']])
        from sqlfuzz.compare import compare
        self.assertIsNotNone(compare(r,Result(True,['v0'],['DOUBLE'],[['0.333334']]),False))

class AggregateOracleTransport(unittest.TestCase):
    def test_decimal_json_digits_and_hugeint_strings_survive(self):
        from sqlfuzz.engine import DuckDB
        result = DuckDB().run([], [(0,"SELECT CAST(12345678901234567890.12345 AS DECIMAL(25,5)), sum(CAST(1 AS INTEGER))")])[0]
        self.assertTrue(result.ok, result.error)
        self.assertEqual(result.rows, [['12345678901234567890.12345', 1]])

    def test_shrunk_key_projection_is_not_certified(self):
        from sqlfuzz.engine import Result
        q = subset.SubsetSelect((Item(Agg('INTEGER','avg',Col('INTEGER','a')),'v0'),), subset.File('t1'), group=(Col('INTEGER','id'),))
        self.assertFalse(subset_cli.exact_aggregate_oracle(q, {'t1':[]}, Result(True,['v0'],['DOUBLE'],[])))

if __name__ == "__main__":
    unittest.main()
