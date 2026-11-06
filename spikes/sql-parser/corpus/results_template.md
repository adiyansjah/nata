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

{{CORPUS}}

All three generators are seeded; query *i* of the generated corpus is `random.Random(seed * 1_000_003 + i)`, mangle *i* is `random.Random(seed * 7_919 + i)`. The generated queries come from the sqlfuzz generator (`gen.expr`, `gen.boolean`, its data pools), rejection-sampled to the subset, and each tree is rendered twice: as sqlfuzz does (every operator parenthesised) and with the fewest parentheses SQL precedence allows, so that the precedence rules are exercised and each tree gives a cross-check (both renderings must give one AST).

## (i) Acceptance

{{ACCEPT_A}}

(`edge` = 39 dialect corner cases with no fixed expectation; the arms agree on 35 to 39 of them, the disagreements are the ones listed.)

{{ACCEPT_B}}

{{ACCEPT_C}}

{{AGREE}}

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

{{LITERALS}}

The sign is a unary minus above the unsigned text in both arms; DuckDB's own parser folds it into the literal text (`-0.0`). Neither path touches a float.

## (iii) Error positions

{{ERRORS}}

"Within 1 token" is measured with an independent tokenizer (`corpus/report.py`) on the mangled text: the token that holds the reported position against the token that holds the first changed character. A deleted character has no token of its own, so its edit point is the character after it. DuckDB's position is 0-based in its own text (its `EXPLAIN` prefix is added back). Many mangled queries are still valid (the edit made another valid query), those are not in this table; errors that show only later in the text (an unterminated string, a lost parenthesis) cap what any parser can reach. The hand parser's advantage over `sqlparser` here comes mostly from reporting the offending token itself where `sqlparser` reports the token after a word it took as an alias or type.

## (iv) Dependencies, licenses, `unsafe`, build cost

`cargo deny --manifest-path spikes/sql-parser/Cargo.toml --config deny.toml check` (the repository's `deny.toml`, cargo-deny 0.20.2, advisory database fetched): **advisories ok, bans ok, licenses ok, sources ok.** (The spike crate needed a `license` field; the dependency licenses are MIT, Apache-2.0, `Apache-2.0 WITH LLVM-exception`, Unicode-3.0 and, for `memchr`, `Unlicense` OR MIT, all accepted.)

Textual `unsafe` inventory (`scripts/unsafe-inventory.py spike-sql-parser`; the script's stand-in for `cargo geiger`, an upper bound that counts keywords, not expanded macros).

With the crate's default features (`recursive-protection` on):

```
{{UNSAFE_DEFAULT}}
```

Without it (`default-features = false, features = ["std"]`; same script with `--no-default-features --features arm-a` added to its `cargo metadata` call):

```
{{UNSAFE_NOSTACKER}}
```

New against the main workspace's `Cargo.lock` (142 packages): with the default features `sqlparser`, `recursive`, `recursive-proc-macro-impl`, `stacker` (40 `unsafe`), `psm` (49, and inline assembly), `ar_archive_writer`, `object` (28); without it only `sqlparser` (0 `unsafe`; its `log` dependency is already in the workspace). `libc`, `cc`, `memchr`, `syn` are already in the workspace.

| build (clean `cargo build --release --bin parse_cli`, 8 cores) | time | binary | stripped |
|---|---|---|---|
| hand parser only | 1.0 s | 0.60 MB | 0.47 MB |
| + `sqlparser`, no `recursive-protection` | 32.0 s | 9.79 MB | 7.56 MB |
| + `sqlparser`, default features | 40.7 s | 10.01 MB | 7.74 MB |

## Parse time

In-process, release build, {{NQ}} queries (the 230 in-subset test-set queries and the 10,000 generated ones, mean {{MEANB}} bytes), 20 repetitions of 5 parses per query and arm, the arms interleaved and rotated in every repetition, drop of the AST included, median of the repetitions per query; the table is the median and the 99th percentile over queries, for 5 whole passes (the range is the noise). The clock costs {{CLOCK}} ns per reading.

{{TIMING}}

`a-generic` and `a-postgres` are the crate plus the conversion plus the guard; `a-generic-parse-only` is the crate alone. **This is the dev machine and not a performance claim beyond the order of magnitude:** the hand parser is about 10 times faster than the crate with its layer (7 times faster than the crate alone), both are far under the card's 1 ms target (the p99 is 30 times under for the crate with its layer and 300 times under for the hand parser; the medians 90 and 900 times), and the first pass of all arms is 5 to 10% faster than the later ones (box-wide drift), so differences under 10% are noise. Binding is not included.

## Robustness

{{ROBUST}}

Peak memory is the growth of the process's peak resident size (`VmHWM`) over its size before the parse; the query text itself (up to 1 MB) is in the baseline. "refused: limit" is the error "expression nested deeper than 1024" (hand, guard) or the crate's own "recursion limit exceeded". No arm panicked on the 10,000 mangled queries, and the hand arm and `sqlparser` (with the layer) never crashed with a stack of 8 MB; the hand arm did not crash at 2 MB either. The crate alone (no guard, no layer) aborts on the long operator chains at any stack size and, without `stacker`, on 1,000 nested parentheses at 2 MB. A 60,000-level `CASE` (1 MB) takes the crate alone 9.6 s to fail; the hand parser and the guard refuse it in about 1 ms and 60 ms.

## Known-bad hand parsers: the checks can fail

Three deliberately wrong versions of the hand parser (`Mutant` in `src/hand.rs`, spike only), each compared with `sqlparser`'s AST, plus `cargo test --release` (13 tests; the last three of them run the mutants):

{{MUTANTS}}

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
