# nata-scan

The row-group pruning decision of the Parquet scan (slice 3, `docs/slices/0001-parquet-scan.md`), as a pure library function: from the statistics of a
row group and a pushed-down predicate (an AND of `column <op> literal`, `IS NULL`, `IS NOT NULL`) to `Skip` or `Keep(reason)`. No I/O, no dependencies,
no `unsafe`. `Skip` means the statistics PROVE that no row can match, under DuckDB 2.0.0 semantics; anything missing, unsound, NaN-affected,
truncated beyond use or of an unsupported type is `Keep`.

The rule table (each rule with the DuckDB check that confirmed its semantics), the type rules and the list of what is not covered are the crate
documentation: `cargo doc -p nata-scan --open`, or the top of `src/lib.rs`. The evidence is in `bench/results/20261105-scan-pruning.md`.

```rust
let decision = nata_scan::can_skip(&row_group_stats, &predicate); // Decision::Skip(reason) | Decision::Keep(reason)
```

## Commands

Everything runs offline on the development machine; no cloud, no network.

```sh
cargo fmt --check
cargo clippy -p nata-scan --all-targets -- -D warnings
cargo test -p nata-scan              # debug build: 40,000 soundness cases
cargo test --release -p nata-scan    # release build (overflow checks on, as in the workspace): 400,000 soundness cases
python3 crates/nata-scan/mutate.py --jobs 6 --out /tmp/mutants.txt   # the mutants, about 90 s on 8 cores; every line must say CAUGHT
python3 crates/nata-scan/mutate.py --list                            # the mutant names
python3 crates/nata-scan/mutate.py --dry                             # only check that every mutant compiles (INVALID otherwise)
```

| Test file | What it shows |
|---|---|
| `tests/rules.rs` | the exact boundary of every rule: the last value that is skipped and the first that is kept (35 tests: all operators, empty, one row, all-NULL, missing and contradictory statistics, floats with NaN, zeros and infinities, NaN literals, decimals at other scales and at the 128-bit limit, integer extremes, text with non-ASCII and truncated bounds, conjunctions) |
| `tests/soundness.rs` | PROPERTY: whenever `can_skip` says Skip, no row of the generated row group matches (row-by-row oracle, SQL semantics). 400,000 cases in release. Also 5 NEGATIVE CONTROLS: deliberately unsound readers (NaN-blind, truncated-as-exact, missing null count as zero, missing bound as zero, NaN bound trusted) that the same property must reject, and a check that the generator reaches the hard situations |
| `tests/effectiveness.rs` | "always Keep" cannot pass: R2 of the card on sorted dates (49 row groups) and sorted integers, clustered random data with a stated minimum, and TIGHTNESS (exact statistics skip exactly when no value in `[min, max]` can match; integers, dates, timestamps, booleans, decimals, floats, text) |
| `tests/duckdb_oracle.rs` | the row oracle of the soundness test against DuckDB itself (2,000 random cases by default; 20,000 were run for the record), and the 53 single facts that the rule table rests on. Skipped with a message if the `duckdb` command line is not found (`NATA_DUCKDB`, `PATH`, `~/.local/bin`); `NATA_REQUIRE_DUCKDB=1` makes a missing DuckDB an error |
| `mutate.py` | 53 mutants of the rules, each ONE text edit of the source applied to a private copy; CAUGHT / SURVIVED / INVALID. INVALID (the edit does not apply once, changes nothing, or does not compile) fails the run |

## Replaying a failure

The soundness test prints the seed and the case number of the first counterexample, with the statistics, the data and the predicate, and the command that
replays only that case:

```sh
NATA_SCAN_SEED=<seed> NATA_SCAN_ONLY=<case> cargo test -p nata-scan --test soundness -- --nocapture
```

Other variables: `NATA_SCAN_CASES` (soundness cases; default 400,000 release, 40,000 debug), `NATA_SCAN_SEED` (decimal or `0x` hex; the default is fixed so that
runs are repeatable; set another one to look further), `NATA_SCAN_DUCKDB_CASES` (default 2,000), `NATA_DUCKDB` (path of the `duckdb` command line).
