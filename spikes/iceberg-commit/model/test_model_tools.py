#!/usr/bin/env python3
"""Tests of the model tooling (no TLC needed): python3 -I -m unittest test_model_tools  (run from this directory).
They check the harness that stands between the model and a verdict:
  - the staging copy gets FRESH mtimes even when the source is old (the stale-mtime bug of earlier spikes);
  - two staging runs in a row both pass their baseline (byte-identical to the real spec, parse-ready), and the second
    never sees leftovers of the first;
  - an edit that does not apply, or applies twice, makes the mutant INVALID rather than silently a no-op;
  - every mutant's edits apply exactly once to the real spec, and the real spec is untouched by all of it;
  - the static lint passes (every action assigns every variable, every cfg name exists)."""
import os, shutil, subprocess, sys, tempfile, time, unittest
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mutate_model as mm, tlc_runner

class Tools(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="icmodel-test-")
        self.real = mm.sha(os.path.join(HERE, mm.SPEC))
    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)
        self.assertEqual(mm.sha(os.path.join(HERE, mm.SPEC)), self.real, "the real spec was modified")

    def test_stage_has_fresh_mtimes(self):
        old = time.time() - 86400 * 30
        probe = os.path.join(self.tmp, "src-copy")
        shutil.copytree(HERE, probe, ignore=shutil.ignore_patterns("work*", "__pycache__"))
        for root, _, files in os.walk(probe):
            for f in files:
                os.utime(os.path.join(root, f), (old, old))
        saved = mm.HERE
        mm.HERE = probe
        try:
            before = time.time() - 2
            d = mm.stage(os.path.join(self.tmp, "stage"))
        finally:
            mm.HERE = saved
        for root, _, files in os.walk(d):
            for f in files:
                self.assertGreaterEqual(os.path.getmtime(os.path.join(root, f)), before, f"{f} has a stale mtime")

    def test_two_stagings_in_a_row_both_pass_their_baseline(self):
        for i in range(2):
            d = mm.stage(os.path.join(self.tmp, "stage"))
            self.assertEqual(mm.sha(os.path.join(d, mm.SPEC)), self.real, f"run {i + 1}: baseline differs from the real spec")
            self.assertTrue(os.path.exists(os.path.join(d, "cfg", "smoke-rest.cfg")))
            # leftovers of the previous run must be gone
            junk = os.path.join(d, "leftover.txt")
            open(junk, "w").write("x")
        d = mm.stage(os.path.join(self.tmp, "stage"))
        self.assertFalse(os.path.exists(os.path.join(d, "leftover.txt")))

    def test_edit_that_does_not_apply_is_invalid(self):
        d, err = mm.make_mutant("probe", self.tmp, edits=[("THIS TEXT IS NOT IN THE SPEC", "x")])
        self.assertIsNotNone(err)
        self.assertIn("0 times", err)

    def test_edit_that_applies_twice_is_invalid(self):
        d, err = mm.make_mutant("probe", self.tmp, edits=[("Refs(s)", "Refs(t)")])
        self.assertIsNotNone(err)
        self.assertRegex(err, r"applies (?:[2-9]|[1-9]\d+) times")

    def test_every_mutant_applies_exactly_once_and_changes_the_spec(self):
        for name in mm.MUTANTS:
            d, err = mm.make_mutant(name, self.tmp)
            self.assertIsNone(err, name)
            self.assertNotEqual(mm.sha(os.path.join(d, mm.SPEC)), self.real, f"{name} changed nothing")
        for name in mm.REQUIRED:
            self.assertIn(name, mm.MUTANTS)

    def test_dry_run_twice_in_a_row(self):
        for i in range(2):
            p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "mutate_model.py"), "--dry", "--out", os.path.join(self.tmp, "dry")],
                               capture_output=True, text=True)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            self.assertIn("BASELINE staged", p.stdout)

    def test_classify(self):
        self.assertEqual(tlc_runner.classify("Error: Invariant I2_NoLostUpdate is violated.\n5 states generated, 4 distinct states found", 12)[:2],
                         ("VIOLATED", "I2_NoLostUpdate"))
        self.assertEqual(tlc_runner.classify("Model checking completed. No error has occurred.\n9 states generated, 7 distinct states found, 0 states left", 0)[0], "PASS")
        self.assertEqual(tlc_runner.classify("Model checking completed. No error has occurred.", 1)[0], "ERROR")   # exit status must agree
        self.assertEqual(tlc_runner.classify("Error: Evaluation error: bad", 150)[0], "ERROR")
        self.assertEqual(tlc_runner.classify("", 0)[0], "ERROR")

    def test_lint(self):
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "lint_spec.py")], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)

if __name__ == "__main__":
    unittest.main()
