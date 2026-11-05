# THROWAWAY SPIKE: the network exchange, stage A (verification-plan item 28)

A real TCP exchange, std only (threads, blocking sockets), to answer for ADR 0004 what the in-process exchange cannot: what happens on
a network. Delete the code once ADR 0004 is accepted; keep `bench/results/` and the lessons.

* `src/wire.rs`: frame format (`magic | kind | seq | len | payload`), defended against bad magic, unknown kinds, absurd lengths (checked
  before allocating), control frames with payloads, and streams cut inside a frame.
* `src/flow.rs`: one stream per TCP connection. **Credit-based flow control** (the receiver grants a window and gives bytes back when the
  CONSUMER takes a batch, so buffered bytes are bounded however slow the consumer is), **ordering** (batch numbers without gaps; Eos carries
  the count), **cancel**, **liveness** (every received byte is a sign of life; ping when idle; a silent peer is shut down after `dead_after`),
  and a **completion handshake** (see below).
* `src/shuffle.rs`: N nodes, all-to-all hash shuffle; every node's result is checked against a pure function of the parameters.
  `run_inproc` is the baseline: the same shuffle between threads with bounded channels.
* `gcp-check.sh`: the whole check in Linux network namespaces with `tc netem` (job `netexchsim`); `mutants/run.sh`: deliberate bugs.

Run: `cargo test --release` (22 tests, loopback), `mutants/run.sh`, and `NX_QUICK=1 ./gcp-check.sh /tmp/out` on a machine with root or passwordless
sudo (it builds `nx0..nx3`, a bridge and veth pairs, and removes them on exit). On GCP: `scripts/gcp/orchestrate.sh run --job netexchsim`.

## What the namespace runs found (2026-11-05, development machine, quick mode)

**A process that exits right after sending its last frame can lose that frame.** Closing a socket that still holds unread incoming data
makes Linux send RST and DROP the data it has not transmitted yet, the end-of-stream frame included. The receiver then reports "connection
reset" on a correct run. On loopback this almost never happens; on a path with 10 ms of delay 10 of 25 short runs failed. The fix is a
completion handshake: after Eos the sender half-closes and stays until the receiver's consumer has taken the whole stream and closed its
side (`finish_send` then `wait_closed`; a node sends Eos on every stream before it waits on any, so no cycle). Stage S1b of `gcp-check.sh`
repeats short runs on a delayed path; S1c shows the stage fails against a build without the fix (`--features bug-no-linger`: 13 of 25 failed
against 0 of 25 with it).

## What this does NOT cover

One VM, one kernel: namespaces joined by a bridge have loopback-class bandwidth and netem delay and loss, no real NIC, no cross-zone path, no
congestion from other tenants, no packet reordering beyond netem's. One stream per connection (no multiplexing, so no head-of-line blocking
between streams of a query), no TLS, no compression, rows are raw 16-byte pairs and not Arrow batches, the coordinator and the query-level
rules of ADR 0004 (re-run a task, fence attempts) are not here: this is the stream layer under them. The in-process baseline is a bounded
channel, not the real `spikes/exchange-loom` exchange. Stage B (separate VMs in one zone, `netexchreal`) needs a multi-VM mode in
`orchestrate.sh` and does not exist yet.
