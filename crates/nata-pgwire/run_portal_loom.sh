#!/usr/bin/env bash
# Bound 2 locally; pass LOOM_MAX_PREEMPTIONS=3 on the heavy closing commit.
# One build/exploration at a time. A timeout, empty selection or build error fails.
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUSTFLAGS="${RUSTFLAGS:-} --cfg nata_portal_loom"
export CARGO_TARGET_DIR="${PORTAL_LOOM_TARGET:-${CARGO_TARGET_DIR:-target}/portal-loom}"
export LOOM_MAX_PREEMPTIONS="${LOOM_MAX_PREEMPTIONS:-2}"
mkdir -p "$CARGO_TARGET_DIR"
printf 'Signature: 8a477f597d28d172789f06886806bc55\n' > "$CARGO_TARGET_DIR/CACHEDIR.TAG"
if ! cargo test --locked -j 4 -p nata-pgwire --test portal_loom --no-run --message-format=json > "$CARGO_TARGET_DIR/artifacts.jsonl"; then
    python3 - "$CARGO_TARGET_DIR/artifacts.jsonl" <<'PYERROR'
import json,sys
for line in open(sys.argv[1]):
    x=json.loads(line)
    if x.get('reason')=='compiler-message':
        print(x['message'].get('rendered',''),file=sys.stderr)
PYERROR
    exit 1
fi
bin=$(python3 - "$CARGO_TARGET_DIR/artifacts.jsonl" <<'PY'
import json,sys
xs=[x['executable'] for l in open(sys.argv[1]) if (x:=json.loads(l)).get('reason')=='compiler-artifact' and x.get('target',{}).get('name')=='portal_loom' and x.get('executable')]
assert len(xs)==1, xs
print(xs[0])
PY
)
count=0
while read -r name; do
    count=$((count+1))
    timeout "${PORTAL_LOOM_TIMEOUT:-600}" "$bin" "$name" --exact --nocapture
 done < <("$bin" --list | sed -n 's/: test$//p')
[ "$count" -eq 7 ]
