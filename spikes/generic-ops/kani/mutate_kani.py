#!/usr/bin/env python3
"""Mutation-check the Kani proofs. Reads the 15 mutations from ../mutate.py (no copy, no drift), applies each to a
TEMP copy of kernels.rs (the real file is never written), and runs the harnesses on the temp copy, relevant ones
first, stopping at the first that fails verification. Usage: mutate_kani.py [name-substring ...]"""
import subprocess, shutil, sys, os, re, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
GEN = os.path.dirname(HERE)
src = open(os.path.join(GEN, "mutate.py")).read()
M = eval(src[src.index("M=["):].split("\n]\n")[0][2:] + "\n]")
orig = open(os.path.join(GEN, "src/kernels.rs")).read()
ONLY = sys.argv[1:]
# cheapest-first harness order; the last row count each (3 for grouped sums, 4 otherwise)
FILTERS = [f"filter_range_{t}_rows4_{sh}" for t in ("i64", "i32") for sh in ("none", "s0", "s1", "s2", "s3", "s4")]
ALL = ["each_visits_the_selection_in_order_rows4"] + FILTERS + ["mul_logic_matches_the_spec_rows4",
       "sub_const_left_logic_matches_the_spec_rows4", "sub_const_left_exact_matches_the_spec_rows4",
       "group_sums_dense_matches_the_spec_rows3", "group_sums_masked_matches_the_spec_rows3",
       "sum_matches_the_spec_rows4"]
# Every harness named here must exist: a missing name makes Kani return no result, which must never look like a catch
# (the first GCP run counted it as 'CAUGHT by COMPILE ERROR' for two mutants).
_lib = open(os.path.join(HERE, "src/lib.rs")).read()
_missing = [h for h in ALL if f"fn {h}(" not in _lib]
if _missing:
    sys.exit(f"harness names not found in src/lib.rs: {_missing}")
def relevant(name):
    n = name.lower()
    if "masked group" in n: return [h for h in ALL if "masked" in h]
    if "dense group" in n: return [h for h in ALL if "dense" in h]
    if "filter" in n: return [h for h in ALL if "filter" in h]
    if n.startswith("sum"): return [h for h in ALL if h.startswith("sum")]
    if n.startswith("mul"): return [h for h in ALL if h.startswith("mul")]
    if n.startswith("sub"): return [h for h in ALL if h.startswith("sub")]
    return ALL
tmp = tempfile.mkdtemp(prefix="kmut-")
os.makedirs(tmp + "/generic-ops/src")
shutil.copytree(HERE, tmp + "/generic-ops/kani", ignore=shutil.ignore_patterns("target", "Cargo.lock", "__pycache__"))
K = tmp + "/generic-ops/src/kernels.rs"
def run(h, cap=400):
    r = subprocess.run([tmp + "/generic-ops/kani/run1.sh", h, str(cap)], capture_output=True, text=True).stdout.strip()
    return r
def verdict(line):
    return "SUCCESSFUL" if ": SUCCESSFUL" in line else ("TIMEOUT" if "TIMEOUT" in line else ("COMPILE?" if "NO RESULT" in line else "FAILED"))
results = []
open(K, "w").write(orig)
print("baseline (unmutated copy):", verdict(run(ALL[0])), flush=True)
for name, old, new, idx in M:
    if ONLY and not any(o in name for o in ONLY): continue
    if orig.count(old) <= idx:
        print(f"SKIP (pattern not found): {name}"); continue
    parts = orig.split(old)
    open(K, "w").write(old.join(parts[:idx+1]) + new + old.join(parts[idx+1:]))
    rel = relevant(name); order = rel + [h for h in ALL if h not in rel]
    caught = None; invalid = None; ran = []
    for h in order:
        line = run(h); v = verdict(line); ran.append((h, v))
        if v == "FAILED": caught = h; break
        if v == "COMPILE?": invalid = h; break
    tail = "; ".join(f"{h.rsplit('_matches',1)[0]}={v}" for h, v in ran)
    status = ("CAUGHT by " + caught) if caught else (f"INVALID (no result for {invalid}: compile error or unknown harness; NOT a catch)" if invalid else "SURVIVED")
    results.append(status.split()[0])
    print(f"{status} | {name} | {tail}", flush=True)
open(K, "w").write(orig)
shutil.rmtree(tmp, ignore_errors=True)
print(f"SUMMARY: caught {results.count('CAUGHT')} of {len(results)}; survived {results.count('SURVIVED')}; invalid {results.count('INVALID')}", flush=True)
sys.exit(0 if results.count('CAUGHT') == len(results) else 1)
