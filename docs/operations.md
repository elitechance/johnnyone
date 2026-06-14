# Operations — build, run, debug, deploy

Practical, hard-won notes for a fresh agent/contributor. The [README](../README.md)
is the overview; this file is the "things that bite you" companion.

## Toolchain that must actually be present (Linux / Ubuntu 26.04)

```bash
# Rust (the binary compiles native code)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# Tauri build deps + pkg-config (the Rust build fails without the -dev headers)
sudo apt-get install -y pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

- **`tmux` must be installed AND on the binary's `PATH`.** Provider TUIs run in
  tmux panes; if `tmux` (or the provider CLI like `claude` / `grok`) isn't
  found, terminal sessions fail with `Failed to run tmux [...]: No such file or
  directory (os error 2)` and the web UI stays stuck on **ATTACHING / DETACHED**.
  That error surfaces in the **browser console** ("Terminal command was
  rejected: …"), not just the desktop log.

## Building the binary

### Recurring gotcha: blank Tauri window — "Could not connect to localhost"

**Symptom:** JohnnyOne window is blank; webview shows
`Could not connect to localhost: Connection refused`. The desktop log may still
look healthy (`Embedded host GraphQL listener up addr=127.0.0.1:7788`, relay
registering) — backend and webview are separate; a working `:7788` does **not**
mean the UI loaded.

**Root cause:** The binary was built in Tauri **dev webview mode**. The webview
tries to load `http://localhost:4201` (Angular dev server per `devUrl` in
`desktop/src-tauri/tauri.conf.json`) instead of embedded assets at
`tauri://localhost`. This happens when `cargo build --release` runs **without**
enabling the `tauri/custom-protocol` feature on the `tauri` crate.

**Correct builds (pick one):**

All default build paths **delete `dist/host-app` first** so orphaned hashed
chunks and stale `index.html` references cannot linger. The cargo step also
`touch`es `build.rs` so the binary re-embeds the fresh dist.

```bash
# Default — wipes dist, rebuilds host-app, embeds into release binary
npm run build:desktop
# (runs scripts/build-desktop.sh)

# Full Tauri pipeline — beforeBuildCommand in tauri.conf.json also wipes dist
cd desktop/src-tauri
npx tauri build --no-bundle

# Manual equivalent when `npx tauri build` fails (e.g. inotify limit below)
rm -rf dist/host-app
npx nx build host-app
cd desktop/src-tauri
touch build.rs
cargo build --release --bin johnnyone-desktop --features tauri/custom-protocol
```

**Wrong — produces the blank window:**

```bash
cargo build --release --bin johnnyone-desktop          # missing tauri/custom-protocol
touch build.rs && cargo build --release --bin ...      # same trap
```

**Run after a good build:**

```bash
./scripts/run-desktop.sh
```

`run-desktop.sh` sets `JOHNNYONE_WORKER_URL` to the live hub
(`https://johnnyone.ethan-353.workers.dev`) and starts the binary with
`DISPLAY=:0`.

**Verify the binary before launching:**

```bash
# Production webview — should print dev=false and custom_protocol
grep -E 'dev=|custom_protocol' desktop/src-tauri/target/release/build/tauri-*/output

# Embedded UI URL (dev binary also contains the devUrl string in config metadata)
strings desktop/src-tauri/target/release/johnnyone-desktop | grep -o 'tauri://localhost' | head -1
```

If `cargo:dev=true` appears in the active `tauri-*/output` and there is no
`cargo:rustc-cfg=custom_protocol`, rebuild with one of the correct commands above.

**Dev mode (intentional):** `cargo tauri dev` / `nx run desktop:tauri-dev` —
spawns `nx serve host-app --port 4201`; webview loads from `:4201` with HMR.
Requires the dev server running alongside the binary. Dev mode does **not**
wipe `dist/host-app` (no production embed).

### Stale embedded UI after a "successful" build

If the window still shows old UI after `npm run build:desktop`:

1. Confirm dist was wiped — `dist/host-app` should only contain files from the
   latest build (one `main-*.js`, matching hashes in `index.html`).
2. Force Nx to ignore cache: `rm -rf dist/host-app && npx nx build host-app --skip-nx-cache`
   then rerun `npm run build:desktop` or the manual cargo step above.
3. Nuclear — corrupt/moved `target/` cache (see below): `cargo clean` in
   `desktop/src-tauri`, then `npm run build:desktop`.

What each clean step removes:

| Step | Removes |
|------|---------|
| `rm -rf dist/host-app` (default in all production builds) | Orphaned JS chunks, stale `index.html` |
| `touch build.rs` (default in `build-desktop.sh`) | Forces Tauri codegen to re-read dist |
| `npx nx build host-app --skip-nx-cache` | Nx-restored dist from an old cache entry |
| `cargo clean` in `desktop/src-tauri` | Rust `target/` including old embedded assets |

### `npx tauri build` blocked by inotify watch limit

