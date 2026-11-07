#!/usr/bin/env bash
# Real-bucket steps of the walking skeleton (card 0006), for the parent's `heavy` job on a VM whose service account may READ the bucket. The bucket and prefix
# come from the git-ignored `spikes/object-store-baseline/gcs.env` (NATA_OBJ_BUCKET=, NATA_OBJ_PREFIX=; the write-test identity's env file, shipped with the
# tree) and are never printed or written to a result. Nata reads with the VM's metadata-server identity; the upload and cleanup steps of run-reference.sh use the
# write-test identity.
#
#   scripts/object-store/gcs.sh smoke        the card's example query over gs:// with default settings (first 3 rows)
#   scripts/object-store/gcs.sh upload-check copy the differential data directory under <prefix>/check (needs the write-test identity)
#   scripts/object-store/gcs.sh check        the four differential sets, Nata reading gs://<bucket>/<prefix>/check
#   scripts/object-store/gcs.sh cells <sf1|sf10>   Nata against DuckDB httpfs per cell, against the frozen ceilings
set -euo pipefail
unset AI_AGENT
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
set -a; . "$REPO/spikes/object-store-baseline/gcs.env"; set +a
: "${NATA_OBJ_BUCKET:?}" "${NATA_OBJ_PREFIX:?}" "${WORK:?}"
NATA=${NATA_BIN:-$REPO/target/release/nata}
DATA=${NATA_CHECK_DATA:-$WORK/data/tpch-sf0.01}
case "${1:?step}" in
  smoke)
    "$NATA" sql --csv -c "SELECT l_orderkey FROM 'gs://$NATA_OBJ_BUCKET/$NATA_OBJ_PREFIX/sf1/mid1/part-0000.parquet' WHERE l_orderkey > 0 LIMIT 10" | head -3 ;;
  upload-check)
    [ -d "$DATA" ] || { scripts/gen-tpch.sh 0.01 "$DATA" && scripts/gen-sql-check-data.sh "$DATA"; }
    rm -rf "$WORK/check-up"; mkdir -p "$WORK/check-up/check"; cp -r "$DATA/." "$WORK/check-up/check/"
    OSB=${CARGO_TARGET_DIR:-$WORK/target-osb}/release/osb
    "$OSB" upload --dir "$WORK/check-up" ;;
  check)
    bash "$HERE/check.sh" "$DATA" "gs://$NATA_OBJ_BUCKET/$NATA_OBJ_PREFIX/check" ;;
  cells)
    python3 -I "$HERE/cells.py" --nata "$NATA" --layouts "${2:?sf1|sf10}" --rounds "${NATA_CELLS_ROUNDS:-5}" --out "${RESULTS:-$WORK}/cells-$2.csv" ;;
  *) echo "unknown step"; exit 2 ;;
esac
