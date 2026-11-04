#!/usr/bin/env bash
# Tests of kani-crate.sh with a stub in place of Kani: phases, resume, retry of timeouts/killed runs, and the defect of the
# first GCP run: Kani's output (hundreds of KB) is held in a shell variable called `out` by run1.sh; if the driver exports
# a variable of that name, every exec in run1.sh fails with 'Argument list too long' and the harness reports NO RESULT.
set -u
HERE=$(cd "$(dirname "$0")/.." && pwd); K="${KANI_CRATE:-$HERE/kani-crate.sh}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
mkdir -p "$T/spike/src" "$T/tmp"
cat > "$T/spike/src/lib.rs" <<'RS'
#[kani::proof]
fn alpha_rows1() {}
#[kani::proof]
#[kani::unwind(3)]
fn beta_rows2() {}
#[kani::proof]
fn gamma_rows2() {}
#[kani::proof]
fn delta_rows2() {}
RS
cat > "$T/spike/run1.sh" <<'EOF2'
#!/usr/bin/env bash
# like the real run1.sh: the whole "Kani output" (400 KB) goes into a variable called out, then external commands run
h=$1; cap=$2
out=$(head -c 400000 /dev/zero | tr '\0' x)
d=$(date +%s) || { echo "$h: NO RESULT (exec failed)"; exit 0; }
case "$h" in
  beta*) if [ "$cap" -lt 1000 ]; then echo "$h: TIMEOUT after ${cap}s | wall ${cap}s"; else echo "$h: SUCCESSFUL | wall 1s"; fi ;;
  gamma*) echo "$h: FAILED | wall 1s | failing checks: 1" ;;
  delta*) if [ "$cap" -lt 1000 ]; then echo "$h: NO RESULT (CBMC failed: killed) | wall 1s"; else echo "$h: SUCCESSFUL | wall 1s"; fi ;;
  *) echo "$h: SUCCESSFUL | wall 1s" ;;
esac
EOF2
chmod +x "$T/spike/run1.sh"
export SPIKE=$T/spike TMPDIR=$T/tmp
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
"$K" all "$T/out" 2 900 > "$T/all.txt" 2>&1
grep -q "alpha_rows1: SUCCESSFUL" "$T/all.txt" && ok "a harness with big output is parsed (no exec failure)" || bad "big output: $(cat "$T/all.txt")"
grep -q "exec failed" "$T/all.txt" && bad "an exec failed: the driver exports a variable that run1.sh uses" || ok "no exec failure"
grep -q "gamma_rows2: FAILED" "$T/all.txt" && ok "a failing harness is reported as FAILED" || bad "gamma"
[ "$(wc -l < "$T/out/table-all.txt")" = 4 ] && ok "four result lines" || bad "table has $(wc -l < "$T/out/table-all.txt") lines"
# resume: nothing is run again
: > "$T/calls"; sed -i "s|^h=\$1; cap=\$2\$|h=\$1; cap=\$2; echo x >> $T/calls|" "$T/spike/run1.sh"
grep -q "echo x >> $T/calls" "$T/spike/run1.sh" || bad "test setup: the call counter was not installed"
"$K" all "$T/out" 2 900 > /dev/null 2>&1
[ ! -s "$T/calls" ] && ok "resume skips finished harnesses (0 calls)" || bad "resume re-ran $(wc -l < "$T/calls") harnesses"
# retry: beta (TIMEOUT) and delta (NO RESULT) again with the larger cap; alpha and gamma are not re-run
"$K" timeouts "$T/out" 2 3600 > "$T/to.txt" 2>&1
grep -q "beta_rows2: SUCCESSFUL" "$T/to.txt" && grep -q "delta_rows2: SUCCESSFUL" "$T/to.txt" && ok "timeouts and killed runs are retried with the larger cap" || bad "retry: $(cat "$T/to.txt")"
[ ! -e "$T/out/alpha_rows1.timeouts.txt" ] && [ ! -e "$T/out/gamma_rows2.timeouts.txt" ] && ok "finished and failed harnesses are not retried" || bad "retried too much"
# the mechanism of the defect, shown on its own: with an exported `out` the stub's exec fails
( export out=1; out=$(head -c 400000 /dev/zero | tr '\0' x); date +%s > /dev/null 2>&1 ) && bad "an exported big variable did not break exec (the test would not catch the defect)" || ok "an exported big \`out\` does break exec (so the test above can catch it)"
[ "$fail" = 0 ] && echo "all kani-crate tests passed" || { echo "SOME TESTS FAILED"; exit 1; }
