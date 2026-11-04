#!/usr/bin/env python3
"""Mutation check of the simulation harness.

Every mutant is a deliberate bug in the toy protocol (or a deliberate break of the determinism rules). Each is
applied to a COPY of the sources (the real tree is never written; its hash is compared before and after), the
copy is built, and the seed sweep (or the determinism check) must catch it:

  * sweep mutants: `simctl sweep 0 N` must report failing seeds. The first failing seed is then REPLAYED and must
    give the identical failure line.
  * determinism mutants: `simctl hashes` run in 3 separate processes must disagree (or disagree with itself
    inside one process).
  * the unmutated control must pass every seed.

A mutant whose build fails, whose binary is identical to the control's or to another mutant's (cargo reused a stale
binary), or whose sweep crashes is INVALID and never counts as caught. Files are copied with shutil.copy (fresh
mtimes), never copy2: preserved mtimes once made cargo reuse the previous mutant's binary.

usage: run_mutants.py [--seeds N] [--jobs J] [--work DIR] [--only a,b] [--list] [--check-anchors]
                      [--baseline-only] [--selftest-stale]
exit: 0 every mutant caught, replayed identically, control clean; 1 otherwise (survivor, invalid, replay mismatch,
      dirty control, real sources changed).
"""
import hashlib
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SIM_ROOT = os.path.dirname(HERE)  # spikes/sim-harness
SPIKES = os.path.dirname(SIM_ROOT)  # spikes
REPO = os.path.dirname(SPIKES)

# Files the build needs, relative to SPIKES (the real tree). The workspace manifest of the copy is written here.
COPY_DIRS = [("sim-harness/sim", ["Cargo.toml", "src", "tests"])]
COPY_FILES = [
    "operator-trait/src/api.rs",
    "operator-trait/src/batch.rs",
    "operator-trait/src/ops.rs",
    "operator-trait/src/sched.rs",
    "operator-trait/src/sync.rs",
    "exchange-loom/src/exchange.rs",
]
WORKSPACE = '[workspace]\nresolver = "3"\nmembers = ["sim"]\n'

NO_RECHECK = "MUTATION"

