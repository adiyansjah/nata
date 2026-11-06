#!/usr/bin/env python3
"""Mutation check of the TRACE-VALIDATION CHECKER itself (docs/decisions/0006: a check counts only if it is shown to fail
on a known-bad version). Three parts, all on the corpus in testdata/traces (real traces of the Rust suite, store backend):

 1. CORRUPTED TRACES. Each is a real, VALID trace damaged in one way (an event dropped, two events swapped, a wrong version
    number, a wrong answer, a wrong attempt, a wrong outcome ...). The unmutated checker must say INVALID for every one
    (a corruption that is benign is a bug of this script, never a pass). Two derived traces that must stay VALID (a writer
    killed inside the catalog call; a torn last line) guard the other direction.
 2. MUTANTS OF THE CHECKER: one-line edits of trace_to_tla.py, TraceSpec.tla and trace_check.py, each applied (exactly once)
    to a private copy of the tooling. The mutated checker runs on the whole corpus; the mutant is
      CAUGHT    some trace no longer gets the verdict it must get (a VALID trace turns INVALID, a corrupted or known-bad one turns
                VALID), or some trace cannot be processed any more;
      SURVIVED  every verdict is unchanged (the checker cannot see this bug: a failure);
      INVALID   an edit does not apply exactly once, or every trace fails (a broken mutant is never evidence).
 3. MODEL MUTANTS (--model-mutants): the known-bad traces replayed against the nine mutants of the MODEL (mutate_model.py).
    Informational: it shows which protection of the model is the reason a known-bad trace is rejected (the rejection should
    disappear when that protection is removed).

usage: mutate_trace_check.py --jar tla2tools.jar --java java --out DIR [--work DIR] [--jobs N] [--only a,b] [--list]
                             [--no-mutants] [--model-mutants] [--corpus DIR]
Exit 1 if the baseline is not as expected, or a mutant SURVIVED or is INVALID."""
import argparse, json, os, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mutate_model

CORPUS = os.path.join(HERE, "testdata", "traces")
TOOLING = ["trace_check.py", "trace_to_tla.py", "IcebergCommit.tla", "TraceSpec.tla"]

# ---------------------------------------------------------------------------------------------------- corruptions
def ix(evs, a, op=None, att=None, nth=0, **kw):
    """Index of the nth event with these fields."""
    n = 0
    for k, e in enumerate(evs):
        if e["a"] == a and (op is None or e.get("op") == op) and (att is None or e.get("att") == att) \
                and all(e.get(f) == v for f, v in kw.items()):
            if n == nth:
                return k
            n += 1
    raise KeyError((a, op, att, nth, kw))

def drop(evs, *picks):
    gone = {id(evs[ix(evs, *p[0], **p[1])]) for p in picks}
    return [e for e in evs if id(e) not in gone]

def group(evs, a, op, att, **kw):
    """The deliver-intent and deliver lines of one catalog call (same seq)."""
    d = evs[ix(evs, a, op, att, **kw)]
    return [e for e in evs if e["seq"] == d["seq"] and e["a"] in ("deliver", "deliver-intent")]

def c_drop_send(evs):
    s = evs[ix(evs, "send", "w2o0", 1)]
    return [e for e in evs if e is not s]

def c_drop_deliver(evs):
    g = group(evs, "deliver", "w1o0", 1)
    return [e for e in evs if e not in g]

def c_drop_upload_finish(evs):
    s = evs[ix(evs, "upload-finish", "w1o0")]
    return [e for e in evs if e is not s]

def swap_seq(a, b):
    a["seq"], b["seq"] = b["seq"], a["seq"]

def c_swap_build_send(evs):
    swap_seq(evs[ix(evs, "build", "w2o0", 2)], evs[ix(evs, "send", "w2o0", 2)])
    return evs

def c_swap_deliveries(evs):
    g1, g2 = group(evs, "deliver", "w1o0", 1), group(evs, "deliver", "w2o0", 1)
    s1, s2 = g1[0]["seq"], g2[0]["seq"]
    for e in g1:
        e["seq"] = s2
    for e in g2:
        e["seq"] = s1
    return evs

def bump(a, op, att, field, delta=1, nth=0, **kw):
    def f(evs):
        evs[ix(evs, a, op, att, nth, **kw)][field] += delta
        return evs
    return f

