#!/usr/bin/env python3
"""Tests of the model tooling (no TLC needed): python3 -I -m unittest test_model_tools  (run from this directory).
They check the harness that stands between the model and a verdict:
  - the staging copy gets FRESH mtimes even when the source is old (the stale-mtime bug of earlier spikes);
  - two staging runs in a row both pass their baseline (byte-identical to the real spec, parse-ready), and the second
    never sees leftovers of the first;
  - an edit that does not apply, or applies twice, makes the mutant INVALID rather than silently a no-op;
  - every mutant's edits apply exactly once to the real spec, and the real spec is untouched by all of it;
  - the static lint passes (every action assigns every variable, every cfg name exists);
  - the targeted re-run mode of run_model.py (class RunModelTargeted, with testdata/fake-java instead of TLC): the list of runs
    and their order, per-config time limits, a budget that clamps every run, TIMEOUT / NOT-RUN / ERROR as mismatches, an unknown
    name as a usage error, and resuming a restarted run."""
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
        self.assertEqual(tlc_runner.classify("Model checking completed. No error has been found.\n9 states generated, 7 distinct states found, 0 states left", 0)[0], "PASS")
        self.assertEqual(tlc_runner.classify("Model checking completed. No error has been found.", 1)[0], "ERROR")   # exit status must agree
        self.assertEqual(tlc_runner.classify("Error: Evaluation error: bad", 150)[0], "ERROR")
        self.assertEqual(tlc_runner.classify("", 0)[0], "ERROR")

    def test_lint(self):
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "lint_spec.py")], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)

FAKE_JAVA_SRC = os.path.join(HERE, "testdata", "fake-java")

