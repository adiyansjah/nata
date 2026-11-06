# SQL parser bake-off: `sqlparser` against a hand-written parser (rule R1 of card 0002)

Date: 2026-11-06. Machine: the dev box (8 vCPU x86-64, Linux 6.12, Rust 1.99.1, DuckDB 2.0.0, cargo-deny 0.20.2), local only, no GCP. The load on the box was not controlled (a load average of about 1 from other work).
Card: `docs/slices/0002-select-where-limit.md`, part 3 (rule R1) and part 4 (this spike). Code: `spikes/sql-parser/` (throwaway; its own Cargo workspace; deleted when ADR 0007 is accepted, this file stays). Reproduce: `spikes/sql-parser/corpus/run-all.sh`.

## Question and verdict

Question: does the `sqlparser` crate meet R1 (i) to (iv) for the slice-4 subset, or must we write our own parser?

**Verdict: R1 is not triggered. Reuse `sqlparser`.** None of the four criteria failed, on these conditions: `default-features = false, features = ["std"]` (without the crate's `recursive-protection` feature), the PostgreSQL dialect (not the generic one), a conversion layer that refuses what the subset does not have (about 570 to 860 lines), a token-level depth guard in front of it (about 70 lines), and a parse thread with at least 8 MB of stack (see "Close calls").

| R1 criterion | Result | Decided by |
|---|---|---|
| (i) parses every query of the test set and the 22 TPC-H queries | **passes.** All 230 in-subset test-set queries and all 10,000 generated ones parse to the same AST as the hand parser in the PostgreSQL dialect; the crate alone parses 22 of 22 TPC-H queries in the generic dialect (the conversion layer then refuses all 22 with a position, as the subset must). The generic dialect gets 2 of 230 and 12 of 10,000 wrong (BETWEEN precedence) and accepts a trailing comma. | tables (a), (b), (c) below |
| (ii) keeps the text of numbers | **passes.** 47 of 47 literals keep their exact text in all three crate configurations, including `1e400`, 38-digit and 30-digit decimals, `4.9e-324`, `-0.0`. | table (ii) |
| (iii) line and column of an error | **passes, narrowly.** The crate puts the position in the message text, not in a field. It has none for an error at the end of the input (0.5% of the errors here: `Expected: an expression, found: EOF`); a 6-line rule in the layer reports the end of the text there. With it 100% of 6,404 to 6,421 errors carry line and column. | table (iii) |
| (iv) `unsafe` or a license that cargo-deny refuses | **passes for the crate; fails for its default feature set.** `cargo deny check`: advisories, bans, licenses, sources all ok. `sqlparser` itself has 0 `unsafe`. Its default `recursive-protection` feature adds `stacker`, `psm` (assembly) and `object`: 117 `unsafe` keywords in 3 crates that the main workspace does not have, and a C compiler at build time. Without the feature there is no new `unsafe`. | section (iv) |

Close calls the owner should see (these are costs, not R1 failures):

1. **Deep nesting needs either `unsafe` or a big stack.** Without `stacker`, the crate needs 6 MB of stack for 1,000 nested parentheses and 8 MB for 1,000 nested `CASE`, and aborts at a 2 MB stack (the size of a tokio worker thread). With `stacker` it needs 64 to 128 KB for the parse, but then the `unsafe` of criterion (iv) is in the tree. The card's "1,000 deep parses, deeper is an error" therefore means: parse on a dedicated thread with a stack of 16 MB (8 MB is the measured minimum for `CASE`, so no margin), and keep the guard below.
2. **The crate has no bound on the length of an operator chain.** A 250,000-term `a AND a AND ...` (1.5 MB) aborts the process (stack overflow when the tree is dropped), even with `stacker`. The guard fixes that; without it the crate alone fails the card's "no stack overflow" row.
3. **The AST hides some syntax**, so the layer cannot refuse it: `LIMIT ALL` is dropped (the query is accepted as unlimited; a test pins this), and the generic dialect accepts `SELECT a, FROM ...`. The PostgreSQL dialect fixes the second.
4. **Cost to own, for the owner's information** (rule says reuse). The hand parser is 980 lines of code (150-column rustfmt; 1,255 lines at rustfmt's default width), including a 112-word keyword list and the lexer, with no dependency and no `unsafe`; the conversion layer plus guard is 577 lines (857 at the default width) plus the crate. The hand parser is 10 times faster, gives better error positions, adds 0.5 MB to a binary where the crate adds 7.1 to 7.3 MB (stripped), builds in 1 s instead of 32 to 41 s, and its stack need is 1.5 to 2 MB for 1,000 nested parentheses or `CASE` (stack frames shrunk once; a thin margin at 2 MB). It also needed more fixes during the spike (5 bugs found by the corpora, listed below) than the layer did (3). If the owner prefers it, the way back is cheap: the AST boundary and the `sqlparser` test oracle are already here.

## What was built and measured

- `src/ast.rs`: the AST of the subset (queries, `*`, expressions with `AS` aliases, `DATE '...'`, `CASE`, `CAST` to 9 types, `BETWEEN`, `IN (list)`, `IS [NOT] NULL`). Numbers are kept as text. Equality ignores spans.
- `src/hand.rs` (arm B): lexer, precedence-climbing parser, byte spans on expressions, line and column on every error, `not supported yet: <what>` for what is recognisably outside the subset, a depth limit of 1,024 (1,000 plus a margin) that counts parentheses, `NOT`, unary minus, `CASE`, and the length of operator chains, 112 reserved words taken from DuckDB 2.0.0's `duckdb_keywords()`.
- `src/sqlp.rs` (arm A): `sqlparser` 0.63.0 (latest on crates.io on 2026-11-06), GenericDialect and PostgreSqlDialect, the conversion (every field of `Query` and `Select` is destructured so a new field in a later release is a compile error), the end-of-input rule, the depth guard.
- Not in the subset on purpose (both arms refuse): `TRUE`/`FALSE` literals, unary `+`, `::` casts, `FROM`-less `SELECT`, implicit aliases, `OFFSET` before or without `LIMIT`. The existing `check/queries/decimal` files use `::` and no `FROM`: they are outside this subset, which `docs/dialect.md` should state.
- Strictness chosen to match DuckDB where DuckDB is strict: comparisons do not chain (`a = b = c` is an error in DuckDB 2.0.0), `BETWEEN`/`IN` do not chain, the 112 reserved words cannot be unquoted names, `a = b BETWEEN 1 AND 2` is `a = (b BETWEEN 1 AND 2)`.

### Corpora

| corpus | size | how made | seed |
|---|---|---|---|
| (a) test set | 382: 230 in the subset, 113 outside it or invalid, 39 dialect edge cases | written by hand from the card (`corpus/testset.py`) | - |
| (b) TPC-H | 22 | DuckDB 2.0.0 `tpch_queries()` | - |
| (c) generated | 10000 = 5000 trees x 2 renderings | sqlfuzz `gen.expr`/`gen.boolean` rejection-sampled to the subset (`corpus/gen_fuzz.py`) | 20261106 |
| (d) mangled | 10000 | one edit (delete, swap, insert) of the 230 in-subset queries of (a) (`corpus/gen_mangle.py`) | 20261106 |

