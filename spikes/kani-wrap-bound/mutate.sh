#!/usr/bin/env bash
# Show that the harnesses can fail: a negative control and three mutants, each in a temporary copy (the real files are
# never written). Every one must be CAUGHT (a harness reports FAILED with a failing check). INVALID (does not compile)
# never counts as caught. Exit 1 if anything is not CAUGHT.
set -u
here=$(cd "$(dirname "$0")" && pwd); cap=${1:-600}; rc=0
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fresh() { rm -rf "$T/s"; mkdir -p "$T/s/spikes/null-path/src"; cp -r "$here" "$T/s/spikes/kani-wrap-bound"; rm -rf "$T/s/spikes/kani-wrap-bound/target"
          cp "$here/../null-path/src/wrapadd.rs" "$T/s/spikes/null-path/src/"; [ -f "$here/../../rust-toolchain.toml" ] && cp "$here/../../rust-toolchain.toml" "$T/s/"; }
verdict() { # name harness [extra cargo kani arguments]
  local name=$1 h=$2; shift 2
  out=$(cd "$T/s/spikes/kani-wrap-bound" && timeout "$cap" cargo kani --harness "$h" "$@" 2>&1)
  if echo "$out" | grep -q 'error\[E\|could not compile'; then v=INVALID
  elif echo "$out" | grep -q '^VERIFICATION:- FAILED' && echo "$out" | grep -q 'Status: FAILURE'; then v=CAUGHT
  elif echo "$out" | grep -q '^VERIFICATION:- SUCCESSFUL'; then v=SURVIVED
  else v="NO RESULT"; fi
  echo "$name ($h): $v"; [ "$v" = CAUGHT ] || rc=1
}
mut() { # file old new first|last : replace one occurrence, fail loudly if it is not there
  python3 -I - "$@" <<'PY' || { echo "mutant NOT APPLIED in $1"; rc=1; }
import sys; p,o,n,w=sys.argv[1:5]; s=open(p).read()
assert o in s, "pattern not found"
s = s.replace(o,n,1) if w=="first" else n.join(s.rsplit(o,1))
open(p,"w").write(s)
PY
}
fresh; verdict negative-control-wide-addend negative_control_wide_addend_wraps --features negative
# mutant 1: the wrapping add of the sums adds 1 when the addend is 0 (a NULL row) -> the step harness must see it
fresh; mut "$T/s/spikes/null-path/src/wrapadd.rs" 'if WRAP { *a = a.wrapping_add(x) } else { *a += x }' 'if WRAP { *a = a.wrapping_add(x).wrapping_add((x == 0) as i128) } else { *a += x }' first
       verdict add128-null-adds-one sum_step_never_wraps
# mutant 2: the wrapping add of the counters sets the low bit of the addend -> the counter harness must see it
fresh; mut "$T/s/spikes/null-path/src/wrapadd.rs" 'if WRAP { *a = a.wrapping_add(x) } else { *a += x }' 'if WRAP { *a = a.wrapping_add(x | 1) } else { *a += x }' last
       verdict add64-sets-low-bit counter_step_never_wraps
# mutant 3: the induction allows k = u64::MAX (then k + 1 overflows) -> the step harness must see it
fresh; mut "$T/s/spikes/kani-wrap-bound/src/lib.rs" 'kani::assume(k < u64::MAX);' 'kani::assume(k <= u64::MAX);' first
       verdict bound-allows-k-max sum_step_never_wraps
exit $rc
