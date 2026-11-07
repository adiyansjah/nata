#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
export TMPDIR="$WORK/wire-browse-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-browse"
pg_bin=$(pg_config --bindir)
pg_data="$WORK/wire-browse-pg"
"$pg_bin/initdb" -D "$pg_data" -A trust -U nata --no-locale --encoding=UTF8 > "$RESULTS/wire-browse/initdb.log"
"$pg_bin/pg_ctl" -D "$pg_data" -l "$RESULTS/wire-browse/postgresql.log" -o "-h 127.0.0.1 -p 55432 -k $TMPDIR" start
trap '"$pg_bin/pg_ctl" -D "$pg_data" stop -m immediate' EXIT
"$pg_bin/createdb" -h 127.0.0.1 -p 55432 -U nata nata
psql -h 127.0.0.1 -p 55432 -U nata -d nata -f "$root/scripts/wirecapture/fixture.sql" > "$RESULTS/wire-browse/fixture.log"
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/compare.py" \
    --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" \
    --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' \
    --work "$RESULTS/wire-browse/comparison"