# name: (kind, file relative to sim/, anchor, replacement, what the bug is)
M = {
    "retry_counts_output_twice": (
        "sweep", "src/protocol.rs",
        "                t.rows.clear();\n",
        "                // MUTATION: the rows of the abandoned attempt are kept, so the retry adds them a second time\n",
        "a retried task keeps the rows of its abandoned attempt",
    ),
    "cancel_skips_restarted_worker": (
        "sweep", "src/protocol.rs",
        "w.cancel_acked = false; // the new incarnation must be cancelled too",
        "w.cancel_acked = true; // MUTATION: a restarted worker is taken for already cancelled",
        "the coordinator never cancels a worker that crashed and restarted during the cancellation",
    ),
    "window_off_by_one": (
        "sweep", "src/protocol.rs",
        "while t.next_new < n && t.next_new.saturating_sub(t.base) < window {",
        "while t.next_new < n && t.next_new.saturating_sub(t.base) <= window { // MUTATION: one frame too many in flight",
        "backpressure window off by one",
    ),
    "lost_wake_when_window_full": (
        "sweep", "src/protocol.rs",
        "if now.saturating_sub(sent_at) > RTO {",
        "if now.saturating_sub(sent_at) > RTO && t.next_new.saturating_sub(t.base) < window { // MUTATION: no retransmit timer while the window is full",
        "a dropped message while the window is full is never retransmitted (lost wake-up)",
    ),
    "no_retry_when_last_frame_seen": (
        "sweep", "src/protocol.rs",
        "if t.state == TState::Running && t.worker == w {",
        "if t.state == TState::Running && t.worker == w && t.last_seq.is_none() { // MUTATION: a task whose last frame arrived is never retried",
        "the coordinator forgets to retry a task whose worker crashed during the final batch",
    ),
    "leak_at_shutdown": (
        "sweep", "src/protocol.rs",
        "        self.tasks.clear();\n",
        "        std::mem::forget(std::mem::take(&mut self.tasks)); // MUTATION: buffers are leaked when the worker drops its tasks\n",
        "a worker leaks its task buffers when it cancels, shuts down or loses its lease",
    ),
    "stale_attempt_accepted": (
        "sweep", "src/protocol.rs",
        "let stale = !running || t.state != TState::Running || t.attempt != attempt;",
        "let stale = !running || t.state != TState::Running; // MUTATION: the attempt number is not checked",
        "output of a superseded attempt is accepted into the current one",
    ),
    "duplicate_frame_applied": (
        "sweep", "src/protocol.rs",
        "if seq == t.next_seq {",
        "if seq <= t.next_seq { // MUTATION: a duplicate of an already applied frame is applied again",
        "a duplicated frame is applied twice",
    ),
    "store_error_means_empty_input": (
        "sweep", "src/protocol.rs",
        '            Err(_) => {\n                tries += 1;\n                env.event("store_retry"',
        '            Err(_) => break Vec::new(), // MUTATION: a failed read is taken for an empty input\n'
        '            #[allow(unreachable_patterns)]\n'
        '            Err(_) if false => {\n                tries += 1;\n                env.event("store_retry"',
        "a transient object store error is treated as an empty file (rows silently lost)",
    ),
    "cancel_done_without_acks": (
        "sweep", "src/protocol.rs",
        "        if waiting == 0 {",
        "        if true { // MUTATION: the cancellation is declared complete before any worker has acknowledged",
        "the coordinator reports the cancellation complete without waiting for the workers",
    ),
    "det_hashmap_iteration_order": (
        "determinism", "src/protocol.rs",
        "type WMap<V> = BTreeMap<NodeId, V>;",
        "type WMap<V> = std::collections::HashMap<NodeId, V>; // MUTATION: iteration order of a HashMap",
        "the coordinator iterates its workers in HashMap order (random per process)",
    ),
    "det_wall_clock_in_trace": (
        "determinism", "src/sim.rs",
        "        let now = self.exec.now();\n        self.trace.borrow_mut().ev(now, node, kind, a, b);",
        "        let now = self.exec.now() ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0); // MUTATION: wall clock\n"
        "        self.trace.borrow_mut().ev(now, node, kind, a, b);",
        "the wall clock leaks into the trace",
    ),
    "det_global_counter": (
        "determinism", "src/protocol.rs",
        "    Coord { env, cfg, tasks, workers, phase: Phase::Running, cancel_started: false }.run().await;",
        "    {\n"
        "        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // MUTATION: global state\n"
        "        env.event(\"run_counter\", RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0);\n"
        "    }\n"
        "    Coord { env, cfg, tasks, workers, phase: Phase::Running, cancel_started: false }.run().await;",
        "a process-global counter leaks into the trace (same in separate processes, different in the second run of one process)",
    ),
}


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        h.update(f.read())
    return h.hexdigest()


def real_files():
    out = []
    for d, items in COPY_DIRS:
        for it in items:
            p = os.path.join(SPIKES, d, it)
            if os.path.isdir(p):
                for root, _, files in os.walk(p):
                    for fn in files:
                        out.append(os.path.join(root, fn))
            else:
                out.append(p)
    out += [os.path.join(SPIKES, f) for f in COPY_FILES]
    return sorted(out)


def tree_hash():
    h = hashlib.sha256()
    for p in real_files():
        h.update(os.path.relpath(p, SPIKES).encode() + b"\0" + sha256_file(p).encode() + b"\0")
    return h.hexdigest()


def fresh_copy(dest):
    """Copy the build inputs of the real tree into `dest` with FRESH mtimes (shutil.copy, never copy2)."""
    if os.path.isdir(dest):
        shutil.rmtree(dest)
    for p in real_files():
        rel = os.path.relpath(p, SPIKES)
        q = os.path.join(dest, "spikes", rel)
        os.makedirs(os.path.dirname(q), exist_ok=True)
        shutil.copy(p, q)  # NOT copy2: preserved mtimes make cargo reuse the previous build
    with open(os.path.join(dest, "spikes", "sim-harness", "Cargo.toml"), "w") as f:
        f.write(WORKSPACE)


