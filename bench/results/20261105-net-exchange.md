# Item 28, stage A: a real TCP exchange between processes in network namespaces (2026-11-05)

**Verdict: PASS (24 of 24 checks), on a SIMULATED network: N processes in Linux network namespaces on ONE Spot VM, joined by a bridge, with `tc netem`
delay, loss and rate on every node's egress. This is not a network between machines (stage B).** Code: `spikes/net-exchange/` (std only, threads and
blocking sockets), commit `8b36430`. Run `netexchsim-arm64`, job `netexchsim`, one `n4a-highcpu-8` Spot VM (Neoverse-N3, 8 vCPU, kernel
6.1.0-54-cloud-arm64), 05:26 to 05:43 UTC: 1,008 s of VM time, 1 incarnation, 0 preemptions, resources left after cleanup: none, **$0.028**.
Raw outputs: `bench/gcp-runs/netexchsim-arm64/results/net-exchange/` (not in git: `summary.txt`, `results.md`, `runs/`).

## What was tested

4 nodes, all-to-all hash shuffle, 20,000,000 rows per node of 16 bytes (raw key/value pairs, not Arrow batches), 4 MiB of credit per stream unless
stated. Every node's rows and checksum must equal a pure function of the parameters (`netx expect`), and the largest inbound queue and the largest
in-flight volume of every node must stay within the credit. 22 unit tests on loopback (wire format; hostile peers: bad magic, unknown kind, absurd
length, truncation, gaps/repeats/reorders, wrong Eos count, a sender that ignores its credit, garbage, silence, a crash) and 8 deliberate bugs
(`mutants/run.sh`): baseline green twice, **8 of 8 caught**.

## Findings

**1. Correct on every profile, bounded buffering.** All 6 profiles x 3 reps and all 5 credit sizes gave exact results on every node. With slow
consumers (300 microseconds per batch), credit 256 KiB, region profile: the largest inbound queue was 245,408 bytes (limit 262,144) and senders
waited 4,104 times. A control with a huge credit buffers far more than the small credit (unit test), so the bound is not an accident of a fast consumer.

**2. A process that exits right after its last frame can lose that frame (a bug the loopback tests could not see).** Closing a socket that still holds
unread incoming data makes Linux send RST and DROP the data it has not transmitted yet, the end-of-stream frame included; the peer reports "connection
reset" on a correct run. First found on a development machine (10 of 25 short runs failed on a 10 ms path; loopback almost never shows it). The fix is
a completion handshake: after Eos the sender half-closes and waits until the receiver's consumer has taken the whole stream and closed its side
(Eos is sent on every stream before any wait, so no cycle). On GCP: **0 of 30 short runs failed with the fix, 17 of 30 failed against a build without
it** (feature `bug-no-linger`), so the stage can fail.

**3. The credit must cover bandwidth x round-trip time.** 20 ms round trip (10 ms each way), 12 streams, 10,000,000 rows per node:

| credit per stream | seconds | MB/s moved, all nodes |
|---|---|---|
| 64 KiB | 15.604 | 31 |
| 256 KiB | 3.391 | 142 |
| 1 MiB | 0.979 | 490 |
| 4 MiB | 0.496 | 968 |
| 16 MiB | 0.503 | 954 |

Throughput grows with the credit (64 KiB per 20 ms is 3.3 MB/s per stream, 12 streams about 39 MB/s, measured 31) until about 1 GB/s, where 4 and
16 MiB are the same. The plateau is not explained (CPU, the bridge and the per-node sender thread are candidates; not investigated).

**4. Throughput by network profile** (median of 3; credit 4 MiB; "load" is the 1-minute load average at the start of the profile):

| profile | netem on each egress | load | seconds | MB/s moved, all nodes |
|---|---|---|---|---|
| clean | none | 0.38 | 0.311 | 3,087 |
| lan | delay 0.2 ms | 0.43 | 0.312 | 3,077 |
| zone | delay 1 ms +- 0.2 | 0.43 | 0.911 | 1,054 |
| region | delay 10 ms +- 2, loss 0.05% | 0.48 | 0.980 | 980 |
| wide | delay 25 ms +- 5, loss 1% | 0.44 | 209.911 | 5 |
| capped | delay 10 ms, rate 400 mbit | 0.15 | 7.041 | 136 |

In-process baseline (same shuffle between threads, bounded channels, no sockets): 3,620 MB/s. So the socket exchange reaches 85% of it on a clean
path, and 29% at 1 ms of delay (the credit is then a fraction of the bandwidth-delay product; finding 3). **The wide row is TCP, not the exchange:** 12
streams at 1% loss and a 50 ms round trip give about 0.4 MB/s each, which is what the TCP loss formula predicts (about 1.22 x MSS / (RTT x sqrt(loss))). It
needs several connections per path, a different congestion control or a smaller loss rate, not a change to the exchange; it is also the profile most
affected by the caveat below.

**5. Failures are detected fast and never end in a wrong or partial answer.**
- **Cancel** (node 0 cancels after 300 batches; each node had 400,000,000 rows to send; clean network): node 0 reported "cancelled" 7 ms after it started
  and the three others 8 ms after they started, all four "cancelled", all four processes gone 0.02 s after the launch. So the cancel took about 1 ms to reach
  the other nodes here; it was not measured on a path with delay.
- **kill -9 of node 3** mid-query, clean and region profile: every survivor exits "peer lost" (code 4) 0.02 to 0.09 s after the kill (the kernel closes
  the sockets at once); none exits 0; no process is left.
- **Partition** (node 3's link goes down, its sockets stay open; liveness timeout 3,000 ms, ping every 250 ms): all four nodes, the partitioned one too,
  exit "peer lost" 3.01 to 3.03 s after the fault ("silent for 3001 ms"), never before the timer. Only the liveness timer can see this case.
- Cosmetic: some messages read "peer lost: peer lost: ..." (a doubled prefix).

## Caveats

- **Segmentation offload was ON in this run** (`ethtool` was not installed on the VM: the startup script installed it without refreshing the package
  lists, so the install silently failed; fixed after the run in `vm-startup.sh`, with a test). With offload on, netem drops a whole super-segment (up to
  64 KiB) as one packet, so loss is rarer events and coarser bursts than real packet loss at the same percentage. The loss-bearing profiles (region, wide)
  are therefore indicative, not comparable with real packet loss. Delay-only and rate profiles are not affected. A re-run with offload off is cheap
  (this run cost $0.028) and not done yet.
- Loopback-class bandwidth between namespaces on one VM: no real NIC, no cross-zone path, no other tenants, no reordering beyond netem's.
- One stream per connection (no multiplexing, so no head-of-line blocking between a query's streams), no TLS, no compression, raw rows, not Arrow
  batches. The in-process baseline is a bounded channel, not the real `spikes/exchange-loom` exchange.
- This is the stream layer under ADR 0004's query-level rules. Re-running a task under a new attempt, fencing, the coordinator, leases and `release`
  are not here. The checks cover the transport assumptions those rules rely on: a failed peer becomes a prompt error, a stream is never silently
  incomplete, a cancel reaches every node, a silent peer is noticed.
- ARM64 only. x86-64 not run.
