#!/usr/bin/env python3
"""Mutation-check the Kani proofs of crates/nata-kernels.

Reads the 22 mutations from crates/nata-kernels/scripts/mutate_guards.py (parsed, no copy, no drift), applies each
to a TEMP copy of crates/nata-kernels/src/kernels.rs (the real file is never written), and runs the relevant
harnesses of this crate on the temp copy, cheapest first, until one FAILS verification.

Verdicts per mutant:
  CAUGHT    a relevant harness returned VERIFICATION FAILED on the mutated code (and the unmutated code verifies);
  SURVIVED  every relevant harness returned SUCCESSFUL (the proofs cannot see this breakage);
  INVALID   no result: Kani returned nothing (compile error, unknown harness name), a harness timed out with no
            failure, the pattern was not found, or the output was not understood. NEVER counted as a catch;
  SKIPPED   the mutation targets src/arrow_path.rs, which these proofs do not include (not a catch, not a survivor).

Usage: mutate_kani.py [--rows R] [--cap SECONDS] [--heavy] [--inject-bad-harness NAME] [name-substring ...]
  --rows R      row bound of the harnesses used (default 4; harness names are <family>_rows<R>)
  --cap S       per-harness time cap in seconds (default 400)
  --heavy       also run the division harnesses that use the REAL 64-bit checked_div (rows <= 2; GCP-sized)
  --inject-bad-harness NAME   self-test: put a harness NAME in front of the first selected mutant's list WITHOUT the
                              up-front existence check; the run must report that mutant INVALID, not CAUGHT.
"""
import ast, os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
CRATE = os.path.join(ROOT, "crates", "nata-kernels")
GUARDS = os.path.join(CRATE, "scripts", "mutate_guards.py")

# ---- arguments ----
args = sys.argv[1:]
def opt(name, default=None, flag=False):
    if name in args:
        i = args.index(name)
        if flag:
            args.pop(i); return True
        v = args[i + 1]; del args[i:i + 2]; return v
    return default
ROWS = int(opt("--rows", 4)); CAP = int(opt("--cap", 400)); HEAVY = opt("--heavy", flag=True)
BAD = opt("--inject-bad-harness")
ONLY = args

# ---- the mutations: parsed from mutate_guards.py ----
tree = ast.parse(open(GUARDS).read())
consts, M = {}, None
for node in tree.body:
    if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name):
        n = node.targets[0].id
        if n == "M":
            M = eval(compile(ast.Expression(node.value), GUARDS, "eval"), {}, dict(consts))
        elif isinstance(node.value, ast.Constant):
            consts[n] = node.value.value
if not M:
    sys.exit("could not read the mutation list M from " + GUARDS)
K = consts["K"]

# ---- the harnesses that exist (a fn following #[kani::proof]) ----
lib = open(os.path.join(HERE, "src/lib.rs")).read().splitlines()
EXISTING = set()
for i, line in enumerate(lib):
    if "#[kani::proof]" in line:
        for nxt in lib[i + 1:i + 8]:
            m = re.match(r"\s*fn (\w+)\(", nxt)
            if m:
                EXISTING.add(m.group(1)); break

def h(family, rows=ROWS):
    return f"{family}_rows{rows}"
def relevant(name):
    """Harnesses that can see this mutation, cheapest first."""
    n = name
    exact = min(ROWS, 2)
    if n.startswith("div_const_left"):
        r = [h("div_const_left_logic_matches_the_spec")]
        if HEAVY: r.append(h("div_const_left_exact_matches_the_spec", exact))
        return r
    if n.startswith("div"):
        r = [h("div_logic_matches_the_spec"), h("case_guarded_division_matches_the_spec")]
        if HEAVY:
            # a mutant that swaps the primitive itself is caught by the logic harness only because the stand-in no
            # longer matches (weak); with --heavy try the harness that uses the REAL checked_div first
            if "rounds toward" in n: r.insert(0, h("div_exact_matches_the_spec", exact))
            else: r.append(h("div_exact_matches_the_spec", exact))
        return r
    if n.startswith("guard split_ne"):
        return [h("filter_ne_matches_the_spec"), h("split_ne_partitions_the_selection"), h("case_guarded_division_matches_the_spec")]
    if n.startswith("merge_branches"):
        return [h("merge_branches_matches_the_spec"), h("case_guarded_division_matches_the_spec")]
    return []

