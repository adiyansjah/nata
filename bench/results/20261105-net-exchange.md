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

- **Segmentation offload state: unconfirmed in all three runs.** Each run printed "ethtool not installed". The first explanation I wrote (the startup script installed
  it without refreshing the package lists) was a guess, and a fix for it did not change the message. The defect that is PROVEN: `gcp-check.sh` looked for ethtool with
  `command -v ethtool` as the unprivileged job user, and ethtool lives in `/usr/sbin`, which is not on that user's PATH (checked on the development machine, where it is installed and
  found only through sudo). So it may have been installed on the VMs all along and simply not found, in which case offload was never switched off. With offload on, netem drops a whole
  super-segment (up to 64 KiB) as one packet, so loss is rarer and burstier than real packet loss at the same percentage: the **loss profiles (region, wide) are indicative only**. Delay
  and rate profiles are not affected. The detection is fixed in `gcp-check.sh` (through sudo, and the state is read back and checked: `CHECK offloads-off`); no GCP run has used it yet.
- Loopback-class bandwidth between namespaces on one VM: no real NIC, no cross-zone path, no other tenants, no reordering beyond netem's.
- One stream per connection (no multiplexing, so no head-of-line blocking between a query's streams), no TLS, no compression, raw rows, not Arrow
  batches. The in-process baseline is a bounded channel, not the real `spikes/exchange-loom` exchange.
- This is the stream layer under ADR 0004's query-level rules. Re-running a task under a new attempt, fencing, the coordinator, leases and `release`
  are not here. The checks cover the transport assumptions those rules rely on: a failed peer becomes a prompt error, a stream is never silently
  incomplete, a cancel reaches every node, a silent peer is noticed.
- x86-64: see the re-run below (one run).

## Re-run on ARM64 and x86-64 (`netexchsim-arm64-r2`, `netexchsim-x86-r2`, 06:38 to 06:55 UTC, same day)

Same job and code (`6a57bee` plus the startup fix). Both finished with `VERDICT PASS` and **24 of 24 checks on each machine**; 1,003 s and 996 s of VM time, resources left after cleanup: none,
**$0.027** (`n4a-highcpu-8`) and **$0.019** (`c3d-highcpu-8`, AMD EPYC 9B14). Mutants 8 of 8 caught on both. The exit-race stage: 0 of 30 runs failed with the fix on both; **12 of 30 (ARM64) and 17 of 30
(x86-64) failed against the build without it**, so the stage can fail on both architectures.

| result | first run, ARM64 | re-run, ARM64 | re-run, x86-64 |
|---|---|---|---|
| clean, MB/s moved (load at start) | 3,087 (0.38) | 3,478 (0.36) | 1,616 (**1.84**) |
| lan, 0.2 ms | 3,077 | 3,127 | 1,524 (load 1.77) |
| zone, 1 ms | 1,054 | 1,085 | 1,069 |
| **region, 10 ms + 0.05% loss** | **980** | **82** | **698** |
| wide, 25 ms + 1% loss | 5 | 4 | 5 |
| capped, 10 ms + 400 mbit | 136 | 134 | 150 |
| credit sweep at 20 ms RTT, 64 KiB / 256 KiB / 1 MiB / 4 MiB / 16 MiB | 31 / 142 / 490 / 968 / 954 | 31 / 141 / 486 / 970 / 968 | 31 / 141 / 488 / 792 / 794 |
| cancel: nodes report after | 7 to 8 ms | 7 to 8 ms | 33 to 34 ms |
| kill -9: survivors fail after | 0.02 to 0.09 s | 0.02 to 0.05 s | 0.03 to 0.07 s |
| partition (timer 3 s): all nodes fail after | 3.01 to 3.03 s | 3.03 s | 3.03 s |
| exit-race build without the fix fails | 17 of 30 | 12 of 30 | 17 of 30 |

**Corrections to what the first run suggested.**
- **The region profile (random loss) is not reproducible: the same job gave 980, 82 and 698 MB/s.** The first record's region row (980 MB/s) must not be quoted as a measurement of "10 ms plus 0.05% loss". Random loss with TCP gives
  a few retransmission stalls whose count varies from run to run, and offload may have been on (above). The reproducible results are the profiles without random loss: zone, capped and the credit sweep agree to within about 3% on ARM64
  across the two runs. The wide profile (1% loss) is slow every time (4 to 5 MB/s) for the TCP reason given above.
- **The x86-64 machine was not quiet at the start of the clean, lan, zone, region and wide profiles** (1-minute load 1.77 to 2.37; capped started at 0.00). The cause is not known. The x86-64 clean and lan numbers (1,616 and 1,524
  MB/s against 3,478 and 3,127 on ARM64) are therefore not comparable with ARM64; the credit sweep at 4 MiB and above is also lower on x86-64 (792 against 970 MB/s). Delay-bound results (zone, capped, the sweep below 4 MiB) agree across the machines.
- The cancel took 33 ms on x86-64 against 7 ms on ARM64 (the time the nodes reported after they started; one run each).

Everything in the "What the namespace runs found" list holds on both architectures: exact results, bounded buffering (largest inbound queue 244,432 and 245,872 bytes of 262,144), fast failure detection, the liveness timer firing at its setting, and the exit race with its fix.
