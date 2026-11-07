#!/usr/bin/env bash
# item35-style dependency setup for the parent's heavy job; never invoked by
# the checker, which accepts the same default clients in an existing environment.
set -euo pipefail
: "${WORK:?on-disk job workspace}"
sudo apt-get update -qq
# pgjdbc 42.7.x needs Java 8+; use the distro default (17 on Debian 12,
# 21 on Debian 13) rather than requiring a package absent from bookworm.
sudo apt-get install -y -qq default-jdk-headless postgresql-client python3-venv curl
python3 -m venv "$WORK/aggregate-client"
"$WORK/aggregate-client/bin/pip" install 'psycopg[binary]' adbc-driver-postgresql==1.13.0 pyarrow pgcli
curl -fsSL https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.7/postgresql-42.7.7.jar -o "$WORK/aggregate-pgjdbc.jar"
