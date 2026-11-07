#!/usr/bin/env bash
# Same runnable matrix for local smoke and either CPU's generic heavy job.
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
if ! command -v javac >/dev/null || ! command -v psql >/dev/null || ! command -v pg_config >/dev/null || [ ! -x "$(pg_config --bindir)/initdb" ]; then
    sudo apt-get update -qq
    sudo apt-get install -y default-jdk-headless postgresql postgresql-client libpq-dev python3-venv
fi
export TMPDIR="$WORK/wire-skeleton-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-skeleton"
python3 -m venv "$WORK/wire-skeleton-venv"
"$WORK/wire-skeleton-venv/bin/pip" install --quiet 'psycopg[binary]==3.3.8' 'pgcli==4.7.1' 'adbc-driver-postgresql==1.13.0' 'pyarrow==26.0.1'
curl -fsSL --retry 3 https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.14/postgresql-42.7.14.jar -o "$WORK/wire-skeleton-pgjdbc.jar"
cargo build -p nata
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-skeleton/matrix.py" --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" --work "$RESULTS/wire-skeleton" --jar "$WORK/wire-skeleton-pgjdbc.jar"
