import os, random, subprocess, sys, tempfile, unittest
HERE = os.path.dirname(os.path.abspath(__file__)); ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
from sqlfuzz import gen
from sqlfuzz.cli import build, main
from sqlfuzz.compare import compare
from sqlfuzz.engine import DuckDB, Result
from sqlfuzz.render import *
from sqlfuzz.shrink import shrink

NO = frozenset(); NAN = float("nan")
R = lambda rows, types=("",), ok=True, error="": Result(ok, ["c"] * len(rows[0]) if rows else ["c"], list(types), rows, error)


class Compare(unittest.TestCase):
    def test_nan_and_negative_zero_equal_themselves(self):
        self.assertIsNone(compare(R([[NAN]]), R([[NAN]]), False))
        self.assertIsNone(compare(R([[0.0]]), R([[-0.0]]), False))
        self.assertIsNotNone(compare(R([[NAN]]), R([[0.0]]), False))
        self.assertIsNotNone(compare(R([[None]]), R([[NAN]]), False))
        self.assertIsNone(compare(R([[float("inf")]]), R([[float("inf")]]), False))
        self.assertIsNotNone(compare(R([[float("inf")]]), R([[float("-inf")]]), False))

    def test_float_tolerance_is_relative_and_ints_are_exact(self):
        self.assertIsNone(compare(R([[1e9 + 0.0]]), R([[1e9 * (1 + 1e-12)]]), False))
        self.assertIsNotNone(compare(R([[1.0]]), R([[1.001]]), False))
        self.assertIsNotNone(compare(R([[10 ** 17]]), R([[10 ** 17 + 1]]), False))

    def test_decimals_compare_exactly_by_value(self):
        t = ("DECIMAL(10,2)",)
        self.assertIsNone(compare(R([["1.50"]], t), R([[1.5]], t), False))
        self.assertIsNotNone(compare(R([["0.30"]], t), R([[0.30000000000000004]], t), False))

    def test_unordered_is_a_multiset_and_ordered_is_not(self):
        a, b = R([[1], [2], [2]]), R([[2], [1], [2]])
        self.assertIsNone(compare(a, b, False))
        self.assertIsNotNone(compare(a, b, True))
        self.assertIsNotNone(compare(R([[1], [2], [2]]), R([[1], [1], [2]]), False))     # same size, different multiset
        self.assertIsNotNone(compare(R([[1]]), R([[1], [1]]), False))

    def test_errors(self):
        err = R([], ok=False, error="boom")
        self.assertIsNone(compare(err, err, False))                      # both fail: agree
        self.assertIsNotNone(compare(err, R([[1]]), False))              # the target answered where the oracle raised
        self.assertIsNotNone(compare(R([[1]]), err, False))
        self.assertEqual(compare(R([[1]]), R([], ok=False, error="not supported yet: x"), False), "SKIP")

    def test_null_is_not_empty_string_or_zero(self):
        self.assertIsNotNone(compare(R([[None]]), R([[""]]), False))
        self.assertIsNotNone(compare(R([[None]]), R([[0]]), False))
        self.assertIsNotNone(compare(R([[True]]), R([[1]]), False))


class Generator(unittest.TestCase):
    def test_same_seed_same_cases(self):
        a, b = build(7, 3, 40, 25), build(7, 3, 40, 25)
        self.assertEqual(gen.setup_sql(a[0]), gen.setup_sql(b[0]))
        self.assertEqual([q.sql(NO) for q in a[1]], [q.sql(NO) for q in b[1]])
        self.assertNotEqual([q.sql(NO) for q in a[1]], [q.sql(NO) for q in build(7, 4, 40, 25)[1]])

    def test_data_contains_the_adversarial_values(self):
        sql = " ".join(s for b in range(30) for s in gen.setup_sql(build(1, b, 1, 25)[0]))
        for needle in ["'NaN'::DOUBLE", "-0.0", "'Infinity'::DOUBLE", "Straße", "😀", "2147483647", "-9223372036854775808", "NULL"]:
            self.assertIn(needle, sql)

    def test_every_query_kind_is_generated(self):
        rng = random.Random(0); kinds = set()
        from sqlfuzz.cli import kind_of
        for _ in range(400): kinds.add(kind_of(gen.gen_query(rng)))
        self.assertTrue({"project", "aggregate", "join-inner", "join-left", "join-right", "join-full", "semi-anti", "window", "setop"} <= kinds, kinds)


