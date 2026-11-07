#!/bin/bash
# Invoked only by job.sh on the work disk; source and results never hold large data.
set -euo pipefail
unset AI_AGENT
cd "$SRC"
mode=${1:?bench or clients}
mkdir -p "$WORK/pgwire-work" "$RESULTS/$mode"
if [ "$mode" = bench ] || [ "$mode" = clients ]; then
  sudo systemctl stop postgresql
  pg_bin=$(find /usr/lib/postgresql -path '*/bin/initdb' -printf '%h\n' | sort -V | tail -1)
  # A preempted VM leaves this run-owned reference data directory on its work disk.
  rm -rf -- "$WORK/pg-ref"
  "$pg_bin/initdb" -D "$WORK/pg-ref" --auth=trust --no-locale > "$RESULTS/bench/postgres-setup.txt"
  "$pg_bin/pg_ctl" -D "$WORK/pg-ref" -l "$WORK/pg-ref.log" -o "-h 127.0.0.1 -k $WORK/pg-ref -p 5545 -c fsync=off -c autovacuum=off -c max_parallel_workers_per_gather=0 -c jit=off" start >> "$RESULTS/bench/postgres-setup.txt"
  trap '"$pg_bin/pg_ctl" -D "$WORK/pg-ref" stop -m immediate' EXIT
  psql -X -h 127.0.0.1 -p 5545 -U nata -d postgres -c "CREATE ROLE test LOGIN SUPERUSER" -c "CREATE DATABASE nata OWNER test" >> "$RESULTS/bench/postgres-setup.txt"
  psql -X -h 127.0.0.1 -p 5545 -U nata -d postgres -c "SELECT version()" >> "$RESULTS/bench/postgres-setup.txt"
fi
if [ "$mode" = bench ]; then
  export PGWIRE_RAW="$WORK/target-ws/release/pgwire-raw" PGWIRE_REFERENCE_PORT=5545
  cat crates/nata-bench/pgwire-source-tree.txt > "$RESULTS/bench/source-tree.txt"
  python3 crates/nata-bench/pgwire-bench.py "$WORK/target-ws/release/nata" "$WORK/pgwire-work" "$RESULTS/bench"
  CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-pgwire --test reliability one_byte_per_millisecond_reader_is_bounded -- --nocapture > "$RESULTS/bench/slow-reader.txt" 2>&1
  "$pg_bin/pg_ctl" -D "$WORK/pg-ref" stop -m fast >> "$RESULTS/bench/postgres-setup.txt"
  trap - EXIT
  CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-pgwire > "$RESULTS/bench/protocol.txt" 2>&1
  if [ "$(uname -m)" = aarch64 ]; then
    CARGO_TARGET_DIR="$WORK/target-ws" NATA_PGWIRE_SEED=322661912839 NATA_PGWIRE_SCENARIOS=2000 NATA_PGWIRE_LIVE_CASES=10000 NATA_PGWIRE_CONTENTION=$(nproc) cargo test --release -p nata-pgwire --test reliability -- --nocapture > "$RESULTS/bench/final-live.txt" 2>&1
    python3 -u crates/nata-pgwire/mutate.py --work "$WORK/pgwire-mutants" > "$RESULTS/bench/final-mutants.txt" 2>&1
  fi
  # A second executable differs only in accepted-socket TCP_NODELAY.
  cp crates/nata-pgwire/src/server.rs "$WORK/server.rs.saved"
  trap 'cp "$WORK/server.rs.saved" crates/nata-pgwire/src/server.rs' EXIT
  cp "$WORK/target-ws/release/nata" "$WORK/nata-nodelay-on"
  sed -i 's/socket.set_nodelay(true)?;/socket.set_nodelay(false)?;/' crates/nata-pgwire/src/server.rs
  CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata
  cp "$WORK/target-ws/release/nata" "$WORK/nata-nodelay-off"
  cp "$WORK/server.rs.saved" crates/nata-pgwire/src/server.rs
  trap - EXIT
  python3 crates/nata-bench/pgwire-latency.py "$WORK/nata-nodelay-on" "$WORK/nata-nodelay-off" "$WORK/latency" "$RESULTS/bench/nodelay.json"
  exit
