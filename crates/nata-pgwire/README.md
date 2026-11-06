# nata-pgwire

Checked Postgres wire 3.0 codec derived from the hand-codec bake-off, selected by
ADR 0008. Own Rust forbids unsafe. No pgwire dependency.

The frontend checks the fixed header before reserving the body: startup 10,000
bytes, regular frames 1,114,112 bytes (length included, tag excluded), SQL 1 MiB.
Counts, UTF-8, terminators and exact consumption are checked. Decoding preserves
extended-message fields for step 3; server support is limited to simple queries.

`cargo test -p nata-pgwire` includes the seeded 10,000-case corpus for each of
startup and regular frames. Replay with `NATA_PGWIRE_SEED` and increase with
`NATA_PGWIRE_CASES`; both print at test startup (`-- --nocapture`).
