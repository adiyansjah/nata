#!/usr/bin/env python3
"""Mutation check of the word-wise kernels (src/wk.rs, src/bits.rs).

Applies one deliberate breakage at a time to a PRIVATE COPY of the project (never to the real sources),
runs the unit tests and, for a mutant that survives them, the end-to-end `check` and `nullcheck`
(which compare the whole engine with the generic-ops engine, DuckDB-identical, and the scalar reference).
Prints CAUGHT (by what) or SURVIVED per mutant. A survivor means a missing test: add one, re-run.

Usage: mutate.py <lineitem.parquet> [only-substring ...]    (env MUTATE_SKIP_E2E=1: unit tests only; MUTATE_TEST=<name part>: run only those unit tests)
Exit status 1 if any mutant survived everything, or if an old pattern was not found (stale script).
"""
import os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
LI = sys.argv[1]
ONLY = sys.argv[2:]

# (name, file, old, new)   `old` must occur exactly once in the file.
W = "src/wk.rs"
B = "src/bits.rs"
E = "src/wengine.rs"
M = [
    # ---- filter
    ("filter dense: predicate not ANDed with validity", W,
     "                if let Some(v) = valid {\n                    m &= v[w];\n                }\n                out[w] = m;",
     "                out[w] = m;"),
    ("filter dense: predicate not ANDed with the incoming selection", W,
     "                m &= sel.word(rows, w);\n                if let Some(v) = valid {",
     "                if let Some(v) = valid {"),
    ("filter dense: validity word of the wrong index", W,
     "                    m &= v[w];", "                    m &= v[w.saturating_sub(1)];"),
    ("filter dense: bit index off by one when packing the predicate", W,
     "m |= (((x >= lo) & (x <= hi)) as u64) << j;", "m |= (((x >= lo) & (x <= hi)) as u64) << ((j + 1) & 63);"),
    ("filter dense: lower bound exclusive", W,
     "m |= (((x >= lo) & (x <= hi)) as u64) << j;", "m |= (((x > lo) & (x <= hi)) as u64) << j;"),
    ("filter dense: upper bound exclusive", W,
     "m |= (((x >= lo) & (x <= hi)) as u64) << j;", "m |= (((x >= lo) & (x < hi)) as u64) << j;"),
    ("filter dense: All returned when one row short", W,
     "if count == rows {\n                Sel::All", "if count + 1 >= rows {\n                Sel::All"),
    ("filter dense: density test off (always a mask)", W,
     "} else if o.dense_div > 0 && count * o.dense_div >= rows {\n                Sel::Mask(out)",
     "} else if o.dense_div > 0 {\n                Sel::Mask(out)"),
    ("filter idx: validity bit tested at the wrong row", W,
     "((v[(i >> 6) as usize] >> (i & 63)) & 1 == 1)) as usize;", "((v[(i >> 6) as usize] >> ((i + 1) & 63)) & 1 == 1)) as usize;"),
    ("filter idx: validity ignored", W,
     "k += ((x >= lo) & (x <= hi) & ((v[(i >> 6) as usize] >> (i & 63)) & 1 == 1)) as usize;",
     "k += ((x >= lo) & (x <= hi)) as usize;"),
    # ---- subtraction
    ("sub: error flag not masked by validity (dense)", W,
     "        let live = sel.word(rows, w) & va.map_or(!0, |m| m[w]);\n        if live == 0 {\n            continue; // nothing",
     "        let live = sel.word(rows, w);\n        if live == 0 {\n            continue; // nothing"),
    ("sub: error flag not masked by selection (dense)", W,
     "        let live = sel.word(rows, w) & va.map_or(!0, |m| m[w]);\n        if live == 0 {\n            continue; // nothing",
     "        let live = va.map_or(!0, |m| m[w]);\n        if live == 0 {\n            continue; // nothing"),
    ("sub: word skipped when it has live rows instead of none", W,
     "        if live == 0 {\n            continue; // nothing", "        if live != 0 {\n            continue; // nothing"),
    ("sub: deferred check forgets the mask", W,
     "ov | ((c.overflowing_sub(*x).1 as u64) << j)) & live;", "ov | ((c.overflowing_sub(*x).1 as u64) << j)) & !0;"),
    ("sub: deferred check never takes the slow path", W,
     "            if any {\n                bad |= ac.iter().enumerate()", "            if false {\n                bad |= ac.iter().enumerate()"),
    ("sub: immediate check bit index off by one", W,
     "                let (q, o) = c.overflowing_sub(*x);\n                *r = q;\n                ov |= (o as u64) << j;",
     "                let (q, o) = c.overflowing_sub(*x);\n                *r = q;\n                ov |= (o as u64) << ((j + 1) & 63);"),
    ("sub: error flag not masked by validity (idx)", W,
     "            bad |= o & ok;\n        }\n        return if bad { overflow(\"subtraction\")",
     "            bad |= o;\n        }\n        return if bad { overflow(\"subtraction\")"),
    ("sub: output validity dropped (dense)", W,
     "    Ok(Out { v, valid: va.map(<[u64]>::to_vec) })", "    Ok(Out { v, valid: None })"),
    # ---- multiplication
    ("mul: error flag not masked by validity (dense)", W,
     "        let live = sel.word(rows, w) & valid.as_ref().map_or(!0, |m| m[w]);", "        let live = sel.word(rows, w);"),
    ("mul: error flag not masked by selection (dense)", W,
     "        let live = sel.word(rows, w) & valid.as_ref().map_or(!0, |m| m[w]);", "        let live = valid.as_ref().map_or(!0, |m| m[w]);"),
    ("mul: output validity is OR instead of AND of the operands", W,
     "va.map_or(!0, |m| m[w]) & vb.map_or(!0, |m| m[w]) & word_mask(rows, w)",
     "(va.map_or(0, |m| m[w]) | vb.map_or(0, |m| m[w])) & word_mask(rows, w)"),
    ("mul: second operand's validity ignored (dense)", W,
     "va.map_or(!0, |m| m[w]) & vb.map_or(!0, |m| m[w]) & word_mask(rows, w)", "va.map_or(!0, |m| m[w]) & word_mask(rows, w)"),
    ("mul: second operand's validity ignored (idx)", W,
     "let ok = bit(va, i) & bit(vb, i);", "let ok = bit(va, i);"),
    ("mul: error flag not masked by validity (idx)", W,
     "            bad |= o & ok;\n        }\n        return if bad { overflow(\"multiplication\")",
     "            bad |= o;\n        }\n        return if bad { overflow(\"multiplication\")"),
    ("mul: overflow never reported (dense)", W,
     "    if bad != 0 {\n        return overflow(\"multiplication\");", "    if false {\n        return overflow(\"multiplication\");"),
    ("mul: only the first word is checked for overflow (deferred path)", W,
     "(x.overflowing_mul(*y).1 as u64) << j)) & live;", "(x.overflowing_mul(*y).1 as u64) << j)) & if w == 0 { live } else { 0 };"),
    ("mul: last partial word not checked (immediate path)", W,
     "            bad |= ov & live;\n        }\n    }\n    if bad != 0 {\n        return overflow(\"multiplication\");",
     "            if 64 * (w + 1) <= rows { bad |= ov & live; }\n        }\n    }\n    if bad != 0 {\n        return overflow(\"multiplication\");"),
    # ---- single sum
    ("sum: popcount of the wrong word (selection only, not validity)", W,
     "        n += (sw & vw).count_ones() as u64;", "        n += sw.count_ones() as u64;"),
    ("sum: NULL rows not subtracted", W,
     "        let mut nulls = sw & !vw;\n        while nulls != 0 {\n            block -= chunk",
     "        let mut nulls = 0u64;\n        while nulls != 0 {\n            block -= chunk"),
    ("sum: NULL rows to subtract not limited to the selection", W,
     "        let mut nulls = sw & !vw;\n        while nulls != 0 {\n            block -= chunk",
     "        let mut nulls = !vw & word_mask(rows, w);\n        while nulls != 0 {\n            block -= chunk"),
    ("sum: subtracts the wrong row (off by one)", W,
     "            block -= chunk[nulls.trailing_zeros() as usize] as i128;",
     "            block -= chunk[(nulls.trailing_zeros() as usize + 1) % chunk.len()] as i128;"),
    ("sum: bit index off by one in the masked sum", W,
     "                block += (x as i128) & -(((sw >> j) & 1) as i128);", "                block += (x as i128) & -(((sw >> ((j + 1) & 63)) & 1) as i128);"),
    ("sum: selection ignored in the masked sum", W,
     "                block += (x as i128) & -(((sw >> j) & 1) as i128);", "                block += x as i128;"),
    ("sum: whole-word shortcut taken for a partial selection", W,
     "        if sw == word_mask(rows, w) {\n            for &x in chunk {\n                block += x as i128;",
     "        if sw != 0 {\n            for &x in chunk {\n                block += x as i128;"),
    ("sum: idx path ignores validity", W,
     "            s += (v[i] as i128) * (ok as i128);\n            n += ok as u64;\n        }\n        return (s, n);",
     "            s += v[i] as i128;\n            n += ok as u64;\n        }\n        return (s, n);"),
    # ---- grouped sums
    ("group_sums: NULL rows subtracted from the wrong column", W,
     "add128(&mut acc[g][k], (vs[k][j] as i128).wrapping_neg());", "add128(&mut acc[g][(k + 1) % K], (vs[k][j] as i128).wrapping_neg());"),
    ("group_sums: NULL rows to subtract not limited to the selection", W,
     "                let mut nw = sw & !valid[w];", "                let mut nw = !valid[w] & word_mask(rows, w);"),
    ("group_sums: NULL rows added instead of subtracted", W,
     "add128(&mut acc[g][k], (vs[k][j] as i128).wrapping_neg());", "add128(&mut acc[g][k], vs[k][j] as i128);"),
    ("group_sums: NULL count not incremented", W,
     "                    add64(&mut nulls[k][g], 1);\n                }\n            }\n        }\n    }\n}",
     "                    add64(&mut nulls[k][g], 0);\n                }\n            }\n        }\n    }\n}"),
    ("group_sums: NULL row's group read from the wrong row", W,
     "                    let g = gs[j] as usize % NG;\n                    add128", "                    let g = gs[(j + 1) % n] as usize % NG;\n                    add128"),
    ("group_sums: count ignores the selection bit", W,
     "                add64(&mut count[g], b);\n                for k in 0..K {\n                    add128(&mut acc[g][k], (vs[k][j] as i128) & -(b as i128));",
     "                add64(&mut count[g], 1);\n                for k in 0..K {\n                    add128(&mut acc[g][k], (vs[k][j] as i128) & -(b as i128));"),
    ("group_sums: values not masked by the selection", W,
     "                    add128(&mut acc[g][k], (vs[k][j] as i128) & -(b as i128));\n                }\n            }\n        }",
     "                    add128(&mut acc[g][k], vs[k][j] as i128);\n                }\n            }\n        }"),
    ("group_sums: selection word of the wrong index", W,
     "        let sw = sel.word(rows, w);\n        let base = 64 * w;\n        let n = (rows - base).min(64);\n        let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);\n        let gs",
     "        let sw = sel.word(rows, w.saturating_sub(1));\n        let base = 64 * w;\n        let n = (rows - base).min(64);\n        let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);\n        let gs"),
    ("group_sums (per-row masks): validity word of the wrong column", W,
     "let m: [u64; K] = std::array::from_fn(|j| sw & cols[j].1.map_or(!0, |x| x[w]));",
     "let m: [u64; K] = std::array::from_fn(|j| sw & cols[(j + 1) % K].1.map_or(!0, |x| x[w]));"),
    ("group_sums (per-row masks): NULL counter counts the valid rows", W,
     "let null_w: [u64; K] = std::array::from_fn(|j| sw & !m[j]);\n            let base = 64 * w;",
     "let null_w: [u64; K] = std::array::from_fn(|j| m[j]);\n            let base = 64 * w;"),
    ("group_sums: idx path counts a NULL as a value", W,
     "                add128(&mut acc[g][j], (cols[j].0[i] as i128) * (ok as i128));", "                add128(&mut acc[g][j], cols[j].0[i] as i128);"),
    ("group_sums: last partial word walks past the rows", W,
     "        let n = (rows - base).min(64);\n        let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);\n        let gs",
     "        let n = 64.min(rows + 64 - base);\n        let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);\n        let gs"),
    ("group_sums: idx path increments the next column's NULL counter", W,
     "                add64(&mut nulls[j][g], !ok as u64);", "                add64(&mut nulls[(j + 1) % K][g], !ok as u64);"),
    ("group_sums: per-row-mask path increments the next column's NULL counter", W,
     "                    add64(&mut nulls[k][g], (null_w[k] >> j) & 1);", "                    add64(&mut nulls[(k + 1) % K][g], (null_w[k] >> j) & 1);"),
    # ---- bits
    ("validity words: tail of the last word not cleared", B,
     "        *last &= word_mask(rows, nwords(rows) - 1);", "        let _ = last;"),
    ("validity words: bit offset of a sliced buffer ignored", B,
     "read_word(bytes, off + 64 * k)", "read_word(bytes, 64 * k)"),
    ("read_word: funnel shift uses the wrong count", B,
     "lo | (get(byte + 8) << (64 - shift))", "lo | (get(byte + 8) << (63 - shift))"),
    ("read_word: aligned fast path reads the wrong byte", B,
     "bytes[byte..byte + 8].try_into()", "bytes[byte + 1..byte + 9].try_into()"),
    ("mask_to_idx: sparse loop clears too many bits", B,
     "                k += 1;\n                m &= m - 1;", "                k += 1;\n                m &= m - 1;\n                m &= m.wrapping_sub(1);"),
    ("mask_to_idx: dense compress advances on the wrong bit", B,
     "                k += ((m >> j) & 1) as usize;", "                k += ((m >> ((j + 1) & 63)) & 1) as usize;"),
    ("mask_to_idx: base of a dense word off by one", B,
     "        let base = (64 * w) as u32;", "        let base = (64 * w) as u32 + (m.count_ones() >= DENSE_WORD) as u32;"),
    ("Sel::All word: tail mask forgotten", B,
     "Sel::All => word_mask(rows, w),", "Sel::All => !0,"),
    ("word_mask: full-word boundary wrong", B,
     "    if left >= 64 { !0 } else { (1u64 << left) - 1 }", "    if left > 64 { !0 } else { (1u64 << (left % 64)) - 1 }"),
    # ---- engine
    ("engine: null-rejecting filter marks the wrong column valid", E,
     "            let k = b.idx(name);\n            b.known_valid[k] = true;", "            let k = b.idx(name);\n            let n = b.known_valid.len();\n            b.known_valid[(k + 1) % n] = true;"),
    ("engine: null-rejecting filter applied even when the option is off", E,
     "        if b.opts.null_reject {\n            let k = b.idx(name);", "        if true {\n            let k = b.idx(name);"),
    ("engine: bitmap selection converted when the consumer works on words", E,
     "Sel::Mask(m) if !dense => std::borrow::Cow::Owned(Sel::Idx(mask_to_idx(m))),", "Sel::Mask(m) if dense => std::borrow::Cow::Owned(Sel::Idx(mask_to_idx(m))),"),
    ("engine: non-NULL count of a group wrong", E,
     "nn: [self.count[g] - self.nulls[0][g],", "nn: [self.count[g] - self.nulls[1][g],"),
]