fi
python3 -m venv "$WORK/client-venv"
"$WORK/client-venv/bin/pip" install --quiet 'psycopg[binary]' adbc-driver-postgresql==1.13.0 pyarrow pgcli
# Official Maven metadata selects the newest 42.x release, not an assumed version.
version=$(curl -fsSL https://repo.maven.apache.org/maven2/org/postgresql/postgresql/maven-metadata.xml | python3 -c 'import sys,xml.etree.ElementTree as E; v=[x.text for x in E.parse(sys.stdin).findall("./versioning/versions/version") if x.text.startswith("42.") and all(y.isdigit() for y in x.text.split("."))];print(max(v,key=lambda s:tuple(map(int,s.split(".")))))')
curl -fsSL "https://repo.maven.apache.org/maven2/org/postgresql/postgresql/$version/postgresql-$version.jar" -o "$WORK/pgjdbc.jar"
psql --version > "$RESULTS/clients/distribution.txt"
java -version >> "$RESULTS/clients/distribution.txt" 2>&1
# Exact fixture used by the checked default-driver probes.
"$WORK/client-venv/bin/python" - "$WORK/pgwire-work" <<'PY'
import pyarrow as a, pyarrow.parquet as p, sys, datetime, decimal
from pathlib import Path
w=Path(sys.argv[1]);types=[a.bool_(),a.int8(),a.int16(),a.int32(),a.int64(),a.float32(),a.float64(),a.decimal128(12,3),a.string(),a.date32()]
rows=[[True,-128,-32768,-2147483648,-9223372036854775808,-0.0,float('nan'),decimal.Decimal('123.450'),'',datetime.date(1970,1,1)],[False,127,32767,2147483647,9223372036854775807,float('inf'),-float('inf'),decimal.Decimal('-0.001'),'secret-cell-a,b\né',datetime.date(2024,2,29)],[None]*10]
p.write_table(a.table({n:a.array([r[i] for r in rows],type=t) for i,(n,t) in enumerate(zip(['bo','ti','si','i','b','r','d','dec','s','dt'],types))}),w/'types.parquet')
p.write_table(a.table({'s':['x'*1024]*1048576}),w/'large.parquet',row_group_size=8192)
PY
"$WORK/target-ws/release/nata" serve --listen 127.0.0.1:5544 --threads 8 > "$WORK/server.out" 2> "$RESULTS/clients/server.log" &
server=$!
proxy=
trap 'kill $proxy "$server" 2>/dev/null || true; wait 2>/dev/null || true; "$pg_bin/pg_ctl" -D "$WORK/pg-ref" stop -m immediate' EXIT
python3 crates/nata-bench/wire-trace.py 5546 5544 "$RESULTS/clients/frontend-trace.txt" &
proxy=$!
sleep 1
javac -cp "$WORK/pgjdbc.jar" -d "$WORK/pgwire-work" crates/nata-pgwire/tests/Clients4.java crates/nata-bench/Corpus35.java
java -cp "$WORK/pgwire-work:$WORK/pgjdbc.jar" Clients4 5546 "$WORK/pgwire-work/types.parquet" "$WORK/pgwire-work/large.parquet" > "$RESULTS/clients/pgjdbc-native.txt" 2>&1
"$WORK/client-venv/bin/python" crates/nata-pgwire/tests/clients4.py 5546 "$WORK/pgwire-work/types.parquet" "$WORK/pgwire-work/large.parquet" > "$RESULTS/clients/psycopg-native.txt" 2>&1
"$WORK/client-venv/bin/python" crates/nata-bench/client35-extra.py 5546 "$WORK/pgwire-work/types.parquet" "$WORK/pgwire-work/large.parquet" "$RESULTS/clients" > "$RESULTS/clients/extra-native.txt" 2>&1
"$WORK/client-venv/bin/python" crates/nata-bench/adbc35.py 5546 "$WORK/pgwire-work/types.parquet" "$RESULTS/clients/adbc.json" > "$RESULTS/clients/adbc.txt" 2>&1
mkdir -p "$WORK/pgwire-work/jdbc"
java -cp "$WORK/pgwire-work:$WORK/pgjdbc.jar" Corpus35 5546 check/queries/sql "$SRC/data/client-check" "$WORK/pgwire-work/jdbc" > "$RESULTS/clients/jdbc.txt" 2>&1
"$WORK/client-venv/bin/python" crates/nata-bench/item35.py "$WORK/target-ws/release/nata" 5546 data/client-check "$RESULTS/clients" "$WORK/pgwire-work/jdbc" > "$RESULTS/clients/corpus.txt" 2>&1

"$WORK/client-venv/bin/python" crates/nata-pgwire/tests/postgres_oracle.py 5544 5545 "$WORK/pgwire-oracle" > "$RESULTS/clients/postgres-oracle.txt" 2>&1
cat crates/nata-bench/pgwire-source-tree.txt > "$RESULTS/clients/source-tree.txt"
