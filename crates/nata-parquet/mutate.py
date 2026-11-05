#!/usr/bin/env python3
"""Mutation check of the NEW code of the Parquet scan (crates/nata-parquet): the footer adapter, the filter parser, the exact row filter, the reader, the
report, the CSV text, the errors, and the build configuration. Each mutant is ONE exact text edit (or a list of edits applied together) of a source file,
applied to a private copy of the workspace pieces; the tests of the crate run on the copy.
  CAUGHT   = a test fails
  SURVIVED = the tests pass: a gap in the TESTS (fix the tests, never weaken the mutant)
  INVALID  = an edit does not apply exactly once, changes nothing, or the copy does not compile; an INVALID mutant counts as neither caught nor missed and fails the run
The unmutated copy runs first and must pass. The real tree is hashed before and after and must not change. The match ignores whitespace, so rustfmt does
not break the mutants. Style: crates/nata-scan/mutate.py.
The tests run in this order and stop at the first failing group (fast groups first): lib, adapter, report, errors, overflow_config, special_files,
soundness_files, differential_lineitem, differential_edge. The DuckDB tests need the `duckdb` command line (and pyarrow through uvx, optional).
usage: mutate.py [--list] [--dry] [--jobs N] [--work DIR] [--out FILE] [name ...]"""
import argparse, concurrent.futures, hashlib, os, re, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
GROUPS = [["--lib"], ["--test", "adapter"], ["--test", "report"], ["--test", "errors"], ["--test", "overflow_config"], ["--test", "special_files"],
          ["--test", "soundness_files"], ["--test", "differential_lineitem"], ["--test", "differential_edge"]]