On Linux, `npx tauri build --no-bundle` may fail with:

```
failed to watch .../Cargo.toml: OS file watch limit reached
```

Workarounds:

1. Raise the limit (needs sudo): `sudo sysctl -w fs.inotify.max_user_watches=524288`
2. Stop other file-watching dev servers (wrangler, `ng serve`, etc.) to free watches
3. Use the manual cargo fallback above with `--features tauri/custom-protocol`

### Moving the project directory corrupts the Tauri build cache

`tauri-build` bakes absolute `OUT_DIR` paths into `target/`. If the repo is
moved/renamed (e.g. `personal/johnnyone` → `personal/apps/johnnyone`), the next
build fails with:

```
failed to read plugin permissions: ... /target/release/build/tauri-*/out/permissions/app/.../app_hide.toml: No such file or directory
```

Fix: `cargo clean` in `desktop/src-tauri`, then `npx tauri build --no-bundle`
again (full recompile, ~2 min).

## Local-dev launch identity

`lokal.yaml` defines the seeded dev IDs the binary expects when run against the
**deployed** dev worker:

```bash
JOHNNYONE_TENANT_ID=00000000-0000-0000-0000-000000000001
JOHNNYONE_USER_ID=00000000-0000-0000-0000-000000000002
```

For the web relay to actually route to a running binary, the logged-in user's
JWT subject must match `JOHNNYONE_USER_ID` — otherwise the worker resolves a
different (or no) `desktop_node` and RPC/terminal calls never reach it.

## Providers (CLI TUIs)

Terminal-mode providers run the CLI in a tmux pane (`desktop/src-tauri/src/
terminal.rs::provider_command`). Currently: `claude_code`, `codex`, `cline`,
`ollama`, `grok`, `shell`. Adding one touches: `providers/mod.rs` (the
`CliProvider` enum + `as_str`/`from_str`/`default_command`),
`services/providers.rs` (detection list), `terminal.rs::provider_command`
(launch args), the chat-mode matches in `commands/chat.rs`, `services/
chat_host.rs`, `agent/mod.rs` (reject or implement), and the frontend dropdowns
(`web/.../terminal.page.html`, `ui/.../provider-selector.component.ts`).

- **Grok** supports both modes: terminal TUI (`grok --always-approve` in a tmux
  pane) and chat/planner headless (`grok --output-format streaming-json --single`
  with `-r` for session resume).

## Terminal attach path (debugging "stuck attaching")

The terminal does NOT use the GraphQL relay-RPC path. The browser opens a
**separate** WebSocket to the worker (`/api/relay/ws?...&clientType=mobile`);
the worker's `ChatRelayDO` forwards `terminal_command`/`visual_subscribe` to the
desktop (`clientType=desktop`) and streams `terminal_screen` frames back. Both
sides pair by `idFromName(nodeId)`, so they must agree on the same online node.
The desktop logs `terminal_command received from relay` and
`terminal_command failed error=…` (in `agent/mod.rs`) to localize failures.

## Deploying — use the `lokal` CLI

Deploy with the **`lokal` CLI** (preferred — wraps build + Cloudflare deploy).
**Live production uses `--env prod`**, not `dev`.

```bash
# From personal/apps/johnnyone/
npm run deploy:worker                   # worker only → lokal cf worker deploy --env prod
PATH="$PWD/node_modules/.bin:$PATH" \
  lokal cf pages deploy --env prod      # web/Pages only (nx must be on PATH)
lokal cf deploy --env prod              # worker + Pages, both
lokal cf db migrate --env prod          # apply D1 migrations (remote)
lokal cf db status --env prod           # see applied vs pending migrations
lokal cf worker secrets list --env prod # verify JWT_SECRET is set
```

### Resource names (lokal CLI convention)

Lokal names Cloudflare resources from `lokal.yaml` `project.slug` + `--env`:

| Env | Worker script | D1 database | Example URL |
|---|---|---|---|
| **prod** | `johnnyone` | `johnnyone-db` | `https://johnnyone.ethan-353.workers.dev` |
| **dev** | `johnnyone-dev` | `johnnyone-dev-db` | `https://johnnyone-dev.ethan-353.workers.dev` |
| **qa** | `johnnyone-qa` | `johnnyone-qa-db` | `https://johnnyone-qa.ethan-353.workers.dev` |

Notes:

- `worker/worker.yaml` `name: hub` is **legacy** — it is **not** part of the
  deployed worker name anymore (old names like `johnnyone-hub` /
  `johnnyone-dev-hub` are obsolete).
- The middle segment in `*.workers.dev` URLs (here `ethan-353`) is assigned by
  Cloudflare per account — not something we configure in the repo. Record the
  full hostname from deploy output.
- Pages project name is always `johnnyone` (slug from `lokal.yaml`), regardless
  of worker env → `https://johnnyone.pages.dev`.

### Post-deploy smoke check