def apply(name, dest):
    kind, rel, anchor, repl, _ = M[name]
    path = os.path.join(dest, "spikes", "sim-harness", "sim", rel)
    with open(path) as f:
        text = f.read()
    n = text.count(anchor)
    if n != 1:
        raise RuntimeError(f"mutation {name}: anchor found {n} times in {rel} (must be exactly once)")
    with open(path, "w") as f:  # a fresh write: fresh mtime
        f.write(text.replace(anchor, repl))
    return kind


def env_for(work):
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(work, "target")
    env.setdefault("CARGO_BUILD_JOBS", str(os.cpu_count() or 2))
    if "RUSTUP_TOOLCHAIN" not in env:
        try:
            with open(os.path.join(REPO, "rust-toolchain.toml")) as f:
                m = re.search(r'channel\s*=\s*"([^"]+)"', f.read())
            if m:
                env["RUSTUP_TOOLCHAIN"] = m.group(1)
        except OSError:
            pass
    return env


def build(dest, work, log):
    cwd = os.path.join(dest, "spikes", "sim-harness")
    r = subprocess.run(
        ["cargo", "build", "--release", "-p", "nata-sim", "--bin", "simctl"],
        cwd=cwd, env=env_for(work), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    with open(log, "w") as f:
        f.write(r.stdout)
    if r.returncode != 0:
        return None
    return os.path.join(work, "target", "release", "simctl")


def run(cmd, timeout=3600):
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=timeout)
    return r.returncode, r.stdout


def parse_line(out, prefix):
    for ln in out.splitlines():
        if ln.startswith(prefix + " "):
            return ln[len(prefix) + 1:]
    return None


def sweep(binary, seeds, jobs, extra=()):
    return run([binary, "sweep", "0", str(seeds), "--jobs", str(jobs), *extra])


def baseline(work, tag, seeds, jobs):
    """Fresh copy of the unmutated tree, built and swept: must pass every seed. Returns (ok, binary hash, text)."""
    dest = os.path.join(work, "copy")
    fresh_copy(dest)
    b = build(dest, work, os.path.join(work, f"build-{tag}.log"))
    if b is None:
        return False, None, f"control {tag}: DOES NOT COMPILE (see build-{tag}.log)"
    h = sha256_file(b)
    rc, out = sweep(b, seeds, jobs, ["--require-coverage"])
    with open(os.path.join(work, f"sweep-{tag}.txt"), "w") as f:
        f.write(out)
    summary = parse_line(out, "SWEEP") or "(no summary)"
    ok = rc == 0
    return ok, h, f"control {tag}: {'PASS' if ok else 'FAIL rc=%d' % rc} binary={h[:12]} {summary}\n" + (
        "" if ok else "".join(ln + "\n" for ln in out.splitlines() if ln.startswith(("FIRST_FAIL", "FAILKIND", "COVERAGE_REQUIRED"))))


def determinism_check(binary, first=0, count=64):
    """3 separate processes must print identical hashes, and each must agree with itself in-process."""
    outs, rcs = [], []
    for _ in range(3):
        rc, out = run([binary, "hashes", str(first), str(count)])
        rcs.append(rc)
        outs.append([ln for ln in out.splitlines() if ln.startswith(("HASH", "INPROCESS_MISMATCH", "COLLISION"))])
    inproc = any(rc == 4 for rc in rcs) or any(ln.startswith("INPROCESS_MISMATCH") for o in outs for ln in o)
    cross = not (outs[0] == outs[1] == outs[2])
    return inproc, cross, rcs