def setf(a, op, att, field, value, nth=0, **kw):
    def f(evs):
        evs[ix(evs, a, op, att, nth, **kw)][field] = value
        return evs
    return f

def c_relabel(evs):
    evs[ix(evs, "upload-begin", "w3o0")]["op"] = "w2o0"
    return evs

def c_crash_after_done(evs):
    fin = evs[ix(evs, "final")]
    evs.append({"a": "crash", "op": "w1o0", "step": "after-commit", "att": 0, "seq": fin["seq"] - 0.5, "sub": 1})
    return evs

def c_dup_conflict_as_ok(evs):
    e = evs[ix(evs, "deliver", None, 1, dup=2)]
    e.update(res="ok", new=e["exp"] + 1)
    return evs

def c_recover_retry_as_done(evs):
    e = evs[ix(evs, "recover-retry", "w1o0")]
    e["a"] = "recover-done"
    return evs

def c_final_rows(evs):
    f = evs[ix(evs, "final")]
    f["rows"] = f["rows"][1:]
    return evs

def c_recover_done_to_retry(evs):
    e = evs[ix(evs, "recover-done", "w1o0")]
    e["a"] = "recover-retry"
    return evs

def c_kill_in_catalog(evs):
    """A writer SIGKILLed inside the catalog call after the create happened: the intent line remains, nothing else of it."""
    g = [e for e in group(evs, "deliver", "w1o0", 1) if e["a"] == "deliver"]
    gone = {id(g[0])} | {id(e) for e in evs if e.get("op") == "w1o0" and e["a"] in ("receive", "outcome")}
    return [e for e in evs if id(e) not in gone]

def c_torn_line(evs):
    evs.append("TORN")
    return evs

# name: (base trace, expected, description, function)
DERIVED = {
 "drop-send": ("lost_update_correct", "detect", "the send of w2o0 attempt 1 is missing: its delivery has no request", c_drop_send),
 "drop-deliver": ("lost_update_correct", "detect", "the catalog's delivery of w1o0 is missing (intent and outcome): w1o0 reads an answer nobody gave", c_drop_deliver),
 "drop-upload-finish": ("lost_update_correct", "detect", "the file of w1o0 is never durable but the writer goes on", c_drop_upload_finish),
 "swap-build-send": ("lost_update_correct", "detect", "w2o0 sends before it builds its manifest list", c_swap_build_send),
 "swap-deliveries": ("lost_update_correct", "detect", "the catalog handles w2o0's request before w1o0's, the versions in the events unchanged", c_swap_deliveries),
 "wrong-cur": ("lost_update_correct", "detect", "a delivery says the catalog was one version ahead of what it was", bump("deliver", "w2o0", 1, "cur")),
 "wrong-send-exp": ("lost_update_correct", "detect", "a send expects one version more than the writer read", bump("send", "w3o0", 1, "exp")),
 "wrong-new": ("lost_update_correct", "detect", "a commit created version +1 of what the model creates", bump("deliver", "w1o0", 1, "new", res="ok")),
 "wrong-recover-ver": ("lost_update_correct", "detect", "a retry decision says the writer re-read one version too late", bump("recover-retry", "w2o0", 1, "ver")),
 "wrong-answer": ("lost_update_correct", "detect", "a writer reads 'ok' after the catalog said conflict", setf("receive", "w2o0", 1, "res", "ok")),
 "wrong-attempt": ("lost_update_correct", "detect", "a manifest list of attempt 3 is built by a writer on attempt 2", bump("build", "w2o0", 2, "att")),
 "wrong-outcome": ("lost_update_correct", "detect", "a writer reports aborted although the model has it done", setf("outcome", "w1o0", None, "res", "aborted")),
 "wrong-final-rows": ("lost_update_correct", "detect", "the table ends with one row fewer than the model's", c_final_rows),
 "wrong-final-ver": ("lost_update_correct", "detect", "the table ends one version ahead of the model", bump("final", None, None, "ver")),
 "wrong-planned": ("delete_then_compact_correct", "detect", "the writer planned one more delete target than the model computes", bump("planned", "w1o0", None, "targets")),
 "wrong-planned-inputs": ("delete_then_compact_correct", "detect", "the compaction has one more input file than the model's", bump("planned", "w2o0", None, "inputs")),
 "dup-conflict-as-ok": ("duplicate_request_correct", "detect", "the second delivery of a duplicated request says ok", c_dup_conflict_as_ok),
 "recover-done-as-retry": ("unknown_outcome_correct", "detect", "after an unknown outcome the writer retries although its commit is in the table", c_recover_done_to_retry),
 "recover-done-without-commit": ("lost_request_correct", "detect", "after a lost request the writer claims to have found its own commit", c_recover_retry_as_done),
 "relabelled-writer": ("lost_update_correct", "detect", "the upload of w3o0 is attributed to w2o0 (a second upload of w2o0)", c_relabel),
 "crash-after-done": ("lost_update_correct", "detect", "a writer crashes after it finished", c_crash_after_done),
 "killed-in-catalog-call": ("lost_update_correct", "pass", "w1o0 SIGKILLed inside the catalog call (intent without outcome; the create happened)", c_kill_in_catalog),
 "torn-last-line": ("lost_update_correct", "pass", "a torn last line (a writer killed mid-write) is ignored", c_torn_line),
}

