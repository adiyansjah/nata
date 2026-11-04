#!/bin/bash
# Builds the Apache Iceberg REST fixture (the project's reference REST catalog server: org.apache.iceberg.rest.RESTCatalogServer
# over a JDBC/SQLite catalog) from the verified 1.12.0 source release. Needs the JDK of setup-tools.sh and network access to
# Maven Central / the Gradle plugin portal / services.gradle.org (the Gradle wrapper pins the Gradle distribution by sha256).
#   usage: build-catalog.sh <tools-dir> <build-dir>
# Result: <tools-dir>/iceberg-rest-adapter.jar and <tools-dir>/CATALOG.txt (version, jar sha256).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=tools.env
. "$HERE/tools.env"
TOOLS=$(cd "${1:?usage: build-catalog.sh <tools-dir> <build-dir>}" && pwd)
BUILD=${2:?usage: build-catalog.sh <tools-dir> <build-dir>}
mkdir -p "$BUILD"; BUILD=$(cd "$BUILD" && pwd)
[ -x "$TOOLS/jdk/bin/java" ] || die "run setup-tools.sh first"

if [ -f "$TOOLS/iceberg-rest-adapter.jar" ] && [ -f "$TOOLS/CATALOG.txt" ]; then say "catalog jar already built"; cat "$TOOLS/CATALOG.txt"; exit 0; fi

tarball="$BUILD/apache-iceberg-$ICEBERG_VERSION.tar.gz"
ok=0
for u in $ICEBERG_SRC_URLS; do
  if fetch_verified "$u" "$ICEBERG_SRC_SHA512" "$tarball"; then ok=1; break; fi
done
[ "$ok" = 1 ] || die "could not download a source release that matches the pinned sha512"
rm -rf "$BUILD/src"; mkdir -p "$BUILD/src"
tar -xzf "$tarball" -C "$BUILD/src" --strip-components=1
cd "$BUILD/src"
export JAVA_HOME="$TOOLS/jdk" PATH="$TOOLS/jdk/bin:$PATH"
export GRADLE_USER_HOME="$BUILD/gradle-home"
say "gradle: :iceberg-open-api:shadowJar (cold build; several minutes)"
./gradlew --no-daemon --console=plain -Dorg.gradle.jvmargs=-Xmx4g :iceberg-open-api:shadowJar -x test 2>&1 | tail -40
jar=$(ls open-api/build/libs/iceberg-open-api-test-fixtures-runtime-*.jar 2>/dev/null | head -1 || true)
[ -n "$jar" ] || die "the shadow jar was not produced (open-api/build/libs/ has: $(ls open-api/build/libs 2>&1 | tr '\n' ' '))"
cp "$jar" "$TOOLS/iceberg-rest-adapter.jar"
{
  echo "Apache Iceberg REST fixture $ICEBERG_VERSION built from the source release (sha512 $ICEBERG_SRC_SHA512)"
  echo "jar: $(basename "$jar") sha256 $(sha256sum "$TOOLS/iceberg-rest-adapter.jar" | cut -d' ' -f1)"
  echo "built with: $("$TOOLS/jdk/bin/java" -version 2>&1 | head -1)"
} > "$TOOLS/CATALOG.txt"
cat "$TOOLS/CATALOG.txt"