def stale_selftest(work):
    """Reproduce the defect that once made cargo reuse the previous mutant's binary (reverting a file with
    shutil.copy2, which keeps the old mtime), and show that the fix (shutil.copy) and the guard (binary hashes
    must differ) work. Exit 1 only if the FIX does not work."""
    name = "window_off_by_one"
    dest = os.path.join(work, "copy")
    fresh_copy(dest)
    apply(name, dest)
    b = build(dest, work, os.path.join(work, "build-stale-1.log"))
    if b is None:
        print("STALE-SELFTEST: mutant does not compile"); return 1
    h_mut = sha256_file(b)
    rel = os.path.join("src", "protocol.rs")
    real = os.path.join(SPIKES, "sim-harness", "sim", rel)
    mutated = os.path.join(dest, "spikes", "sim-harness", "sim", rel)
    shutil.copy2(real, mutated)  # the DEFECTIVE revert: an old mtime, content back to the original
    b = build(dest, work, os.path.join(work, "build-stale-2.log"))
    h_bad = sha256_file(b)
    reproduced = h_bad == h_mut
    print(f"STALE-SELFTEST: revert with copy2 -> binary {'is STILL the mutant (cargo reused the stale build: defect reproduced)' if reproduced else 'was rebuilt (defect not reproduced on this machine)'}")
    shutil.copy(real, mutated)  # the FIX: a fresh mtime
    b = build(dest, work, os.path.join(work, "build-stale-3.log"))
    h_fix = sha256_file(b)
    fixed = h_fix != h_mut
    print(f"STALE-SELFTEST: revert with copy -> binary {'differs from the mutant (rebuilt): fix works' if fixed else 'is the mutant: FIX DOES NOT WORK'}")
    print(f"STALE-SELFTEST: the guard (distinct binary hashes) {'would have flagged' if reproduced else 'was not needed for'} the defective revert")
    return 0 if fixed else 1