def build_corpus(src, dst):
    """Originals (judged by the meta the suite wrote) and the derived traces. Returns {name: expected status}."""
    shutil.rmtree(dst, ignore_errors=True)
    os.makedirs(dst)
    want = {}
    for f in sorted(os.listdir(src)):
        if f.endswith(".jsonl"):
            shutil.copy(os.path.join(src, f), os.path.join(dst, f))
            meta = json.loads(open(os.path.join(src, f)).readline())
            want[f[:-6]] = "VALID" if meta["expect"] == "pass" else "INVALID"
    for name, (base, expect, desc, fn) in DERIVED.items():
        evs = [json.loads(l) for l in open(os.path.join(src, base + ".jsonl")) if l.strip()]
        out = fn(evs)
        for e in out:
            if isinstance(e, dict) and e["a"] == "meta":
                e.update(expect=expect, mode="corrupted" if expect == "detect" else "derived", scenario=name, table=name)
        with open(os.path.join(dst, "x_" + name + ".jsonl"), "w") as fh:
            for e in out:
                fh.write((e if isinstance(e, str) else json.dumps(e, sort_keys=True)) + ("\n" if not isinstance(e, str) else ""))
        want["x_" + name] = "VALID" if expect == "pass" else "INVALID"
    return want

# ---------------------------------------------------------------------------------------------------- checker mutants
# name: (description, file, [(old, new)])
MUTANTS = {
 "tr-drops-upload-await": ("the translator forgets the upload-await events", "trace_to_tla.py",
    [('elif a in ("upload-begin", "upload-finish", "upload-await", "crash"):', 'elif a in ("upload-begin", "upload-finish", "crash"):')]),
 "tr-version-offset": ("every version in the trace is shifted by one", "trace_to_tla.py",
    [('v0 = sd["new"]  ', 'v0 = sd["new"] - 1  ')]),
 "tr-answer-flipped": ("a delivery's answer is turned around (ok <-> conflict)", "trace_to_tla.py",
    [('cur=cur, res=e["res"],', 'cur=cur, res=("conflict" if e["res"] == "ok" else "ok"),')]),
 "tr-ignores-keep": ("a duplicated request is delivered once and removed", "trace_to_tla.py",
    [('keep=bool(e.get("keep"))', 'keep=False')]),
 "tr-no-duplicate-move": ("the second delivery of a duplicate is not moved after the answer is read", "trace_to_tla.py",
    [('if e["a"] == "deliver" and e.get("dup") == 2 and e.get("res") == "conflict":', 'if False:')]),
 "tr-file-order": ("events are ordered by position in the file, not by sequence number", "trace_to_tla.py",
    [('return sorted(evs, key=lambda e: (e["seq"], e.get("sub", 1), e["_n"]))', 'return sorted(evs, key=lambda e: e["_n"])')]),
 "tr-no-crash-budget": ("the model gets no crash budget", "trace_to_tla.py",
    [('crash=sum(1 for t in tr if t["a"] == "crash"),', 'crash=0,')]),
 "tr-no-loss-budget": ("the model gets no lost-request budget", "trace_to_tla.py",
    [('loss=sum(1 for t in tr if t["a"] == "drop"))', 'loss=0)')]),
 "tr-intent-never-applied": ("an intent without an outcome is always taken as 'the create never happened'", "trace_to_tla.py",
    [('elif seen is not None and seen >= e["exp"] + 1:', 'elif False:')]),
 "tr-torn-line-fatal": ("a torn line makes the whole trace unusable", "trace_to_tla.py",
    [('        except ValueError:\n            torn += 1\n            continue', '        except ValueError:\n            raise BadTrace("torn line")')]),
 "tr-trusts-the-model": ("the version a delivery observed is replaced by the model's own", "trace_to_tla.py",
    [('cur = now if e.get("moved") else e["cur"] - v0', 'cur = now')]),
 "tr-drops-outcome": ("the writers' reported outcomes are not compared", "trace_to_tla.py",
    [('        elif a == "outcome":', '        elif a == "outcome-x":')]),
 "tr-drops-planned": ("the planned counts are not compared", "trace_to_tla.py",
    [('        elif a == "planned":', '        elif a == "planned-x":')]),
 "tr-drops-final": ("the final table is not compared", "trace_to_tla.py",
    [('    for f in final:', '    for f in []:')]),
 "spec-no-cur-guard": ("TraceSpec does not check the version a delivery observed", "TraceSpec.tla",
    [('        /\\ CurVer = e.cur\n', '')]),
 "spec-no-recover-guard": ("TraceSpec does not check the version a recovery read", "TraceSpec.tla",
    [('    /\\ CurVer = e.ver\n    /\\ att[e.w] = e.att', '    /\\ att[e.w] = e.att')]),
 "spec-retry-unvalidated": ("TraceSpec's retry is the model's retry without the validation", "TraceSpec.tla",
    [('Recov(e) /\\ RecoverRetry(e.w)',
      'Recov(e) /\\ ~Mine(e.w) /\\ att[e.w] < MaxAttempts /\\ att\' = [att EXCEPT ![e.w] = att[e.w] + 1] '
      '/\\ base\' = [base EXCEPT ![e.w] = NewBaseOnRetry(e.w)] /\\ exp\' = [exp EXCEPT ![e.w] = CurVer] '
      '/\\ pc\' = [pc EXCEPT ![e.w] = "uploaded"] /\\ UNCHANGED <<ledger, filev, netv, plan, snap, pinned, bud>>')]),
 "spec-done-unchecked": ("TraceSpec accepts RecoverDone whether or not the writer\'s snapshot is in the table", "TraceSpec.tla",
    [('Recov(e) /\\ RecoverDone(e.w)',
      'Recov(e) /\\ pc\' = [pc EXCEPT ![e.w] = "done"] /\\ UNCHANGED <<ledger, filev, netv, att, base, exp, plan, snap, pinned, bud>>')]),
 "spec-outcome-unchecked": ("TraceSpec does not compare the writer's reported outcome", "TraceSpec.tla",
    [('pc[e.w] = e.pc /\\ UNCHANGED vars', 'UNCHANGED vars')]),
 "spec-planned-unchecked": ("TraceSpec does not compare the planned delete targets", "TraceSpec.tla",
    [('/\\ Cardinality(plan[e.w].targets) = e.targets', '/\\ TRUE')]),
 "spec-final-rows-unchecked": ("TraceSpec does not compare the final rows", "TraceSpec.tla",
    [('CurVer = e.ver /\\ LiveRows(CurMeta.cur) = e.rows', 'CurVer = e.ver')]),
 "spec-stops-early": ("TraceSpec reports VALID one event before the end", "TraceSpec.tla",
    [('NotDone == i <= Len(Trace)', 'NotDone == i <= Len(Trace) - 1')]),
 "check-deadlock-is-valid": ("a deadlock of TLC is reported as VALID", "trace_check.py",
    [('return "INVALID", "deadlock", last', 'return "VALID", "", last')]),
}

