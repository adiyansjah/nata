#!/usr/bin/env bash
# The differential query sets (scan, sql, sql-differences, aggregate) against DuckDB twice: with Nata reading the local files, and with Nata reading the
# SAME bytes from object storage (card 0006, walking skeleton). The reference always reads the local directory.
#
#   scripts/object-store/check.sh <data-dir> [fake | s3://bucket/prefix | gs://bucket/prefix]
#
# <data-dir> is the directory the CI uses (TPC-H files plus sqlcheck/, see scripts/gen-tpch.sh and scripts/gen-sql-check-data.sh). `fake` (the default)
# serves a copy of it from an in-process loopback endpoint; an s3:// or gs:// prefix must already hold a copy (upload it with the identity that may write
# there, then run this with a read-only identity and the credentials in the environment). Exit code 0 only if every set passes in every mode.
set -u -o pipefail
unset AI_AGENT
data=${1:?usage: check.sh <data-dir> [fake|s3://...|gs://...]}
store=${2:-fake}
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
cargo build -q -p nata -p nata-check || exit 1
bin=${CARGO_TARGET_DIR:-$root/target}/debug
fail=0
for set in scan sql sql-differences aggregate; do
  for mode in local "$store"; do
    printf '== %-16s %s\n' "$set" "$mode"
    if [ "$mode" = local ]; then extra=(); else extra=(--object-store "$mode"); fi
    "$bin/nata-check" --data "$data" --queries "check/queries/$set" "${extra[@]}" 2>&1 | tail -3
    [ "${PIPESTATUS[0]}" -eq 0 ] || { echo "FAILED: $set / $mode"; fail=1; }
  done
done
exit $fail
