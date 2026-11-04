#!/bin/bash
# Verification-plan item 30 on a clean VM (GCP). usage: spikes/parquet-write/gcp-check.sh <results-dir> [rows]   (default 500000)
exec "$(dirname "$0")/check.sh" "$1" "${2:-500000}"
