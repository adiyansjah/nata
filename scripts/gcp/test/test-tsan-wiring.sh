#!/usr/bin/env bash
# Execute the real eng-checks sanitizer steps with cargo stubbed, not a build.
set -euo pipefail
HERE=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
cp "$HERE/job.sh" "$T/job.sh"
mkdir -p "$T/work/src" "$T/work/cargo/bin"
echo eng-checks > "$T/work/JOBNAME"
cat > "$T/lib.sh" <<'LIB'
RESULTS="$WORK/results"; mkdir -p "$RESULTS"
boot_id() { echo stubboot0; }
step() {
  local name=$1; shift
  case "$name" in ec-address|ec-thread) "$@" ;; esac
  return 0
}
fail_job() { echo "unexpected failure: $*" >&2; exit 1; }
finish_job() { :; }
LIB
cat > "$T/work/cargo/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
printf 'TSAN_OPTIONS=%s\nRUSTFLAGS=%s\nARGS=%s\n' "${TSAN_OPTIONS:-}" "$RUSTFLAGS" "$*"
CARGO
chmod +x "$T/work/cargo/bin/cargo"
cat > "$T/work/cargo/bin/rustc" <<'RUSTC'
#!/usr/bin/env bash
echo 'host: x86_64-unknown-linux-gnu'
RUSTC
chmod +x "$T/work/cargo/bin/rustc"
WORK="$T/work" TSAN_OPTIONS=history_size=3 bash "$T/job.sh"
grep -qx 'TSAN_OPTIONS=history_size=3' "$T/work/results/eng-address.txt"
grep -qx "TSAN_OPTIONS=history_size=3:suppressions=$T/work/src/scripts/tsan-suppressions.txt" "$T/work/results/eng-thread.txt"
grep -qx 'RUSTFLAGS=-Zsanitizer=thread' "$T/work/results/eng-thread.txt"
grep -qx 'ARGS=+nightly-2026-08-21 test -Zbuild-std --target x86_64-unknown-linux-gnu --workspace --lib --tests --no-fail-fast' "$T/work/results/eng-thread.txt"
grep -qx 'exit status: 0' "$T/work/results/eng-thread-exit.txt"
echo 'TSan wiring: existing options preserved; suppression only in thread step; full workspace command retained'
