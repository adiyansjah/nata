#!/bin/bash
# expect-fail.sh PATTERN COMMAND...   -- succeeds only if COMMAND fails AND its output matches PATTERN (egrep).
# For checks that a deliberately broken variant is CAUGHT (a loom model finds a deadlock, a test panics). A command
# that fails for another reason (a compile error) does NOT count, because then nothing was caught.
set -u
pattern=$1; shift
out=$("$@" 2>&1); rc=$?
printf '%s\n' "$out" | tail -n 40
if [ "$rc" -eq 0 ]; then echo "expect-fail: the command SUCCEEDED, so the broken variant was NOT caught" >&2; exit 1; fi
if printf '%s\n' "$out" | grep -Eq "$pattern"; then echo "expect-fail: failed as expected (matched: $pattern)"; exit 0; fi
echo "expect-fail: failed (exit $rc) but the output does not match '$pattern': not counted as caught" >&2
exit 1
