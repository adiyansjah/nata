#!/bin/bash
# Content identity on exported VM tarballs too (no .git needed); excludes records.
set -euo pipefail
cd "$(dirname "$0")/.."
{ find crates scripts check .github -type f ! -path '*/target/*' ! -path '*/__pycache__/*' ! -name pgwire-source-tree.txt -print0; printf '%s\0' Cargo.toml Cargo.lock rust-toolchain.toml deny.toml; } | sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1