A, F, R, C, E, S, K = "src/adapter.rs", "src/filter.rs", "src/reader.rs", "src/csv.rs", "src/error.rs", "src/schema.rs", "src/civil.rs"
ROOTTOML = "ROOT:Cargo.toml"
M = {
 # --- the footer-statistics adapter
 "adapter-reads-deprecated-min-max": (A, "if stats.is_min_max_deprecated() || !order_is_usable(order, kind) {", "if !order_is_usable(order, kind) {"),
 "adapter-ignores-column-order": (A, "if stats.is_min_max_deprecated() || !order_is_usable(order, kind) {", "if stats.is_min_max_deprecated() {"),
 "adapter-trusts-ieee-total-order": (A, "        _ => false,\n    }\n}\n\n/// The column order of leaf", "        ColumnOrder::IEEE_754_TOTAL_ORDER => true,\n        _ => false,\n    }\n}\n\n/// The column order of leaf"),
 "adapter-sort-order-not-compared": (A, "so == expected && expected != SortOrder::UNDEFINED", "expected != SortOrder::UNDEFINED"),
 "adapter-inexact-bound-taken-as-exact": [(A, "let make = |value: Value| Some(Bound { value, exact });", "let make = |value: Value| Some(Bound { value, exact: true });"),
                                          (A, "Bound::utf8_from_bytes(bytes, exact)", "Bound::utf8_from_bytes(bytes, true)")],
 "adapter-text-bound-inexact-taken-as-exact": (A, "Bound::utf8_from_bytes(bytes, exact)", "Bound::utf8_from_bytes(bytes, true)"),
 "adapter-millisecond-timestamps-not-converted": (A, "Unit::Millis => make(Value::TimestampMicros(t.checked_mul(1000)?)),", "Unit::Millis => make(Value::TimestampMicros(t)),"),
 "adapter-millisecond-conversion-wraps": (A, "Unit::Millis => make(Value::TimestampMicros(t.checked_mul(1000)?)),", "Unit::Millis => make(Value::TimestampMicros(t.wrapping_mul(1000))),"),
 "adapter-invents-a-nan-count": (A, "out.null_count = stats.null_count_opt();", "out.null_count = stats.null_count_opt();\n    out.nan_count = Some(0);"),
 "adapter-missing-null-count-as-zero": (A, "out.null_count = stats.null_count_opt();", "out.null_count = stats.null_count_opt().or(Some(0));"),
 "adapter-decimal-sign-extension-lost": (A, "let pad = if negative { 0xFFu8 } else { 0u8 };", "let pad = 0u8;"),
 "adapter-date-sentinels-trusted": (A, "if d == i32::MAX || d <= -i32::MAX {\n                return None;\n            }", ""),
 "adapter-fixed-length-bound-of-wrong-width-trusted": (A, "if i32::try_from(bytes.len()).ok() != Some(type_length) {\n                return None;\n            }", ""),
 "adapter-negative-row-count-accepted": (A, "let rows = u64::try_from(rg.num_rows()).ok()?;", "let rows = u64::try_from(rg.num_rows().max(0)).ok()?;"),
 "adapter-column-order-list-indexed-unchecked": (A, ".and_then(|orders| orders.get(index))", ".and_then(|orders| Some(&orders[index]))"),
 "adapter-statistics-of-the-next-column": (A, "match rg.columns().get(leaf.index) {", "match rg.columns().get(leaf.index + 1) {"),
 "schema-unsigned-integers-pruned-as-signed": (S, "Kind::Int { signed: false, .. } => ColumnType::Unsupported,", "Kind::Int { signed: false, .. } => ColumnType::Int64,"),
 "schema-nested-column-accepted": (S, "if d.path().parts().len() != 1 || d.max_rep_level() != 0 || d.max_def_level() > 1 {", "if d.path().parts().len() != 1 && false {"),
 "schema-time-zone-timestamps-accepted": (S, "if adjusted {\n        return Err(\"TIMESTAMP WITH TIME ZONE (a timestamp adjusted to UTC)\".into());\n    }", ""),
 "schema-nanosecond-timestamps-as-micros": (S, "PqUnit::NANOS => Err(\"TIMESTAMP with nanosecond precision\".into()),", "PqUnit::NANOS => Ok(Kind::Timestamp(Unit::Micros)),"),
 # --- the filter parser
 "parse-ne-as-eq": (F, '("<>", CmpOp::Ne),', '("<>", CmpOp::Eq),'),
 "parse-le-after-lt": (F, '("<=", CmpOp::Le),\n        (">=", CmpOp::Ge),', '("<", CmpOp::Lt),\n        (">=", CmpOp::Ge),'),
 "parse-string-without-quotes-accepted": (F, '.ok_or_else(|| "write the text in single quotes, \'like this\'".to_string())?;', '.unwrap_or(text);'),
 "parse-integer-fraction-rounded": (F, "&& digits.contains('.') {", "&& digits.contains('.') && false {"),
 "parse-decimal-literal-rounded-to-two-digits": (F, "let scale = frac_part.len();", "let scale = frac_part.len().min(2);"),
 "parse-float-overflow-accepted": (F, 'if v.is_infinite() && !special {\n            return Err(format!("the number is out of range for {name}"));\n        }\n        Ok(Literal::F64(v))', "Ok(Literal::F64(v))"),
 "parse-timestamp-fraction-rounded": (K, 'if f.len() > 6 {', 'if f.len() > 60 {'),
 "parse-date-day-not-validated": (K, "day < 1 || day > days_in_month(year, month)", "day < 1 || day > 31"),
 "parse-date-leap-rule-wrong": (K, "year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)", "year % 4 == 0"),
 # --- the exact row filter
 "filter-nan-not-equal-to-itself": (F, "(true, true) => Ordering::Equal,", "(true, true) => Ordering::Less,"),
 "filter-nan-below-numbers": (F, "(true, false) => Ordering::Greater,", "(true, false) => Ordering::Less,"),
 "filter-nan-not-above-infinity": (F, "(true, false) => Ordering::Greater,", "(true, false) => if b.is_infinite() { Ordering::Equal } else { Ordering::Greater },"),
 "filter-negative-zero-below-positive-zero": (F, "(false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),", "(false, false) => a.total_cmp(&b),"),
 "filter-decimal-literal-rounded-to-the-column-scale": (F, "prim::<Decimal64Type>(col, op, mask, |v| { cmp_decimal(i128::from(v), s, *unscaled, *scale) })",
   "prim::<Decimal64Type>(col, op, mask, |v| cmp_decimal(i128::from(v), s, *unscaled / 10i128.pow(u32::from(scale.saturating_sub(s))), (*scale).min(s)))"),
 "filter-decimal128-literal-rounded-to-the-column-scale": (F, "prim::<Decimal128Type>(col, op, mask, |v| cmp_decimal(v, s, *unscaled, *scale))",
   "prim::<Decimal128Type>(col, op, mask, |v| cmp_decimal(v, s, *unscaled / 10i128.pow(u32::from(scale.saturating_sub(s))), (*scale).min(s)))"),
 "filter-decimal-overflow-sign-flipped": (F, "if overflowed > 0 { Ordering::Greater } else { Ordering::Less }", "if overflowed > 0 { Ordering::Less } else { Ordering::Greater }"),
 "filter-decimal-rescale-wraps": (F, "10i128.checked_pow(u32::from(by)).and_then(|p| x.checked_mul(p))", "Some(x.wrapping_mul(10i128.wrapping_pow(u32::from(by))))"),
 "filter-null-rows-compare-true": (F, "*m = nulls.is_none_or(|n| n.is_valid(i)) && holds(op, ord(i));", "*m = holds(op, ord(i));"),
 "filter-is-null-inverted": (F, "*m = *m && nulls.as_ref().is_some_and(|n| n.is_null(i));", "*m = *m && nulls.as_ref().is_none_or(|n| n.is_valid(i));"),
 "filter-is-not-null-keeps-nulls": (F, "*m = *m && nulls.as_ref().is_none_or(|n| n.is_valid(i));", "*m = *m;"),
 "filter-text-compared-as-signed-bytes": (F, "apply(a.nulls(), mask, op, |i| { a.value(i).as_bytes().cmp(l.as_bytes()) });",
   "apply(a.nulls(), mask, op, |i| a.value(i).bytes().map(|b| b as i8).cmp(l.bytes().map(|b| b as i8)));"),
 "filter-binary-compared-as-signed-bytes": (F, "apply(a.nulls(), mask, op, |i| a.value(i).cmp(l.as_slice()));",
   "apply(a.nulls(), mask, op, |i| a.value(i).iter().map(|b| *b as i8).cmp(l.iter().map(|b| *b as i8)));"),
 "filter-dictionary-text-compared-as-signed-bytes": (F, ".map(|j| { values.is_valid(j) && holds(op, values.value(j).as_bytes().cmp(l.as_bytes())) })", ".map(|j| values.is_valid(j) && holds(op, values.value(j).bytes().map(|b| b as i8).cmp(l.bytes().map(|b| b as i8))))"),
 "filter-dictionary-table-one-short": (F, ".map(|j| { values.is_valid(j) && holds(op, values.value(j).as_bytes().cmp(l.as_bytes())) })", ".map(|j| values.is_valid(j) && holds(op, values.value(j).as_bytes().cmp(l.as_bytes())) && j + 1 < values.len())"),
 "filter-dictionary-key-under-null-slot-used": (F, "*m = keys.is_valid(i)\n                        && match", "*m = true\n                        && match"),
 "filter-terms-or-ed": (F, "Test::Cmp(op, lit) => eval_cmp(col.as_ref(), *op, lit, &mut mask)?,",
   "Test::Cmp(op, lit) => { let mut m2 = vec![true; mask.len()]; eval_cmp(col.as_ref(), *op, lit, &mut m2)?; for (a, b) in mask.iter_mut().zip(m2) { *a = *a || b; } }"),
 "filter-int-literal-cast-to-32-bits": (F, "(DataType::Int32, Literal::Int(l)) => { prim::<Int32Type>(col, op, mask, |v| i128::from(v).cmp(l)) }",
   "(DataType::Int32, Literal::Int(l)) => prim::<Int32Type>(col, op, mask, |v| i128::from(v).cmp(&i128::from(*l as i32))),"),
 "filter-int8-literal-cast-to-8-bits": (F, "(DataType::Int8, Literal::Int(l)) => { prim::<Int8Type>(col, op, mask, |v| i128::from(v).cmp(l)) }",
   "(DataType::Int8, Literal::Int(l)) => prim::<Int8Type>(col, op, mask, |v| i128::from(v).cmp(&i128::from(*l as i8))),"),
 "filter-unsigned-64-compared-as-signed": (F, "(DataType::UInt64, Literal::Int(l)) => { prim::<UInt64Type>(col, op, mask, |v| i128::from(v).cmp(l)) }",
   "(DataType::UInt64, Literal::Int(l)) => prim::<UInt64Type>(col, op, mask, |v| i128::from(v as i64).cmp(l)),"),
 "filter-millisecond-timestamps-not-scaled": (F, "prim::<TimestampMillisecondType>(col, op, mask, |v| { (i128::from(v) * 1000).cmp(&i128::from(*l)) })", "prim::<TimestampMillisecondType>(col, op, mask, |v| i128::from(v).cmp(&i128::from(*l)))"),
 "filter-float32-compared-against-narrowed-literal": (F, "let l = f64::from(*l);\n            prim::<Float32Type>", "let l = f64::from(*l as f32 + 1e-7);\n            prim::<Float32Type>"),
 "filter-boolean-order-reversed": (F, "apply(a.nulls(), mask, op, |i| a.value(i).cmp(l));\n            Ok(())\n        }\n        (DataType::Int8", "apply(a.nulls(), mask, op, |i| l.cmp(&a.value(i)));\n            Ok(())\n        }\n        (DataType::Int8"),
 # --- the reader and the plan
 "reader-pruning-never-applied": (R, "let skipped = if options.prune && pushed > 0 {", "let skipped = if false {"),
 "reader-second-term-not-used-for-pruning": (R, ".filter_map(|(t, k)| t.scan_term(*k))\n            .collect(),", ".filter_map(|(t, k)| t.scan_term(*k))\n            .take(1)\n            .collect(),"),
 "reader-skipped-row-groups-are-read": (R, ".filter(|(_, rg)| rg.skipped.is_none())", ".filter(|_| true)"),
 "reader-skips-the-last-row-group": (R, "f.row_groups\n                .iter()\n                .enumerate()", "f.row_groups\n                .iter()\n                .take(f.row_groups.len().saturating_sub(1))\n                .enumerate()"),
 "reader-skips-the-first-row-group": (R, "f.row_groups\n                .iter()\n                .enumerate()", "f.row_groups\n                .iter()\n                .skip(1)\n                .enumerate()"),
 "reader-row-groups-in-reverse-order": (R, "f.row_groups\n                .iter()\n                .enumerate()", "f.row_groups\n                .iter()\n                .enumerate()\n                .rev()"),
 "reader-reads-the-next-row-group": (R, ".with_row_groups(vec![rg])", ".with_row_groups(vec![rg + 1])"),
 "reader-row-group-numbers-off-by-one": (R, "let mut err = ScanError::new(kind).in_file(path);\n    if let Some(rg) = row_group {\n        err = err.in_row_group(rg);", "let mut err = ScanError::new(kind).in_file(path);\n    if let Some(rg) = row_group {\n        err = err.in_row_group(rg + 1);"),
 "reader-files-sorted-by-name": (R, "for path in paths {\n            let (file, p)", "let mut sorted: Vec<PathBuf> = paths.to_vec();\n        sorted.sort();\n        for path in &sorted {\n            let (file, p)"),
 "reader-repeated-file-scanned-once": (R, "for path in paths {\n            let (file, p)", "for path in paths {\n            if files.last().is_some_and(|f: &Arc<FileScan>| f.path == *path) {\n                continue;\n            }\n            let (file, p)"),
 "reader-filter-column-not-read-unless-projected": (R, "let indices: Vec<usize> = spec_ref\n        .read\n        .iter()", "let indices: Vec<usize> = spec_ref\n        .out\n        .iter()"),
 "reader-projects-before-filtering": (R, "let mask = filter::evaluate(&f.terms, &batch)", "let mask = filter::evaluate(&f.terms, &batch.project(&[0]).unwrap_or_else(|_| batch.clone()))"),
 "reader-later-files-not-checked-against-the-first": (R, "if have != *kind {", "if false {"),
 "reader-magic-numbers-not-checked": (R, "if &head != b\"PAR1\" {", "if false {"),
 "reader-no-error-on-wrong-types": (R, ".is_some_and(|c| c.data_type() == ty)", ".is_some_and(|c| c.data_type() == ty || true)"),
 "reader-limit-one-too-many": (R, "let mut remaining = limit;", "let mut remaining = limit.map(|l| l + 1);"),
 "reader-limit-zero-still-reads": (R, "if remaining == Some(0) {\n            break;\n        }\n        let out = task.run()?;", "let out = task.run()?;"),
 "report-skipped-counted-twice": (R, "if rg.skipped.is_some() {\n                    skipped += 1;", "if rg.skipped.is_some() {\n                    skipped += 2;"),
 "report-total-counts-only-kept-row-groups": (R, "for rg in &f.row_groups {\n                total += 1;", "for rg in &f.row_groups {\n                total += u64::from(rg.skipped.is_none());"),
 "report-read-count-not-incremented": (R, "report.row_groups_read += 1;", ""),
 "report-bytes-not-added": (R, "report.bytes_read += out.bytes_read;", ""),
 "report-bytes-of-chunk-reads-not-counted": (R, "let b = self.file.get_bytes(start, length)?;\n        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);", "let b = self.file.get_bytes(start, length)?;"),
 "report-rows-read-counts-returned": (R, "out.rows_read += batch.num_rows() as u64;", "out.rows_read += 0;"),
 "report-pruning-flag-always-applied": (R, "let applied = options.prune && pushed > 0;", "let applied = true;"),
 "report-not-read-counts-skipped-too": (R, "self.row_groups_total.saturating_sub(self.row_groups_skipped).saturating_sub(self.row_groups_read),", "self.row_groups_total.saturating_sub(self.row_groups_read),"),
 # --- the CSV text
 "csv-null-printed-empty": (C, 'None => buf.extend_from_slice(b"NULL"),', 'None => buf.extend_from_slice(b""),'),
 "csv-delete-character-not-quoted": (C, "b <= 31 || b == b'\"' || b == b'\\'' || b == b',' || b >= 127", "b <= 31 || b == b'\"' || b == b'\\'' || b == b',' || b > 127"),
 "csv-non-ascii-not-quoted": (C, "|| b == b',' || b >= 127", "|| b == b','"),
 "csv-embedded-quote-not-doubled": (C, "if b == b'\"' {\n                out.push(b'\"');\n            }", ""),
 "csv-float-positional-threshold-moved": (C, "if !(-4..16).contains(&e) {", "if !(-5..16).contains(&e) {"),
 "csv-negative-nan-printed-positive": (C, 'return if x.is_sign_negative() { "-nan".into() } else { "nan".into() };\n    }\n    if x.is_infinite() {\n        return if x < 0.0 { "-inf".into() } else { "inf".into() };\n    }\n    if x == 0.0 {\n        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };\n    }\n    let (digits, e) = shortest(&format!("{:e}", x.abs()));\n    layout(x < 0.0, &digits, e)\n}\n\npub fn format_f32', 'return "nan".into();\n    }\n    if x.is_infinite() {\n        return if x < 0.0 { "-inf".into() } else { "inf".into() };\n    }\n    if x == 0.0 {\n        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };\n    }\n    let (digits, e) = shortest(&format!("{:e}", x.abs()));\n    layout(x < 0.0, &digits, e)\n}\n\npub fn format_f32'),
 "csv-negative-zero-printed-positive": (C, 'return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };\n    }\n    let (digits, e) = shortest(&format!("{:e}", x.abs()));\n    layout(x < 0.0, &digits, e)\n}\n\npub fn format_f32', 'return "0.0".into();\n    }\n    let (digits, e) = shortest(&format!("{:e}", x.abs()));\n    layout(x < 0.0, &digits, e)\n}\n\npub fn format_f32'),
 "csv-decimal-scale-dropped": (C, "if scale > 0 {\n        while digits.len() <= scale {", "if scale > 100 {\n        while digits.len() <= scale {"),
 "csv-blob-quote-not-escaped": (C, "if (32..=126).contains(&b) && b != b'\"' && b != b'\\'' && b != b'\\\\' {", "if (32..=126).contains(&b) && b != b'\\'' && b != b'\\\\' {"),
 "csv-blob-escape-lowercase": (C, 's.push_str(&format!("\\\\x{b:02X}"));', 's.push_str(&format!("\\\\x{b:02x}"));'),
 "csv-timestamp-fraction-zeros-kept": (K, "out.push_str(f.trim_end_matches('0'));", "out.push_str(&f);"),
 "csv-date-before-year-1-without-bc": (K, 'format!("{:04}-{m:02}-{d:02} (BC)", 1 - y)', 'format!("{:04}-{m:02}-{d:02}", 1 - y)'),
 # --- errors
 "error-message-not-redacted": (E, "if c == '\\'' || c == '\"' || c == '`' {\n                    quote = Some(c);\n                } else if !c.is_control() {\n                    out.push(c);\n                }", "if !c.is_control() {\n                    out.push(c);\n                }"),
 "error-without-row-group": (E, "self.row_group = Some(row_group);", "self.row_group = None;"),
 "error-without-file-name": (E, "self.file = Some(file.to_path_buf());", "self.file = None;"),
 "error-not-supported-without-the-words": (E, 'ErrorKind::NotSupported(what) => write!(f, "not supported yet: {what}"),', 'ErrorKind::NotSupported(what) => write!(f, "unsupported: {what}"),'),
 # --- the build configuration (ADR 0003 decision 6): dependencies built WITH overflow checks panic in arrow-data
 "build-dependencies-overflow-checks-on-in-release": (ROOTTOML, '[profile.release.package."*"]\noverflow-checks = false', '[profile.release.package."*"]\noverflow-checks = true'),
 "build-workspace-overflow-checks-off-in-release": (ROOTTOML, "[profile.release]\noverflow-checks = true", "[profile.release]\noverflow-checks = false"),
}


