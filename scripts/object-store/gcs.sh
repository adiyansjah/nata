#!/usr/bin/env bash
# Real-bucket steps for the parent; read the write-test env file at runtime via
# NATA_OBJ_ENV_FILE (absolute private path, supplied by the parent, never logged).
set -euo pipefail
unset AI_AGENT
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
set -a; . "${NATA_OBJ_ENV_FILE:?set the private runtime env file path}"; set +a
: "${NATA_OBJ_BUCKET:?}" "${NATA_OBJ_PREFIX:?}" "${WORK:?}"
NATA=${NATA_BIN:-$REPO/target/release/nata}
DATA=${NATA_CHECK_DATA:-$WORK/data/tpch-sf0.01}
case "${1:?step}" in
  smoke)
    "$NATA" sql --csv -c "SELECT l_orderkey FROM 'gs://$NATA_OBJ_BUCKET/$NATA_OBJ_PREFIX/sf1/mid1/part-0000.parquet' WHERE l_orderkey > 0 LIMIT 10" | head -3 ;;
  upload-check)
    scripts/gen-tpch.sh 0.01 "$DATA"
    scripts/gen-sql-check-data.sh "$DATA"
    rm -rf "$WORK/check-up"; mkdir -p "$WORK/check-up/check"; cp -r "$DATA/." "$WORK/check-up/check/"
    OSB=${CARGO_TARGET_DIR:-$WORK/target-osb}/release/osb
    "$OSB" upload --dir "$WORK/check-up" ;;
  check)
    bash "$HERE/check.sh" "$DATA" "gs://$NATA_OBJ_BUCKET/$NATA_OBJ_PREFIX/check" ;;
  cells)
    python3 -I "$HERE/cells.py" --nata "$NATA" --layouts "${2:?sf1|sf10}" --rounds "${NATA_CELLS_ROUNDS:-5}" --workers 8 --spike "${NATA_SPIKE_BIN:-${CARGO_TARGET_DIR:-$WORK/target-osb}/release/osb}" --expected "${NATA_OSB_OUT:-${RESULTS:-$WORK}/osb}/expected.txt" --out "${RESULTS:-$WORK}/cells-$2.csv" ;;
  *) echo "unknown step"; exit 2 ;;
esac
