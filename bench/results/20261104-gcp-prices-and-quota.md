# GCP lookups for the ARM64 run (read-only)

Done 2026-11-04 with the user's `gcloud` login, the user's GCP project (id omitted from this public file), after the user said yes to read-only lookups. Nothing was created, started or changed. The access token was used for the API call and never printed.

## Sources

- **Prices:** Cloud Billing Catalog API, Compute Engine service `6F81-5844-456A`, USD, fetched 2026-11-03 21:28 UTC (7 pages, 33,567 SKUs); the SKUs used have effective time 2026-11-03T08:00:00Z. Region SKUs listed under `us-central1` ("Americas"). The Google pricing web pages could not be read (JavaScript-rendered), so this API is the price source.
- **Availability and quota:** `gcloud compute machine-types list` and `gcloud compute regions describe us-central1`.
- **Billing and network:** `gcloud billing projects describe`, `gcloud compute networks list`, `gcloud compute firewall-rules list`.

## Prices (us-central1, USD)

| item | price | SKU |
|---|---:|---|
| C4A Arm core, Spot | 0.01601 / h | F7F5-A97E-ABE4 |
| C4A Arm RAM, Spot | 0.00182 / GiB-h | 99F8-1896-9379 |
| C4A Arm core, on-demand | 0.03086 / h | B568-2638-C170 |
| C4A Arm RAM, on-demand | 0.00351 / GiB-h | 3431-39A9-5749 |
| T2A Arm core, Spot | 0.01278 / h | E199-F647-CA44 |
| T2A Arm RAM, Spot | 0.00174 / GiB-h | EA81-716E-0AA9 |
| T2A Arm core, on-demand | 0.02490 / h | 7F42-2C35-6872 |
| T2A Arm RAM, on-demand | 0.00340 / GiB-h | C5C1-548B-71FE |
| Balanced persistent disk | 0.10 / GiB-month | 6AE1-525F-8B80 |
| External IPv4, Spot VM | 0.0025 / h | 4AF8-7C1F-39C4 |
| External IPv4, standard VM | 0.005 / h | C054-7F72-A02E |

Machine price = vCPUs x core price + GiB x RAM price (standard shapes have 4 GiB per vCPU).

## Estimates for the proposed run (boot disk 20 GiB, data disk 30 GiB kept 0.5 h longer than the VM, external IP, egress negligible)

| machine | kind | VM-hours | VM $/h | total |
|---|---|---:|---:|---:|
| c4a-standard-4 (4 vCPU, 16 GiB) | on-demand | 1.5 | 0.1796 | 0.289 |
| c4a-standard-8 (8 vCPU, 32 GiB) | Spot | 1.5 cap | 0.1863 | 0.296 |
| t2a-standard-16 (16 vCPU, 64 GiB) | Spot | 1.0 | 0.3158 | 0.327 |
| t2a-standard-16 | Spot | 1.5 cap | 0.3158 | 0.490 |
| c4a-standard-16 (16 vCPU, 64 GiB) | Spot | 1.0 | 0.3726 | 0.384 |
| c4a-standard-16 | Spot | 1.5 cap | 0.3726 | 0.575 |

If the orchestrator died with the VM already deleted, only the 30 GiB data disk would remain: 3.00 USD per month (0.10 per day).

## Availability, quota, billing, network

- C4A machine types (4, 8, 16 vCPU) exist in us-central1-a, -b, -c and -f; T2A in -a, -b and -f.
- Regional quotas: `CPUS` 200 (usage 0), `T2A_CPUS` 96, `INSTANCES` 24, `IN_USE_ADDRESSES` 8, `DISKS_TOTAL_GB` 4096. **No `C4A_CPUS` entry is listed** and **`PREEMPTIBLE_CPUS` is 0**. I believe Spot VMs then draw on the regular CPU quota, and that C4A counts against `CPUS`, but I did not verify either; if wrong, VM creation fails with a quota error and nothing is created.
- Billing is enabled on the project (billing account id omitted from this public file).
- The `default` network exists and `default-allow-ssh` (tcp:22 from anywhere) is enabled, so `gcloud compute ssh` should work. The project also has firewall rules from earlier, unrelated work, so it holds other resources: any run must act only on resources it labelled itself.

## Limits

- Prices come from the catalog at the time shown and can change. Spot availability in the chosen zone, and the preemption rate, are not knowable in advance.
- Estimates assume the run finishes within the stated VM-hours; the orchestrator's VM-hour cap is what bounds the worst case.
