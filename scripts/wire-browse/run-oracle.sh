#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
export TMPDIR="$WORK/wire-browse-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-browse-${PG_MAJOR:-default}"
# PG_MAJOR is explicit; PG15 remains the Debian 12 default oracle.
if [ -n "${PG_MAJOR:-}" ]; then
    pg_bin="/usr/lib/postgresql/$PG_MAJOR/bin"
else
    pg_bin=$(pg_config --bindir)
fi
"$pg_bin/postgres" --version
if [ -n "${PG_MAJOR:-}" ]; then
    "$pg_bin/postgres" --version | grep -E "PostgreSQL\) $PG_MAJOR\."
fi
pg_data="$WORK/wire-browse-pg-${PG_MAJOR:-default}"
[ ! -e "$pg_data" ] || { echo "refusing to reuse oracle cluster" >&2; exit 1; }
"$pg_bin/initdb" -D "$pg_data" -A trust -U nata --no-locale --encoding=UTF8 > "$RESULTS/wire-browse-${PG_MAJOR:-default}/initdb.log"
"$pg_bin/pg_ctl" -D "$pg_data" -l "$RESULTS/wire-browse-${PG_MAJOR:-default}/postgresql.log" -o "-h 127.0.0.1 -p 55432 -k $TMPDIR" start
trap '"$pg_bin/pg_ctl" -D "$pg_data" stop -m immediate' EXIT
"$pg_bin/createdb" -h 127.0.0.1 -p 55432 -U nata nata
"$pg_bin/psql" -v ON_ERROR_STOP=1 -h 127.0.0.1 -p 55432 -U nata -d nata -f "$root/scripts/wirecapture/fixture.sql" > "$RESULTS/wire-browse-${PG_MAJOR:-default}/fixture.log"
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/compare.py" \
    --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" \
    --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' \
    --work "$RESULTS/wire-browse-${PG_MAJOR:-default}/comparison"
