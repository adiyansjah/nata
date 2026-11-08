#!/usr/bin/env bash
# Read-only heavy probe, using this VM's metadata identity; no keys or token files.
# Run once before the job and again after upload, before cleanup.
set +x
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
set -a
. "${NATA_OBJ_ENV_FILE:?set the private runtime env file path}"
set +a
: "${NATA_OBJ_BUCKET:?}" "${NATA_OBJ_PREFIX:?}"
exec python3 -I "$HERE/gcs-diagnose.py" "$@"
