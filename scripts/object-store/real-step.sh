#!/usr/bin/env bash
# Filter stdout/stderr and file artifacts BEFORE the heavy runner saves results.
set +x
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
set -a
. "${NATA_OBJ_ENV_FILE:?set the private runtime env file path}"
set +a
export CARGO_BUILD_JOBS=4
exec python3 -I "$HERE/redact.py" --run "$@"