class RunModelTargeted(unittest.TestCase):
    """run_model.py with a fake java: what the `icebergmodel` GCP job relies on."""
    DEFAULT_RERUN = ["three-store-ser", "three-rest-snap-i4", "live-2", "live-3"]

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="icmodel-run-")
        self.out = os.path.join(self.tmp, "out")
        self.log = os.path.join(self.tmp, "calls.log")
        self.fake_java = os.path.join(self.tmp, "fake-java")      # a copy with the executable bit set, whatever an upload did to the original
        shutil.copy(FAKE_JAVA_SRC, self.fake_java)
        os.chmod(self.fake_java, 0o755)
    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_model(self, *args, env=None, timeout=120):
        e = dict(os.environ, FAKE_LOG=self.log, **(env or {}))
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "run_model.py"), "--jar", "x.jar", "--java", self.fake_java,
                            "--out", self.out, "--work", os.path.join(self.tmp, "work"), *args],
                           capture_output=True, text=True, env=e, timeout=timeout)
        return p
    def rows(self, p):
        return {l.split()[0]: l for l in p.stdout.splitlines() if l.split() and l.split()[0] not in ("run", "model:", "SANY:", "TOTAL")}
    def called(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as fh:
            return [l.split()[0] for l in fh]

    def test_list_names_every_run(self):
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "run_model.py"), "--list"], capture_output=True, text=True)
        names = p.stdout.split()
        self.assertEqual(len(names), 24)
        for n in self.DEFAULT_RERUN + ["canary-Unknown", "canary-ReaderPinned"]:
            self.assertIn(n, names)
        self.assertEqual(sum(n.startswith("canary-") for n in names), 11)

    def test_only_runs_exactly_these_in_this_order(self):
        order = ["canary-Unknown", "three-store-ser", "live-2"]
        p = self.run_model("--only", ",".join(order))
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.called(), order)
        self.assertIn("TOTAL mismatches=0", p.stdout)

    def test_the_rerun_set_with_expected_outcomes_passes(self):
        names = self.DEFAULT_RERUN + [n for n in subprocess.run([sys.executable, "-I", os.path.join(HERE, "run_model.py"), "--list"],
                                       capture_output=True, text=True).stdout.split() if n.startswith("canary-")]
        p = self.run_model("--only", ",".join(names))
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        rows = self.rows(p)
        self.assertEqual(len(rows), 15)
        self.assertRegex(rows["three-rest-snap-i4"], r"OK\s+VIOLATED I4_NoPhantom")
        self.assertRegex(rows["canary-Unknown"], r"REACHED")
        self.assertRegex(rows["live-3"], r"OK\s+PASS")

    def test_unknown_name_is_a_usage_error_not_a_silent_pass(self):
        # the old script skipped names it did not know and reported "TOTAL mismatches=0" for a run that checked nothing
        p = self.run_model("--only", "three-store-ser,three-stor-ser")
        self.assertEqual(p.returncode, 2, p.stdout + p.stderr)
        self.assertIn("three-stor-ser", p.stderr)
        self.assertEqual(self.called(), [])

    def test_timeout_is_a_mismatch_and_says_timeout(self):
        p = self.run_model("--only", "three-store-ser,live-2", "--timeout", "1", env={"FAKE_SLOW": "three-store-ser", "FAKE_SLOW_SECONDS": "30"})
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        rows = self.rows(p)
        self.assertRegex(rows["three-store-ser"], r"MISMATCH\s+TIMEOUT")
        self.assertRegex(rows["live-2"], r"OK")
        self.assertIn("TOTAL mismatches=1", p.stdout)

    def test_error_is_a_mismatch(self):
        p = self.run_model("--only", "live-2", env={"FAKE_BROKEN": "live-2"})
        self.assertEqual(p.returncode, 1)
        self.assertRegex(self.rows(p)["live-2"], r"MISMATCH\s+ERROR")

    def test_big_runs_get_the_longer_limit_and_the_rest_the_shorter(self):
        # both sleep 3 s; --timeout 1 cuts the canary, --timeout-big 6 lets the three-* run finish
        p = self.run_model("--only", "three-store-ser,canary-Unknown", "--timeout", "1", "--timeout-big", "6",
                           env={"FAKE_SLOW": "three-store-ser canary-Unknown", "FAKE_SLOW_SECONDS": "3"})
        rows = self.rows(p)
        self.assertRegex(rows["three-store-ser"], r"OK\s+PASS", p.stdout)
        self.assertRegex(rows["canary-Unknown"], r"NOT-REACHED\s+TIMEOUT", p.stdout)
        self.assertEqual(p.returncode, 1)

    def test_budget_not_started_runs_are_not_run_mismatches(self):
        p = self.run_model("--only", "live-2,live-3,canary-Unknown", "--budget", "1", env={"FAKE_SLOW": "live-2", "FAKE_SLOW_SECONDS": "2"})
        rows = self.rows(p)
        self.assertRegex(rows["live-3"], r"NOT-RUN")
        self.assertRegex(rows["canary-Unknown"], r"NOT-RUN")
        self.assertEqual(self.called(), ["live-2"])
        self.assertEqual(p.returncode, 1)
        self.assertIn("TOTAL mismatches=2", p.stdout)

    def test_clamp_cuts_a_run_to_the_budget_left(self):
        # without --clamp-to-budget a 100 s limit lets a 20 s run overrun a 3 s budget; with it the run is cut at the budget
        t0 = time.time()
        p = self.run_model("--only", "live-2,live-3", "--timeout", "100", "--budget", "3", "--min-slice", "1", "--clamp-to-budget",
                           env={"FAKE_SLOW": "live-2", "FAKE_SLOW_SECONDS": "20"})
        self.assertLess(time.time() - t0, 15)
        rows = self.rows(p)
        self.assertRegex(rows["live-2"], r"MISMATCH\s+TIMEOUT no result after [1-3]s", p.stdout)
        self.assertRegex(rows["live-3"], r"NOT-RUN\s+only 0s of the time budget of 3s are left", p.stdout)
        self.assertEqual(p.returncode, 1)

    def test_resume_skips_a_run_that_already_ended_as_expected(self):
        p1 = self.run_model("--only", "live-2,live-3", "--timeout", "1", env={"FAKE_SLOW": "live-3", "FAKE_SLOW_SECONDS": "30"})
        self.assertEqual(p1.returncode, 1)
        os.remove(self.log)
        p2 = self.run_model("--only", "live-2,live-3", "--resume")
        self.assertEqual(p2.returncode, 0, p2.stdout + p2.stderr)
        self.assertEqual(self.called(), ["live-3"], "only the run that had not ended as expected is repeated")
        self.assertIn("(resumed)", self.rows(p2)["live-2"])
        # without --resume everything runs again
        os.remove(self.log)
        self.run_model("--only", "live-2,live-3")
        self.assertEqual(self.called(), ["live-2", "live-3"])

    def test_default_behaviour_is_unchanged_without_the_new_flags(self):
        p = self.run_model("--only", "smoke-rest,appends2-rest,canary-Conflict")
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(self.called(), ["smoke-rest", "appends2-rest", "canary-Conflict"])

if __name__ == "__main__":
    unittest.main()
