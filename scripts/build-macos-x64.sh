#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-apple-darwin"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script must run on macOS."
  exit 1
fi

cd "$ROOT_DIR"

echo "Installing Rust target: $TARGET"
rustup target add "$TARGET"

echo "Installing frontend dependencies"
yarn install --frozen-lockfile

echo "Building macOS Intel package"
yarn tauri build --target "$TARGET"

echo "Done. Artifacts:"
echo "  src-tauri/target/$TARGET/release/bundle"