# Model mutants of this script only (the mutants of mutate_model.py are in that file): name -> {file: [(old, new)]}.
# The known-bad no-CAS writer of the code does not ask the catalog to skip its check: it REFRESHES the version it expects
# right before the commit while its manifest list stays built on the stale table. None of the nine model mutants is that.
EXTRA_MODEL_MUTANTS = {
 "send-refreshes-exp": {
    "IcebergCommit.tla": [("[w |-> w, k |-> att[w], exp |-> exp[w], snap |-> snap[w],", "[w |-> w, k |-> att[w], exp |-> CurVer, snap |-> snap[w],"),
                          ("[ver |-> exp[w] + 1, lastSeq |-> s.seq,", "[ver |-> CurVer + 1, lastSeq |-> s.seq,")],
    "TraceSpec.tla": [("att[e.w] = e.att /\\ exp[e.w] = e.exp /\\ Send(e.w)", "att[e.w] = e.att /\\ Send(e.w)")]},
}

def where(detail):
    import re
    m = re.search(r"event (\d+) of", detail or "")
    return m.group(1) if m else "-"

def stage_tools(dst, file=None, edits=None):
    shutil.rmtree(dst, ignore_errors=True)
    os.makedirs(dst)
    for f in TOOLING:
        shutil.copy(os.path.join(HERE, f), os.path.join(dst, f))
    if file:
        err = mutate_model.apply_edits(os.path.join(dst, file), edits)
        if err:
            return err
        if file.endswith(".py"):
            import py_compile
            try:
                py_compile.compile(os.path.join(dst, file), doraise=True, cfile=os.path.join(dst, "x.pyc"))
            except py_compile.PyCompileError as e:
                return f"the mutant does not compile: {e}"
    return None

