#!/usr/bin/env bash
# Refuse identifying or secret data in the repository: GCP project ids of the owner's form, GCP project numbers
# in resource paths, billing account ids, private keys, SSH public keys and Gmail addresses.
# The patterns are generic on purpose: this file must not contain the values it protects.
#
#   scripts/check-leaks.sh --tree           every tracked file (CI runs this)
#   scripts/check-leaks.sh --staged         the staged diff (run before `git commit`)
#   scripts/check-leaks.sh --range A..B     the added lines of a commit range (run before `git push`)
#
# Exit 0: clean. Exit 1: a match (printed with its file and line). Exit 2: usage error.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

PATTERNS=(
  'homelab-[0-9]{3,}'                              # the owner's GCP project id form
  'projects/[0-9]{9,}/'                            # a GCP project number inside a resource path
  '[0-9]{12}-compute@developer\.gserviceaccount'   # the default compute service account (contains the number)
  'billingAccounts/[0-9A-F]{6}-[0-9A-F]{6}-[0-9A-F]{6}'
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'
  'ssh-(rsa|ed25519) AAAA[0-9A-Za-z+/]{20,}'
  '[A-Za-z0-9._%+-]+@gmail\.com'
)
# Third-party sources copied verbatim (their authors' addresses are public and not ours).
EXCLUDE=(':(exclude)spikes/*/vendor/**' ':(exclude)scripts/check-leaks.sh' ':(exclude)scripts/test-check-leaks.sh')
RE=$(IFS='|'; echo "${PATTERNS[*]}")

case "${1:-}" in
  --tree)   out=$(git grep -nIE -e "$RE" -- . "${EXCLUDE[@]}" || true) ;;
  --staged) out=$(git diff --cached -U0 -- . "${EXCLUDE[@]}" | grep -nE '^\+' | grep -E -e "$RE" || true) ;;
  --range)  [ -n "${2:-}" ] || { echo "usage: $0 --range A..B" >&2; exit 2; }
            out=$(git log -p -U0 "$2" -- . "${EXCLUDE[@]}" | grep -E '^\+' | grep -E -e "$RE" || true) ;;
  *) echo "usage: $0 --tree | --staged | --range A..B" >&2; exit 2 ;;
esac

if [ -n "$out" ]; then
  echo "check-leaks: identifying or secret data found (value masked):" >&2
  printf '%s\n' "$out" | sed -E 's/[0-9]{6,}/<digits>/g; s/homelab-[0-9]+/homelab-<n>/g; s/[A-Za-z0-9._%+-]+@gmail\.com/<email>/g' | cut -c1-200 >&2
  exit 1
fi
echo "check-leaks: clean (${1#--})"