# Every heavy command goes through HEAVY (e.g. scripts/local-heavy.sh on the shared dev machine), so that two heavy
# commands never run together; this script itself is light. Empty on a VM of our own.
HEAVY = os.environ.get("HEAVY", "")


def sh(cmd, cwd, env):
    r = subprocess.run(f"{HEAVY} {cmd}", cwd=cwd, shell=True, capture_output=True, text=True, env=env)
    return r.returncode == 0, (r.stdout + r.stderr)


def main():
    work = tempfile.mkdtemp(prefix="null-path-mutate-")
    try:
        for f in ("Cargo.toml", "Cargo.lock"):
            shutil.copy(os.path.join(HERE, f), work)
        # Fresh mtimes (shutil.copy, not copy2): the mutation target directory keeps the LAST mutant's binaries from the previous
        # invocation, and cargo decides what is fresh from mtimes. A copy that keeps the old source mtimes looks older than
        # that build, so cargo reuses the previous mutant and the unmutated baseline fails (found by the orchestrator,
        # 2026-11-04: two invocations in a row; same defect as spikes/hash-ops/mutate.py).
        shutil.copytree(os.path.join(HERE, "src"), os.path.join(work, "src"), copy_function=shutil.copy)
        # A target directory of its own, seeded with a copy of the compiled dependencies (arrow, parquet) so that they
        # are not rebuilt. It must NOT be the real project's target directory: cargo's artifact names for a workspace
        # member do not depend on the source path, so a mutant's test binary written to the shared directory was
        # later reused by `cargo test` in the real tree (the 2026-11-04 incident, see the results file).
        target = os.environ.get("MUTATE_TARGET", os.path.join(HERE, "target", "mutate-target"))
        real = os.path.realpath(os.path.join(HERE, "target"))
        assert os.path.realpath(target) != real, "the mutation run must not share the project's target directory"
        if not os.path.isdir(os.path.join(target, "release")):
            os.makedirs(target, exist_ok=True)
            seed = os.path.join(HERE, "target", "release")
            # Seeding with compiled dependencies only saves time. On a clean machine nothing has been built in the real
            # tree yet (found on the first GCP run): then the dependencies are simply built in the mutation target.
            if os.path.isdir(seed):
                shutil.copytree(seed, os.path.join(target, "release"), symlinks=True)
        env = dict(os.environ, CARGO_BUILD_JOBS="2", CARGO_TARGET_DIR=target)
        only_test = os.environ.get("MUTATE_TEST", "")  # run just the unit tests whose name contains this (to show a given test kills a mutant)
        stages = [("unit tests", f"cargo test --release -q --no-fail-fast {only_test} 2>&1")]
        if not os.environ.get("MUTATE_SKIP_E2E"):
            stages += [("check", f"cargo run --release -q -- check {LI}"), ("nullcheck", f"cargo run --release -q -- nullcheck {LI}")]

        def run_stages():
            """Name of the first stage that fails, None if all pass; 'COMPILE' if the mutant does not build."""
            for name, cmd in stages:
                ok, out = sh(cmd, work, env)
                if not ok:
                    return "COMPILE" if "could not compile" in out else name
            return None

        base = run_stages()
        print("baseline (unmutated copy) fails at:", base)
        if base:
            return 1
        caught_unit = caught_e2e = survived = stale = 0
        originals = {}
        for name, f, old, new in M:
            if ONLY and not any(o in name for o in ONLY):
                continue
            path = os.path.join(work, f)
            if f not in originals:
                originals[f] = open(os.path.join(HERE, f)).read()
            src = originals[f]
            if src.count(old) != 1:
                print(f"STALE (pattern occurs {src.count(old)} times): {name}", flush=True)
                stale += 1
                continue
            open(path, "w").write(src.replace(old, new))
            try:
                by = run_stages()
            finally:
                open(path, "w").write(src)
            if by == "COMPILE":
                print(f"STALE (mutant does not compile): {name}", flush=True)
                stale += 1
            elif by is None:
                print(f"SURVIVED: {name}", flush=True)
                survived += 1
            elif by == "unit tests":
                print(f"CAUGHT by unit tests: {name}", flush=True)
                caught_unit += 1
            else:
                print(f"CAUGHT only by end-to-end {by}: {name}", flush=True)
                caught_e2e += 1
        total = caught_unit + caught_e2e + survived
        print(f"\n{total} mutants: {caught_unit} caught by unit tests, {caught_e2e} caught only end to end, {survived} survived, {stale} stale/uncompilable")
        return 1 if survived or stale else 0
    finally:
        shutil.rmtree(work, ignore_errors=True)


sys.exit(main())
