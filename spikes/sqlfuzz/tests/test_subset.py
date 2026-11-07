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
        banned = [" JOIN ", " GROUP BY ", " ORDER BY ", " OVER (", " UNION ", " EXISTS ", "SELECT DISTINCT", " LIKE ", " DISTINCT FROM ", "TIMESTAMP", "COALESCE", " / "]
        for batch in range(30):
            for q in subset_cli.build(1, batch, 40, 25)[1]:
                sql = q.sql(NO)
                self.assertTrue(sql.startswith("SELECT ") and " FROM t" in sql, sql)
                for b in banned: self.assertNotIn(b, sql, sql)
                self.assertNotRegex(sql, r"\b\d+\.\d+e\d+", "no float literal syntax")

    def test_the_queries_cover_what_the_subset_has(self):
        text = " ".join(q.sql(NO) for b in range(40) for q in subset_cli.build(2, b, 40, 25)[1])
        for k in ["CASE WHEN ", "CAST(", "BETWEEN ", " IN (", "IS NULL", "IS NOT NULL", "NOT ", " AND ", " OR ", " % ", " // ", "LIMIT ", "OFFSET ", "WHERE "]:
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
        self.assertIn("6 of 6 subset faults caught; control clean", r.stdout)


if __name__ == "__main__":
    unittest.main()
