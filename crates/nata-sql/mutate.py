#!/usr/bin/env python3
"""Mutation check of the SQL front end (binder, typing, pushdown, parse guard). Each mutant is ONE exact text edit (or a list of edits applied together) of a
source file of this crate; `cargo test --release -p nata-sql` runs on a private copy of the crate in a private workspace (the other crates are symlinked).
  CAUGHT   = a test fails
  SURVIVED = the tests pass: a gap in the TESTS (fix the tests, never weaken the mutant)
  INVALID  = the edit does not apply exactly once, changes nothing, or the copy does not compile; an INVALID mutant counts as neither caught nor missed
             and fails the run
The unmutated copy runs first and must pass. The real tree is hashed before and after and must not change. The match ignores whitespace, so rustfmt does
not break the mutants. Mutants run one after the other on one shared target directory (the dependencies compile once).
usage: mutate.py [--list] [--work DIR] [--out FILE] [name ...]"""
import argparse, hashlib, os, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
P, B, T, X, S, PA = "src/plan.rs", "src/bind.rs", "src/types.rs", "src/texpr.rs", "src/source.rs", "src/parse.rs"

M = {
 # ---- pushdown (rule R2)
 "push-through-or": (P, "Some(vec![lo, hi]) } _ => None,", "Some(vec![lo, hi]) } ExprKind::Or(v) => { let mut all = Vec::new(); for x in v { all.extend(try_push(x, cols)?); } Some(all) } _ => None,"),
 "not-lost-in-pushdown": (P, "Some(vec![lo, hi]) } _ => None,", "Some(vec![lo, hi]) } ExprKind::Not(x) => try_push(x, cols), _ => None,"),
 "pushed-conjunct-also-left-in-the-filter": (P, "Some(terms) => pushed.extend(terms),", "Some(terms) => { pushed.extend(terms); remaining.push(c); }"),
 "conjunct-dropped-from-the-filter-when-not-pushed": (P, "None => remaining.push(c),", "None => drop(c),"),
 "operator-flip-wrong": (X, "CmpOp::Lt => CmpOp::Gt,", "CmpOp::Lt => CmpOp::Ge,"),
 "not-between-pushed-as-between": (P, "negated: false,", "negated: _,"),
 "is-null-and-is-not-null-swapped": (P, "test: if *negated { Test::IsNotNull } else { Test::IsNull },", "test: if *negated { Test::IsNull } else { Test::IsNotNull },"),
 "between-bounds-swapped": [(P, "cmp_term(name.clone(), kind, CmpOp::Ge, low)?", "cmp_term(name.clone(), kind, CmpOp::Le, low)?"), (P, "cmp_term(name, kind, CmpOp::Le, high)?", "cmp_term(name, kind, CmpOp::Ge, high)?")],
 "pushed-decimal-literal-loses-its-scale": (P, "digits(*u).max(u32::from(scale)).max(1)).ok()?, scale, })", "digits(*u).max(u32::from(scale)).max(1)).ok()?, scale: 0, })"),
 "pushed-date-literal-off-by-one": (P, "(Kind::Date, Value::Date(d)) => Some(Literal::Date(*d)),", "(Kind::Date, Value::Date(d)) => Some(Literal::Date(*d + 1)),"),
 "pushed-text-literal-lower-cased": (P, "Some(Literal::Text(s.clone()))", "Some(Literal::Text(s.to_lowercase()))"),
 "filter-only-columns-not-read": (P, "for r in &remaining {", "for r in remaining.iter().take(0) {"),
 "offset-off-by-one": (P, "offset: offset.unwrap_or(0),", "offset: offset.unwrap_or(0).saturating_add(1),"),
 "limit-printed-below-the-filter": [
   (P, 'if let Some(l) = &self.limit { line(depth, format!("Limit {} offset {}", l.n, l.offset)); depth += 1; }', ""),
   (P, 'if let Some(f) = &self.filter { line(depth, format!("Filter {}", render_expr(&f.predicate, &names)),); depth += 1; }',
       'if let Some(f) = &self.filter { line(depth, format!("Filter {}", render_expr(&f.predicate, &names)),); depth += 1; } if let Some(l) = &self.limit { line(depth, format!("Limit {} offset {}", l.n, l.offset)); depth += 1; }')],
 # ---- binder and typing
 "nested-negation-retyped-as-a-literal": (B, 'Ok(Expr::new(ExprKind::Neg(Box::new(x)), ty, pos))', 'if let ExprKind::Literal(v) = &x.kind {\n            match v {\n                Value::Int(i) => return Ok(int_literal(-i128::from(*i), pos)),\n                Value::Decimal(u) if x.ty.is_decimal() => {\n                    return Ok(Expr::literal(Value::Decimal(-*u), x.ty, pos));\n                }\n                _ => {}\n            }\n        }\nOk(Expr::new(ExprKind::Neg(Box::new(x)), ty, pos))'),
 "quoted-name-folded-to-lower-case": (B, "if id.quoted { id.name.clone() } else {", "if false { id.name.clone() } else {"),
 "string-compared-with-number-accepted": (T, "if l == r || (l.is_exact_numeric() && r.is_exact_numeric()) {", "if l == r || (l.is_exact_numeric() && r.is_exact_numeric()) || (l == SqlType::Varchar && r.is_exact_numeric()) || (r == SqlType::Varchar && l.is_exact_numeric()) {"),
 "string-compared-with-date-accepted": (T, "if l == r || (l.is_exact_numeric() && r.is_exact_numeric()) {", "if l == r || (l.is_exact_numeric() && r.is_exact_numeric()) || (l == SqlType::Varchar && r == SqlType::Date) || (r == SqlType::Varchar && l == SqlType::Date) {"),
 "float-compared-with-decimal-accepted": (T, "if (l.is_float() && r.is_decimal()) || (l.is_decimal() && r.is_float()) {", "if false {"),
 "not-lost": (B, "Ok(Expr::new(ExprKind::Not(Box::new(x)), SqlType::Boolean, pos))", "Ok(x)"),
 "alias-ignored": (B, "Some(a) => fold(a),", "Some(_) => default_name(expr),"),
 "name-resolved-to-the-wrong-column": (B, "Ok(Expr::new(ExprKind::Column(i), ty, pos))", "Ok(Expr::new(ExprKind::Column(i.saturating_sub(1)), ty, pos))"),
 "sign-of-a-negative-literal-lost": (B, "if negative { unscaled = -unscaled; }", "if false { unscaled = -unscaled; }"),
 "exponent-sign-flipped": (B, "as i64::try_from(int_part.len()).map_err(|_| too_many())? + exp;".replace("as ", ""), "i64::try_from(int_part.len()).map_err(|_| too_many())? - exp;"),
 "integer-literal-width": (B, "if let Ok(i) = i32::try_from(v) {", "if let Ok(i) = i16::try_from(v) {"),
 "date-literal-off-by-one": (B, "Expr::literal(Value::Date(d), SqlType::Date, pos)", "Expr::literal(Value::Date(d + 1), SqlType::Date, pos)"),
 "decimal-literal-scale-off-by-one": (B, "Ok(decimal_literal(unscaled, scale, pos))", "Ok(decimal_literal(unscaled, scale.saturating_sub(1), pos))"),
 "null-never-gets-its-type": (B, "fn retype_null(e: &mut Expr, ty: SqlType) { if e.is_null_literal() {", "fn retype_null(e: &mut Expr, ty: SqlType) { if false && e.is_null_literal() {"),
 "suggestion-distance-3": (B, "edit_distance(name, &c.name, 2)", "edit_distance(name, &c.name, 3)"),
 "where-needs-no-boolean": (B, "if e.ty != SqlType::Boolean { return Err(SqlError::bind(", "if false { return Err(SqlError::bind("),
 "and-or-operands-unchecked": (B, "self.need_boolean(&left, what)?;", "let _ = what;"),
 "in-list-takes-any-expression": (B, "if !it.is_literal() {", "if false {"),
 "case-scale-takes-the-minimum": (T, "scale = scale.max(u32::from(s));", "scale = scale.min(u32::from(s));"),
 "division-scale-5": (T, "let s = (s1 + 6).min(MAX_DIGITS);", "let s = (s1 + 5).min(MAX_DIGITS);"),
 "addition-precision-plus-one-dropped": (T, "let p = (p1 - s1).max(p2 - s2) + s + 1;", "let p = (p1 - s1).max(p2 - s2) + s;"),
 "multiplication-cap-at-18-dropped": (T, "let p = if p1 <= 18 && p2 <= 18 && s < 18 {", "let p = if false {"),
 "remainder-precision-takes-the-maximum": (T, "Ok(dec((p1 - s1).min(p2 - s2) + s, s))", "Ok(dec((p1 - s1).max(p2 - s2) + s, s))"),
 # ---- the parse stage
 "nesting-limit-off-by-one": (PA, "if open_depth + run > MAX_NESTING {", "if open_depth + run >= MAX_NESTING {"),
 "chain-limit-off-by-one": (PA, "if f.counts[k] as usize >= MAX_CHAIN_TERMS {", "if f.counts[k] as usize > MAX_CHAIN_TERMS {"),
 "limit-all-not-detected": (PA, "Keyword::ALL if prev_limit => {", "Keyword::ALL if false => {"),
 "error-at-the-end-reported-at-the-start": (PA, "SqlError::syntax(Pos::end_of(text), msg)", "SqlError::syntax(Pos::START, msg)"),
 "integer-division-read-as-division": (PA, ".is_some_and(|p| self.int_div_at.binary_search(&p).is_ok())", ".is_some_and(|_| false)"),
 "chained-comparison-accepted": (PA, "if left_cmp || right_cmp {", "if false {"),
 "reserved-word-accepted-as-a-name": (PA, "if is_reserved_word(&id.value) {", "if false && is_reserved_word(&id.value) {"),
}


