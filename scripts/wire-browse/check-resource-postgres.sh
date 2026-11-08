#!/usr/bin/env bash
# Local PG17 regression: largest reference fixture, repeated replacement, teardown,
# then the resource gate smoke. Supply an existing pinned client venv and Nata.
set -euo pipefail
: "${RESOURCE_PYTHON:?}" "${NATA_BROWSE_BIN:?}" "${WORK:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
source "$root/scripts/wire-browse/resource-postgres.sh"
pg_bin=/usr/lib/postgresql/17/bin
mkdir -p "$WORK"
WORK=$(cd "$WORK" && pwd)
mkdir "$WORK/socket"
"$pg_bin/initdb" -D "$WORK/pg" -A trust -U nata --no-locale --encoding=UTF8 > "$WORK/initdb.log"
# Durability is outside this disposable lock-capacity check; avoid per-DDL fsync waits.
"$pg_bin/pg_ctl" -D "$WORK/pg" -l "$WORK/postgresql.log" -o "-h '' -k $WORK/socket -c max_locks_per_transaction=$resource_pg_locks -c synchronous_commit=off" start
trap '"$pg_bin/pg_ctl" -D "$WORK/pg" stop -m immediate' EXIT
export RESOURCE_TEST_DSN="host=$WORK/socket user=nata dbname=postgres"
"$RESOURCE_PYTHON" - "$root" "$resource_max_tables" "$resource_pg_locks" <<'PY'
import os, sys
sys.path.insert(0, sys.argv[1] + '/scripts/wire-browse')
import psycopg
from resources import setup_postgres
with psycopg.connect(os.environ['RESOURCE_TEST_DSN'], autocommit=True) as pg:
    assert int(pg.execute('SHOW max_locks_per_transaction').fetchone()[0]) == int(sys.argv[3])
    for width in (1, 64):
        setup_postgres(pg, int(sys.argv[2]), width)
        assert pg.execute("SELECT count(*) FROM information_schema.tables WHERE table_schema='resource_scale'").fetchone()[0] == int(sys.argv[2])
    pg.execute('DROP SCHEMA resource_scale CASCADE')
    assert pg.execute("SELECT count(*) FROM pg_namespace WHERE nspname='resource_scale'").fetchone()[0] == 0
print('PASS: 10,000 tables at widths 1 and 64, replacement and final teardown')
PY
"$RESOURCE_PYTHON" "$root/scripts/wire-browse/resources.py" --nata "$NATA_BROWSE_BIN" --work "$WORK/smoke" --smoke --postgres "$RESOURCE_TEST_DSN"
