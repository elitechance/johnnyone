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

- Use **`npx tauri build --no-bundle`** (from `desktop/src-tauri`). It uses the
  repo's `@tauri-apps/cli` devDependency — no `cargo install` needed.
- `cargo tauri build --no-bundle` is equivalent but additionally requires the
  `cargo-tauri` subcommand (`cargo install tauri-cli`).
- **Never** use `cargo build --release` to produce a runnable binary — it
  compiles and the `:7788` listener binds, but the Tauri window stays blank
  (`Could not connect to localhost`). The Nx `desktop-build` target and lokal's
  `desktop.prod` build both use this wrong command; don't use them for a
  runnable binary.

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

- **Grok** is terminal-mode only: launched as `grok --always-approve`. It has no
  chat-mode streaming runner yet, so chat/planner reject it explicitly.

## Terminal attach path (debugging "stuck attaching")

The terminal does NOT use the GraphQL relay-RPC path. The browser opens a
**separate** WebSocket to the worker (`/api/relay/ws?...&clientType=mobile`);
the worker's `ChatRelayDO` forwards `terminal_command`/`visual_subscribe` to the
desktop (`clientType=desktop`) and streams `terminal_screen` frames back. Both
sides pair by `idFromName(nodeId)`, so they must agree on the same online node.
The desktop logs `terminal_command received from relay` and
`terminal_command failed error=…` (in `agent/mod.rs`) to localize failures.

## Deploying — use the `lokal` CLI

Deploy with the **`lokal` CLI** (preferred — wraps build + Cloudflare deploy):

```bash
lokal cf deploy --env dev          # worker + Pages, both
lokal cf worker deploy --env dev   # worker only
lokal cf pages deploy --env dev    # web/Pages only
lokal cf db migrate --env dev      # apply D1 migrations
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
npx wrangler pages deploy dist/web/browser --project-name johnnyone-dev   # needs `wrangler login` / CLOUDFLARE_API_TOKEN
```
