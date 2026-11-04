#!/usr/bin/env python3
"""Verification of docs/decimal-rules.md against an independent reference and DuckDB 2.0.0.

    python3 -I spikes/decimal-verify/verify.py [--full] [--cache-dir DIR] [--mutants all|a,b,c] [--skip-mutations] [--json OUT]

Default = quick profile: 29 decimal types (841 type pairs), small value sets, 4 representative mutants; it checks that
the harness works and takes seconds (DuckDB queries included). It is NOT the verification.
--full = the verification: all 779 types (606,841 type pairs per operator), boundary-biased values for 24 types,
exhaustive TINYINT x TINYINT, every mutant (--list-mutants). Run it on a quiet machine: tens of minutes, mostly DuckDB.

DuckDB answers are cached in <cache-dir>/<profile>/ ; use a NEW --cache-dir for a run you want to rely on, delete it to
re-query. The mutation stage re-runs the same checks against deliberately broken copies of the reference using the
cached DuckDB answers, so it needs no extra DuckDB time. Exit code 1 if the unmutated reference has any failure or (with
mutation checking on) a mutant survives.
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def parse_args():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cache-dir", default="/tmp/nata-dv-cache",
                    help="where DuckDB answers are cached (a subdirectory per profile is used: quick/ or full/). "
                         "Use a NEW directory for a run you want to rely on.")
    ap.add_argument("--full", action="store_true",
                    help="the full verification: all 779 decimal types (606,841 type pairs), the full value sets, "
                         "exhaustive TINYINT x TINYINT, every mutant. Takes tens of minutes of DuckDB time on a quiet "
                         "machine. Without it: the quick profile (29 types, small value sets, 4 mutants), seconds.")
    ap.add_argument("--skip-mutations", action="store_true")
    ap.add_argument("--mutants", default=None,
                    help="comma-separated mutant names, or 'all' (default: all with --full, 4 representative ones otherwise)")
    ap.add_argument("--list-mutants", action="store_true")
    ap.add_argument("--json", default=None, help="write the counts, failures and mutation table here")
    return ap.parse_args()


ARGS = parse_args()
os.environ["NATA_DV_PROFILE"] = "full" if ARGS.full else "quick"  # must be set before the modules below are imported

import extras
import intarith
import typecheck as tc
import values as vl
from rules_ref import MUTATIONS, NT, PROFILE, TYPES, Ref

QUICK_MUTANTS = ["add_cap_exact_sum", "round_neg_prec_old_rule", "mod_prec_old_conservative", "int_mul_widen_bigint"]

T0 = time.time()


def log(msg):
    print(f"[{time.time() - T0:7.1f}s] {msg}", flush=True)


class Suite:
    """Runs every check for one reference (possibly mutated). `fails` maps check name -> number of failures."""

    def __init__(self, ref, cache_dir, ops=None, verbose=False):
        self.ref, self.cache, self.verbose = ref, cache_dir, verbose
        self.ops = ops  # None = everything
        self.fails = {}
        self.findings = {}
        self.finding_scope = {}
        self.counts = {}
        self.samples = {}

    def add_finding(self, key, count, scope):
        """A difference that is not a failure (a document inconsistency, a DuckDB difference not in section 9)."""
        self.findings[key] = count
        self.finding_scope[key] = scope

    def want(self, key):
        return self.ops is None or key in self.ops

    def fail(self, name, n=1, sample=None):
        self.fails[name] = self.fails.get(name, 0) + n
        if sample is not None and name not in self.samples:
            self.samples[name] = sample

    def run(self):
        ref, cd = self.ref, self.cache
        # ---- doc examples (always)
        n, f, fd = tc.check_doc_examples(ref)
        self.counts["doc examples"] = n
        for x in f:
            self.fail("doc_examples", 1, x)
        for k, c in fd.items():
            self.add_finding(k, c, "doc")
        # ---- 2a invariants, exhaustive
        for op in tc.OPS:
            if not self.want(op):
                continue
            n, viol = tc.check_invariants(ref, op)
            self.counts[f"invariants {op}: type pairs"] = n
            for k, (c, s) in viol.items():
                self.fail(f"invariant[{op}] {k}", c, s)
        if self.want("%"):
            # decision 3: the remainder fits the new type (witnesses at the extremes) and the bound is the exact supremum
            pairs, attained, nbad = tc.check_mod_witnesses(ref)
            self.counts["% witnesses: type pairs, pairs where a witness remainder has exactly P digits"] = {"pairs": pairs, "P attained": attained, "overflow raised": nbad}
            if nbad:
                self.fail("mod_witness_overflow", nbad)
            n, nb = tc.check_mod_bound_bruteforce()
            self.counts["% bound = min(A', B'-1) is the exact supremum (brute force, A', B' in 1..40)"] = {"cases": n, "wrong": nb}
            if nb:
                self.fail("mod_bound_formula_wrong", nb)
        # ---- 2c type matrix vs DuckDB
        for op in tc.OPS:
            if not self.want(op):
                continue
            codes = tc.duck_type_matrix(op, cd)
            cnt, bad = tc.compare_types(ref, op, codes)
            self.counts[f"duckdb types {op}"] = cnt
            if cnt["disagree"]:
                self.fail(f"duckdb_types[{op}]", cnt["disagree"], bad[0])
        if self.want("unify"):
            codes = tc.duck_type_matrix("unify", cd)
            cnt, bad = tc.compare_unify(ref, codes)
            self.counts["duckdb types unify (COALESCE)"] = cnt
            if cnt["disagree"]:
                self.fail("duckdb_types[unify]", cnt["disagree"], bad[0] if bad else None)
        # ---- 2b/2c values
        for gname, types, valfn in (
            ("small", vl.SMALL_TYPES, vl.small_values),
            ("big", vl.BIG_TYPES, vl.boundary_values),
        ):
            groups = vl.build_groups(types, valfn)
            for op in tc.OPS:
                if not self.want(op):
                    continue
                duck = vl.duck_values(op, gname, groups, cd)
                st = {}
                bad = vl.check_op_values(ref, op, gname, groups, duck, st)
                self.counts[f"values {gname} {op}"] = st
                for k, c in st.items():
                    if k.startswith("FINDING:"):
                        self.add_finding(f"{gname} {op}: {k[9:]}", c, op)
                    if k.startswith("MISMATCH:"):
                        self.fail(f"values[{gname} {op}] {k[9:]}", c, bad[0] if bad else None)
                if op == "/":
                    nb = vl.ref_only_division_checks(ref, groups, st)
                    if nb:
                        self.fail(f"values[{gname} /] division_by_zero_must_be_an_error", nb)
        # ---- division ties (exact half-way cases) + HUGEINT second implementation
        if self.want("/"):
            cases, ties = extras.tie_cases()
            hug = extras.duck_hugeint_division(cases, cd)
            st = {}
            bad = extras.check_ties(ref, cases, hug, st)
            self.counts["division ties"] = st
            for b in bad:
                self.fail(f"ties[/] {b[0]}", 1, b)
        # ---- AVG
        if self.want("avg"):
            groups = extras.avg_groups()
            duck = extras.duck_avg(groups, cd)
            st = {}
            bad = extras.check_avg(ref, groups, duck, st)
            self.counts["avg"] = st
            for b in bad:
                self.fail(f"avg {b[0]}", 1, b)
        # ---- ROUND/CAST/FLOOR/CEIL/TRUNC
        if self.want("round") or self.want("cast"):
            groups = extras.func_groups()
            duck = extras.duck_funcs(groups, cd)
            st = {}
            bad = extras.check_funcs(ref, groups, duck, st)
            self.counts["round/cast/floor/ceil/trunc values"] = st
            for b in bad:
                self.fail(f"funcs {b[0]}", 1, b)
            ft = extras.duck_func_types(cd)
            st2 = {}
            bad = extras.check_func_types(ref, ft, st2)
            self.counts["function result types"] = st2
            for k, c in st2.items():
                if k.startswith("FINDING:"):
                    self.add_finding(k[9:], c, "round")
            for b in bad:
                self.fail("func_types", 1, b)
        # ---- decision 4: integer with integer, and integer with decimal
        if any(self.want(k) for k in ("i+", "i-", "i*", "i%", "i//", "i/")):
            intarith.run_int_checks(self)
        intarith.run_mixed_checks(self)
        return self


def prepare_duckdb(cd):
    """Query DuckDB for everything once (cached)."""
    for op in tc.OPS + ["unify"]:
        t = time.time()
        tc.duck_type_matrix(op, cd)
        log(f"DuckDB type matrix {op}: done ({time.time() - t:.0f}s)")
    for gname, types, valfn in (("small", vl.SMALL_TYPES, vl.small_values), ("big", vl.BIG_TYPES, vl.boundary_values)):
        groups = vl.build_groups(types, valfn)
        for op in tc.OPS:
            t = time.time()
            vl.duck_values(op, gname, groups, cd)
            log(f"DuckDB values {gname} {op}: done ({time.time() - t:.0f}s)")
    cases, _ = extras.tie_cases()
    extras.duck_hugeint_division(cases, cd)
    extras.duck_avg(extras.avg_groups(), cd)
    extras.duck_funcs(extras.func_groups(), cd)
    extras.duck_func_types(cd)
    log("DuckDB: ties, AVG, functions done")
    intarith.prepare(cd)
    log("DuckDB: integer arithmetic and integer-with-decimal done")


def category(name):
    """Which kind of check raised a failure: D = compared with DuckDB, I = invariant/oracle over all types,
    E = the document's own examples, R = reference-only exact property / second implementation (decimal module,
    plain Python integers)."""
    if name.startswith("invariant["):
        return "I"
    if name == "doc_examples":
        return "E"
    ref_only = ("decimal_module", "not_nearest", "ref_overflow_but_value_fits", "div0_not_an_error", "division_by_zero_must", "avg_not_nearest",
                "avg_overflow", "avg_type_invalid", "avg_int_digits", "avg_scale", "ref_error", "ref_bind_error_on_valid_pair", "ref_bind_error_but",
                "oracle_vs_ref", "mod_cannot_overflow")
    if any(k in name for k in ref_only) and "duckdb" not in name.replace("ref_bind_error_but_duckdb_returns", ""):
        return "R"
    return "D"


def print_counts(counts):
    for k, v in counts.items():
        print(f"  {k}: {v}")


def overflow_report():
    r, ex = extras.overflow_analysis()
    print("\n== 2d. Decimal64 overflow analysis (all type pairs with both operands <= 18 digits) ==")
    for k, v in r.items():
        print(f"  {k}: {v}")
    print("  examples:")
    for k, v in ex.items():
        print(f"    {k}: {v}")
    return r, ex


def selected_mutants(args):
    if args.mutants in (None, ""):
        names = list(MUTATIONS) if args.full else QUICK_MUTANTS
    elif args.mutants == "all":
        names = list(MUTATIONS)
    else:
        names = [x.strip() for x in args.mutants.split(",") if x.strip()]
    for n in names:
        if n not in MUTATIONS:
            sys.exit(f"unknown mutant {n!r}; --list-mutants shows them")
    return names


def finding_changed(base, s):
    """A mutant is also caught if a finding changed: a different count, or a finding it should still produce is gone."""
    for k, c in s.findings.items():
        if base.findings.get(k) != c:
            return True
    for k, c in base.findings.items():
        scope = base.finding_scope[k]
        if k not in s.findings and (scope == "doc" or s.want(scope)):
            return True
    return False


def main():
    args = ARGS
    if args.list_mutants:
        for name, (ops, desc) in MUTATIONS.items():
            print(f"{name:36s} {sorted(ops)}  {desc}")
        return 0
    cd = os.path.join(args.cache_dir, PROFILE)
    log(f"profile {PROFILE}: {NT} decimal types ({NT * NT} type pairs); DuckDB cache {cd}")
    log("querying DuckDB (cached after the first run)")
    prepare_duckdb(cd)

    log("baseline: unmutated reference")
    base = Suite(Ref(), cd).run()
    print("\n== BASELINE (unmutated reference) ==")
    print_counts(base.counts)
    print(f"  FAILURES: {base.fails or 'none'}")
    print("  FINDINGS (not failures: a difference between the document and itself or DuckDB, not listed in section 9):")
    for k, c in base.findings.items():
        print(f"    {c:8d}  {k}")
    for k, s in base.samples.items():
        print(f"    {k}: {s}")
    ov, ex = overflow_report()

    results = {"profile": PROFILE, "baseline_fails": base.fails, "findings": base.findings, "counts": base.counts,
               "overflow": ov, "mutations": {}}
    if not args.skip_mutations:
        names = selected_mutants(args)
        print(f"\n== MUTATION CHECK ({len(names)} of {len(MUTATIONS)} mutants) ==")
        survivors = []
        for name in names:
            ops, desc = MUTATIONS[name]
            t = time.time()
            s = Suite(Ref([name]), cd, ops=set(ops) | {"doc"}).run()
            caught = bool(s.fails) or finding_changed(base, s)
            print(f"  {'CAUGHT  ' if caught else 'SURVIVED'} {name:36s} {desc}")
            if caught:
                top = sorted(s.fails.items(), key=lambda kv: -kv[1])[:4] or [("findings changed", 1)]
                print("           by: " + "; ".join(f"{k} x{v}" for k, v in top) + f"   ({time.time() - t:.0f}s)")
            else:
                survivors.append(name)
            cats = sorted({category(k) for k in s.fails})
            print(f"           categories: {''.join(cats) or 'findings-only'}")
            results["mutations"][name] = {"caught": caught, "categories": cats, "by": sorted(s.fails)}
        print(f"\n  mutations caught: {len(names) - len(survivors)} of {len(names)}; survivors: {survivors or 'none'}")
    if args.json:
        with open(args.json, "w") as f:
            json.dump(results, f, indent=1, default=str)
    survived = [n for n, m in results["mutations"].items() if not m["caught"]]
    return 1 if (base.fails or survived) else 0


if __name__ == "__main__":
    sys.exit(main())
