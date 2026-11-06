#!/bin/bash
# A missing metric, invalid sample, and a known slowdown must fail the gate.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
printf 'metric,median_secs\nquery,1\n' > "$t/base"
"$here/regress.sh" "$t/base" "$t/base"
printf 'metric,median_secs\nquery,1.3\n' > "$t/new"
if "$here/regress.sh" "$t/base" "$t/new"; then exit 1; fi
printf 'metric,median_secs\nother,1\n' > "$t/new"
if "$here/regress.sh" "$t/base" "$t/new"; then exit 1; fi
printf 'metric,median_secs\nquery,nan\n' > "$t/new"
if "$here/regress.sh" "$t/base" "$t/new"; then exit 1; fi