def main(argv):
    seeds, jobs, work = 20000, os.cpu_count() or 2, os.path.join("/tmp", f"sim-mutants-{os.getpid()}")
    only, list_only, base_only, selftest, check_anchors = None, False, False, False, False
    i = 1
    while i < len(argv):
        a = argv[i]
        if a == "--seeds":
            seeds = int(argv[i + 1]); i += 1
        elif a == "--jobs":
            jobs = int(argv[i + 1]); i += 1
        elif a == "--work":
            work = argv[i + 1]; i += 1
        elif a == "--only":
            only = argv[i + 1].split(","); i += 1
        elif a == "--list":
            list_only = True
        elif a == "--baseline-only":
            base_only = True
        elif a == "--selftest-stale":
            selftest = True
        elif a == "--check-anchors":
            check_anchors = True
        else:
            print(__doc__); return 2
        i += 1
    if list_only:
        for n, (kind, _, _, _, what) in M.items():
            print(f"{n:34s} {kind:12s} {what}")
        return 0
    if check_anchors:  # no build: every anchor must be found exactly once in the real sources
        work = work or "/tmp"
        d = os.path.join(work, "anchor-check")
        bad = 0
        for n in M:
            fresh_copy(d)
            try:
                apply(n, d)
                print(f"anchor ok   {n}")
            except RuntimeError as e:
                print(f"anchor BAD  {e}"); bad += 1
        shutil.rmtree(d, ignore_errors=True)
        return 1 if bad else 0
    names = only or list(M)
    for n in names:
        if n not in M:
            print(f"unknown mutant {n}"); return 2
    os.makedirs(work, exist_ok=True)
    if selftest:
        return stale_selftest(work)
    before = tree_hash()
    bad = 0
    print(f"MUTANTS work={work} seeds={seeds} jobs={jobs} real-tree-hash={before[:16]}")

    ok, base_hash, text = baseline(work, "1", seeds, jobs)
    print(text, end="")
    if not ok:
        print("MUTANTS ABORT: the unmutated control does not pass"); return 1
    if base_only:
        ok2, h2, text2 = baseline(work, "2", seeds, jobs)
        print(text2, end="")
        same = h2 == base_hash
        print(f"MUTANTS two baselines in a row: both {'PASS' if ok and ok2 else 'NOT PASS'}, binaries {'identical' if same else 'DIFFER (rustc not bit reproducible here; the pass/fail is what counts)'}")
        after = tree_hash()
        print(f"MUTANTS real sources {'UNCHANGED' if after == before else 'CHANGED'}")
        return 0 if ok and ok2 and after == before else 1

    seen_hashes = {base_hash: "control"}
    results = []
    dest = os.path.join(work, "copy")
    for n in names:
        kind, rel, _, _, what = M[n]
        fresh_copy(dest)
        try:
            apply(n, dest)
        except RuntimeError as e:
            print(f"MUTANT {n}: INVALID {e}"); bad += 1; results.append((n, "INVALID")); continue
        b = build(dest, work, os.path.join(work, f"build-{n}.log"))
        if b is None:
            print(f"MUTANT {n}: INVALID (does not compile, see build-{n}.log)"); bad += 1; results.append((n, "INVALID")); continue
        h = sha256_file(b)
        if h in seen_hashes:
            print(f"MUTANT {n}: INVALID (binary identical to {seen_hashes[h]}: cargo reused a stale build)"); bad += 1; results.append((n, "INVALID")); continue
        seen_hashes[h] = n
        if kind == "sweep":
            rc, out = sweep(b, seeds, jobs)
            with open(os.path.join(work, f"sweep-{n}.txt"), "w") as f:
                f.write(out)
            summary = parse_line(out, "SWEEP") or "(no summary)"
            if rc == 0:
                print(f"MUTANT {n}: SURVIVED all {seeds} seeds  [{what}]"); bad += 1; results.append((n, "SURVIVED")); continue
            if rc != 1:
                print(f"MUTANT {n}: INVALID (sweep exited {rc}, not a clean failure report)"); bad += 1; results.append((n, "INVALID")); continue
            first = parse_line(out, "FIRST_FAIL")
            m = re.search(r"seed=(\d+)", first or "")
            if not m:
                print(f"MUTANT {n}: INVALID (no FIRST_FAIL line)"); bad += 1; results.append((n, "INVALID")); continue
            seed = m.group(1)
            rc2, out2 = run([b, "replay", seed])
            rep = parse_line(out2, "REPLAY")
            same = rep == first
            kinds = ",".join(ln.split(None, 1)[1] for ln in out.splitlines() if ln.startswith("FAILKIND"))
            print(f"MUTANT {n}: CAUGHT  first failing seed {seed}  [{summary}]  kinds: {kinds}")
            print(f"    first failure : {first}")
            print(f"    replay        : {'IDENTICAL' if same else 'MISMATCH -> ' + str(rep)}")
            if not same:
                bad += 1
                results.append((n, "REPLAY-MISMATCH"))
            else:
                results.append((n, "CAUGHT"))
        else:
            inproc, cross, rcs = determinism_check(b)
            if inproc or cross:
                print(f"MUTANT {n}: CAUGHT by the determinism check (same seed, different trace: "
                      f"{'inside one process' if inproc else 'between processes'})  [{what}]")
                results.append((n, "CAUGHT"))
            else:
                print(f"MUTANT {n}: SURVIVED the determinism check  [{what}]"); bad += 1; results.append((n, "SURVIVED"))

    # The control again, in the same copy directory the mutants used: a stale binary would make it fail.
    ok_b, h_b, text_b = baseline(work, "after", seeds, jobs)
    print(text_b, end="")
    if not ok_b:
        bad += 1
    if h_b in seen_hashes and seen_hashes[h_b] != "control":
        print(f"MUTANTS the control after the mutants has the binary of mutant {seen_hashes[h_b]}: stale build reuse"); bad += 1
    after = tree_hash()
    print(f"MUTANTS real sources {'UNCHANGED' if after == before else 'CHANGED (the real tree was modified!)'}")
    if after != before:
        bad += 1
    caught = sum(1 for _, s in results if s == "CAUGHT")
    print(f"MUTANTS SUMMARY caught={caught} of {len(names)}  not-caught-or-invalid={bad}")
    return 0 if bad == 0 and caught == len(names) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