# up-front check: every harness name this run can use exists in src/lib.rs
used = set()
for name, f, *_ in M:
    if f == K and (not ONLY or any(o in name for o in ONLY)):
        used.update(relevant(name))
missing = sorted(x for x in used if x not in EXISTING)
if missing:
    sys.exit(f"harness names not found in src/lib.rs (wrong --rows?): {missing}")

# ---- temp copy: tmp/spikes/kani-nata-kernels + tmp/crates/nata-kernels/src/kernels.rs ----
tmp = tempfile.mkdtemp(prefix="knk-mut-")
shutil.copytree(HERE, tmp + "/spikes/kani-nata-kernels", ignore=shutil.ignore_patterns("target", "__pycache__"))
os.makedirs(tmp + "/crates/nata-kernels/src")
KT = tmp + "/crates/nata-kernels/" + K
orig = open(os.path.join(CRATE, K)).read()
open(KT, "w").write(orig)
RUN1 = tmp + "/spikes/kani-nata-kernels/run1.sh"

def run(hname):
    return subprocess.run([RUN1, hname, str(CAP)], capture_output=True, text=True).stdout.strip()
def verdict(line):
    if ": SUCCESSFUL |" in line: return "SUCCESSFUL"
    if ": FAILED" in line: return "FAILED"
    if "TIMEOUT" in line: return "TIMEOUT"
    return "NONE"   # NO RESULT or anything not understood

results = []
try:
    # the unmutated copy must verify on the cheapest relevant harness, else nothing below means anything
    base_h = h("merge_branches_matches_the_spec", min(ROWS, 1))
    bv = verdict(run(base_h))
    print(f"baseline (unmutated copy, {base_h}): {bv}", flush=True)
    if bv != "SUCCESSFUL":
        sys.exit("baseline does not verify; mutation results would be meaningless")
    injected = False
    for name, f, old, new, idx in M:
        if ONLY and not any(o in name for o in ONLY): continue
        if f != K:
            results.append("SKIPPED"); print(f"SKIPPED  | {name} | mutates {f}, not included in these proofs", flush=True); continue
        parts = orig.split(old)
        if len(parts) - 1 <= idx:
            results.append("INVALID"); print(f"INVALID  | {name} | pattern not found in kernels.rs", flush=True); continue
        mutated = old.join(parts[:idx + 1]) + new + old.join(parts[idx + 1:])
        if mutated == orig:
            results.append("INVALID"); print(f"INVALID  | {name} | mutation changes nothing", flush=True); continue
        open(KT, "w").write(mutated)
        order = relevant(name)
        if BAD and not injected:
            order = [BAD] + order; injected = True
        caught = invalid = None; ran = []
        for hn in order:
            v = verdict(run(hn)); ran.append((hn, v))
            if v == "FAILED": caught = hn; break
            if v in ("NONE", "TIMEOUT"): invalid = (hn, v); break
        tail = "; ".join(f"{a}={b}" for a, b in ran)
        if caught:
            status = "CAUGHT"; why = "by " + caught
            if "rounds toward" in name and "_exact_" not in caught:
                why += " (via the stand-in for checked_div: weaker than an exact-primitive harness, see results)"
        elif invalid:
            status = "INVALID"; why = f"no usable result for {invalid[0]} ({invalid[1]}: compile error, unknown harness or timeout); NOT a catch"
        else:
            status = "SURVIVED"; why = "every relevant harness verified"
        results.append(status)
        print(f"{status:8} | {name} | {why} | {tail}", flush=True)
        open(KT, "w").write(orig)
finally:
    shutil.rmtree(tmp, ignore_errors=True)
c = results.count
print(f"SUMMARY: caught {c('CAUGHT')} of {len(results) - c('SKIPPED')} in scope; survived {c('SURVIVED')}; invalid {c('INVALID')}; skipped (out of scope) {c('SKIPPED')}", flush=True)
sys.exit(0 if c("SURVIVED") == 0 and c("INVALID") == 0 else 1)
