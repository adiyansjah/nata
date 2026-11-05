Raw outputs of the GCP run iceberg-full-arm64 (job `iceberg`, one n4a-highcpu-8 Spot VM, 2026-11-05 00:48-03:05 UTC).
Subset of bench/gcp-runs/iceberg-full-arm64/ (local, not committed). What was left out or changed:
- catalog.log (7.3 MB, the REST fixture's request log) and journals-rest.tgz / journals-store.tgz (200 KB): not copied.
- model-lint.txt, rust-build.txt and every mutants/*.tlc.txt: TRUNCATED to their last 50 lines (a first line says so).
  The model/*.tlc.txt progress logs are small (9 KB at most) and are whole.
- report.txt: the project id is replaced by <removed>. machine-vm1.txt: the line holding the project path of the machine type is removed.
- gcloud.log, the throwaway ssh key and state/ are not copied.
