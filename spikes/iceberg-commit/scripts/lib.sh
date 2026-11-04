#!/bin/bash
# Shared helpers. Source it: . "$(dirname "$0")/lib.sh"
say() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
die() { say "ERROR: $*" >&2; exit 1; }

# fetch_verified URL SHA256 DEST   (also: sha512 when the hash has 128 hex digits)
fetch_verified() {
  local url=$1 want=$2 dest=$3 got tool
  if [ -f "$dest" ]; then
    got=$(hash_of "$dest" "$want")
    [ "$got" = "$want" ] && { say "cached $dest"; return 0; }
    rm -f "$dest"
  fi
  mkdir -p "$(dirname "$dest")"
  say "download $url"
  curl -fsSL --retry 3 --retry-delay 5 -o "$dest.part" "$url" || { rm -f "$dest.part"; return 1; }
  got=$(hash_of "$dest.part" "$want")
  if [ "$got" != "$want" ]; then
    echo "checksum mismatch for $url: expected $want, got $got" >&2
    rm -f "$dest.part"
    return 2
  fi
  mv "$dest.part" "$dest"
}
hash_of() { # hash_of FILE EXPECTED -> picks sha256 or sha512 by the length of EXPECTED
  if [ "${#2}" -eq 128 ]; then sha512sum "$1" | cut -d' ' -f1; else sha256sum "$1" | cut -d' ' -f1; fi
}
arch_tag() { case "$(uname -m)" in aarch64|arm64) echo AARCH64 ;; x86_64) echo X64 ;; *) die "unsupported architecture $(uname -m)" ;; esac; }
