# decimal-clients (THROWAWAY spike)

Do real clients survive Arrow `Decimal64` at the protocol edge? Results: `bench/results/20261104-decimal-clients.md`.
Delete this directory once ADR 0003 decision 3 is settled; keep the results file.

## Setup (no sudo, no system packages)

    cd spikes/decimal-clients
    uv venv .venv && uv pip install --python .venv/bin/python pyarrow pandas adbc_driver_manager adbc_driver_flightsql \
        jdk4py JPype1 'psycopg[binary]' asyncpg pg8000
    uv venv --python 3.12 .venv-pg && uv pip install --python .venv-pg/bin/python pgserver 'psycopg[binary]'   # real PostgreSQL 16.2
    mkdir -p /tmp/nata-dc-jars && cd /tmp/nata-dc-jars && V=19.0.0 \
        && curl -sSO https://repo1.maven.org/maven2/org/apache/arrow/flight-sql-jdbc-driver/$V/flight-sql-jdbc-driver-$V.jar \
        && curl -sSO https://repo1.maven.org/maven2/org/apache/arrow/flight-sql-jdbc-driver/$V/flight-sql-jdbc-driver-$V.jar.sha1 \
        && echo "$(cat flight-sql-jdbc-driver-$V.jar.sha1)  flight-sql-jdbc-driver-$V.jar" | sha1sum -c
    cd - && CARGO_BUILD_JOBS=2 cargo build --release        # the Rust arrow-flight Flight SQL server

## Run (set PYTHONDONTWRITEBYTECODE=1 to keep the tree clean)

    export PYTHONDONTWRITEBYTECODE=1
    .venv/bin/python probe_flight.py          # pyarrow.flight + ADBC Flight SQL against a pyarrow.flight server
    .venv/bin/python probe_rust.py            # the same questions against the Rust arrow-flight server
    .venv/bin/python probe_jdbc.py            # Flight SQL JDBC driver 19.0.0 (pyarrow server)
    .venv/bin/python probe_jdbc.py --rust     # ... against the Rust server
    .venv-pg/bin/python verify_numeric.py     # numeric encoder vs real PostgreSQL 16.2, 6 mutants
    .venv/bin/python probe_pgwire.py          # psycopg3, asyncpg, pg8000 against a toy Postgres-protocol server

Files: `common.py` (the table), `flight_server.py` (pyarrow Flight + hand-rolled Flight SQL), `src/main.rs` (Rust Flight SQL server),
`pg_numeric.py` (the `numeric` encoder), `pgwire_server.py` (toy Postgres v3 server). Java is not installed on this machine: the JVM is the
JRE inside the `jdk4py` wheel, driven through JPype (no `javac`).
