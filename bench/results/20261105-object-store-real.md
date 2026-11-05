# Item 27: the object-store read path against a REAL bucket (2026-11-05)

The simulated-store results of `20261104-object-store-scan.md` said how the read path behaves against a store with a modelled latency tail. This is the same harness (`spikes/object-store-scan/`) against
**Google Cloud Storage**: a new Standard-class bucket in `us-central1`, read from VMs in `us-central1-a` through GCS's native API with `object_store` 0.14, authenticating with a read-only service account attached to the VM
(no key exists anywhere). Data: TPC-H SF1 `lineitem`, 6,001,215 rows, written four ways (795 MiB each in total): `big1` (one file, 4 row groups), `mid1` (one file, 49 row groups), `small1` (one file, 293 row groups),
`files100` (100 files). Two queries per layout (`sum`, `group`), 5 rounds, policies A to E, a control twin and a local-disk control. **No DuckDB reference on the real store** (see the end).

Runs `objscan-real2-arm64` (`n4a-highcpu-8`, Neoverse-N3, 8 vCPU) and `objscan-real2-x86` (`c3d-highcpu-8`, AMD EPYC 9B14), 2026-11-05, job `objscan` with `orchestrate.sh --vm-service-account`. Each: exit status 0, 419 passing
checks (unit tests and the simulated-store tests with their known-bad versions), `RESULT bench REAL: 320 runs, 0 not ok`, leak scan clean (no configured value of the store anywhere in the results), 1 incarnation, 0 preemptions, resources
left after cleanup: none. VM time 2,210 s and 2,363 s: **$0.060 and $0.045**. (A first pair of runs, $0.039 and $0.030, ended early at two defects fixed since: the DuckDB reference arm could not authenticate, and the bucket prefix
collided with a local directory name in the leak scan. Storage and requests cost cents; not itemised.) Raw: `bench/gcp-runs/objscan-real2-{arm64,x86}/results/object-store-scan/` (not in git).

## Safety result: the VM's identity cannot write

Both VMs tried to write a probe object to the test prefix with their own identity and were refused: `READ-ONLY OK: the write was refused ... lacked the necessary privileges`. The service account holds `objectViewer` on that one bucket
and no project role. (It could not be tested from outside: acting as it needs a role on the operator's account, so the job tests itself on the VM.)

## What the real store looks like

- **Latency of one request:** time to first byte **p50 25 to 36 ms** in every cell on both machines (median 29 to 30 ms), **p99 42 to 188 ms** (ARM64 worst 188 ms, x86-64 worst 99 ms); total request time p50 **26 to 60 ms** for the
  requests of these layouts (the largest merged ones about 64 MiB). 1-minute load at the start of each run: ARM64 min 0.0, median 0.2, max 0.4.
- **The simulated store was conservative.** Over the 40 cells of policies A to E the real store was faster than the simulated one with the 'gcs' latency profile in every cell: wall time real / simulated **0.52 to 0.95 (median 0.72)
  on ARM64, 0.49 to 0.94 (median 0.74) on x86-64**. The conclusions drawn from the simulated store hold on the real one, and its numbers were pessimistic by about a quarter.
- **Request parallelism is the dominant factor.** Sixteen requests in flight (B) against one at a time (A): **2.3 to 13.5 times faster**, the more so the more requests a query needs (big1 sum 3.4x ARM64 / 2.3x x86-64; small1 group 13.0x / 13.5x;
  files100 group 12.2x / 13.4x).
- **Merging nearby ranges and reading several row groups per unit (E) wins where there are many row groups, at a price in bytes.** Best policy per layout (ARM64 wall, x86-64 in brackets): `small1` E 0.71 to 0.75 s (0.74 to 0.86) against B 2.66 to 3.39 s,
  at 303 to 411% extra bytes fetched (192 to 197 MiB for a query that needs 38 to 45 MiB); `mid1` E 0.26 to 0.29 s (0.30 to 0.43), 13 to 116% extra; `files100` C or E about 1.05 s with 13 to 121% extra; `big1` B, C or D 0.18 to 0.22 s (0.24 to 0.31) with
  no waste. A policy that wins on time can lose on money and bandwidth in a cloud: the waste column matters.
- **Against local disk, remote reads are slower in proportion to the number of requests**, not to the bytes: the best real policy is 1.3 to 2.1 times slower than local on `big1` (9 requests), 2.2 to 4.8 times on `mid1` (the 4.8 is the ARM64 `sum` cell, where E reads 116% extra bytes), 4.6 to 13 times on `small1` and
  5.4 to 19.5 times on `files100`.
- **The two machines agree on the shape.** Differences are in the CPU-bound cells: the local-disk control is 2 to 3 times slower on the x86-64 VM (`big1` sum 0.231 s against 0.083 s), and so are the real-store `big1` cells (0.31 against 0.18 s). Policy rankings
  per layout agree except where two policies are within noise. **The noise is not small in every cell:** the median wall of B over B-twin (the same configuration twice) is 1.00 to 1.14 on ARM64 and 0.99 to 1.23 on x86-64 across the 8 cells, so a difference between two policies of under about 15% (ARM64) or 25% (x86-64) is not a result in the cells where the twin differs most. The large effects above (3x to 13x) are far outside it; the order of B, C and D on `big1` is not.

## What this does not show

- **No DuckDB reference over the real store.** DuckDB reads a store through its S3 layer, which needs an HMAC key pair; the VM's service account cannot provide one, and an access token was not put into DuckDB. On the simulated store DuckDB was faster than
  our best policy on `mid1` and `small1`; whether it is on the real store is unknown. Our arms are compared with the simulated store and with each other only.
- One bucket in one region, Standard class, read from the same region (no cross-region path, no other storage class, no multi-region bucket); 795 MiB, SF1; one query at a time (no concurrent queries sharing the connections); the tail beyond p99 is
  noise-dominated at 5 rounds; request counts are costed at list prices from memory, not from a bill.
- The latency of GCS for these sizes (about 30 ms to first byte) is for ONE bucket at ONE time of day. Another day may differ.
- Credentials come from the metadata server; the cost of the first token fetch is inside the first request of each run.
