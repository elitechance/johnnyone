#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist/host-app"
TAURI_DIR="$ROOT/desktop/src-tauri"
BIN="$TAURI_DIR/target/release/johnnyone-desktop"

export PATH="${HOME}/.cargo/bin:${PATH:-}"

echo "Removing stale host-app dist ($DIST)..."
rm -rf "$DIST"

# Angular keeps a persistent build cache in .angular/cache that can serve stale
# Ionic/Angular compiled assets even with --skip-nx-cache. Wipe it so the embedded
# webview assets are always freshly compiled.
echo "Removing stale Angular build cache ($ROOT/.angular/cache)..."
rm -rf "$ROOT/.angular/cache"

echo "Building host-app..."
cd "$ROOT"
# --skip-nx-cache: a cache replay restores the cache entry but does NOT re-emit
# dist/host-app/browser after the rm above, so cargo's frontendDist path vanishes
# and tauri::generate_context! panics. Force a real build every time.
npx nx build host-app --skip-nx-cache

if [[ ! -f "$DIST/browser/index.html" ]]; then
  echo "host-app build did not emit $DIST/browser/index.html" >&2
  exit 1
fi

echo "Building desktop binary (production webview)..."
cd "$TAURI_DIR"
touch build.rs
cargo build --release --bin johnnyone-desktop --features tauri/custom-protocol

echo "Built $BIN"