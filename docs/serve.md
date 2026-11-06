# Local Postgres wire server

```
nata serve --listen 127.0.0.1:5432
psql -h 127.0.0.1 -U test -d nata -c "SELECT * FROM 'data.parquet' LIMIT 10"
```

This is Nata SQL over Postgres wire 3.0, not the PostgreSQL SQL dialect. See
[dialect.md](dialect.md). Step 2 supports startup, SIMPLE Query, empty queries,
EXPLAIN / EXPLAIN ANALYZE, text rows, structured errors, cancel and Terminate.
Extended protocol and driver session chatter are explicitly unsupported until
step 3. Errors of queries restore ReadyForQuery; malformed frames close the socket.

Authentication is trust and there is no TLS. SSL and GSS encryption probes receive
`N`. The default listener is loopback only. `--unsafe-listen` explicitly bypasses
this restriction and is **unsafe until TLS and authentication exist**. The server
can read files accessible to its process; use it only with trusted local clients.

`--max-connections` defaults to 64 (allowed range 1–64). Eight reserved startup /
cancel slots keep cancellation available at full session capacity. Excess sockets
close immediately; startup has a five-second total deadline including encryption
probes. A full session set returns `53300` after startup. Startup is capped at
10,000 bytes, regular frontend messages at 1,114,112 bytes including their length,
and SQL at 1 MiB; headers are checked before body allocation.

One query is admitted at a time across all sessions. `--threads` sets that query's
CPU workers (default: available parallelism); this conservative first server does
not create one full CPU pool per connected session. Planning, file metadata and
execution run off the two-thread tokio I/O runtime. Pending queries remain
cancellable. The existing executor owns bounded row-group windows; its sink sends
into a 32-row bridge. A slow socket fills that bridge and stops scan production.
It never collects the complete result.

Each query gets a fresh `ScanCancel`. BackendKeyData uses a session id and an OS
random secret. A CancelRequest must match both; idle, stale and wrong keys have no
effect. Cancel returns `57014`, `canceling statement due to user request`; workers
are joined before ReadyForQuery. Disconnect closes the bridge and cancels workers.
If cancellation interrupts a partially written packet, that same write may finish
for up to one second before ErrorResponse; a reader that remains blocked is closed
so that partial frames are never followed by an error packet. `signal-hook` and
its SIGINT watcher remain CLI-only (`nata sql`); sessions install no signal hooks.

Errors carry no cell values or source-library messages. Codes are `42601` syntax,
`42804` bind (class 42), `0A000` unsupported, `22000` evaluation / unrepresentable
wire value, `58030` source I/O, `XX000` internal, `08P01` malformed protocol.
Position `P` is a 1-based character offset in the whole query, including newlines.
RowDescription follows ADR 0008 (signed TINYINT as text, fixed type sizes, numeric
typmod); NULL has length -1, empty text length zero. Dates outside years 1–9999 and
text containing a zero byte fail explicitly with class 22, never change OIDs.

Logs contain session id, duration, row count and SQLSTATE for each query;
SQL text, cell values and cancel secrets are not logged. No TLS/authentication,
transaction semantics, COPY, binary format or catalog compatibility is claimed.
