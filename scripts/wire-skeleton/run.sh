#!/usr/bin/env bash
# Same runnable matrix for local smoke and either CPU's generic heavy job.
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
# Any installed server major is enough (the closeout chain adds PGDG, whose libpq-dev moves pg_config's bindir). Never install the
# unversioned `postgresql` package: with PGDG configured it pulls the newest major and opens an interactive debconf prompt
# (found 2026-11-08: runs s10close2-* hung 40 minutes and installed PostgreSQL 18).
if ! command -v javac >/dev/null || ! command -v psql >/dev/null || ! command -v pg_config >/dev/null || ! compgen -G '/usr/lib/postgresql/*/bin/initdb' >/dev/null; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q default-jdk-headless postgresql-15 postgresql-client-15 libpq-dev python3-venv
fi
export TMPDIR="$WORK/wire-skeleton-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-skeleton"
python3 -m venv "$WORK/wire-skeleton-venv"
"$WORK/wire-skeleton-venv/bin/pip" install --quiet 'psycopg[binary]==3.3.8' 'pgcli==4.7.1' 'adbc-driver-postgresql==1.13.0' 'pyarrow==26.0.1'
curl -fsSL --retry 3 https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.14/postgresql-42.7.14.jar -o "$WORK/wire-skeleton-pgjdbc.jar"
cargo build --locked -p nata -j 4
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-skeleton/matrix.py" --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" --work "$RESULTS/wire-skeleton" --jar "$WORK/wire-skeleton-pgjdbc.jar"
