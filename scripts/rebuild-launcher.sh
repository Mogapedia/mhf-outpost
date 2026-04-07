#!/usr/bin/env bash
# Rebuild resources/mhf-iel-cli.exe from the vendored source in vendor/mhf-iel/.
#
# This is the only authoritative way to update the bundled launcher binary.
# `mhf-outpost` itself is built for the host's native target, but mhf-iel-cli
# must be a 32-bit Windows PE so it can host mhfo-hd.dll in-process. We use
# `cargo xwin` to cross-compile from any host without needing Wine or MSVC.
#
# Prerequisites (one-time setup):
#   rustup target add i686-pc-windows-msvc
#   cargo install cargo-xwin
#
# The first run of cargo-xwin will download the Microsoft CRT/Windows SDK
# headers (~hundreds of MB) and prompt you to accept the Microsoft EULA.
#
# Usage:
#   ./scripts/rebuild-launcher.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR="$ROOT/vendor/mhf-iel"
OUT="$ROOT/resources/mhf-iel-cli.exe"

if [ ! -d "$VENDOR" ]; then
  echo "error: vendor/mhf-iel not found at $VENDOR" >&2
  exit 1
fi

if ! command -v cargo-xwin >/dev/null 2>&1; then
  echo "error: cargo-xwin is not installed" >&2
  echo "  install with: cargo install cargo-xwin" >&2
  echo "  and:          rustup target add i686-pc-windows-msvc" >&2
  exit 1
fi

echo "Building mhf-iel-cli.exe from $VENDOR …"
(
  cd "$VENDOR"
  cargo xwin build \
    --package mhf-iel-cli \
    --target i686-pc-windows-msvc \
    --release
)

BUILT="$VENDOR/target/i686-pc-windows-msvc/release/mhf-iel-cli.exe"
if [ ! -f "$BUILT" ]; then
  echo "error: build did not produce $BUILT" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT")"
cp "$BUILT" "$OUT"
echo "✓ updated $OUT ($(stat -c%s "$OUT" 2>/dev/null || stat -f%z "$OUT") bytes)"
