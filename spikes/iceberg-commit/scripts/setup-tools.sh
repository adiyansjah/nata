#!/bin/bash
# Installs the pinned JDK and tla2tools.jar into <tools-dir> (idempotent). Prints the versions.
#   usage: setup-tools.sh <tools-dir>
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=tools.env
. "$HERE/tools.env"
TOOLS=${1:?usage: setup-tools.sh <tools-dir>}
mkdir -p "$TOOLS"; TOOLS=$(cd "$TOOLS" && pwd)

tag=$(arch_tag)
url_var="JDK_URL_$tag"; sha_var="JDK_SHA256_$tag"
if [ ! -x "$TOOLS/jdk/bin/java" ]; then
  fetch_verified "${!url_var}" "${!sha_var}" "$TOOLS/downloads/jdk-$tag.tar.gz"
  rm -rf "$TOOLS/jdk.tmp"; mkdir -p "$TOOLS/jdk.tmp"
  tar -xzf "$TOOLS/downloads/jdk-$tag.tar.gz" -C "$TOOLS/jdk.tmp" --strip-components=1
  mv "$TOOLS/jdk.tmp" "$TOOLS/jdk"
fi
fetch_verified "$TLA_URL" "$TLA_SHA256" "$TOOLS/tla2tools.jar"
"$TOOLS/jdk/bin/java" -version 2>&1 | head -1
"$TOOLS/jdk/bin/java" -cp "$TOOLS/tla2tools.jar" tlc2.TLC 2>&1 | head -1 || true
{
  echo "jdk $JDK_VERSION ($tag) sha256 ${!sha_var}"
  echo "tla2tools $TLA_VERSION sha256 $TLA_SHA256"
} > "$TOOLS/VERSIONS.txt"
cat "$TOOLS/VERSIONS.txt"
