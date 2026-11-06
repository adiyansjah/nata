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
import json, os, shutil, subprocess, sys, tempfile, time, unittest
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

TRACES = os.path.join(HERE, "testdata", "traces")

class TraceTools(unittest.TestCase):
    """trace_to_tla.py, trace_check.py and mutate_trace_check.py without TLC (a fake java prints canned TLC output)."""
    def setUp(self):
        import trace_to_tla, trace_check, mutate_trace_check
        self.tt, self.tc, self.mtc = trace_to_tla, trace_check, mutate_trace_check
        self.tmp = tempfile.mkdtemp(prefix="ictrace-test-")
        self.real = {f: mm.sha(os.path.join(HERE, f)) for f in ("IcebergCommit.tla", "TraceSpec.tla", "trace_to_tla.py", "trace_check.py")}
    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)
        for f, h in self.real.items():
            self.assertEqual(mm.sha(os.path.join(HERE, f)), h, f"{f} was modified")

    def ev(self, a, seq, sub=1, **kw):
        return dict(a=a, seq=seq, sub=sub, _n=seq * 10 + sub, **kw)

    def test_every_trace_of_the_corpus_translates_and_renders(self):
        names = sorted(f for f in os.listdir(TRACES) if f.endswith(".jsonl"))
        self.assertGreaterEqual(len(names), 20)
        steps = set()
        for f in names:
            t = self.tt.translate(os.path.join(TRACES, f))
            self.assertTrue(t["trace"], f)
            steps |= {e["a"] for e in t["trace"]}
            out = os.path.join(self.tmp, f)
            self.tt.render(t, out)
            tla = open(os.path.join(out, "Tr.tla")).read()
            self.assertIn("TrTrace == <<", tla)
            cfg = open(os.path.join(out, "Tr.cfg")).read()
            for need in ("SPECIFICATION TraceSpec", "Trace <- TrTrace", "NotDone", "Style = \"STORE\""):
                self.assertIn(need, cfg)
        # every kind of step TraceSpec knows is produced by some trace of the corpus, and nothing else is produced
        spec = open(os.path.join(HERE, "TraceSpec.tla")).read()
        import re
        known = set(re.findall(r'e\.a = "([a-z-]+)"', spec))
        # recover-fail (the attempts are used up) is the one step no trace of the suite takes: 8 attempts are never needed
        self.assertEqual(known - steps, {"recover-fail"})
        self.assertEqual(steps - known, set())

    def test_the_corpus_has_one_expectation_per_trace(self):
        for f in sorted(os.listdir(TRACES)):
            meta = json.loads(open(os.path.join(TRACES, f)).readline())
            self.assertEqual(meta["a"], "meta", f)
            self.assertIn(meta["expect"], ("pass", "detect", "known-mismatch"), f)

    def test_an_intent_without_an_outcome_is_resolved_from_the_next_observed_version(self):
        base = [self.ev("deliver-intent", 5, 0, op="w1o0", att=1, exp=3, cur=3, keep=False, dup=0)]
        applied = self.tt.resolve_intents(base + [self.ev("plan", 7, 2, op="w2o0", ver=4)])
        self.assertEqual([(e["a"], e["res"], e["new"]) for e in applied if e["a"] == "deliver"], [("deliver", "ok", 4)])
        notyet = self.tt.resolve_intents(base + [self.ev("plan", 7, 2, op="w2o0", ver=3)])
        self.assertEqual([e for e in notyet if e["a"].startswith("deliver")], [])          # the create never happened: the request stays in flight
        nothing = self.tt.resolve_intents(base)
        self.assertEqual(nothing, [])
        late = self.tt.resolve_intents([self.ev("deliver-intent", 5, 0, op="w1o0", att=1, exp=2, cur=3, keep=False, dup=0), self.ev("final", 9, 1, ver=3, rows=[])])
        self.assertEqual([e["res"] for e in late if e["a"] == "deliver"], ["conflict"])    # the version existed: the create had to fail
        done = [self.ev("deliver-intent", 5, 0, op="w1o0", att=1, exp=3, cur=3), self.ev("deliver", 5, 0, op="w1o0", att=1, exp=3, cur=3, res="ok", new=4)]
        self.assertEqual(len(self.tt.resolve_intents(done)), 1)                            # an intent with its outcome is dropped

    def test_the_second_delivery_of_a_duplicate_moves_after_the_answer_is_read(self):
        d1 = self.ev("deliver", 5, 0, op="w1o0", att=1, dup=1, keep=True, res="ok", new=1, cur=0, exp=0)
        d2 = self.ev("deliver", 6, 0, op="w1o0", att=1, dup=2, keep=False, res="conflict", cur=1, exp=0)
        rc = self.ev("receive", 7, op="w1o0", att=1, res="ok")
        other = self.ev("send", 8, op="w9o0", att=1, exp=1)
        out = self.tt.commute_duplicates([d1, d2, rc, other])
        self.assertEqual([e["a"] for e in out], ["deliver", "receive", "deliver", "send"])
        self.assertTrue(out[2].get("moved"))
        # a second delivery that is not a conflict (the no-CAS store overwrites) stays where it is
        d2ok = dict(d2, res="ok", new=1)
        self.assertEqual([e["a"] for e in self.tt.commute_duplicates([d1, d2ok, rc])], ["deliver", "deliver", "receive"])

    def test_things_the_model_has_no_action_for_are_unmodelled_never_a_verdict(self):
        def trace(extra):
            evs = [self.ev("meta", 1), self.ev("plan", 2, 2, op="w0o0", ver=1, kind="append", mode="correct", isolation="serializable", max_attempts=8, keys=["a"], rows=[[0, "a"]]),
                   self.ev("deliver", 3, 0, op="w0o0", att=1, exp=1, cur=1, res="ok", new=2), self.ev("outcome", 4, op="w0o0", res="acked"),
                   self.ev("plan", 5, 2, op="w1o0", ver=2, kind="append", mode="correct", isolation="serializable", max_attempts=8, keys=["a"], rows=[[1000000, "a"]])] + extra
            p = os.path.join(self.tmp, "u.jsonl")
            open(p, "w").write("\n".join(json.dumps({k: v for k, v in e.items() if k != "_n"}) for e in evs) + "\n")
            return p
        self.tt.translate(trace([]))                                                      # the skeleton itself is fine
        for extra, exc in [([self.ev("rejected", 6, op="w1o0", att=1, code=503)], self.tt.Unmodelled),
                           ([self.ev("deliver", 6, 0, op="w1o0", att=1, exp=2, cur=2, res="error", err="x")], self.tt.Unmodelled),
                           ([self.ev("outcome", 6, op="w1o0", res="failed")], self.tt.Unmodelled),
                           ([self.ev("send", 6, op="w7o0", att=1, exp=2)], self.tt.BadTrace),          # an event of an operation that never planned
                           ([self.ev("final", 9, error="load: x")], self.tt.Unmodelled)]:
            with self.assertRaises(exc, msg=str(extra)):
                self.tt.translate(trace(extra))

    def test_a_damaged_sequence_counter_is_refused(self):
        evs = [self.ev("meta", 1), self.ev("send", 1, op="w1o0", att=1, exp=0)]
        p = os.path.join(self.tmp, "dup.jsonl")
        open(p, "w").write("\n".join(json.dumps({k: v for k, v in e.items() if k != "_n"}) for e in evs) + "\n")
        with self.assertRaises(self.tt.BadTrace) as cm:
            self.tt.translate(p)
        self.assertIn("counter was damaged", str(cm.exception))

    def test_a_torn_last_line_is_ignored_and_counted(self):
        src = os.path.join(TRACES, "unknown_outcome_correct.jsonl")
        p = os.path.join(self.tmp, "torn.jsonl")
        shutil.copy(src, p)
        with open(p, "a") as fh:
            fh.write('{"a": "send", "seq": 99, "op"')
        a = self.tt.translate(src)
        b = self.tt.translate(p)
        self.assertEqual(a["trace"], b["trace"])
        self.assertEqual(b["torn"], 1)

    def test_versions_are_relative_to_the_seed_and_writers_are_numbered_in_plan_order(self):
        t = self.tt.translate(os.path.join(TRACES, "lost_update_correct.jsonl"))
        self.assertEqual(t["version_offset"], 2)
        self.assertEqual(t["trace"][0], dict(a="plan", w=1, ver=0))
        self.assertEqual([t["writers"][w] for w in sorted(t["writers"])], ["w1o0", "w2o0", "w3o0"])
        self.assertEqual(t["consts"]["isolation"], "serializable")
        self.assertEqual((t["consts"]["crash"], t["consts"]["dup"], t["consts"]["loss"]), (0, 0, 0))
        d = self.tt.translate(os.path.join(TRACES, "duplicate_request_correct.jsonl"))
        self.assertEqual(d["consts"]["dup"], 1)
        c = self.tt.translate(os.path.join(TRACES, "crash_append_after_commit.jsonl"))
        self.assertEqual(c["consts"]["crash"], 1)
        l = self.tt.translate(os.path.join(TRACES, "lost_request_correct.jsonl"))
        self.assertEqual(l["consts"]["loss"], 1)
        s = self.tt.translate(os.path.join(TRACES, "phantom_snapshot_isolation_correct.jsonl"))
        self.assertEqual(s["consts"]["isolation"], "snapshot")
        self.assertNotIn("I4_NoPhantom", open(os.path.join(self.tmp, "x") if False else os.devnull).read() or "")
        self.tt.render(s, os.path.join(self.tmp, "snap"))
        self.assertNotIn("I4_NoPhantom", open(os.path.join(self.tmp, "snap", "Tr.cfg")).read())     # I4 holds under serializable only

    def test_classify_tlc_output(self):
        states = "State 1: <Initial predicate>\n/\\ i = 1\n\nState 2: <x>\n/\\ i = 2\n"
        self.assertEqual(self.tc.classify("Error: Invariant NotDone is violated.\nError: The behavior up to this point is:\n" + states + "/\\ i = 9\n", 12), ("VALID", "", 9))
        self.assertEqual(self.tc.classify("Error: Deadlock reached.\nError: The behavior up to this point is:\n" + states, 11), ("INVALID", "deadlock", 2))
        self.assertEqual(self.tc.classify("Error: Invariant I2_NoLostUpdate is violated.\n" + states, 12), ("INVALID", "invariant I2_NoLostUpdate violated", 2))
        self.assertEqual(self.tc.classify("Model checking completed. No error has been found.", 0)[0], "ERROR")      # cannot happen on a trace: not a verdict
        self.assertEqual(self.tc.classify("java.lang.NullPointerException: x", 1)[0], "ERROR")
        self.assertEqual(self.tc.classify("", 0)[0], "ERROR")

    def test_judge(self):
        j = self.tc.judge
        self.assertEqual(j({"expect": "pass"}, "VALID", False), "OK")
        self.assertEqual(j({"expect": "pass"}, "INVALID", False), "MISMATCH")
        self.assertEqual(j({"expect": "detect"}, "INVALID", False), "OK")
        self.assertEqual(j({"expect": "detect"}, "VALID", False), "MISMATCH")
        self.assertEqual(j({"expect": "known-mismatch"}, "INVALID", False), "OK")
        self.assertEqual(j({"expect": "known-mismatch"}, "VALID", False), "MISMATCH")      # the finding went away: look at it
        self.assertEqual(j({"expect": "detect"}, "ERROR", False), "MISMATCH")               # an error never counts as a detection
        self.assertEqual(j({"expect": "informational"}, "INVALID", False), "INFO")
        self.assertEqual(j({"expect": "detect"}, "INVALID", True), "MISMATCH")              # --all-valid
        self.assertEqual(j({}, "VALID", True), "OK")

    def run_check(self, canned, traces, *args):
        fake = os.path.join(self.tmp, "fake-tlc-java")
        open(fake, "w").write("#!/bin/sh\ncat \"$FAKE_TLC_OUT\"\nexit ${FAKE_TLC_RC:-0}\n")
        os.chmod(fake, 0o755)
        out = os.path.join(self.tmp, "canned.txt")
        open(out, "w").write(canned)
        return subprocess.run([sys.executable, "-I", os.path.join(HERE, "trace_check.py"), "--jar", "x.jar", "--java", fake, "--traces", traces,
                               "--out", os.path.join(self.tmp, "out"), "--work", os.path.join(self.tmp, "work"), "--jobs", "2", *args],
                              capture_output=True, text=True, env=dict(os.environ, FAKE_TLC_OUT=out, FAKE_TLC_RC="12"))

    def test_trace_check_runs_every_trace_reports_and_exits_by_expectation(self):
        d = os.path.join(self.tmp, "tr")
        os.makedirs(d)
        for n in ("lost_update_correct", "lost_update_bad_no_cas"):
            shutil.copy(os.path.join(TRACES, n + ".jsonl"), d)
        valid = "Error: Invariant NotDone is violated.\nState 1: x\n/\\ i = 7\n"
        p = self.run_check(valid, d)
        self.assertEqual(p.returncode, 1, p.stdout)                              # the bad writer's trace was "VALID": a mismatch
        self.assertIn("lost_update_bad_no_cas", p.stdout)
        self.assertRegex(p.stdout, r"lost_update_bad_no_cas\s+bad-no-cas\s+expect detect\s+VALID\s+MISMATCH")
        self.assertRegex(p.stdout, r"lost_update_correct\s+correct\s+expect pass\s+VALID\s+OK")
        dead = "Error: Deadlock reached.\nState 1: x\n/\\ i = 19\n"
        p = self.run_check(dead, d)
        self.assertRegex(p.stdout, r"lost_update_bad_no_cas\s+bad-no-cas\s+expect detect\s+INVALID\s+OK\s+.*the model does not allow event 19 of \d+: [a-z-]+ by model writer")
        self.assertIn("mismatches=1", p.stdout)                                  # the correct writer's trace was refused
        p = self.run_check(dead, d, "--all-valid")
        self.assertEqual(p.returncode, 1)
        self.assertIn("mismatches=2", p.stdout)
        # TLC that prints nothing recognisable is an ERROR, which fails the run
        p = self.run_check("", d)
        self.assertEqual(p.returncode, 1)
        self.assertIn("error=2", p.stdout)
        # a directory without traces is a usage error
        e = os.path.join(self.tmp, "empty")
        os.makedirs(e)
        self.assertEqual(self.run_check(valid, e).returncode, 2)

    def test_every_checker_mutant_applies_exactly_once_and_compiles(self):
        for name, (desc, f, edits) in self.mtc.MUTANTS.items():
            err = self.mtc.stage_tools(os.path.join(self.tmp, "m-" + name), f, edits)
            self.assertIsNone(err, name)
            self.assertNotEqual(mm.sha(os.path.join(self.tmp, "m-" + name, f)), self.real.get(f, mm.sha(os.path.join(HERE, f))), f"{name} changed nothing")
        self.assertGreaterEqual(len(self.mtc.MUTANTS), 10)
        bad = self.mtc.stage_tools(os.path.join(self.tmp, "bad"), "TraceSpec.tla", [("NO SUCH TEXT", "x")])
        self.assertIn("0 times", bad)

    def test_every_corruption_builds_from_the_corpus_and_changes_the_trace(self):
        dst = os.path.join(self.tmp, "corpus")
        want = self.mtc.build_corpus(TRACES, dst)
        for name, (base, expect, desc, fn) in self.mtc.DERIVED.items():
            out = open(os.path.join(dst, "x_" + name + ".jsonl")).read()
            self.assertNotEqual(out.splitlines()[1:], open(os.path.join(TRACES, base + ".jsonl")).read().splitlines()[1:], name)
            self.assertEqual(want["x_" + name], "INVALID" if expect == "detect" else "VALID")
        self.assertGreaterEqual(sum(1 for v in self.mtc.DERIVED.values() if v[1] == "detect"), 4)


if __name__ == "__main__":
    unittest.main()