```bash
curl -s https://johnnyone.ethan-353.workers.dev/graphql \
  -H 'content-type: application/json' \
  -d '{"query":"{ __typename }"}'
# expect: {"data":{"__typename":"Query"}}
```

If you get `error code: 1101` or a worker exception, see **JWT_SECRET** below.

### Deploy gotchas

**1. JWT_SECRET on a new worker name**

`JWT_SECRET` is required in production. Cloudflare stores it as a **write-only**
secret — you cannot read it back from the API.

- **First deploy** to a *new* worker script name: lokal skips the pre-deploy
  secrets check (worker did not exist yet). The upload succeeds even when
  `JWT_SECRET` is missing.
- **Symptom:** deploy prints `✓ Worker: johnnyone` but GraphQL returns
  `error code: 1101`.
- **Fix:** set the secret on the **new** script name before calling the worker
  live:

  ```bash
  # Use the SAME value as the old worker if you want existing logins to keep working
  lokal cf worker secrets set --env prod --name JWT_SECRET --value '<secret>'

  # Or generate a fresh one (all users must log in again):
  echo -n "$(openssl rand -base64 48)" | \
    lokal cf worker secrets set --env prod --name JWT_SECRET
  ```

  When migrating from a legacy name (`johnnyone-hub`), list secrets on the old
  script to confirm `JWT_SECRET` exists, then **re-set the same value** on
  `johnnyone`. Only a human with the original value can do this — CF will not
  export it.

  ```bash
  lokal cf worker secrets list --env prod   # target: johnnyone
  # legacy script (manual wrangler / dashboard): johnnyone-hub
  ```

**2. D1 migrations on an existing prod database**

`lokal cf worker deploy` runs pending migrations before uploading the worker.
If the prod D1 already has tables from a legacy deploy, a migration that uses
bare `ALTER TABLE … ADD COLUMN` can fail with `duplicate column name`.

- Check state: `lokal cf db status --env prod`
- Prefer **idempotent** migrations (`CREATE TABLE IF NOT EXISTS` with all
  columns inline; avoid redundant `ALTER`s on re-runnable paths).
- Do **not** assume a fresh database — `johnnyone-db` may already contain
  tenants, `desktop_nodes`, etc.

**3. Nx build cache (stale Pages bundle)**

If `lokal cf pages deploy` / `nx build web` reports cache hits after you changed
`web/` or `ui/`, force a rebuild before deploying:

```bash
rm -rf .nx/cache dist/web dist/ui
npx nx build web --skip-nx-cache
```

**4. `lokal cf pages deploy` needs `nx` on PATH**

The pages deploy runs `nx build web` in a subshell. If you see `nx: not found`,
prefix PATH:

```bash
PATH="$PWD/node_modules/.bin:$PATH" lokal cf pages deploy --env prod
```

**5. Lokal CLI changes require a rebuild**

If you change resource naming or deploy logic in `lokal/apps/lokal-infra/`,
rebuild and relink the CLI before deploying:

```bash
cd lokal/apps/lokal-infra
npm run -w @lokal/cli build
```

### Setting up the `lokal` CLI (not installed by default)

The CLI source lives at **`lokal/apps/lokal-infra/`**. Build + link it once:

```bash
cd lokal/apps/lokal-infra
npm install --legacy-peer-deps   # AWS-CDK deps have a peer conflict; plain install ERESOLVE-fails
npm run -w @lokal/cli build
npm link -w @lokal/cli            # exposes `lokal` on PATH
lokal --help                     # verify; `lokal cf --help` for Cloudflare ops
```

> The lokal-infra `README.md` is stale — it documents the older AWS/CDK modules
> (`api/db/auth/spa/dns`) and does **not** mention the Cloudflare `cf` module,
> but the CLI does implement `lokal cf …` (see
> `packages/cli/cloudformation/cdk/bin/cli.ts`).

### Cloudflare credentials (required by `lokal cf`)

`lokal cf` resolves credentials from **either** (see
`packages/cli/cloudflare/config.ts`):

- env: `CLOUDFLARE_API_TOKEN` + `CLOUDFLARE_ACCOUNT_ID`, **or**
- `~/.lokal/cf.yaml` with `api_token` + `account_id` for the account (johnnyone
  uses account `elitechance`, per `lokal.yaml`).

These are the **maintainer's secrets** for the `elitechance` account. An agent
can build the web bundle and the binary, and set up the lokal CLI, but **cannot
authenticate to Cloudflare without those credentials** — the human must provide
them (env vars for the session, or a populated `~/.lokal/cf.yaml`).

### Last-resort fallback (no lokal)

```bash
rm -rf .nx/cache dist/web dist/ui && npx nx build web --skip-nx-cache
npx wrangler pages deploy dist/web/browser --project-name johnnyone   # needs `wrangler login` / CLOUDFLARE_API_TOKEN
# Worker: use lokal cf worker deploy --env prod — wrangler.toml is generated under ~/.lokal/simulators/johnnyone/prod/
```
