#!/usr/bin/env bash
# PGDG is configured by install-pg16.sh. Freeze the accepted psql deviation.
set -euo pipefail
version=$(apt-cache madison postgresql-client-17 | awk '$3 ~ /^17[.]11-/ { print $3; exit }')
[ -n "$version" ] || { echo 'accepted psql 17.11 unavailable; deviation needs parent revisit' >&2; exit 1; }
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "postgresql-client-17=$version"
[ "$(psql --version | awk '{print $3}')" = 17.11 ]
