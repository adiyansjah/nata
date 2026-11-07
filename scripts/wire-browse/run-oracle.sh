#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
oracle_label="${PG_MAJOR:-default}${ORACLE_EXTRA:+-$ORACLE_EXTRA}"
export TMPDIR="$WORK/wire-browse-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-browse-$oracle_label"
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
pg_data="$WORK/wire-browse-pg-$oracle_label"
[ ! -e "$pg_data" ] || { echo "refusing to reuse oracle cluster" >&2; exit 1; }
"$pg_bin/initdb" -D "$pg_data" -A trust -U nata --no-locale --encoding=UTF8 > "$RESULTS/wire-browse-$oracle_label/initdb.log"
"$pg_bin/pg_ctl" -D "$pg_data" -l "$RESULTS/wire-browse-$oracle_label/postgresql.log" -o "-h 127.0.0.1 -p 55432 -k $TMPDIR" start
trap '"$pg_bin/pg_ctl" -D "$pg_data" stop -m immediate' EXIT
"$pg_bin/createdb" -h 127.0.0.1 -p 55432 -U nata nata
"$pg_bin/psql" -v ON_ERROR_STOP=1 -h 127.0.0.1 -p 55432 -U nata -d nata -f "$root/scripts/wirecapture/fixture.sql" > "$RESULTS/wire-browse-$oracle_label/fixture.log"
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/compare.py" \
    --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" \
    --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' \
    --work "$RESULTS/wire-browse-$oracle_label/comparison"

# Extra heavy gates share this exact VM and PostgreSQL cluster; trap owns shutdown.
case "${ORACLE_EXTRA:-}" in
  "") ;;
  resources) "$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/resources.py" --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" --work "$RESULTS/resources" --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' ;;
  resources-smoke) "$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/resources.py" --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" --work "$RESULTS/resources-smoke" --smoke --tables 64 1024 --widths 1 64 --workers 1 8 --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' ;;
  p16) "$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-browse/p16.py" --nata "$WORK/target-p16/release/nata" --baseline "$WORK/target-p16-baseline/release/nata" --jar "$WORK/wire-skeleton-pgjdbc.jar" --work "$RESULTS/p16" --postgres 'host=127.0.0.1 port=55432 user=nata dbname=nata' ;;
  *) echo 'unknown oracle extra gate' >&2; exit 2 ;;
esac
