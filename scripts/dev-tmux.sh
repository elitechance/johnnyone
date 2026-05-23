#!/usr/bin/env bash
# Launch the JohnnyOne local dev stack in tmux.
#
# Layout:
#   session "johnnyone"
#     window "backend"  -> pane 0: host (Rust, :7788)   | pane 1: edge worker (:7714)
#     window "client"   -> pane 0: web (Angular, :4200)
#
# The deployed web client is always live on Cloudflare Pages at
# https://johnnyone-dev-web.pages.dev/. This script runs everything LOCALLY for dev.
#
# Process commands + env mirror lokal.yaml (local.processes). If you change one,
# change the other. See personal/docs/johnnyone/runbooks/local-dev.md.
#
# Phase 0 / Phase 1 gap (multi-user-saas plan): `nx build web` and `nx serve web`
# will fail until the new web/ Nx project is created in Phase 1. The script still
# starts host + edge successfully; the web pane errors out and exits. Until Phase 1
# lands, dev with just `npm run start:host` + `npm run start:worker`.
#
# Usage: ./scripts/dev-tmux.sh         attach to the session after start
#        ./scripts/dev-tmux.sh --no-attach   start detached
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SESSION=johnnyone
ATTACH=1
[[ "${1:-}" == "--no-attach" ]] && ATTACH=0

# 1. Free stale ports and any prior session — stale processes serve cached code.
for port in 7788 7714 4200; do
  lsof -ti :"$port" | xargs -r kill -9 2>/dev/null || true
done
tmux kill-session -t "$SESSION" 2>/dev/null || true

# 2. pre_sim hook (lokal.yaml local.hooks.pre_sim).
#    Will fail until web/ exists (Phase 1) — that's OK; the host+edge panes still come up.
( cd "$ROOT" && npx nx build web ) || echo "(pre_sim: nx build web failed — Phase 1 not landed yet; continuing)"

# Poll a GraphQL/HTTP endpoint until it answers (mirrors lokal.yaml `ready` checks).
wait_url() {
  local url="$1" name="$2" tries=0
  echo "waiting for $name ($url) ..."
  until curl -sf "$url" >/dev/null 2>&1; do
    sleep 1
    tries=$((tries + 1))
    if (( tries > 120 )); then echo "timed out waiting for $name" >&2; exit 1; fi
  done
}

# 3. Window "backend": host first, then edge (edge depends_on host).
tmux new-session -d -s "$SESSION" -n backend -c "$ROOT/desktop/src-tauri"
tmux send-keys -t "$SESSION:backend" \
  'JOHNNYONE_HOST_ADDR=127.0.0.1:7788 \
JOHNNYONE_WORKER_URL=http://127.0.0.1:7714 \
JOHNNYONE_TENANT_ID=00000000-0000-0000-0000-000000000001 \
JOHNNYONE_USER_ID=00000000-0000-0000-0000-000000000002 \
cargo run --bin johnnyone-host' C-m

wait_url http://127.0.0.1:7788/graphql host

tmux split-window -t "$SESSION:backend" -h -c "$ROOT"
tmux send-keys -t "$SESSION:backend.1" \
  'HOST_GRAPHQL_URL=http://127.0.0.1:7788/graphql \
npx lokal cf worker sim --project johnnyone --worker-dir worker --port 7714' C-m

wait_url http://127.0.0.1:7714/graphql edge

# 4. Window "client": web (depends_on edge).
tmux new-window -t "$SESSION" -n client -c "$ROOT/web"
tmux send-keys -t "$SESSION:client" 'npx ng serve --port 4200' C-m

tmux select-window -t "$SESSION:backend"

echo "JohnnyOne dev stack started in tmux session '$SESSION'."
echo "  host    http://127.0.0.1:7788/graphql"
echo "  edge    http://127.0.0.1:7714/graphql"
echo "  web     http://127.0.0.1:4200  (fails until Phase 1 lands web/)"

if [[ "$ATTACH" == "1" ]]; then
  tmux attach -t "$SESSION"
else
  echo "Attach with: tmux attach -t $SESSION"
fi
