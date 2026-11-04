#!/bin/bash
# Starts or stops the reference REST catalog (no Docker): the Iceberg REST fixture over a SQLite JDBC catalog, warehouse on
# the local disk. The fixture is the server whose commit path is the Java reference implementation (CatalogHandlers.updateTable
# -> JdbcTableOperations: UPDATE ... WHERE metadata_location = <expected>).
#   usage: catalog.sh start <tools-dir> <run-dir> [port=8181]     (prints the URL; the warehouse is <run-dir>/warehouse)
#          catalog.sh stop  <run-dir>
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
cmd=${1:?usage: catalog.sh start|stop ...}
case "$cmd" in
  start)
    TOOLS=$(cd "${2:?tools-dir}" && pwd); RUN=${3:?run-dir}; PORT=${4:-8181}
    mkdir -p "$RUN/warehouse"; RUN=$(cd "$RUN" && pwd)
    [ -f "$TOOLS/iceberg-rest-adapter.jar" ] || die "no $TOOLS/iceberg-rest-adapter.jar: run build-catalog.sh"
    if [ -f "$RUN/catalog.pid" ] && kill -0 "$(cat "$RUN/catalog.pid")" 2>/dev/null; then die "a catalog is already running (pid $(cat "$RUN/catalog.pid"))"; fi
    rm -f "$RUN/catalog.db" "$RUN/catalog.db-wal" "$RUN/catalog.db-shm"
    # The fixture reads its configuration from CATALOG_* environment variables (CATALOG_FOO__BAR -> foo-bar, _ -> .).
    env CATALOG_CATALOG__IMPL=org.apache.iceberg.jdbc.JdbcCatalog \
        "CATALOG_URI=jdbc:sqlite:$RUN/catalog.db?journal_mode=WAL" \
        CATALOG_JDBC_USER=user CATALOG_JDBC_PASSWORD=password CATALOG_JDBC_STRICT__MODE=true \
        "CATALOG_WAREHOUSE=$RUN/warehouse" "REST_PORT=$PORT" \
        nohup "$TOOLS/jdk/bin/java" -Xmx2g -cp "$TOOLS/iceberg-rest-adapter.jar" org.apache.iceberg.rest.RESTCatalogServer \
        > "$RUN/catalog.log" 2>&1 &
    echo $! > "$RUN/catalog.pid"
    for _ in $(seq 1 120); do
      if curl -sf "http://127.0.0.1:$PORT/v1/config" > "$RUN/config.json" 2>/dev/null; then
        say "catalog up on port $PORT (pid $(cat "$RUN/catalog.pid")); /v1/config: $(cat "$RUN/config.json")" >&2
        echo "http://127.0.0.1:$PORT"
        exit 0
      fi
      kill -0 "$(cat "$RUN/catalog.pid")" 2>/dev/null || { tail -30 "$RUN/catalog.log" >&2; die "the catalog process exited"; }
      sleep 1
    done
    tail -30 "$RUN/catalog.log" >&2
    die "the catalog did not answer /v1/config within 120 s"
    ;;
  stop)
    RUN=${2:?run-dir}
    if [ -f "$RUN/catalog.pid" ]; then
      pid=$(cat "$RUN/catalog.pid")
      if kill -0 "$pid" 2>/dev/null; then kill "$pid"; for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done; kill -9 "$pid" 2>/dev/null || true; fi
      rm -f "$RUN/catalog.pid"
      say "catalog stopped"
    fi
    ;;
  *) die "unknown command $cmd" ;;
esac