def sha_tree():
    h = hashlib.sha256()
    for base in (HERE, os.path.join(ROOT, "Cargo.toml")):
        if os.path.isfile(base):
            h.update(open(base, "rb").read())
            continue
        for d, _, fs in sorted(os.walk(base)):
            if "target" in d.split(os.sep) or "mutwork" in d:
                continue
            for f in sorted(fs):
                if f.endswith((".rs", ".toml", ".py", ".md")):
                    h.update(open(os.path.join(d, f), "rb").read())
    return h.hexdigest()


def stage(work, name):
    """A private workspace: the real root manifest (lints, release profile) with only the two crates as members, and a copy of each."""
    base = os.path.join(work, name)
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(os.path.join(base, "crates"))
    root = open(os.path.join(ROOT, "Cargo.toml")).read()
    root = re.sub(r"members = \[[^\]]*\]", 'members = ["crates/nata-parquet", "crates/nata-scan"]', root)
    root = re.sub(r"exclude = \[[^\]]*\]\n", "", root)
    open(os.path.join(base, "Cargo.toml"), "w").write(root)
    for crate, parts in (("nata-parquet", ["src", "tests", "Cargo.toml"]), ("nata-scan", ["src", "Cargo.toml"])):
        src_root = os.path.join(ROOT, "crates", crate)
        dst = os.path.join(base, "crates", crate)
        os.makedirs(dst)
        for x in parts:
            s = os.path.join(src_root, x)
            if os.path.isdir(s):
                shutil.copytree(s, os.path.join(dst, x))
            else:
                shutil.copy(s, os.path.join(dst, x))
    # Cargo.lock: the real one, so that --offline resolves the same versions
    shutil.copy(os.path.join(ROOT, "Cargo.lock"), os.path.join(base, "Cargo.lock"))
    return base, os.path.join(base, "crates", "nata-parquet")


