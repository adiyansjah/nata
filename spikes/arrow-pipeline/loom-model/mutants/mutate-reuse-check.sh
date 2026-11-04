#!/usr/bin/env bash
# Regression check for a defect of the mutation drivers (found on GCP, 2026-11-04, twice): two invocations of the driver
# that share one private root (copy + cargo target dir) must BOTH pass their baseline. With restored files that kept
# their old mtime (copy2, copytree defaults), cargo reused the first invocation's mutant binary and the second
# baseline failed. Here mutate.py copies with fresh mtimes (shutil.copy).
#
# usage: mutate-reuse-check.sh                       the two invocations in a row must both pass their baseline
#        mutate-reuse-check.sh --demonstrate-failure the same with the KNOWN-BAD copy (MUT_COPY2=1): the second
#                                                    baseline must FAIL, which shows that this check can fail
# Cheap: MUT_TESTS=quick (the one-worker loom tests, a few hundred executions each); about two builds of the model.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
R=$(mktemp -d "${TMPDIR:-/tmp}/ap-loom-reuse.XXXXXX"); trap 'rm -rf "$R"' EXIT
export MUT_TESTS=quick MUT_TEST_TIMEOUT=${MUT_TEST_TIMEOUT:-1200}
M1=limit_off_by_one_finishes_one_row_early          # caught by the s1 quick test: its binary fails s1
M2=stop_wakes_by_enqueueing_directly_task_runs_twice # caught by s1 too; its baseline includes s1
run() { # tag [env...]
  local tag=$1; shift
  env MUT_ROOT="$R/root" "$@" python3 -I mutate.py "$M1" > "$R/$tag.1.log" 2>&1; rc1=$?
  env MUT_ROOT="$R/root" "$@" python3 -I mutate.py "$M2" > "$R/$tag.2.log" 2>&1; rc2=$?
  printf '%s: invocation 1 rc=%s (%s), invocation 2 rc=%s (%s)\n' "$tag" "$rc1" "$(grep -m1 -E '^(baseline passes|BASELINE)' "$R/$tag.1.log" | cut -c1-40)" "$rc2" "$(grep -m1 -E '^(baseline passes|BASELINE)' "$R/$tag.2.log" | cut -c1-40)"
}
if [ "${1:-}" = "--demonstrate-failure" ]; then
  run bad MUT_COPY2=1
  if [ "$rc1" -eq 0 ] && [ "$rc2" -eq 2 ] && grep -q '^BASELINE FAILS' "$R/bad.2.log"; then
    echo "reuse check can fail: with copy2 the second baseline reused the first mutant's binary and FAILED, as expected"; exit 0
  fi
  echo "FAIL: the known-bad copy did not make the second baseline fail (rc1=$rc1 rc2=$rc2): this check cannot detect the defect"; tail -15 "$R/bad.2.log"; exit 1
fi
run good
if [ "$rc1" -eq 0 ] && [ "$rc2" -eq 0 ] && grep -q '^baseline passes' "$R/good.1.log" && grep -q '^baseline passes' "$R/good.2.log"; then
  echo "reuse check passed: the second baseline did not see the first mutant"; exit 0
fi
echo "FAIL: two invocations in a row did not both pass their baseline"; tail -15 "$R/good.1.log"; tail -15 "$R/good.2.log"; exit 1
