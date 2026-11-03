# Running jobs on a Spot VM (GCP)

Runs `job.sh` on a cheap, reclaimable Spot VM and survives the VM being taken away. Nothing here runs by
itself: `orchestrate.sh run` needs an explicit dollar approval (see "Cost and safety"), and the rule in
`docs/way-of-working.md` (section 8, "Cloud spend") applies: ask first, show the cost, wait for a yes.

## How it survives a preemption

A Spot VM can be reclaimed with about 30 seconds' notice. So the VM is treated as disposable and all state
lives somewhere that outlives it:

| piece | where | survives preemption? |
|---|---|---|
| VM and boot disk | Spot, termination action DELETE | no (recreated) |
| toolchain, build cache, step markers, results | a separate 30 GB **data disk**, kept until the end of the run | yes |
| a copy of state, results and logs | your machine, `bench/gcp-runs/<run>/`, refreshed every 2 minutes | yes |

- **Steps** (`lib.sh`): every unit of work is a step with an atomic done-marker on the data disk. A step killed
  half way has no marker and simply runs again; a finished one is skipped. Long sweeps are split into shards so a
  preemption loses at most one shard. A step that *fails* is recorded and never retried, so a bug cannot loop and
  burn money. A benchmark is one step: if it is interrupted it restarts from the beginning, on one boot.
- **Orchestrator** (`orchestrate.sh`, on your machine): creates the data disk, then a VM, watches it, copies the
  state back, and when the VM vanishes it creates a new one on the same data disk; the startup script finds the
  source and `START` already on the disk and resumes the job. The source is uploaded once.
- **Machine chain:** `--machines "n4a-highcpu-8 c4a-highcpu-8"` is tried in order when a type is out of stock or
  over quota. A chain must use one disk class (the newer ARM families need Hyperdisk, T2A needs persistent disk).
  If no VM can be created in a zone and nothing has run there, the empty data disk moves to the next zone.
- **Preemption notice:** the VM polls the metadata server and sends the job SIGTERM, so it leaves clean markers.

## Cost and safety

1. `orchestrate.sh plan` prints everything a run would create and its **worst-case cost**, and calls nothing.
2. `orchestrate.sh run` refuses unless `--approved-usd` is at least that worst case. The worst case uses the
   dearest machine of the chain for all allowed VM-hours, plus disks, the external IP and a Hyperdisk allowance.
   Prices come from `prices.env` (with their source and date); recompute them before a run.
3. Total VM time is capped (`--max-vm-hours`), preemptions are capped (`--max-preemptions`), and each VM carries
   its own `--max-run-duration`, so it deletes itself even if the orchestrator dies.
4. Everything created is labelled `nata-run=<id>`. On every way out (success, failure, Ctrl-C, `kill`) the script
   deletes by that label, then lists to prove nothing is left, and writes `report.txt`. Resources of other runs
   or other work in the project are never touched. If the orchestrator dies, at most the data disk remains
   (`./orchestrate.sh cleanup --run-id <id>` removes it).
5. The VM has no service account and no scopes, project SSH keys are blocked, and login uses a throwaway key
   placed in the instance's own metadata, so the project's SSH settings are not changed.

## Use

```bash
scripts/gcp/orchestrate.sh plan --job ci                                  # look first; calls nothing
scripts/gcp/orchestrate.sh run  --job ci --approved-usd 0.25              # only after the human said yes
scripts/gcp/orchestrate.sh status --run-id <id>                           # what is still labelled
scripts/gcp/orchestrate.sh cleanup --run-id <id>                          # delete it all
```

Jobs: `ci` (workspace tests and the operator-protocol tests six times), `bench` (SF1 data, kernel benchmark three
times, pull-versus-push, v2), `genops` (the generic-operator experiment: unit tests, correctness checks, benchmarks, DuckDB timings), `all`. Edit `job.sh` to change them.

## What has been verified, and what has not

- **Tested locally, with a fake `gcloud` and `ssh`** (`test/test-orchestrate.sh`, 27 checks, nine deliberate
  breakages of the script each caught): the approval gate; preemptions and resuming on one data disk; the machine
  chain; moving zones; a failing job; the VM-hour budget; `kill` and Ctrl-C; cleanup leaving other runs' resources
  alone; no call to the real `gcloud`. `test/test-steps.sh` tests the step runner against SIGTERM and SIGKILL.
- **Checked offline against the real `gcloud` help text:** every flag the script passes exists.
- **Verified on real Google Cloud** by two runs (`bench/results/20261104-arm64-run1-operator-protocol.md`, `20261104-arm64-run2-benchmarks.md`): Spot creation of N4A and C4A VMs despite `PREEMPTIBLE_CPUS = 0`, Hyperdisk boot and data disks, the startup script mounting the data disk, SSH with the throwaway key, upload, resume markers, state copy-back, and label-based cleanup. Jobs took 3 and 6 minutes.
- **Preemption recovery verified** by a deliberate loss of the VM mid-job (`bench/results/20261104-arm64-run3-preemption-test.md`): a new VM on the same data disk resumed the job, with the interrupted step run again and nothing else repeated.
- **Not yet exercised on real Google Cloud:** the 30-second notice path (the hard loss tested gave no warning), the machine fallback chain, moving zones, `kill` mid-run, and whether Hyperdisk's default IOPS and throughput stay within the free baseline (the cost bound includes an allowance for that).