def run_checker(stage, corpus, out, work, a, only=""):
    """Runs the staged checker; returns {trace: (status, detail)}."""
    cmd = [sys.executable, "-I", os.path.join(stage, "trace_check.py"), "--jar", a.jar, "--java", a.java, "--traces", corpus, "--out", out,
           "--work", work, "--jobs", str(a.jobs), "--model-dir", stage, "--timeout", str(a.timeout)]
    if only:
        cmd += ["--only", only]
    p = subprocess.run(cmd, capture_output=True, text=True)
    path = os.path.join(out, "trace-results.json")
    if not os.path.exists(path):
        return None, p.stdout + p.stderr
    return {r["name"]: (r["status"], r["detail"]) for r in json.load(open(path))}, ""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jar"); ap.add_argument("--java", default="java"); ap.add_argument("--out"); ap.add_argument("--work", default="")
    ap.add_argument("--jobs", type=int, default=6); ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--only", default=""); ap.add_argument("--list", action="store_true")
    ap.add_argument("--no-mutants", action="store_true"); ap.add_argument("--model-mutants", action="store_true")
    ap.add_argument("--corpus", default=CORPUS); ap.add_argument("--dry", action="store_true", help="only check that every edit applies once and compiles")
    a = ap.parse_args()
    if a.dry:
        import tempfile
        tmp = tempfile.mkdtemp(prefix="trmut-dry-")
        try:
            bad = 0
            for name, (desc, file, edits) in MUTANTS.items():
                err = stage_tools(os.path.join(tmp, name), file, edits)
                print(f"{name:28} {'INVALID: ' + err if err else 'ok'}")
                bad += bool(err)
            return 1 if bad else 0
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
    if a.list:
        print("\n".join(list(DERIVED) + list(MUTANTS))); return 0
    if not (a.jar and a.out):
        ap.error("--jar and --out are required")
    out = os.path.abspath(a.out); work = os.path.abspath(a.work or os.path.join(out, "work"))
    os.makedirs(out, exist_ok=True); os.makedirs(work, exist_ok=True)
    corpus = os.path.join(work, "corpus")
    want = build_corpus(a.corpus, corpus)
    bad = False
    # ---- baseline: the unmutated checker on the corpus
    base = os.path.join(work, "stage-baseline")
    stage_tools(base)
    res, err = run_checker(base, corpus, os.path.join(out, "baseline"), os.path.join(work, "wk-baseline"), a)
    if res is None:
        print("BASELINE FAIL: the checker produced no result\n" + err); return 1
    wrong = {n: res.get(n, ("-", ""))[0] for n, w in want.items() if res.get(n, ("-", ""))[0] != w}
    print(f"BASELINE: {len(want)} traces ({sum(1 for n in want if not n.startswith('x_'))} real, {sum(1 for n in want if n.startswith('x_'))} derived); "
          f"unexpected verdicts: {len(wrong)}")
    for n, st in wrong.items():
        print(f"  {n}: got {st}, must be {want[n]}  {res.get(n, ('', ''))[1][:120]}"); bad = True
    print(f"\n{'corrupted / derived trace':26} {'must be':8} {'got':8} what the checker says")
    for name, (b, expect, desc, fn) in DERIVED.items():
        st, detail = res.get("x_" + name, ("-", ""))
        print(f"{name:26} {want['x_' + name]:8} {st:8} {detail[:150]}")
    if bad:
        return 1
    if a.model_mutants:
        allm = dict((m, {"IcebergCommit.tla": v[1]}) for m, v in mutate_model.MUTANTS.items())
        allm.update(EXTRA_MODEL_MUTANTS)
        print(f"\n{'known-bad trace':44} " + " ".join(f"{m[:9]:9}" for m in allm))
        bads = [n for n in want if not n.startswith("x_") and ("_bad_" in n)]
        cross = {}
        for m, files in allm.items():
            st = os.path.join(work, "stage-model-" + m)
            stage_tools(st)
            err = None
            for fl, edits in files.items():
                err = err or mutate_model.apply_edits(os.path.join(st, fl), edits)
            if err:
                print("INVALID model mutant", m, err); bad = True; continue
            only_dir = os.path.join(work, "corpus-bad")
            shutil.rmtree(only_dir, ignore_errors=True); os.makedirs(only_dir)
            for n in bads:
                shutil.copy(os.path.join(corpus, n + ".jsonl"), only_dir)
            r, e = run_checker(st, only_dir, os.path.join(out, "model-" + m), os.path.join(work, "wk-model-" + m), a)
            cross[m] = r or {}
        for n in bads:
            cells = []
            for m in allm:
                s = cross.get(m, {}).get(n, ("-", ""))
                d = s[1]
                cells.append(("VALID" if s[0] == "VALID" else ("inv:" + d.split()[1] if d.startswith("invariant") else "INVALID")) if s[0] in ("VALID", "INVALID") else s[0])
            print(f"{n:44} " + " ".join(f"{c[:9]:9}" for c in cells))
        print("(VALID: the mutated model allows the known-bad trace; INVALID: still refused; inv:Ix: replayed, then invariant Ix violated)")
    if a.no_mutants:
        return 1 if bad else 0
    # ---- mutants of the checker
    names = [n for n in MUTANTS if not a.only or n in a.only.split(",")]
    rows = []
    for name in names:
        desc, file, edits = MUTANTS[name]
        st = os.path.join(work, "stage-" + name)
        err = stage_tools(st, file, edits)
        if err:
            rows.append((name, "INVALID", err)); bad = True; continue
        r, e = run_checker(st, corpus, os.path.join(out, "m-" + name), os.path.join(work, "wk-" + name), a)
        if r is None:
            rows.append((name, "INVALID", "no result: " + e[-200:])); bad = True; continue
        changed = [(n, res[n][0], r.get(n, ("MISSING", ""))[0], r.get(n, ("", ""))[1]) for n in want if r.get(n, ("MISSING", ""))[0] != want[n]]
        # A known-bad or corrupted trace that is still refused, but at a different event, was refused for another reason: the
        # mutant has taken away the check that refused it first (defence in depth must not hide a weakened check).
        moved = [(n, "INVALID", "INVALID", f"refused at event {where(r[n][1])} instead of {where(res[n][1])}") for n in want
                 if want[n] == "INVALID" and r.get(n, ("", ""))[0] == "INVALID" and where(r[n][1]) != where(res[n][1])]
        if not changed and moved:
            n0, _, _, why = moved[0]
            rows.append((name, "CAUGHT", f"{len(moved)} refused trace(s) are refused at another event, e.g. {n0}: {why}"))
            continue
        if len(changed) == len(want):
            rows.append((name, "INVALID", "every trace fails: the mutant is broken, not detected")); bad = True
        elif changed:
            n0, was, now, why = changed[0]
            rows.append((name, "CAUGHT", f"{len(changed)} trace(s) change, e.g. {n0}: must be {want[n0]}, now {now} {why[:60]}"))
        else:
            rows.append((name, "SURVIVED", "every verdict is unchanged")); bad = True
    print(f"\n{'checker mutant':26} {'result':9} what changed")
    for name, res_, what in rows:
        print(f"{name:26} {res_:9} {what[:200]}")
    c = sum(1 for r in rows if r[1] == "CAUGHT")
    print(f"\nSUMMARY CAUGHT={c} SURVIVED={sum(1 for r in rows if r[1] == 'SURVIVED')} INVALID={sum(1 for r in rows if r[1] == 'INVALID')} of {len(rows)}; "
          f"corrupted traces rejected by the unmutated checker: {sum(1 for n, v in DERIVED.items() if v[1] == 'detect' and res['x_' + n][0] == 'INVALID')} of {sum(1 for n, v in DERIVED.items() if v[1] == 'detect')}")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
