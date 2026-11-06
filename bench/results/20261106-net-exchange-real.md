# Network exchange between separate VMs (plan item 28, stage B, 2026-11-06)

Run `netexchreal-arm64-r2` (job `netexchreal`, the orchestrator's new multi-VM mode: one driver and 3 peers, all `n4a-highcpu-8`,
Neoverse-N3, one zone, the project's default network), code `de45457`. Cost: the report counts the driver only ($0.008 for 290 s on the
first run); with the three peers (created about a minute after the driver, deleted with it) each run is about $0.03 (counted by hand;
the report now includes peers, `4594110`). The same exchange binary (`spikes/net-exchange`, `netx`) as stage A, which ran in network
namespaces on one VM (`bench/results/20261105-net-exchange.md`). Script: `spikes/net-exchange/real-check.sh`; node 0 on the driver, node i
on peer i, started over ssh on the private network; exit times taken on the driver when each node's ssh session returns. Raw:
`bench/results/raw/20261106-netexchreal-arm64/`.

Round trip between the VMs: 0.27 to 0.30 ms (ping, average of 5). Every block started settled (load 0.15 to 0.16).

| check | result |
|---|---|
| S1 correctness, 20 million rows per node, credit 4 MiB, 3 reps | PASS: every node's rows and checksum equal `netx expect`, buffered and in-flight bytes within the credit; 1,702 to 1,857 MB/s across the cluster (slowest node 0.52 to 0.56 s); the in-process baseline (same shuffle, threads and channels, no sockets) on the driver: 3,182 MB/s |
| S3 credit sweep, 5 million rows per node | PASS at 64 KiB, 256 KiB, 1 MiB and 4 MiB: 757 to 792 MB/s, flat: at a 0.3 ms round trip even 64 KiB covers the bandwidth-delay product |
| S4 cancel by node 0 after 300 batches | PASS: every node exits as cancelled within 0.25 s; no process left on any VM |
| S5 kill -9 of node 3's process on its VM | PASS: every survivor fails with peer-lost 0.23 s after the kill (reset or broken pipe); none returns a result |
| S6 partition: node 3's VM drops the exchange's TCP port (ssh stays up) | PASS: every node exits with peer-lost 3.29 to 3.30 s after the drop, the liveness timer's 3 s plus one ping interval; no process left |

The first run the same day (`netexchreal-arm64`, raw `first-run/`) passed the same scenarios with the same numbers; its two
"no process left" checks failed because the check, run through ssh, counted its own shell on each peer (3 "left" with none left). Fixed and
shown with a counting test in `de45457`.

Compared with stage A (one VM, network namespaces): the result checks, cancel, kill and partition behave the same on a real network; the
cluster moves 1.7 to 1.9 GB/s here against about 4 GB/s in the namespaces (loopback-class links there, 8-vCPU VMs and the GCP network here).

What this does not show: more than 4 nodes; machines in different zones or a wide-area link (the 0.3 ms round trip makes the credit sweep
flat; stage A's netem profiles covered the delay-bound cases); x86-64 VMs; Arrow batches (the exchange moves raw rows); real query plans
(the shuffle is synthetic); packet loss on a real network (none was injected or seen).
