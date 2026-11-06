# nata-pgwire

Checked Postgres wire 3.0 codec derived from the hand-codec bake-off, selected by
ADR 0008. Own Rust forbids unsafe. No pgwire dependency.

The frontend checks the fixed header before reserving the body: startup 10,000
bytes, regular frames 1,114,112 bytes (length included, tag excluded), SQL 1 MiB.
Counts, UTF-8 text, terminators and exact consumption are checked. Bind preserves
opaque binary parameters for type-aware validation at the session boundary.

`cargo test -p nata-pgwire` includes the seeded 10,000-case corpus for each of
startup and regular frames. Replay with `NATA_PGWIRE_SEED` and increase with
`NATA_PGWIRE_CASES`; both print at test startup (`-- --nocapture`).

`server::serve` runs the loopback-only simple and named/unnamed extended server. See
[serve.md](../../docs/serve.md) for admission, cancellation, bounds and security.
`server::Metrics` exposes cumulative produced rows, active workers and sessions
without SQL or values, so resource tests can distinguish backpressure from a
small result and check that disconnect/cancel release query admission.

`tests/protocol.rs` is a raw-protocol client with recorded sequences and real
Parquet execution. It optionally invokes `psql` when it is on PATH. Run
`tests/corpus.py --nata ../../target/debug/nata --data <data-dir>` from this
directory after generating `sqlcheck` and TPC-H fixtures with the root scripts.
`mutate.py` tests private workspace copies; INVALID never counts as caught.


`tests/reliability.rs` checks live malformed sequences, kill/disconnect/cancel
under injected file delays and optional CPU contention, 64 sessions plus reserved
sockets, and a one-byte/ms reader of a million-row result. Linux /proc provides
fd/thread/RSS oracles; scan gauges track handles, footers and retained buffers.
Normal defaults are 28 lifecycle and 100 live-fuzz cases; for the recorded run:

```sh
mkdir -p "$HOME/tmp-4b4"
TMPDIR="$HOME/tmp-4b4" NATA_PGWIRE_SEED=322661912839 \
NATA_PGWIRE_SCENARIOS=2000 NATA_PGWIRE_LIVE_CASES=10000 \
NATA_PGWIRE_CONTENTION=$(nproc) \
cargo test --release -p nata-pgwire --test reliability -- --nocapture
```

`tests/Clients4.java` and `tests/clients4.py` exercise default pgjdbc and psycopg
preparation, types, parameters, NULLs, errors and cancel. Pass port, all-type
fixture path and large fixture path; fixture generation is in tests/support.
Step-3 probes are retained as historical controls and expect the old refusals.
