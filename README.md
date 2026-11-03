# Nata

Distributed SQL engine for the lakehouse, written in Rust. Pre-alpha: it cannot
run a query yet. See `docs/vision.md`, `docs/feature-map.md` and
`docs/way-of-working.md`.

## Setup

```sh
# Rust is pinned in rust-toolchain.toml; rustup installs it on first use.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# DuckDB CLI (correctness oracle and benchmark baseline), v2.0.0:
#   https://github.com/duckdb/duckdb/releases/tag/v2.0.0

scripts/gen-tpch.sh 0.01 data/tpch-sf0.01   # tiny TPC-H as Parquet (gitignored)
```

## Tools

| Tool | Job |
|---|---|
| `nata` | Run a node / CLI client. Every command answers `not supported yet`. |
| `nata-check` | Run `check/queries/*` on DuckDB and Nata and compare. |
| `nata-bench` | Time `check/queries/*` on one engine, write `bench/results/`. |

```sh
cargo build --workspace
cargo run -p nata-check -- --data data/tpch-sf0.01 --queries check/queries/smoke
cargo run -p nata-bench -- --data data/tpch-sf0.01 --queries check/queries/smoke
```

`nata-check` exits non-zero until every query matches DuckDB. Today every query
reports `UNSUPPORTED`, which is the correct answer.

Query sets in `check/queries/`: `smoke/`, `decimal/` (same behaviour as DuckDB,
or both engines must error) and `decimal-differences/` (where Nata deliberately
differs; see `docs/decimal-rules.md`). Per-query directives, as `--` comment
lines at the top of a `.sql` file:

| directive | meaning |
|---|---|
| `-- expect-error` | both engines must fail |
| `-- expect-error-nata: <text>` | only Nata runs, and must fail with a message containing `<text>` |
| `-- tolerance: <relative>` | numeric cells compared within that relative tolerance (`0` ignores formatting only) |

## Before you push

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
