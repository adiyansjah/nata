#!/bin/bash
# Installs the pinned JDK 21 and Apache Polaris into <tools-dir> (idempotent). Both downloads are verified against the checksums in tools.env before
# anything is extracted, and extracted into their OWN new directories (nothing is run from a download directory).
#   usage: setup-polaris.sh <tools-dir>
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=tools.env
. "$HERE/tools.env"
TOOLS=${1:?usage: setup-polaris.sh <tools-dir>}
mkdir -p "$TOOLS"; TOOLS=$(cd "$TOOLS" && pwd)

tag=$(arch_tag); url_var="JDK21_URL_$tag"; sha_var="JDK21_SHA256_$tag"
if [ ! -x "$TOOLS/jdk21/bin/java" ]; then
  fetch_verified "${!url_var}" "${!sha_var}" "$TOOLS/downloads/jdk21-$tag.tar.gz"
  rm -rf "$TOOLS/jdk21.tmp"; mkdir -p "$TOOLS/jdk21.tmp"
  tar -xzf "$TOOLS/downloads/jdk21-$tag.tar.gz" -C "$TOOLS/jdk21.tmp" --strip-components=1
  mv "$TOOLS/jdk21.tmp" "$TOOLS/jdk21"
fi
"$TOOLS/jdk21/bin/java" -version 2>&1 | head -1

if [ ! -x "$TOOLS/polaris/bin/server" ]; then
  fetch_verified "$POLARIS_URL" "$POLARIS_SHA512" "$TOOLS/downloads/polaris-bin-$POLARIS_VERSION.tgz"
  # the sha256 of this spike's own earlier computation must agree too (two independent values for the same file)
  got=$(sha256sum "$TOOLS/downloads/polaris-bin-$POLARIS_VERSION.tgz" | cut -d' ' -f1)
  [ "$got" = "$POLARIS_SHA256" ] || die "polaris sha256 $got differs from the recorded $POLARIS_SHA256"
  rm -rf "$TOOLS/polaris.tmp"; mkdir -p "$TOOLS/polaris.tmp"
  tar -xzf "$TOOLS/downloads/polaris-bin-$POLARIS_VERSION.tgz" -C "$TOOLS/polaris.tmp" --strip-components=1
  mv "$TOOLS/polaris.tmp" "$TOOLS/polaris"
fi
[ -x "$TOOLS/polaris/bin/server" ] || die "no $TOOLS/polaris/bin/server after the install"
{
  echo "apache polaris $POLARIS_VERSION sha512 $POLARIS_SHA512"
  echo "jdk $JDK21_VERSION ($tag) sha256 ${!sha_var}"
} > "$TOOLS/POLARIS.txt"
cat "$TOOLS/POLARIS.txt"