def cargo(base, work, name, args):
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(work, "target-" + name), CARGO_TERM_COLOR="never",
               NATA_TPCH_DIR=os.path.join(ROOT, "data", "tpch-sf0.01"), NATA_REQUIRE_DUCKDB="1")
    p = subprocess.run(["cargo"] + args, cwd=base, env=env, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def run_groups(base, work, slot):
    """Run the test groups in order; return (all_passed, output of the first failing group, its name)."""
    for g in GROUPS:
        rc, out = cargo(base, work, slot, ["test", "--release", "--offline", "-p", "nata-parquet"] + g)
        if rc != 0:
            return False, out, " ".join(g)
    return True, "", ""


def run_one(args):
    n, work, dry, slot = args
    edits = M[n] if isinstance(M[n], list) else [M[n]]
    slot_name = "slot%d" % slot
    base, dst = stage(work, slot_name)
    for f, old, new in edits:
        p = os.path.join(base, "Cargo.toml") if f.startswith("ROOT:") else os.path.join(dst, f)
        t = open(p).read()
        rx = re.compile(r"\s*".join(re.escape(c) for c in old if not c.isspace()))
        found = list(rx.finditer(t))
        if len(found) != 1:
            return n, "INVALID", "edit applies %d times" % len(found)
        if re.sub(r"\s+", "", old) == re.sub(r"\s+", "", new):
            return n, "INVALID", "edit changes nothing"
        open(p, "w").write(t[: found[0].start()] + new + t[found[0].end():])
    if dry:
        rc, out = cargo(base, work, slot_name, ["check", "--release", "--offline", "-p", "nata-parquet", "--tests"])
        if rc != 0:
            return n, "INVALID", "does not compile: " + " ".join(l for l in out.splitlines() if l.startswith("error"))[:300]
        return n, "compiles", ""
    # compile first, so that a compile error is INVALID and not a (false) catch
    rc, out = cargo(base, work, slot_name, ["test", "--release", "--offline", "-p", "nata-parquet", "--no-run"])
    if rc != 0:
        return n, "INVALID", "does not compile: " + " ".join(l for l in out.splitlines() if l.startswith("error"))[:300]
    ok, out, group = run_groups(base, work, slot_name)
    if ok:
        return n, "SURVIVED", "the tests cannot see this bug"
    failed = sorted(set(l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")))
    return n, "CAUGHT", "[%s] %s" % (group, ", ".join(failed[:4]) or "a failing test (panic)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("names", nargs="*")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--dry", action="store_true")
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--work", default=os.path.expanduser("~/nata-parquet-mutwork"))
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.list:
        print("\n".join(M))
        return 0
    for n in a.names:
        if n not in M:
            print("unknown mutant", n)
            return 2
    os.makedirs(a.work, exist_ok=True)
    before = sha_tree()
    t0 = time.time()
    if not a.dry:
        base, _ = stage(a.work, "baseline")
        ok, out, group = run_groups(base, a.work, "baseline")
        if not ok:
            print("BASELINE FAILED in group %s\n%s" % (group, out[-3000:]))
            return 1
        print("baseline ok (%.0fs)" % (time.time() - t0), flush=True)
    names = a.names or list(M)
    results = {}
    slots = [[n for i, n in enumerate(names) if i % a.jobs == s] for s in range(a.jobs)]

    def worker(s):
        res = []
        for n in slots[s]:
            r = run_one((n, a.work, a.dry, s))
            print("%-9s %s: %s" % (r[1], r[0], r[2]), flush=True)
            res.append(r)
        return res

    with concurrent.futures.ThreadPoolExecutor(a.jobs) as ex:
        for res in ex.map(worker, range(a.jobs)):
            for n, status, detail in res:
                results[n] = (status, detail)
    lines = ["%-9s %s: %s" % (results[n][0], n, results[n][1]) for n in names]
    print("\n".join(lines))
    counts = {k: sum(1 for s, _ in results.values() if s == k) for k in ("CAUGHT", "SURVIVED", "INVALID", "compiles")}
    print("\nmutants: %d; %s; %.0fs" % (len(names), ", ".join("%s %d" % (k, v) for k, v in counts.items() if v), time.time() - t0))
    if a.out:
        open(a.out, "w").write("\n".join(lines) + "\n")
    if sha_tree() != before:
        print("THE REAL TREE CHANGED DURING THE RUN")
        return 1
    return 0 if counts["CAUGHT"] == len(names) or a.dry and counts["compiles"] == len(names) else 1


sys.exit(main())
