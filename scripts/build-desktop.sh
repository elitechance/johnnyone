#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist/host-app"
TAURI_DIR="$ROOT/desktop/src-tauri"
BIN="$TAURI_DIR/target/release/johnnyone-desktop"

export PATH="${HOME}/.cargo/bin:${PATH:-}"

echo "Removing stale host-app dist ($DIST)..."
rm -rf "$DIST"

echo "Building host-app..."
cd "$ROOT"
npx nx build host-app

echo "Building desktop binary (production webview)..."
cd "$TAURI_DIR"
touch build.rs
cargo build --release --bin johnnyone-desktop --features tauri/custom-protocol

echo "Built $BIN"