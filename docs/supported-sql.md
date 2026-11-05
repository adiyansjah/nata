# Supported SQL

This page is the contract: if it is not listed here, it is not supported, and
Nata must answer `not supported yet: <what>`.

Currently supported: nothing. `nata sql` returns `not supported yet: SQL execution`.

## Developer command (not SQL)

`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv` reads Parquet files (slice 3, step 2a, single thread) and prints CSV: projection pushdown, row-group
pruning from the footer statistics and an exact row filter. Example, the year range of the slice card:
`nata scan lineitem.parquet --columns l_orderkey,l_shipdate --filter 'l_shipdate >= 1994-01-01' --filter 'l_shipdate < 1995-01-01' --csv`. Types, literals and limits: `crates/nata-parquet/README.md`.
