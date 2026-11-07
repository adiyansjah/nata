#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
# Declared source fallback only. A real UI installation still supplies shipped-JAR proof.
curl -fsSL --retry 3 https://repo.maven.apache.org/maven2/org/postgresql/postgresql/42.7.2/postgresql-42.7.2.jar -o "$WORK/dbeaver-pgjdbc.jar"
DBEAVER_JAR="$WORK/dbeaver-pgjdbc.jar" "$WORK/wire-skeleton-venv/bin/python" scripts/wire-skeleton/matrix.py --nata "${CARGO_TARGET_DIR:-target}/debug/nata" --work "$RESULTS/dbeaver-matrix" --jar "$WORK/wire-skeleton-pgjdbc.jar"
