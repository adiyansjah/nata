#!/bin/bash
# Starts and stops an Apache Polaris server as a THROWAWAY test catalog on loopback, with a catalog backed by local disk, and prints its REST base URL.
#   polaris.sh start <tools-dir> <run-dir> [port=8181]   (prints the base URL on stdout; writes <run-dir>/polaris.env with the client credentials)
#   polaris.sh stop  <run-dir>
# What is deliberately insecure, because Polaris refuses it by default and a fixture needs it: the in-memory metastore, local-disk (FILE) storage
# (the setting is called ALLOW_INSECURE_STORAGE_TYPES), the default realm, and `polaris.readiness.ignore-severe-issues`. The server listens on 127.0.0.1 only.
# The root client secret is random per run, kept in <run-dir>/polaris.creds and polaris.env (mode 600) and never printed.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
CATALOG_NAME=spikecat
cmd=${1:?usage: polaris.sh start|stop ...}
case "$cmd" in
  start)
    TOOLS=$(cd "${2:?tools-dir}" && pwd); mkdir -p "${3:?run-dir}"; RUN=$(cd "$3" && pwd); PORT=${4:-8181}; MGMT=$((PORT + 1))
    [ -x "$TOOLS/polaris/bin/server" ] && [ -x "$TOOLS/jdk21/bin/java" ] || die "run setup-polaris.sh first"
    if [ -f "$RUN/polaris.pid" ] && kill -0 "$(cat "$RUN/polaris.pid")" 2>/dev/null; then die "a Polaris server is already running (pid $(cat "$RUN/polaris.pid"))"; fi
    rm -rf "$RUN/warehouse"; mkdir -p "$RUN/warehouse"
    umask 077
    cid="spike$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')"; sec=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
    printf '%s %s\n' "$cid" "$sec" > "$RUN/polaris.creds"
    umask 022
    cat > "$RUN/polaris.properties" <<PROPS
# Throwaway test fixture on loopback (see polaris.sh).
polaris.features."ALLOW_INSECURE_STORAGE_TYPES"=true
polaris.features."SUPPORTED_CATALOG_STORAGE_TYPES"=["FILE","S3","GCS","AZURE"]
polaris.readiness.ignore-severe-issues=true
quarkus.http.host=127.0.0.1
quarkus.http.port=$PORT
quarkus.management.port=$MGMT
quarkus.log.file.enabled=false
PROPS
    # The distribution's own launcher (bin/server) is used unchanged; configuration and the secret reach it only through -D options and the environment.
    env JAVA_HOME="$TOOLS/jdk21" "POLARIS_BOOTSTRAP_CREDENTIALS=POLARIS,$cid,$sec" \
        "POLARIS_JAVA_OPTS=-Dquarkus.config.locations=$RUN/polaris.properties -Xmx2g" \
        nohup "$TOOLS/polaris/bin/server" > "$RUN/polaris.log" 2>&1 &
    echo $! > "$RUN/polaris.pid"
    # whatever fails from here on, a half-started server must not be left running
    started_ok=0
    trap '[ "$started_ok" = 1 ] || { kill "$(cat "$RUN/polaris.pid" 2>/dev/null)" 2>/dev/null || true; rm -f "$RUN/polaris.pid"; }' EXIT
    up=0
    for _ in $(seq 1 180); do
      if curl -sf "http://127.0.0.1:$MGMT/q/health/ready" > /dev/null 2>&1; then up=1; break; fi
      kill -0 "$(cat "$RUN/polaris.pid")" 2>/dev/null || { tail -30 "$RUN/polaris.log" | cut -c1-300 >&2; die "the Polaris process exited"; }
      sleep 1
    done
    [ "$up" = 1 ] || { tail -30 "$RUN/polaris.log" | cut -c1-300 >&2; die "Polaris was not ready within 180 s"; }
    base="http://127.0.0.1:$PORT"
    tok=$(curl -sf -X POST "$base/api/catalog/v1/oauth/tokens" -d "grant_type=client_credentials&client_id=$cid&client_secret=$sec&scope=PRINCIPAL_ROLE:ALL" \
          | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])') || die "could not get a token with the bootstrap credentials"
    # catalog with FILE storage, then the right to manage its content for the root principal's role (the documented quick-start steps)
    call() { # call METHOD PATH JSON -> exit status 0 for 2xx or 409 (already there)
      local code; code=$(curl -s -o "$RUN/last-response.txt" -w '%{http_code}' -X "$1" "$base/api/management/v1$2" -H "Authorization: Bearer $tok" -H 'Content-Type: application/json' -d "$3")
      case "$code" in 2??|409) return 0 ;; *) echo "management call $1 $2 returned $code: $(head -c 300 "$RUN/last-response.txt")" >&2; return 1 ;; esac
    }
    call POST /catalogs "{\"catalog\":{\"name\":\"$CATALOG_NAME\",\"type\":\"INTERNAL\",\"readOnly\":false,\"properties\":{\"default-base-location\":\"file://$RUN/warehouse\"},\"storageConfigInfo\":{\"storageType\":\"FILE\",\"allowedLocations\":[\"file://$RUN/warehouse\"]}}}" || die "create catalog failed"
    call PUT "/catalogs/$CATALOG_NAME/catalog-roles/catalog_admin/grants" '{"grant":{"type":"catalog","privilege":"CATALOG_MANAGE_CONTENT"}}' || die "grant failed"
    call PUT "/principal-roles/service_admin/catalog-roles/$CATALOG_NAME" '{"catalogRole":{"name":"catalog_admin"}}' || die "role assignment failed"
    umask 077
    printf 'export IC_REST_CLIENT_ID=%s\nexport IC_REST_CLIENT_SECRET=%s\nexport IC_REST_WAREHOUSE=%s\n' "$cid" "$sec" "$CATALOG_NAME" > "$RUN/polaris.env"
    started_ok=1
    say "Polaris up on port $PORT (pid $(cat "$RUN/polaris.pid")), catalog '$CATALOG_NAME' on local disk; credentials in $RUN/polaris.env (mode 600)" >&2
    echo "$base/api/catalog"
    ;;
  stop)
    RUN=${2:?run-dir}
    if [ -f "$RUN/polaris.pid" ]; then
      pid=$(cat "$RUN/polaris.pid")
      if kill -0 "$pid" 2>/dev/null; then kill "$pid"; for _ in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done; kill -9 "$pid" 2>/dev/null || true; fi
      rm -f "$RUN/polaris.pid"
      say "Polaris stopped"
    fi
    ;;
  *) die "unknown command $cmd" ;;
esac
