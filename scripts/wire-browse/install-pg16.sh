#!/usr/bin/env bash
# Official PGDG repository, Debian 12 only. Heavy step; never run implicitly locally.
set -euo pipefail
. /etc/os-release
[ "$ID" = debian ] && [ "$VERSION_CODENAME" = bookworm ]
sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates curl gnupg
curl -fsS https://www.postgresql.org/media/keys/ACCC4CF8.asc -o "$WORK/pgdg.asc"
# Pin the official signing key, not an arbitrary downloaded key.
fingerprint=$(gpg --show-keys --with-colons "$WORK/pgdg.asc" | awk -F: '$1=="fpr" {print $10; exit}')
[ "$fingerprint" = B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8 ]
sudo install -m 644 "$WORK/pgdg.asc" /usr/share/keyrings/nata-pgdg.asc
printf '%s\n' 'deb [signed-by=/usr/share/keyrings/nata-pgdg.asc] https://apt.postgresql.org/pub/repos/apt bookworm-pgdg main' | sudo tee /etc/apt/sources.list.d/nata-pgdg.list >/dev/null
sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y postgresql-16 postgresql-client-16
mkdir -p "$RESULTS/pg16"
apt-cache policy postgresql-16 > "$RESULTS/pg16/package-policy.txt"
grep -q 'bookworm-pgdg/main' "$RESULTS/pg16/package-policy.txt"
/usr/lib/postgresql/16/bin/postgres --version | tee "$RESULTS/pg16/version.txt"
grep -q 'PostgreSQL) 16\.' "$RESULTS/pg16/version.txt"
