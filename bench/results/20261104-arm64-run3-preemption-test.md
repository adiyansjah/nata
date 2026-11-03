# ARM64 run 3: a deliberate preemption, and recovery on real Google Cloud

Run id `nata-run3` (`scripts/gcp/orchestrate.sh run --job ci`, with `PULL_SECONDS=15 POLL_SECONDS=10`), 2026-11-03 22:18-22:23 UTC (2026-11-04 local). The aim was to exercise, on real Spot hardware, what until now only the fake-`gcloud` tests covered: losing the VM in the middle of the job.

## Machine, place and cost

| | |
|---|---|
| Machine type | `n4a-highcpu-8`, Spot (the chain contained only this type); two VMs, `nata-run3-vm1` and `nata-run3-vm2` |
| Zone | us-central1-a |
| Disks | 20 GB and 30 GB Hyperdisk Balanced; the 30 GB data disk was shared by both VMs |
| Cap / worst-case bound | 0.06 USD / 0.058 USD (the unverified Hyperdisk allowance of 0.03 USD is half of it) |
| VM time counted | 211 s over 2 VMs |
| Estimated cost | about 0.006 USD for the VM time (0.0984 USD/h x 211 s) plus a fraction of a cent for disks and IP; estimated from catalog prices, the bill was not checked |
| Left behind | nothing (verified by label after cleanup) |

## What was done

A watcher deleted `nata-run3-vm1` with `gcloud compute instances delete` once six steps had been copied back to the local machine. For a Spot VM with termination action DELETE that is the same observable event as a preemption: the VM and its boot disk vanish, the data disk survives. It is a hard loss with no warning, so the 30-second notice path (the VM sending SIGTERM to the job) was **not** exercised.

## Result

| time (UTC) | event |
|---|---|
| 22:18:41 | VM 1 created |
| 22:20:15 | boot `d73f4eb6` has finished `protocol-tests-3` |
| about 22:20:2x | VM 1 deleted from outside while `protocol-tests-4` was running |
| 22:20:26 | orchestrator: `ended: preempted` |
| 22:20:47 | VM 2 created on the same data disk (no second upload of the source) |
| 22:21:40 | boot `5d86212e` records its machine and resumes the job (about 75 s after the deletion, including boot and mount) |
| 22:21:55-22:22:24 | boot `5d86212e` finishes `protocol-tests-4` (from the start), `-5`, `-6` |
| 22:22:33 | job finished; cleanup verified at 22:22:58 |

- The report says 2 incarnations, 1 preemption, 11 steps completed. Every step has exactly one done-marker; the steps before the loss carry boot `d73f4eb6`, those after carry `5d86212e`; the interrupted step ran again from the beginning, as designed.
- The recovery needed no human action and no state was lost (the data disk, with its toolchain and step markers, survived).

## What this changes

- Verified on real Google Cloud: preemption recovery (recreate on the same data disk, resume from the markers), the machine record per boot, and that cleanup still removes everything after an incarnation change.
- Still not exercised on real Google Cloud: the 30-second notice path, the machine fallback chain, moving zones, and `kill` of the orchestrator mid-run.
