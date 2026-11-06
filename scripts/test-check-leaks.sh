#!/usr/bin/env bash
# Shows that scripts/check-leaks.sh fails on each kind of leak and passes on a clean tree.
# Works in a throwaway clone so the real repository is never touched. The planted values are built
# at run time so that this file itself contains none of them.
set -euo pipefail
ROOT=$(git rev-parse --show-toplevel)
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
git clone -q --no-hardlinks "$ROOT" "$T/r"; cd "$T/r"
git -C "$ROOT" diff HEAD | git apply --allow-empty   # the working tree under test, committed or not
cp "$ROOT/scripts/check-leaks.sh" scripts/check-leaks.sh
git -c user.name=t -c user.email=t@example.invalid commit -qam "test base" --allow-empty
fail=0; n=0
d9() { printf '%s' "$1$1$1$1$1$1$1$1$1"; }
cases=(
  "homelab-$(d9 7)"
  "projects/$(d9 4)123/machineTypes/x"
  "$(d9 1)123-compute@developer.gserviceaccount.com"
  "billingAccounts/0A0A0A-1B1B1B-2C2C2C"
  "-----BEGIN OPENSSH PRIVATE""KEY-----"
  "ssh-ed25519 AAAA$(d9 abc)xyz"
  "someone.name""@gmail.com"
)
cases[4]="-----BEGIN OPENSSH PRIVATE KEY-----"
for c in "${cases[@]}"; do
  n=$((n+1)); echo "x $c y" > leak.txt; git add leak.txt
  if scripts/check-leaks.sh --staged >/dev/null 2>&1; then echo "FAIL case $n not caught (--staged)"; fail=1; fi
  git -c user.name=t -c user.email=t@example.invalid commit -qm leak
  if scripts/check-leaks.sh --tree >/dev/null 2>&1; then echo "FAIL case $n not caught (--tree)"; fail=1; fi
  if scripts/check-leaks.sh --range HEAD~1..HEAD >/dev/null 2>&1; then echo "FAIL case $n not caught (--range)"; fail=1; fi
  git rm -q leak.txt; git -c user.name=t -c user.email=t@example.invalid commit -qm clean
done
scripts/check-leaks.sh --tree >/dev/null 2>&1 || { echo "FAIL clean tree reported as a leak"; fail=1; }
# the masked report must not print the planted value back
echo "x homelab-$(d9 7) y" > leak.txt; git add leak.txt
if scripts/check-leaks.sh --staged 2>&1 | grep -q "$(d9 7)"; then echo "FAIL report prints the value"; fail=1; fi
[ $fail = 0 ] && echo "all check-leaks tests passed ($n leak kinds x 3 modes, clean tree, masking)"
exit $fail
