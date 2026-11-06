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

`server::serve` runs the loopback-only SIMPLE-query server. See
[serve.md](../../docs/serve.md) for admission, cancellation, bounds and security.
`server::Metrics` exposes cumulative produced rows, active workers and sessions
without SQL or values, so resource tests can distinguish backpressure from a
small result and check that disconnect/cancel release query admission.

`tests/protocol.rs` is a raw-protocol client with recorded sequences and real
Parquet execution. It optionally invokes `psql` when it is on PATH. Run
`tests/corpus.py --nata ../../target/debug/nata --data <data-dir>` from this
directory after generating `sqlcheck` and TPC-H fixtures with the root scripts.
`mutate.py` tests private workspace copies; INVALID never counts as caught.