All three generators are seeded; query *i* of the generated corpus is `random.Random(seed * 1_000_003 + i)`, mangle *i* is `random.Random(seed * 7_919 + i)`. The generated queries come from the sqlfuzz generator (`gen.expr`, `gen.boolean`, its data pools), rejection-sampled to the subset, and each tree is rendered twice: as sqlfuzz does (every operator parenthesised) and with the fewest parentheses SQL precedence allows, so that the precedence rules are exercised and each tree gives a cross-check (both renderings must give one AST).

## (i) Acceptance

#### (a) test set: acceptance, same AST

| class | arm | queries | accepted / refused | same AST as hand |
|---|---|---|---|---|
| accept | hand | 230 | 230 | 230 |
| accept | a-generic | 230 | 230 | 228 |
| accept | a-postgres | 230 | 230 | 230 |
| refuse | hand | 113 | 113 refused (84 as `not supported yet`) | - |
| refuse | a-generic | 113 | 111 refused (88 as `not supported yet`) | - |
| refuse | a-postgres | 113 | 112 refused (83 as `not supported yet`) | - |
| edge | hand | 39 | 17 accepted | 39 same outcome and AST as hand |
| edge | a-generic | 39 | 17 accepted | 35 same outcome and AST as hand |
| edge | a-postgres | 39 | 19 accepted | 35 same outcome and AST as hand |

Differences from the expectation or from the hand arm (a):

| arm | id | tag | query | arm says | hand says |
|---|---|---|---|---|---|
| a-generic | t0134 | select-item | `SELECT a = b BETWEEN 1 AND 2 AS r FROM 'f.parquet'` | (query [(between (= (id a) (id b)) (num 1) (num 2)) as r] from "f.parquet") | (query [(= (id a) (between (id b) (num 1) (num 2))) as r] from "f.parquet") |
| a-generic | t0211 | where | `SELECT * FROM 'f.parquet' WHERE a = b BETWEEN 1 AND 2` | (query * from "f.parquet" where (between (= (id a) (id b)) (num 1) (num 2))) | (query * from "f.parquet" where (= (id a) (between (id b) (num 1) (num 2)))) |
| a-generic | t0287 | limit-all | `SELECT a FROM 'f.parquet' LIMIT ALL` | (query [(id a)] from "f.parquet") | - |
| a-generic | t0340 | trailing-comma | `SELECT a, FROM 'f.parquet'` | (query [(id a)] from "f.parquet") | - |
| a-postgres | t0287 | limit-all | `SELECT a FROM 'f.parquet' LIMIT ALL` | (query [(id a)] from "f.parquet") | - |

(`edge` = 39 dialect corner cases with no fixed expectation; the arms agree on 35 to 39 of them, the disagreements are the ones listed.)

#### (b) TPC-H, 22 queries

