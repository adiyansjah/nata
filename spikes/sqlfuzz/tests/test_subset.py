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
