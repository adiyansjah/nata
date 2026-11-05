Raw outputs of the GCP run icebergmodel-arm64 (job `icebergmodel`, one n4a-highcpu-16 Spot VM, 2026-11-05 03:28-04:19 UTC).
Subset of bench/gcp-runs/icebergmodel-arm64/ (local, not committed). What was left out or changed:
- gcloud.log, the throwaway login key and its metadata file are not copied. state/: only the *.done markers and job.finished are copied.
- report.txt: the line that names the project is removed. machine-f5325ddf.txt: the line holding the project path of the machine type is removed.
- model/*.tlc.txt: the logs of the 11 canaries, three-rest-snap-i4 (and iceberg-model/model-lint.txt) are TRUNCATED to their last 50 lines
  (a first line says so, and says how long the original was). three-store-ser, live-2 and live-3 are whole (70 lines at most).
  The truncated logs lose the counterexample traces (a canary's trace is the proof that its situation is reachable); the one-line verdict of each
  run is in model-summary.txt and in the model/*.row files, which are whole.
