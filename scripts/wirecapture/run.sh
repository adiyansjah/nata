#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}" "${SRC:?}"
cd "$SRC"
export TMPDIR="$WORK/tmp-wcap"; mkdir -p "$TMPDIR" "$RESULTS/wirecapture"
D="$WORK/wirecapture"; mkdir -p "$D"; export WCAP_DIR="$D"
python3 -m venv "$D/venv"
"$D/venv/bin/pip" install --quiet 'psycopg[binary]==3.3.8' pgcli==4.7.1 adbc-driver-postgresql==1.13.0 pyarrow==26.0.1 pglast==8.7
curl -fsSL --retry 3 https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.14/postgresql-42.7.14.jar -o "$D/pgjdbc.jar"
javac -cp "$D/pgjdbc.jar" -d "$D" "$SRC/scripts/wirecapture/Metadata.java"
# Independent std-only Rust reader: no Nata build, no Cargo dependencies.
rustc --edition=2021 -O "$SRC/scripts/wirecapture/raw.rs" -o "$D/raw"
PGBIN=$(dirname "$(find /usr/lib/postgresql -name initdb | sort -V | tail -1)")
[ ! -d "$D/pg" ] || rm -rf "$D/pg"
"$PGBIN/initdb" -D "$D/pg" -A trust -U nata --no-locale --encoding=UTF8 >/dev/null
cat >> "$D/pg/postgresql.conf" <<EOF
listen_addresses='127.0.0.1'
port=55432
unix_socket_directories='$D'
ssl=off
log_statement='all'
log_line_prefix='%a: '
logging_collector=off
EOF
"$PGBIN/pg_ctl" -D "$D/pg" -l "$RESULTS/wirecapture/postgresql.log" start
trap '"$PGBIN/pg_ctl" -D "$D/pg" stop -m immediate' EXIT
createdb -h 127.0.0.1 -p 55432 -U nata nata
psql -X -h 127.0.0.1 -p 55432 -U nata -d nata -f "$SRC/scripts/wirecapture/fixture.sql" >/dev/null
"$D/venv/bin/python" "$SRC/scripts/wirecapture/capture.py" "$RESULTS/wirecapture"
