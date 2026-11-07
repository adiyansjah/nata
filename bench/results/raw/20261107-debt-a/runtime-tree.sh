#!/bin/bash
# Run at the repository root; identical to the VM read-back command.
set -euo pipefail
{ find crates -type f \( -path '*/src/*' -o -name Cargo.toml \) ! -path '*/target/*' -print0; printf '%s\0' Cargo.toml Cargo.lock rust-toolchain.toml; } | sort -z | xargs -0 sha256sum | sha256sum