class Engine(unittest.TestCase):
    def test_duckdb_reports_rows_nulls_nan_and_errors_per_case(self):
        out = DuckDB().run(["CREATE TABLE t (x DOUBLE)", "INSERT INTO t VALUES (NULL), ('NaN'), (-0.0)"],
                           [(1, "SELECT x FROM t ORDER BY x"), (2, "SELECT * FROM nosuch"), (3, "SELECT 1 AS a WHERE false"), (4, "SELECT 5 AS a")])
        self.assertTrue(out[1].ok and out[1].types == ["DOUBLE"] and len(out[1].rows) == 3)
        self.assertTrue(any(r[0] is None for r in out[1].rows))
        self.assertFalse(out[2].ok)
        self.assertTrue(out[3].ok and out[3].rows == [])
        self.assertTrue(out[4].ok and out[4].rows == [[5]])        # an error in case 2 did not shift the others


class Shrinker(unittest.TestCase):
    def test_shrinks_a_query_and_the_rows(self):
        l = lambda n: Col("INTEGER", n)
        q = Select((Item(l("l.id"), "id"), Item(Bin("INTEGER", "+", l("l.a"), Lit("INTEGER", "5")), "x"), Item(l("l.b"), "y")),
                   Table("t1", "l"), where=Bin("BOOLEAN", "AND", Bin("BOOLEAN", ">", l("l.a"), Lit("INTEGER", "1")), IsNull(l("l.b"), True)),
                   order=(Order(l("l.id")),), limit=10, total=True)
        tables = {"t1": [[str(i), str(i), "1", "NULL", "NULL", "NULL", "NULL", "NULL", "NULL", "NULL"] for i in range(1, 9)], "t2": []}
        fails = lambda query, t: "l.a" in query.sql(NO) and any(r[1] == "5" for r in t["t1"])      # the "bug": needs column a and the row with a = 5
        sq, st, n = shrink(q, tables, fails)
        self.assertLess(len(sq.sql(NO)), len(q.sql(NO)) // 2)
        self.assertEqual(len(st["t1"]), 1)
        self.assertIsNone(sq.where); self.assertIsNone(sq.limit)


class Cli(unittest.TestCase):
    def run_cli(self, *a):
        with tempfile.TemporaryDirectory() as d:
            p = subprocess.run([sys.executable, "-I", os.path.join(ROOT, "fuzz.py"), *a, "--out", d], capture_output=True, text=True, timeout=300)
            repro = [open(os.path.join(d, f)).read() for f in sorted(os.listdir(d)) if f.endswith(".sql")]
            return p.returncode, p.stdout + p.stderr, repro

    def test_control_run_is_clean_and_deterministic(self):
        r1 = self.run_cli("run", "--seed", "3", "--cases", "400", "--jobs", "2")
        r2 = self.run_cli("run", "--seed", "3", "--cases", "400", "--jobs", "2")
        self.assertEqual(r1[0], 0, r1[1]); self.assertEqual(r1[1].split("[")[0], r2[1].split("[")[0])

    def test_a_fault_is_found_shrunk_and_the_reproducer_replays(self):
        rc, out, repro = self.run_cli("run", "--seed", "1", "--cases", "400", "--jobs", "2", "--fault", "limit-off-by-one", "--shrink", "1")
        self.assertEqual(rc, 1, out); self.assertTrue(repro, out)
        sql = repro[0]
        self.assertIn("LIMIT", sql.split("-- oracle query:")[1])
        stmts = [l for l in sql.split("\n") if l and not l.startswith("--")]
        p = subprocess.run(["duckdb", "-json"], input="\n".join(stmts) + "\n", capture_output=True, text=True, env={k: v for k, v in os.environ.items() if k != "AI_AGENT"})
        self.assertEqual(p.stderr.strip(), "", "the reproducer must run standalone on DuckDB")

    def adapter(self, bug=""):
        env = dict(os.environ, SQLFUZZ_ADAPTER_BUG=bug)
        cmd = f"{sys.executable} -I {os.path.join(ROOT, 'adapters', 'duckdb_adapter.py')}"
        with tempfile.TemporaryDirectory() as d:
            p = subprocess.run([sys.executable, "-I", os.path.join(ROOT, "fuzz.py"), "run", "--seed", "2", "--cases", "200", "--jobs", "2", "--shrink", "0",
                                "--target-cmd", cmd, "--out", d], capture_output=True, text=True, timeout=300, env=env)
        return p.returncode, p.stdout + p.stderr

    def test_command_adapter_protocol(self):
        self.assertEqual(self.adapter()[0], 0, self.adapter()[1])                          # a faithful adapter: no difference
        self.assertEqual(self.adapter("droprow")[0], 1)                                    # a lossy target is caught
        rc, out = self.adapter("crash"); self.assertEqual(rc, 1); self.assertIn("target failed", out)
        rc, out = self.adapter("unsupported"); self.assertEqual(rc, 0); self.assertIn("target-not-supported", out)   # reported as a gap, not a difference
        self.assertRegex(out, r"target-not-supported [1-9]")


if __name__ == "__main__":
    unittest.main()
