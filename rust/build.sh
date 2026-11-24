#!/bin/bash
# Build the Rust engine and the IDE, then copy the two executables into dist/.
# The build folder lives on the internal disk: the project volume is exFAT and nearly full (DR-34).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Library/Caches/fean-rust-target}"
cd "$here"
cargo build --release "$@"
mkdir -p "$here/../dist"
for bin in fean fean-ide; do
  if [ -f "$CARGO_TARGET_DIR/release/$bin" ]; then
    cp "$CARGO_TARGET_DIR/release/$bin" "$here/../dist/$bin"
  fi
done
ls -l "$here/../dist" | grep -v '^total' | grep -v ' \._' || true