def sha_tree():
    h = hashlib.sha256()
    for d, _, fs in sorted(os.walk(HERE)):
        if "target" in d.split(os.sep):
            continue
        for f in sorted(fs):
            if f.endswith((".rs", ".toml", ".py", ".md", ".txt", ".jsonl")):
                h.update(open(os.path.join(d, f), "rb").read())
    return h.hexdigest()


def stage(work):
    """A private workspace: the real root manifest (lints, release profile), a copy of this crate, symlinks to the crates it depends on."""
    base = os.path.join(work, "ws")
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(os.path.join(base, "crates"))
    root = open(os.path.join(ROOT, "Cargo.toml")).read()
    root = re.sub(r"members = \[[^\]]*\]", 'members = ["crates/nata-sql"]', root)
    root = re.sub(r"exclude = \[[^\]]*\]\n", "", root)
    open(os.path.join(base, "Cargo.toml"), "w").write(root)
    shutil.copy(os.path.join(ROOT, "Cargo.lock"), os.path.join(base, "Cargo.lock"))
    for dep in ["nata-parquet", "nata-scan", "nata-sched"]:
        os.symlink(os.path.join(ROOT, "crates", dep), os.path.join(base, "crates", dep))
    dst = os.path.join(base, "crates", "nata-sql")
    # copy2 would keep the old modification times, and cargo would trust the artifacts of an earlier (mutated) build
    shutil.copytree(HERE, dst, ignore=shutil.ignore_patterns("target", "mutate.py", "__pycache__"), copy_function=shutil.copy)
    return base, dst


