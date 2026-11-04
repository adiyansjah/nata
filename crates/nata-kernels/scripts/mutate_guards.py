#!/usr/bin/env python3
"""Mutation check for the division kernels, the guard filters and the arrow wrappers (ADR 0003 decision 4).

Each mutation is one deliberate breakage of src/kernels.rs or src/arrow_path.rs. For each, `cargo test -p nata-kernels`
must FAIL; a mutation that leaves the tests green is a SURVIVOR and needs a new test. The original files are always
restored (also on Ctrl-C). Usage: mutate_guards.py [substring-of-mutation-name ...]
"""
import os, re, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CRATE = os.path.join(ROOT, "crates", "nata-kernels")
K = "src/kernels.rs"
A = "src/arrow_path.rs"

# (name, file, old text, new text, which occurrence of old (0-based))
M = [
    ("div: error flags ignore the selection (every row is checked)", K,
     "pub fn div(a: Col<i64>, b: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {\n    let ((a, va), (b, vb)) = (a, b);",
     "pub fn div(a: Col<i64>, b: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {\n    let sel = None::<&[u32]>;\n    let ((a, va), (b, vb)) = (a, b);", 0),
    ("div: zero-divisor flag not masked by validity", K, "flags.zero |= z & ok;", "flags.zero |= z;", 0),
    ("div: overflow flag not masked by validity", K, "flags.overflow |= o & ok;", "flags.overflow |= o;", 0),
    ("div: validity ignores the divisor (zero-in-NULL-divisor raises)", K, "let ok = va.ok(i) & vb.ok(i);\n        let (q, z, o) = div_step(a[i], b[i]);", "let ok = va.ok(i);\n        let (q, z, o) = div_step(a[i], b[i]);", 0),
    ("div: zero check dropped", K, "(q, b == 0, a == i64::MIN && b == -1)", "(q, false, a == i64::MIN && b == -1)", 0),
    ("div: MIN / -1 check dropped", K, "(q, b == 0, a == i64::MIN && b == -1)", "(q, b == 0, false)", 0),
    ("div: output validity always true", K, "valid[i] = ok as u8;", "valid[i] = 1;", 2),  # 3rd masked loop = div
    ("div: rounds toward minus infinity instead of zero", K, "a.checked_div(b).unwrap_or(0)", "a.checked_div_euclid(b).unwrap_or(0)", 0),
    ("div: overflow reported ahead of zero", K, "if self.zero {\n            Err(EngineError(\"division by zero\".to_string()))\n        } else if self.overflow {\n            overflow(\"division\")\n        }", "if self.overflow {\n            overflow(\"division\")\n        } else if self.zero {\n            Err(EngineError(\"division by zero\".to_string()))\n        }", 0),
    ("div_const_left: zero flag not masked by validity", K, "let ok = va.ok(i);\n        let (q, z, o) = div_step(c, a[i]);\n        v[i] = q;\n        valid[i] = ok as u8;\n        flags.zero |= z & ok;", "let ok = va.ok(i);\n        let (q, z, o) = div_step(c, a[i]);\n        v[i] = q;\n        valid[i] = ok as u8;\n        flags.zero |= z;", 0),
    ("div_const_left: ignores the selection", K, "pub fn div_const_left(c: i64, a: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {\n    let (a, va) = a;", "pub fn div_const_left(c: i64, a: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {\n    let sel = None::<&[u32]>;\n    let (a, va) = a;", 0),
    ("div_const_left: operands swapped", K, "let (q, z, o) = div_step(c, a[i]);", "let (q, z, o) = div_step(a[i], c);", 0),
    ("guard split_ne: a NULL comparison counts as TRUE", K, "if valid.ok(i) && col[i] != value {", "if !valid.ok(i) || col[i] != value {", 0),
    ("guard split_ne: NULL comparisons vanish from the ELSE branch", K, "        } else {\n            no.push(i as u32);\n        }", "        } else if valid.ok(i) {\n            no.push(i as u32);\n        }", 0),
    ("guard split_ne: ignores the incoming selection", K, ") -> (Vec<u32>, Vec<u32>) {", ") -> (Vec<u32>, Vec<u32>) {\n    let sel = if sel.is_some() { None } else { sel };", 0),
    ("guard split_ne: tests `==` instead of `<>`", K, "valid.ok(i) && col[i] != value", "valid.ok(i) && col[i] == value", 0),
    ("merge_branches: branch NULLs become valid", K, "valid[i] = out.valid.as_ref().map_or(1, |x| x[i]);", "valid[i] = 1;", 0),
    ("merge_branches: later branch does not overwrite", K, "            v[i] = out.v[i];\n            valid[i]", "            v[i] = if valid[i] == 0 { out.v[i] } else { v[i] };\n            valid[i]", 0),
    ("arrow wrapper: compaction skipped (take over all rows)", A, "let idx = UInt32Array::from(sel.to_vec());", "let idx = UInt32Array::from((0..a.len() as u32).collect::<Vec<_>>());", 0),
    ("arrow wrapper: selection not folded (every row kept)", A, "let mut keep = vec![false; a.len()];", "let mut keep = vec![true; a.len()];", 0),
    ("arrow wrapper: selection dropped from the null-buffer union", A, "NullBuffer::union(x.nulls(), Some(&keep))", "NullBuffer::union(x.nulls(), None)", 0),
    ("arrow wrapper: validity bytes inverted", A, "b.iter().map(|&x| x != 0)", "b.iter().map(|&x| x == 0)", 0),
]


def run_tests():
    r = subprocess.run(["cargo", "test", "-p", "nata-kernels", "--offline"], cwd=ROOT, capture_output=True, text=True)
    out = r.stdout + r.stderr
    if r.returncode == 0:
        return None
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
    if failed:
        return failed
    return ["(does not compile / harness error) " + out.strip().splitlines()[-1][:100]]


def main():
    only = sys.argv[1:]
    originals = {f: open(os.path.join(CRATE, f)).read() for f in (K, A)}
    if run_tests() is not None:
        print("baseline is not green; fix the tests first")
        sys.exit(2)
    print("baseline: all tests pass\n")
    survivors = caught = 0
    try:
        for name, f, old, new, idx in M:
            if only and not any(o in name for o in only):
                continue
            text = originals[f]
            if text.count(old) <= idx:
                print(f"SKIP (pattern not found, fix the mutation): {name}")
                survivors += 1
                continue
            parts = text.split(old)
            mutated = old.join(parts[: idx + 1]) + new + old.join(parts[idx + 1 :])
            open(os.path.join(CRATE, f), "w").write(mutated)
            res = run_tests()
            open(os.path.join(CRATE, f), "w").write(text)
            if res is None:
                survivors += 1
                print(f"SURVIVED  {name}", flush=True)
            else:
                caught += 1
                short = ", ".join(x.split("::")[-1] for x in res[:3]) + (f" (+{len(res) - 3} more)" if len(res) > 3 else "")
                print(f"CAUGHT    {name}\n          by: {short}", flush=True)
    finally:
        for f, t in originals.items():
            open(os.path.join(CRATE, f), "w").write(t)
    print(f"\n{caught} of {caught + survivors} mutations caught, {survivors} survived")
    sys.exit(1 if survivors else 0)


main()