| Q | hand | sqlparser + layer (generic) | sqlparser alone (generic) |
|---|---|---|---|
| 1 | unsupported at 4:5: function calls (sum(...)) | unsupported at 20:5: ORDER BY | parses |
| 2 | unsupported at 11:5: table names (only a quoted path or glob, FROM 'fi... | unsupported at 39:5: ORDER BY | parses |
| 3 | unsupported at 3:5: function calls (sum(...)) | unsupported at 21:5: ORDER BY | parses |
| 4 | unsupported at 3:5: function calls (count(...)) | unsupported at 20:5: ORDER BY | parses |
| 5 | unsupported at 3:5: function calls (sum(...)) | unsupported at 24:5: ORDER BY | parses |
| 6 | unsupported at 2:5: function calls (sum(...)) | unsupported at 2:5: function calls (sum(...)) | parses |
| 7 | unsupported at 5:5: function calls (sum(...)) | unsupported at 36:5: ORDER BY | parses |
| 8 | unsupported at 3:5: function calls (sum(...)) | unsupported at 38:5: ORDER BY | parses |
| 9 | unsupported at 4:5: function calls (sum(...)) | unsupported at 29:5: ORDER BY | parses |
| 10 | unsupported at 4:5: function calls (sum(...)) | unsupported at 31:5: ORDER BY | parses |
| 11 | unsupported at 3:5: function calls (sum(...)) | unsupported at 27:5: ORDER BY | parses |
| 12 | unsupported at 3:5: function calls (sum(...)) | unsupported at 30:5: ORDER BY | parses |
| 13 | unsupported at 3:5: function calls (count(...)) | unsupported at 18:5: ORDER BY | parses |
| 14 | unsupported at 2:14: function calls (sum(...)) | unsupported at 2:14: function calls (sum(...)) | parses |
| 15 | unsupported at 1:1: WITH (common table expressions) | unsupported at 1:1: WITH (common table expressions) | parses |
| 16 | unsupported at 5:5: function calls (count(...)) | unsupported at 26:5: ORDER BY | parses |
| 17 | unsupported at 2:5: function calls (sum(...)) | unsupported at 2:5: function calls (sum(...)) | parses |
| 18 | unsupported at 7:5: function calls (sum(...)) | unsupported at 31:5: ORDER BY | parses |
| 19 | unsupported at 2:5: function calls (sum(...)) | unsupported at 2:5: function calls (sum(...)) | parses |
| 20 | unsupported at 5:5: table names (only a quoted path or glob, FROM 'fi... | unsupported at 34:13: ORDER BY | parses |
| 21 | unsupported at 3:5: function calls (count(...)) | unsupported at 36:5: ORDER BY | parses |
| 22 | unsupported at 3:5: function calls (count(...)) | unsupported at 31:5: ORDER BY | parses |

refused: hand 22/22, A-generic 22/22, A-postgres 22/22; parsed by sqlparser alone (generic): 22/22

#### (c) generated queries

| arm | accepted | same AST as hand | both renderings of a tree give one AST |
|---|---|---|---|
| hand | 10000 of 10000 (100.0%) | 10000 of 10000 (100.0%) | 5000 of 5000 (100.0%) |
| a-generic | 10000 of 10000 (100.0%) | 9988 of 10000 (99.9%) | 4988 of 5000 (99.8%) |
| a-postgres | 10000 of 10000 (100.0%) | 10000 of 10000 (100.0%) | 5000 of 5000 (100.0%) |

A-generic differs from hand on 12 queries; the first 4:

- `SELECT * FROM 'data/*.parquet' WHERE f <= id IN (-1, 4, 2147483647)`
  - hand: `(query * from "data/*.parquet" where (<= (id f) (in (id id) (neg (num 1)) (num 4) (num 2147483647))))`
  - generic: `(query * from "data/*.parquet" where (in (<= (id f) (id id)) (neg (num 1)) (num 4) (num 2147483647)))`
- `SELECT\t(b + b * 4294967296) % - (b % b) AS "C 0", dt\tFROM '/tmp/x y/part-0.parquet'\tWHERE (a <= a OR dt < dt) AND a NOT IN (-1, 6) <= a NOT IN (...`
  - hand: `(query [(% (+ (id b) (* (id b) (num 4294967296))) (neg (% (id b) (id b)))) as q:C 0] [(id dt)] from "/tmp/x y/part-0.parquet" where (and (or (<= (id a) (id a)) (< (id dt) (id dt))) (<= (not-in (id a) (neg (num 1)) (num 6)) (not-in (id a) (neg (num 11))))) limit 18446744073709551615 offset 0)`
  - generic: `(query [(% (+ (id b) (* (id b) (num 4294967296))) (neg (% (id b) (id b)))) as q:C 0] [(id dt)] from "/tmp/x y/part-0.parquet" where (and (or (<= (id a) (id a)) (< (id dt) (id dt))) (not-in (<= (not-in (id a) (neg (num 1)) (num 6)) (id a)) (neg (num 11)))) limit 18446744073709551615 offset 0)`
- `Select\na As c0, CAST(NOT s < s AS VARCHAR) As c1\nFrom '/tmp/x y/part-0.parquet'\nWhere (- c >= 1.5 % c) <> a IN (2)`
  - hand: `(query [(id a) as c0] [(cast VARCHAR (not (< (id s) (id s)))) as c1] from "/tmp/x y/part-0.parquet" where (<> (>= (neg (id c)) (% (num 1.5) (id c))) (in (id a) (num 2))))`
  - generic: `(query [(id a) as c0] [(cast VARCHAR (not (< (id s) (id s)))) as c1] from "/tmp/x y/part-0.parquet" where (in (<> (>= (neg (id c)) (% (num 1.5) (id c))) (id a)) (num 2)))`
- `select c, f < d BETWEEN -3.25 AND 1.50, c from '/tmp/x y/part-0.parquet' where c IS NOT NULL AND dt < dt limit 2`
  - hand: `(query [(id c)] [(< (id f) (between (id d) (neg (num 3.25)) (num 1.50)))] [(id c)] from "/tmp/x y/part-0.parquet" where (and (is-not-null (id c)) (< (id dt) (id dt))) limit 2)`
  - generic: `(query [(id c)] [(between (< (id f) (id d)) (neg (num 3.25)) (num 1.50))] [(id c)] from "/tmp/x y/part-0.parquet" where (and (is-not-null (id c)) (< (id dt) (id dt))) limit 2)`

#### agreement of the accept/reject decision with DuckDB's parser

| corpus | arm | queries | agree | ours stricter: not supported yet | ours stricter: syntax rule | arm accepts, DuckDB refuses |
|---|---|---|---|---|---|---|
| (a) test set | hand | 382 | 273 | 84 | 25 | 0 |
| (a) test set | a-generic | 382 | 275 | 96 | 11 | 0 |
| (a) test set | a-postgres | 382 | 276 | 90 | 16 | 0 |
| (c) generated | hand | 10000 | 10000 | 0 | 0 | 0 |
| (c) generated | a-generic | 10000 | 10000 | 0 | 0 | 0 |
| (c) generated | a-postgres | 10000 | 10000 | 0 | 0 | 0 |
| (d) mangled | hand | 10000 | 9356 | 475 | 169 | 0 |
| (d) mangled | a-generic | 10000 | 9384 | 481 | 135 | 0 |
| (d) mangled | a-postgres | 10000 | 9364 | 519 | 116 | 1 |

Classes of the disagreements (every one examined; the class is a rule on the query text, the hand message and DuckDB's own tree):

| corpus | arm | disagreement | reason | n | example |
|---|---|---|---|---|---|
| (a) test set | hand | ours stricter (not supported yet) | outside the subset by design (a `not supported yet` refusal) | 84 | `SELECT count(*) FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | DuckDB laxer: a number followed by letters is a number and a name | 4 | `SELECT 1a FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 3 | `SELECT DATE '2020-01-01' + INTERVAL 1 DAY FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 3 | `SELECT * FROM 'f.parquet' WHERE a IN ()` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 2 | `SELECT a FROM 'f.parquet' LIMIT 1 + 1` |
| (a) test set | hand | ours stricter (syntax rule) | DuckDB laxer: any run of operator characters is an operator | 2 | `SELECT a ->> 'b' FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 2 | `` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `SELECT a FROM 'f.parquet' LIMIT 1.5` |
| (a) test set | hand | ours stricter (syntax rule) | DuckDB laxer: the size of a LIMIT is not a parse matter | 1 | `SELECT a FROM 'f.parquet' LIMIT 18446744073709551616` |
| (a) test set | hand | ours stricter (syntax rule) | DuckDB laxer: any name before a string is a typed literal | 1 | `SELECT CAST(a AS INTEGER[]) FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `SELECT a AS from FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | DuckDB laxer: trailing comma | 1 | `SELECT a, FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `SELECT * FROM 'f.parquet' WHERE a IN (1) IN (2)` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `SELECT a AS 'x' FROM 'f.parquet'` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `;` |
| (a) test set | hand | ours stricter (syntax rule) | other (see the list of the test set) | 1 | `SELECT a FROM 'f.parquet' LIMIT 1e3` |
| (d) mangled | a-postgres | hand accepts, DuckDB refuses | arm laxer: DuckDB says syntax error at or near "L" | 1 | `SELECT a IN (1, NUL L) AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (not supported yet) | outside the subset by design (a `not supported yet` refusal) | 475 | `SELECT a + b + c + dAS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: a number followed by letters is a number and a name | 47 | `SELECT * FROM 'f.parquet' WHERE a NOT BETWEEN 1 AND 2e` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: any run of operator characters is an operator | 40 | `SELECT * FROM 'f.parquet' WHERE CASE WHEN a IS NULL THEN ...` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: any name before a string is a typed literal | 31 | `SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS ...` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: trailing comma | 12 | `SELECT a IN (1, 2, ) AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: ISNULL / NOTNULL | 10 | `SELECT a + 1 ISNULL AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | other: expected an expression, found '..' | 9 | `SELECT a + 1 AS a1, a * 2 AS a2, /a / 2 AS a3 FROM 'a.par...` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: type modifiers are any list | 4 | `SELECT CAST(a AS DECIMAL(-10)) AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | other: expected '..' after IN, found '..' | 3 | `SELECT a IN z(1) AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | other: expected FROM, found '..' | 3 | `SELECT a AS "Quoted Alias",  bAS "x""y" FROM 'a.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: text after a semicolon | 3 | `SELECT * FROM 'a.parquet' WHERE a = 1;7` |
| (d) mangled | hand | ours stricter (syntax rule) | DuckDB laxer: the size of a LIMIT is not a parse matter | 2 | `SELECT * FROM 'a.parquet' LIMIT 18446744073709551651` |
| (d) mangled | hand | ours stricter (syntax rule) | other: expected '..' or '..' in the IN list, found ' | 2 | `SELECT a IN (DATE b'2020-01-01') AS r FROM 'f.parquet'` |
| (d) mangled | hand | ours stricter (syntax rule) | other: unexpected '..' after the end of the query | 1 | `SELECT * FROM 'nation.parquet' WHERE n_commentIS NOT NULL...` |
| (d) mangled | hand | ours stricter (syntax rule) | other: expected FROM, found '..'1994-12-31'..' | 1 | `SELECT d BETWEEN DATE '1994-01-01' AND DATE😀 '1994-12-31'...` |
| (d) mangled | hand | ours stricter (syntax rule) | other: LIMIT needs a whole number, found 5. | 1 | `SELECT * FROM 'a.parquet' LIMIT 5.;` |

How to read the classes: "ours stricter: not supported yet" is a refusal the subset requires. "ours stricter: syntax rule" are queries DuckDB's parser takes and our grammar does not; every one was assigned a reason by a rule on the query text and on DuckDB's own parse tree (the "DuckDB laxer" lines: DuckDB reads `2e` as the number 2 and the name `e`, takes any run of operator characters as an operator including prefix ones, takes any name in front of a string as a typed literal, accepts trailing commas, `ISNULL`, text after a `;`, any list as type modifiers, and does not check the size of a `LIMIT` in the parser). The remaining lines marked "other" in the hand arm (about 20 of 10,000) are the same family: a prefix operator (`/a`), a function-style call after `IN` (`a IN z(1)`), a name in front of a quoted name; none is a case where our grammar is wrong. "arm accepts, DuckDB refuses" is 0 for the hand parser and 1 for `sqlparser` (PostgreSQL): it treats a no-break space inside a name as a name character, DuckDB as white space.

**Disagreements that were real bugs, found by these corpora and fixed in the spike** (each is now a test or covered by one):

| arm | bug | found by | fix |
|---|---|---|---|
| hand | `NOT NOT a` was refused (the operand of `NOT` was parsed above `NOT`'s own level) | (a) test set, AST comparison | `binary(3)` |
| hand | **a panic** on a non-ASCII character right after a number (`1日`): a string slice at a character boundary | (d) mangled queries | slice by character |
| hand | a no-break space (and the other Unicode spaces DuckDB treats as white space) was a name character | (d), DuckDB comparison; the exact set found by probing DuckDB | `is_unicode_space` |
| hand | a stray word after `AS r` was reported as "alias without AS" instead of "expected FROM" (a misleading message) | (d) message sample | alias check |
| hand | stack overflow at a 2 MB stack for 1,000 nested `CASE` | robustness | frames shrunk (errors boxed, cold paths out of line); now passes with little margin |
| A | accepted an empty select list (`SELECT  FROM ...`) and an empty quoted name (`""`) | (a), (d), DuckDB comparison | two checks in the layer |
| A | aborted the process on a 250,000-term operator chain | robustness | the depth guard |
| A | some "alias without AS" refusals had no position (the crate gives those aliases an empty span) | (d) | fall back to the expression's span |

Not fixable from the crate's AST, kept as known differences (pinned by a test): `LIMIT ALL` accepted; in the generic dialect `a = b BETWEEN 1 AND 2` is `(a = b) BETWEEN 1 AND 2`, where PostgreSQL and DuckDB read `a = (b BETWEEN 1 AND 2)`, and a trailing comma is accepted.

## (ii) Literal fidelity

### (ii) literal fidelity: the text of every numeric literal through each arm

| literal | hand | sqlparser generic | sqlparser postgres | mutant: through f64 | DuckDB's serialized literal text |
|---|---|---|---|---|---|
| `0` | same text | same text | same text | same text | 0 |
| `1` | same text | same text | same text | same text | 1 |
| `0.1` | same text | same text | same text | same text | 0.1 |
| `0.10` | same text | same text | same text | CHANGED | 0.10 |
| `0.0` | same text | same text | same text | CHANGED | 0.0 |
| `00012` | same text | same text | same text | CHANGED | 00012 |
| `1.` | same text | same text | same text | CHANGED | 1. |
| `.5` | same text | same text | same text | CHANGED | .5 |
| `123456789012345678901234567890.12` | same text | same text | same text | CHANGED | 123456789012345678901234567890.12 |
| `99999999999999999999999999999999999999` | same text | same text | same text | CHANGED | 99999999999999999999999999999999999999 |
| `0.000000000000000000000000000000000001` | same text | same text | same text | same text | 0.000000000000000000000000000000000001 |
| `1e400` | same text | same text | same text | CHANGED | 1e400 |
| `1E-400` | same text | same text | same text | CHANGED | 1E-400 |
| `1.5e+3` | same text | same text | same text | CHANGED | 1.5e+3 |
| `2.5E3` | same text | same text | same text | CHANGED | 2.5E3 |
| `1e0` | same text | same text | same text | CHANGED | 1e0 |
| `4.9e-324` | same text | same text | same text | CHANGED | 4.9e-324 |
| `1.7976931348623157e308` | same text | same text | same text | CHANGED | 1.7976931348623157e308 |
| `9007199254740993` | same text | same text | same text | CHANGED | 9007199254740993 |
| `18446744073709551616` | same text | same text | same text | CHANGED | 18446744073709551616 |
| `123456789.123456789123456789e-10` | same text | same text | same text | CHANGED | 123456789.123456789123456789e-10 |
| `0.30000000000000004` | same text | same text | same text | same text | 0.30000000000000004 |
| `100000000000000000000.5e-7` | same text | same text | same text | CHANGED | 100000000000000000000.5e-7 |
| `-0` | same text | same text | same text | same text | -0 |
| `-1` | same text | same text | same text | same text | -1 |
| `-0.1` | same text | same text | same text | same text | -0.1 |
| `-0.10` | same text | same text | same text | CHANGED | -0.10 |
| `-0.0` | same text | same text | same text | CHANGED | -0.0 |
| `-00012` | same text | same text | same text | CHANGED | -00012 |
| `-1.` | same text | same text | same text | CHANGED | -1. |
| `-.5` | same text | same text | same text | CHANGED | -.5 |
| `-123456789012345678901234567890.12` | same text | same text | same text | CHANGED | -123456789012345678901234567890.12 |
| `-99999999999999999999999999999999999999` | same text | same text | same text | CHANGED | -99999999999999999999999999999999999999 |
| `-0.000000000000000000000000000000000001` | same text | same text | same text | same text | -0.000000000000000000000000000000000001 |
| `-1e400` | same text | same text | same text | CHANGED | -1e400 |
| `-1E-400` | same text | same text | same text | CHANGED | -1E-400 |
| `-1.5e+3` | same text | same text | same text | CHANGED | -1.5e+3 |
| `-2.5E3` | same text | same text | same text | CHANGED | -2.5E3 |
| `-1e0` | same text | same text | same text | CHANGED | -1e0 |
| `-4.9e-324` | same text | same text | same text | CHANGED | -4.9e-324 |
| `-1.7976931348623157e308` | same text | same text | same text | CHANGED | -1.7976931348623157e308 |
| `-9007199254740993` | same text | same text | same text | CHANGED | -9007199254740993 |
| `-18446744073709551616` | same text | same text | same text | CHANGED | -18446744073709551616 |
| `-123456789.123456789123456789e-10` | same text | same text | same text | CHANGED | -123456789.123456789123456789e-10 |
| `-0.30000000000000004` | same text | same text | same text | same text | -0.30000000000000004 |
| `-100000000000000000000.5e-7` | same text | same text | same text | CHANGED | -100000000000000000000.5e-7 |
| (the 5 literals of a WHERE) | same text | same text | same text | CHANGED | 0.05 |

hand: 47 of 47 kept, a-generic: 47 of 47 kept, a-postgres: 47 of 47 kept, mut-number-via-f64: 10 of 47 kept

The sign is a unary minus above the unsigned text in both arms; DuckDB's own parser folds it into the literal text (`-0.0`). Neither path touches a float.

## (iii) Error positions

### (iii) error positions on the mangled queries (d)

| arm | queries refused | with line and column | position in the edited token | within 1 token of the edit | within 2 tokens | reported at end of text |
|---|---|---|---|---|---|---|
| hand | 6432 | 6432 of 6432 (100.0%) | 4564 of 6432 (71.0%) | 6374 of 6432 (99.1%) | 6413 of 6432 (99.7%) | 26 |
| a-generic | 6404 | 6404 of 6404 (100.0%) | 3613 of 6404 (56.4%) | 5749 of 6404 (89.8%) | 6064 of 6404 (94.7%) | 43 |
| a-generic-nofix | 6404 | 6371 of 6404 (99.5%) | 3599 of 6404 (56.2%) | 5723 of 6404 (89.4%) | 6035 of 6404 (94.2%) | 10 |
| a-postgres | 6422 | 6421 of 6422 (100.0%) | 3386 of 6422 (52.7%) | 5752 of 6422 (89.6%) | 6080 of 6422 (94.7%) | 41 |
| duckdb | 5788 | 5750 of 5788 (99.3%) | 1919 of 5788 (33.2%) | 4656 of 5788 (80.4%) | 5329 of 5788 (92.1%) | 0 |

panics: 0 (all arms)

Within 1 token of the edit, by kind of edit:

| arm | delete | swap | insert |
|---|---|---|---|
| hand | 2090 of 2111 (99.0%) | 2167 of 2174 (99.7%) | 2117 of 2147 (98.6%) |
| a-generic | 1847 of 2094 (88.2%) | 1961 of 2162 (90.7%) | 1941 of 2148 (90.4%) |
| a-generic-nofix | 1836 of 2080 (88.3%) | 1958 of 2159 (90.7%) | 1929 of 2132 (90.5%) |
| a-postgres | 1851 of 2097 (88.3%) | 1961 of 2162 (90.7%) | 1940 of 2162 (89.7%) |
| duckdb | 1502 of 1903 (78.9%) | 1589 of 1908 (83.3%) | 1565 of 1939 (80.7%) |

sqlparser errors that carry NO line and column (without the end-of-text rule of the conversion layer), by message:

| message | n |
|---|---|
| Expected: an expression, found: EOF | 21 |
| Expected: ), found: EOF | 9 |
| Expected: ], found: EOF | 1 |
| Expected: identifier, found: EOF | 1 |
| Expected: SELECT, VALUES, or a subquery in the query body, f | 1 |

A sample of 20 refused mangled queries (random, seed 20261106): the query, the edit, and what each says (line:column, message):

- `SELECT a = 1A ND b = 2 OR c = 3 AS r FROM 'f.parquet'` (swap at character 12)
  - hand (1:12): syntax: trailing characters after numeric literal '1A'
  - sqlparser (1:15): Expected: end of statement, found: ND
  - DuckDB (char 14): syntax error at or near "ND"
- `SELELCT 'x' AS r FROM 'f.parquet'` (insert at character 4)
  - hand (1:1): syntax: expected SELECT or EXPLAIN, found 'SELELCT'
  - sqlparser (1:1): Expected: an SQL statement, found: SELELCT
  - DuckDB (char 17): syntax error at or near "FROM"
- `SELECT * FROM 'a.parquet LIMIT 0` (delete at character 24)
  - hand (1:15): syntax: unterminated string literal
  - sqlparser (1:15): Unterminated string literal
  - DuckDB (char 14): unterminated string literal
- `SELECT a * b AS r FROMQ 'f.parquet'` (insert at character 22)
  - hand (1:19): syntax: expected FROM, found 'FROMQ'
  - sqlparser (1:19): Expected: end of statement, found: FROMQ
  - DuckDB (char 18): syntax error at or near "FROMQ"
- `SELECT C* FROM 'a.parquet' WHERE a > 1 LIMIT 5 OFFSET 2` (insert at character 7)
  - hand (1:11): syntax: expected an expression, found 'FROM'
  - sqlparser (1:8): an alias without AS (write AS name)
  - DuckDB (char 10): syntax error at or near "FROM"
- `SELECT CASE WHEN a THEN CASE WHEN b THEN 1 ELSE 2 END ELSE 3 END AS r FRORM 'f.parquet'` (insert at character 73)
  - hand (1:71): syntax: expected FROM, found 'FRORM'
  - sqlparser (1:71): Expected: end of statement, found: FRORM
  - DuckDB (char 70): syntax error at or near "FRORM"
- `SELECT -(-a) AS r FRMO 'f.parquet'` (swap at character 20)
  - hand (1:19): syntax: expected FROM, found 'FRMO'
  - sqlparser (1:19): Expected: end of statement, found: FRMO
  - DuckDB (char 18): syntax error at or near "FRMO"
- `SLEECT * FROM 'f.parquet' WHERE a + 1 IS NULL` (swap at character 1)
  - hand (1:1): syntax: expected SELECT or EXPLAIN, found 'SLEECT'
  - sqlparser (1:1): Expected: an SQL statement, found: SLEECT
  - DuckDB (char 9): syntax error at or near "FROM"
- `SELET a = b BETWEEN 1 AND 2 AS r FROM 'f.parquet'` (delete at character 4)
  - hand (1:1): syntax: expected SELECT or EXPLAIN, found 'SELET'
  - sqlparser (1:1): Expected: an SQL statement, found: SELET
  - DuckDB (char 6): syntax error at or near "a"
- `SELCT CAST(a + 1 AS BIGINT) * 2 AS r FROM 'f.parquet'` (delete at character 3)
  - hand (1:1): syntax: expected SELECT or EXPLAIN, found 'SELCT'
  - sqlparser (1:1): Expected: an SQL statement, found: SELCT
  - DuckDB (char 6): syntax error at or near "CAST"
- `sselect * from 'a.parquet'` (insert at character 1)
  - hand (1:1): syntax: expected SELECT or EXPLAIN, found 'sselect'
  - sqlparser (1:1): Expected: an SQL statement, found: sselect
  - DuckDB (char 10): syntax error at or near "from"
- `SELECT CAST(CAST(a AS BIGINT) AS DOUBLE) AS r FRO M'f.parquet'` (swap at character 49)
  - hand (1:47): syntax: expected FROM, found 'FRO'
  - sqlparser (1:47): Expected: end of statement, found: FRO
  - DuckDB (char 46): syntax error at or near "FRO"
- `SELECT O* FROM 'f.parquet' WHERE a BETWEEN 0.05 AND 0.07` (insert at character 7)
  - hand (1:11): syntax: expected an expression, found 'FROM'
  - sqlparser (1:8): an alias without AS (write AS name)
  - DuckDB (char 10): syntax error at or near "FROM"
- `SELECT a BEWTEEN b + 1 AND c * 2 AS r FROM 'f.parquet'` (swap at character 11)
  - hand (1:10): unsupported: an alias without AS (write AS name)
  - sqlparser (1:18): Expected: end of statement, found: b
  - DuckDB (char 17): syntax error at or near "b"
- `SELECT NOT a OR NOT b AS r FRO 'f.parquet'` (delete at character 30)
  - hand (1:28): syntax: expected FROM, found 'FRO'
  - sqlparser (1:28): Expected: end of statement, found: FRO
  - DuckDB (char 27): syntax error at or near "FRO"
- `SELECT (a OR b) AD c AS r FROM 'f.parquet'` (delete at character 17)
  - hand (1:17): unsupported: an alias without AS (write AS name)
  - sqlparser (1:20): Expected: end of statement, found: c
  - DuckDB (char 19): syntax error at or near "c"
- `SELECT (a OR b) AND c AS r FRMO 'f.parquet'` (swap at character 29)
  - hand (1:28): syntax: expected FROM, found 'FRMO'
  - sqlparser (1:28): Expected: end of statement, found: FRMO
  - DuckDB (char 27): syntax error at or near "FRMO"
- `SELECT * FROM 'f.parquet' WHERE NOT a O NOT b` (delete at character 39)
  - hand (1:39): syntax: unexpected 'O' after the end of the query
  - sqlparser (1:39): Expected: end of statement, found: O
  - DuckDB (char 38): syntax error at or near "O"
- `SELECT a RO b OR c AS r FROM 'f.parquet'` (swap at character 9)
  - hand (1:10): unsupported: an alias without AS (write AS name)
  - sqlparser (1:13): Expected: end of statement, found: b
  - DuckDB (char 12): syntax error at or near "b"
- `SELECT * FROM 'f.parquet' WHERE a IN (1, 2) AdND b IN (3)` (insert at character 45)
  - hand (1:45): syntax: unexpected 'AdND' after the end of the query
  - sqlparser (1:45): Expected: end of statement, found: AdND
  - DuckDB (char 44): syntax error at or near "AdND"

"Within 1 token" is measured with an independent tokenizer (`corpus/report.py`) on the mangled text: the token that holds the reported position against the token that holds the first changed character. A deleted character has no token of its own, so its edit point is the character after it. DuckDB's position is 0-based in its own text (its `EXPLAIN` prefix is added back). Many mangled queries are still valid (the edit made another valid query), those are not in this table; errors that show only later in the text (an unterminated string, a lost parenthesis) cap what any parser can reach. The hand parser's advantage over `sqlparser` here comes mostly from reporting the offending token itself where `sqlparser` reports the token after a word it took as an alias or type.

## (iv) Dependencies, licenses, `unsafe`, build cost

`cargo deny --manifest-path spikes/sql-parser/Cargo.toml --config deny.toml check` (the repository's `deny.toml`, cargo-deny 0.20.2, advisory database fetched): **advisories ok, bans ok, licenses ok, sources ok.** (The spike crate needed a `license` field; the dependency licenses are MIT, Apache-2.0, `Apache-2.0 WITH LLVM-exception`, Unicode-3.0 and, for `memchr`, `Unlicense` OR MIT, all accepted.)

Textual `unsafe` inventory (`scripts/unsafe-inventory.py spike-sql-parser`; the script's stand-in for `cargo geiger`, an upper bound that counts keywords, not expanded macros).

With the crate's default features (`recursive-protection` on):

```
package spike-sql-parser: 19 packages in its normal+build dependency tree (cargo metadata, this machine's feature resolution)
package                     version     source      unsafe  files  root declares
libc                        0.2.191     registry      1599    886  
memchr                      2.8.3       registry       333     45  
syn                         2.0.119     registry        67     55  
find-msvc-tools             0.1.15      registry        55     11  
psm                         0.1.32      registry        49     11  
stacker                     0.1.25      registry        40     21  
cc                          1.11.0      registry        34     23  
object                      0.39.1      registry        28    185  
proc-macro2                 1.0.107     registry        12     37  
log                         0.4.35      registry         6      9  
shlex                       2.0.1       registry         6      2  
unicode-ident               1.0.27      registry         2      2  
ar_archive_writer           0.5.3       registry         0      9  
cfg-if                      1.0.5       registry         0      1  
quote                       1.0.47      registry         0     24  
recursive                   0.1.1       registry         0      1  
recursive-proc-macro-impl   0.1.1       registry         0      1  
spike-sql-parser            0.0.0       workspace        0     10  
sqlparser                   0.63.0      registry         0     44  
total `unsafe` keywords in code: 2231 in 12 of 19 packages; workspace packages: 0
```

Without it (`default-features = false, features = ["std"]`; same script with `--no-default-features --features arm-a` added to its `cargo metadata` call):

```
package spike-sql-parser: 3 packages in its normal+build dependency tree (cargo metadata, this machine's feature resolution)
package                     version     source      unsafe  files  root declares
log                         0.4.35      registry         6      9  
spike-sql-parser            0.0.0       workspace        0     10  
sqlparser                   0.63.0      registry         0     44  
total `unsafe` keywords in code: 6 in 1 of 3 packages; workspace packages: 0
```

New against the main workspace's `Cargo.lock` (142 packages): with the default features `sqlparser`, `recursive`, `recursive-proc-macro-impl`, `stacker` (40 `unsafe`), `psm` (49, and inline assembly), `ar_archive_writer`, `object` (28); without it only `sqlparser` (0 `unsafe`; its `log` dependency is already in the workspace). `libc`, `cc`, `memchr`, `syn` are already in the workspace.

| build (clean `cargo build --release --bin parse_cli`, 8 cores) | time | binary | stripped |
|---|---|---|---|
| hand parser only | 1.0 s | 0.60 MB | 0.47 MB |
| + `sqlparser`, no `recursive-protection` | 32.0 s | 9.79 MB | 7.56 MB |
| + `sqlparser`, default features | 40.7 s | 10.01 MB | 7.74 MB |

## Parse time

In-process, release build, 10230 queries (the 230 in-subset test-set queries and the 10,000 generated ones, mean 85 bytes), 20 repetitions of 5 parses per query and arm, the arms interleaved and rotated in every repetition, drop of the AST included, median of the repetitions per query; the table is the median and the 99th percentile over queries, for 5 whole passes (the range is the noise). The clock costs 23 ns per reading.

| arm | median per query, us (min to max over 5 passes) | p99, us | p99.9, us | worst query, us |
|---|---|---|---|---|
| hand | 1.10 (1.02 to 1.11) | 3.16 (2.94 to 3.23) | 4.32 (4.07 to 4.40) | 6.8 |
| a-generic | 11.51 (10.42 to 11.55) | 30.67 (28.55 to 31.00) | 42.63 (41.90 to 43.52) | 70.6 |
| a-postgres | 11.33 (10.26 to 11.37) | 30.19 (28.04 to 30.58) | 42.34 (40.31 to 42.99) | 67.1 |
| a-generic-parse-only | 8.17 (7.46 to 8.21) | 21.06 (19.50 to 21.34) | 28.94 (27.66 to 29.39) | 46.7 |

`a-generic` and `a-postgres` are the crate plus the conversion plus the guard; `a-generic-parse-only` is the crate alone. **This is the dev machine and not a performance claim beyond the order of magnitude:** the hand parser is about 10 times faster than the crate with its layer (7 times faster than the crate alone), both are far under the card's 1 ms target (the p99 is 30 times under for the crate with its layer and 300 times under for the hand parser; the medians 90 and 900 times), and the first pass of all arms is 5 to 10% faster than the later ones (box-wide drift), so differences under 10% are noise. Binding is not included.

## Robustness

### robustness: deep, long and huge queries, each in a fresh process, parse on a thread with a 2 MB and an 8 MB stack

`a-generic` is sqlparser with the conversion layer and the depth guard; `a-generic-raw` is the crate alone (parse and drop, no guard, no conversion); `a-no-stacker` is the same as `a-generic` built without the crate's `recursive-protection` feature. Time and peak memory (VmHWM growth) are from the 8 MB run.

| case | hand | a-generic (stacker on) | a-no-stacker | a-generic-raw (crate alone) |
|---|---|---|---|---|
| parens-1000 | 2 MB: ok; 8 MB: ok; 1 ms, +1 MB | 2 MB: ok; 8 MB: ok; 3 ms, +8 MB | 2 MB: **CRASH** (signal 6); 8 MB: ok; 4 ms, +7 MB | 2 MB: ok; 8 MB: ok; 2 ms, +8 MB |
| parens-1500 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +1 MB | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +1 MB | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +1 MB | 2 MB: ok; 8 MB: ok; 4 ms, +10 MB |
| parens-500000 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +0 MB | 2 MB: refused: limit; 8 MB: refused: limit; 44 ms, +84 MB | 2 MB: refused: limit; 8 MB: refused: limit; 48 ms, +84 MB | 2 MB: refused: syntax; 8 MB: refused: syntax; 52 ms, +105 MB |
| ands-1000 | 2 MB: ok; 8 MB: ok; 0 ms, +0 MB | 2 MB: ok; 8 MB: ok; 14 ms, +4 MB | 2 MB: ok; 8 MB: ok; 13 ms, +4 MB | 2 MB: ok; 8 MB: ok; 3 ms, +3 MB |
| ands-1500 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +1 MB | 2 MB: refused: limit; 8 MB: refused: limit; 1 ms, +1 MB | 2 MB: refused: limit; 8 MB: refused: limit; 1 ms, +1 MB | 2 MB: ok; 8 MB: ok; 2 ms, +4 MB |
| ands-250000 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +0 MB | 2 MB: refused: limit; 8 MB: refused: limit; 96 ms, +99 MB | 2 MB: refused: limit; 8 MB: refused: limit; 96 ms, +98 MB | 2 MB: **CRASH** (signal 6); 8 MB: **CRASH** (signal 6) |
| sums-1000 | 2 MB: ok; 8 MB: ok; 0 ms, +1 MB | 2 MB: ok; 8 MB: ok; 11 ms, +4 MB | 2 MB: ok; 8 MB: ok; 11 ms, +4 MB | 2 MB: ok; 8 MB: ok; 2 ms, +3 MB |
| sums-250000 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +0 MB | 2 MB: refused: limit; 8 MB: refused: limit; 68 ms, +92 MB | 2 MB: refused: limit; 8 MB: refused: limit; 66 ms, +92 MB | 2 MB: **CRASH** (signal 6); 8 MB: **CRASH** (signal 6) |
| nots-1000 | 2 MB: ok; 8 MB: ok; 0 ms, +1 MB | 2 MB: ok; 8 MB: ok; 5 ms, +9 MB | 2 MB: **CRASH** (signal 6); 8 MB: ok; 5 ms, +9 MB | 2 MB: ok; 8 MB: ok; 3 ms, +9 MB |
| nots-250000 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +0 MB | 2 MB: refused: limit; 8 MB: refused: limit; 59 ms, +49 MB | 2 MB: refused: limit; 8 MB: refused: limit; 56 ms, +50 MB | 2 MB: refused: syntax; 8 MB: refused: syntax; 71 ms, +75 MB |
| negs-1000 | 2 MB: ok; 8 MB: ok; 0 ms, +1 MB | 2 MB: ok; 8 MB: ok; 3 ms, +7 MB | 2 MB: **CRASH** (signal 6); 8 MB: ok; 8 ms, +6 MB | 2 MB: ok; 8 MB: ok; 3 ms, +7 MB |
| negs-500000 | 2 MB: refused: limit; 8 MB: refused: limit; 0 ms, +0 MB | 2 MB: refused: limit; 8 MB: refused: limit; 48 ms, +84 MB | 2 MB: refused: limit; 8 MB: refused: limit; 46 ms, +84 MB | 2 MB: refused: syntax; 8 MB: refused: syntax; 50 ms, +102 MB |
| cases-1000 | 2 MB: ok; 8 MB: ok; 2 ms, +3 MB | 2 MB: **CRASH** (signal 6); 8 MB: ok; 78 ms, +13 MB | 2 MB: **CRASH** (signal 6); 8 MB: ok; 81 ms, +13 MB | 2 MB: ok; 8 MB: ok; 9 ms, +13 MB |
| cases-60000 | 2 MB: refused: limit; 8 MB: refused: limit; 1 ms, +1 MB | 2 MB: refused: limit; 8 MB: refused: limit; 62 ms, +61 MB | 2 MB: refused: limit; 8 MB: refused: limit; 64 ms, +60 MB | 2 MB: refused: syntax; 8 MB: refused: syntax; 9607 ms, +91 MB |
| inlist-500000 | 2 MB: ok; 8 MB: ok; 54 ms, +45 MB | 2 MB: ok; 8 MB: ok; 385 ms, +274 MB | 2 MB: ok; 8 MB: ok; 402 ms, +274 MB | 2 MB: ok; 8 MB: ok; 289 ms, +274 MB |
| selectlist-350000 | 2 MB: ok; 8 MB: ok; 53 ms, +44 MB | 2 MB: ok; 8 MB: ok; 440 ms, +375 MB | 2 MB: ok; 8 MB: ok; 449 ms, +374 MB | 2 MB: ok; 8 MB: ok; 345 ms, +375 MB |
| string-1000000 | 2 MB: ok; 8 MB: ok; 1 ms, +0 MB | 2 MB: ok; 8 MB: ok; 7 ms, +6 MB | 2 MB: ok; 8 MB: ok; 7 ms, +6 MB | 2 MB: ok; 8 MB: ok; 4 ms, +6 MB |
| comment-1000000 | 2 MB: ok; 8 MB: ok; 0 ms, +0 MB | 2 MB: ok; 8 MB: ok; 4 ms, +3 MB | 2 MB: ok; 8 MB: ok; 6 ms, +3 MB | 2 MB: ok; 8 MB: ok; 2 ms, +3 MB |
| ident-1000000 | 2 MB: ok; 8 MB: ok; 2 ms, +1 MB | 2 MB: ok; 8 MB: ok; 10 ms, +6 MB | 2 MB: ok; 8 MB: ok; 11 ms, +6 MB | 2 MB: ok; 8 MB: ok; 5 ms, +5 MB |

Smallest thread stack (KB, from a ladder 64, 96, 128, ... 65536) at which a 1,000-deep case completes without a crash:

| case (1,000 deep) | hand | a-generic, stacker | crate alone, stacker | a-generic, no stacker | crate alone, no stacker |
|---|---|---|---|---|---|
| parens-1000 | 1536 | 512 | 64 | 6144 | 6144 |
| cases-1000 | 2048 | 3072 | 128 | 8192 | 8192 |
| nots-1000 | 512 | 768 | 64 | 6144 | 6144 |
| negs-1000 | 512 | 768 | 64 | 4096 | 4096 |
| ands-1000 | 96 | 1024 | 96 | 1024 | 96 |
| sums-1000 | 96 | 1024 | 96 | 1024 | 96 |

Peak memory is the growth of the process's peak resident size (`VmHWM`) over its size before the parse; the query text itself (up to 1 MB) is in the baseline. "refused: limit" is the error "expression nested deeper than 1024" (hand, guard) or the crate's own "recursion limit exceeded". No arm panicked on the 10,000 mangled queries, and the hand arm and `sqlparser` (with the layer) never crashed with a stack of 8 MB; the hand arm did not crash at 2 MB either. The crate alone (no guard, no layer) aborts on the long operator chains at any stack size and, without `stacker`, on 1,000 nested parentheses at 2 MB. A 60,000-level `CASE` (1 MB) takes the crate alone 9.6 s to fail; the hand parser and the guard refuse it in about 1 ms and 60 ms.

## Known-bad hand parsers: the checks can fail

Three deliberately wrong versions of the hand parser (`Mutant` in `src/hand.rs`, spike only), each compared with `sqlparser`'s AST, plus `cargo test --release` (13 tests; the last three of them run the mutants):

### known-bad hand parsers: does the comparison with sqlparser catch each?

| corpus | parser | queries | AST differs from sqlparser (postgres) | first difference |
|---|---|---|---|---|
| (a) accepted test set | mut-or-before-and | 230 | 8 | `SELECT a AND b OR c AS r FROM 'f.parquet'` |
| (a) accepted test set | mut-lose-not | 230 | 16 | `SELECT NOT a AS r FROM 'f.parquet'` |
| (a) accepted test set | mut-number-via-f64 | 230 | 10 | `SELECT 9223372036854775808 AS r FROM 'f.parquet'` |
| (a) accepted test set | hand (control, the real parser) | 230 | 0 | - |
| (c) generated | mut-or-before-and | 10000 | 9 | `Select a As "C 0" From 'f.parquet' Where 1 IN (-1) OR f AND f` |
| (c) generated | mut-lose-not | 10000 | 620 | `EXPLAIN ANALYZE SELECT 1.00, 1.5 AS c1 FROM 'data/*.parquet' WHERE ...` |
| (c) generated | mut-number-via-f64 | 10000 | 1296 | `SELECT\tCAST(a AS DOUBLE)\tFROM 'lineitem.parquet'\tWHERE (d >= 1.0...` |
| (c) generated | hand (control, the real parser) | 10000 | 0 | - |

The fixed-answer precedence test (`precedence_and_associativity_against_fixed_answers`) catches the OR/AND mutant by itself, the literal test catches the `f64` mutant (the test requires at least 10 of its 16 literals to change, the table above shows 37 of 47; `1e400` becomes `inf`), and `the_precedence_answers_catch_the_or_and_not_mutants` shows the NOT mutant really drops the `NOT`. The generated corpus alone is weak against the OR/AND mutant (9 of 10,000), because the generator rarely writes an unparenthesised `AND` next to an `OR`: the hand-written test set is what catches it (8 of 230).

## What this does not show

- Nothing about the binder, the executor or the cost of the rest of planning: only the parse and the conversion into a syntax tree. The card's "parse and bind under 1 ms" is met with a factor of 30 or more at the p99 for the parse alone; binding is slice 4 work.
- The DuckDB comparison is of the *parser* (`json_serialize_sql`: parse without binding). Whether DuckDB would later reject a query that its parser takes (a bad `DATE`, an unknown type name) is not tested; the subset parsers do not check `DATE` strings, type ranges (`DECIMAL(40,2)`), identifier existence or `LIMIT` ranges either.
- The test set is written by one author from the card; 230 accepted and 113 refused queries cover each construct, not each interaction. The generated corpus is shallow (depth 1 to 4, up to 3 select items); the deep and the huge cases are the robustness table only. The mangled corpus is one edit at a time, on the 230 accepted queries.
- Error *messages* were compared by reading 20 of them (above); there is no measure of quality, and neither arm offers "did you mean" yet (the card asks for it "where possible").
- Timing is one machine, x86-64 only, in-process, not cold; ARM64 is unmeasured (ADR rule: "unmeasured" is stated). The binary and build sizes are one machine, one compiler.
- `sqlparser` 0.63.0 only. A later release may change the AST (the exhaustive destructuring makes that a compile error) or fix `LIMIT ALL`.
- The `unsafe` inventory is textual (keywords in sources), not `cargo geiger`.
- Stack sizes are a ladder (steps of 1.33 to 2 times), so "minimum" means the first rung that passed; a stack that is 25% bigger than a rung that failed may still fail.
- DuckDB's list of Unicode white space and its reserved words are DuckDB 2.0.0's, found by probing and `duckdb_keywords()`.

## Replay

`corpus/run-all.sh` regenerates the corpora from the seeds (20261106), builds, runs the tables of this file, and writes `target/corpus/tables.md`; `corpus/build_results.py` assembles this file from `corpus/results_template.md`. The test set is `corpus/testset.jsonl` (written by `corpus/testset.py`). Tests: `cargo test --release` in `spikes/sql-parser`; `cargo fmt --check` and `cargo clippy --release --all-targets -- -D warnings` are clean.