def cargo_test(base, work):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(work, "target"), CARGO_TERM_COLOR="never", CARGO_NET_OFFLINE="true")
    try:
        p = subprocess.run(["cargo", "test", "--release", "--no-fail-fast", "-p", "nata-sql"], cwd=base, env=env, capture_output=True, text=True, timeout=1500)
    except subprocess.TimeoutExpired:
        return 99, "timeout"
    return p.returncode, p.stdout + p.stderr


def run_one(name, base, dst, work):
    edits = M[name] if isinstance(M[name], list) else [M[name]]
    touched = {}
    for f, old, new in edits:
        p = os.path.join(dst, f)
        touched.setdefault(f, open(os.path.join(HERE, f)).read())
        t = open(p).read()
        rx = re.compile(r"\s*".join(re.escape(c) for c in old if not c.isspace()))
        found = list(rx.finditer(t))
        if len(found) != 1:
            return "INVALID", "edit applies %d times: %s" % (len(found), old[:60])
        if re.sub(r"\s+", "", old) == re.sub(r"\s+", "", new):
            return "INVALID", "edit changes nothing"
        open(p, "w").write(t[: found[0].start()] + new + t[found[0].end():])
    code, out = cargo_test(base, work)
    for f, orig in touched.items():
        open(os.path.join(dst, f), "w").write(orig)
    if code == 0:
        return "SURVIVED", ""
    if "could not compile" in out or "error[E" in out:
        first = next((l for l in out.splitlines() if l.startswith("error")), "")
        return "INVALID", "does not compile: " + first[:100]
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
    return "CAUGHT", ", ".join(failed[:3]) + (" ..." if len(failed) > 3 else "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--work", default=os.path.join(ROOT, "target", "mutwork-sql"))
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.list:
        print("\n".join(M))
        return 0
    names = a.names or list(M)
    before = sha_tree()
    base, dst = stage(a.work)
    t0 = time.time()
    code, out = cargo_test(base, a.work)
    if code != 0:
        print("the unmutated copy FAILS:\n" + out[-3000:])
        return 2
    print("unmutated copy: tests pass (%.0fs)" % (time.time() - t0), flush=True)
    rows, bad = [], 0
    for n in names:
        t = time.time()
        status, why = run_one(n, base, dst, a.work)
        bad += status != "CAUGHT"
        rows.append((n, status, why))
        print("%-55s %-8s %s (%.0fs)" % (n, status, why, time.time() - t), flush=True)
    after = sha_tree()
    assert before == after, "the real tree changed"
    c = {s: sum(1 for r in rows if r[1] == s) for s in ("CAUGHT", "SURVIVED", "INVALID")}
    print("\n%d mutants: %d CAUGHT, %d SURVIVED, %d INVALID" % (len(rows), c["CAUGHT"], c["SURVIVED"], c["INVALID"]))
    if a.out:
        with open(a.out, "w") as f:
            f.write("| mutant | result | first failing tests |\n|---|---|---|\n")
            for n, s, w in rows:
                f.write("| %s | %s | %s |\n" % (n, s, w.replace("|", "/")))
            f.write("\n%d mutants: %d CAUGHT, %d SURVIVED, %d INVALID\n" % (len(rows), c["CAUGHT"], c["SURVIVED"], c["INVALID"]))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
