#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TARGETS=(
  "macOS x64:x86_64-apple-darwin"
  "macOS arm64:aarch64-apple-darwin"
  "Windows x64:x86_64-pc-windows-msvc"
)

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

ensure_rust_target() {
  local target="$1"

  if ! rustup target list --installed | grep -qx "$target"; then
    echo "Installing Rust target: $target"
    rustup target add "$target"
  fi
}

build_target() {
  local name="$1"
  local target="$2"

  echo
  echo "==> Building $name release package ($target)"
  yarn tauri build --target "$target" --ci
}

require_command yarn
require_command rustup

for item in "${TARGETS[@]}"; do
  ensure_rust_target "${item#*:}"
done

for item in "${TARGETS[@]}"; do
  build_target "${item%%:*}" "${item#*:}"
done

echo
echo "Release packages have been generated under src-tauri/target/*/release/bundle."
