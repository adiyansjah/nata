#!/usr/bin/env bash
# Same runnable matrix for local smoke and either CPU's generic heavy job.
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
# Install only what is missing (found 2026-11-08). Never the unversioned `postgresql` package on Debian 12: with PGDG configured it
# pulls the newest major and opens an interactive debconf prompt (runs s10close2-* hung 40 minutes). The Debian 12 VM oracle is 15;
# other distributions (e.g. the Debian 13 dev VM, PostgreSQL 17) keep their own default server.
pkgs=()
command -v javac >/dev/null || pkgs+=(default-jdk-headless)
command -v pg_config >/dev/null || pkgs+=(libpq-dev)
python3 -c 'import venv, ensurepip' 2>/dev/null || pkgs+=(python3-venv)
if ! command -v psql >/dev/null || ! compgen -G '/usr/lib/postgresql/*/bin/initdb' >/dev/null; then
    if [ "$(. /etc/os-release; echo "${VERSION_CODENAME:-}")" = bookworm ]; then pkgs+=(postgresql-15 postgresql-client-15); else pkgs+=(postgresql postgresql-client); fi
fi
if [ "${#pkgs[@]}" -gt 0 ]; then
    sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q "${pkgs[@]}"
fi
export TMPDIR="$WORK/wire-skeleton-tmp"
mkdir -p "$TMPDIR" "$RESULTS/wire-skeleton"
python3 -m venv "$WORK/wire-skeleton-venv"
"$WORK/wire-skeleton-venv/bin/pip" install --quiet 'psycopg[binary]==3.3.8' 'pgcli==4.7.1' 'adbc-driver-postgresql==1.13.0' 'pyarrow==26.0.1'
curl -fsSL --retry 3 https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.14/postgresql-42.7.14.jar -o "$WORK/wire-skeleton-pgjdbc.jar"
cargo build --locked -p nata -j 4
# Fresh-VM retry files need the same pinned clients and binary without rerunning
# a previously accepted matrix gate.
if [ "${1:-}" = --prepare-only ]; then exit 0; fi
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-skeleton/matrix.py" --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" --work "$RESULTS/wire-skeleton" --jar "$WORK/wire-skeleton-pgjdbc.jar"
