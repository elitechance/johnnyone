#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/desktop/src-tauri/target/release/johnnyone-desktop"
LOG="${JOHNNYONE_DESKTOP_LOG:-/tmp/johnnyone-desktop.log}"

export PATH="${HOME}/.cargo/bin:${PATH:-}"
export DISPLAY="${DISPLAY:-:0}"

# Always use the live worker — plain cargo build + stale env vars are a common footgun.
export JOHNNYONE_WORKER_URL="https://johnnyone.ethan-353.workers.dev"
export JOHNNYONE_TENANT_ID="${JOHNNYONE_TENANT_ID:-00000000-0000-0000-0000-000000000001}"
export JOHNNYONE_USER_ID="${JOHNNYONE_USER_ID:-00000000-0000-0000-0000-000000000002}"

if [[ ! -x "$BIN" ]]; then
  echo "Binary missing. Build first:" >&2
  echo "  npm run build:desktop   # wipes dist/host-app, rebuilds UI + binary" >&2
  exit 1
fi

if pgrep -f "$BIN" >/dev/null 2>&1; then
  echo "johnnyone-desktop already running"
  exit 0
fi

nohup env \
  JOHNNYONE_WORKER_URL="$JOHNNYONE_WORKER_URL" \
  JOHNNYONE_TENANT_ID="$JOHNNYONE_TENANT_ID" \
  JOHNNYONE_USER_ID="$JOHNNYONE_USER_ID" \
  "$BIN" >>"$LOG" 2>&1 &
echo "Started johnnyone-desktop (log: $LOG)"