# JohnnyOne

> "Number 5 is alive!" — Short Circuit (1986)

A personal-AI agent platform shaped as a small multi-user SaaS. Each user
installs a single **desktop binary** on their own machine; everyone uses the
same hosted **web client** from anywhere. The desktop binary owns all data
locally — sessions, messages, planner state, agent CLI subprocesses — and the
Cloudflare Worker is a thin relay between the web browser and that binary.

| Surface | URL |
|---|---|
| Web client | https://johnnyone.pages.dev |
| Worker GraphQL | https://johnnyone.ethan-353.workers.dev/graphql |
| Worker relay WebSocket | wss://johnnyone.ethan-353.workers.dev/api/relay/ws |
| Desktop binary (Linux) | `desktop/src-tauri/target/release/johnnyone-desktop` (built locally) |
| Mac binary | not yet — needs a Mac to build |

## Architecture

One Tauri desktop binary per user, one Worker for everyone, one Pages site for everyone:

```mermaid
flowchart TB
  subgraph hosted["Hosted on Cloudflare"]
    web["Web client (Pages)<br/>Ionic 8 / Angular 19"]
    worker["Worker (johnnyone)<br/>GraphQL + relay-RPC"]
    do["ChatRelayDO<br/>per-user WebSocket relay"]
    d1[("D1 registry<br/>tenants, users,<br/>desktop_nodes, channels")]
  end

  subgraph user["User's machine"]
    bin["johnnyone-desktop (Tauri)<br/>30 MB Linux binary"]
    embed["Embedded host (axum)<br/>127.0.0.1:7788"]
    sqlite[("SQLite<br/>sessions, messages,<br/>plans, settings")]
    tmux["tmux runtime<br/>+ provider CLIs:<br/>Claude Code, Codex,<br/>Cline, Ollama"]
  end

  web -- "GraphQL HTTPS<br/>(JWT in Authorization header)" --> worker
  worker --- d1
  worker -. "relay-RPC over WebSocket<br/>(scoped by tenant + user)" .-> do
  do <-. "outbound WS from user's machine<br/>kept alive by desktop binary" .-> bin

  bin --- embed
  embed --- sqlite
  bin --- tmux

  classDef hostedNode fill:#eef,stroke:#229
  classDef userNode fill:#fee,stroke:#922
  class hosted,web,worker,do,d1 hostedNode
  class user,bin,embed,sqlite,tmux userNode
```

### The big picture

1. User installs the desktop binary on their machine. The binary registers
   the machine as their `desktop_node` (one row in D1) and keeps an outbound
   WebSocket open to `ChatRelayDO`.
2. User opens the hosted web client in any browser, logs in, gets a JWT.
3. Every action in the web client (open a session, send a message, browse the
   filesystem) is a GraphQL request to the worker. The worker resolves the
   user's online `desktop_node` from D1 and forwards the call via
   relay-RPC over the open WebSocket to that user's binary.
4. The desktop binary handles the call locally (writes to its SQLite, spawns
   provider CLIs in tmux panes, etc.) and streams responses back the same way.

The web client holds **no user data**. Everything lives in the user's local
SQLite. If two users have a binary running, each only ever sees their own
data — the worker scopes every relay-RPC by `WHERE tenant_id = ? AND user_id = ?`.

### Why a single binary

Originally split into two: `johnnyone-host` (headless daemon) + `johnnyone-desktop`
(Tauri shell). Today both are folded into a single `johnnyone-desktop` Tauri
binary that opens a window AND runs the embedded host listener. One binary to
install, one process to supervise. See `personal/docs/johnnyone/decisions/`
for the rationale.

## Project structure

```
johnnyone/
  web/                      Web client — Ionic 8 / Angular 19 (the hosted Pages app)
    src/app/pages/            login, terminal, planning, development, settings, install
    src/app/services/         auth, auth.guard, relay-terminal (WS for terminal stream)
  host-app/                 Tauri-window control panel — login / status / providers
    src/app/                  Embedded inside the desktop binary's webview
  ui/                       Shared Angular component library
    src/components/           chat-window, terminal-screen, message-bubble, ...
    src/services/             JohnnyApiService — single GraphQL client used by web + host-app
  desktop/
    src-tauri/              Rust — single Tauri binary (johnnyone-desktop)
      src/main.rs             Tauri entrypoint: opens window + spawns embedded host
      src/host/               axum-based GraphQL listener on :7788
      src/providers/          CLI runners: claude_code, codex, cline, ollama_cli
      src/terminal.rs         tmux runtime (create/capture/send-keys, screen mirror)
      src/services/           agent_plans, chat_host, providers, sessions, ...
      src/agent/              outbound WS client + RPC dispatch
      src/db/                 SQLite + migrations
    project.json              Nx targets: desktop-dev / desktop-build / tauri-dev / tauri-build
  worker/                   Cloudflare Worker — GraphQL gateway
    resolvers/ai/             AI resolvers (all routed to host via relay-RPC)
    resolvers/channels/       Channel adapters (Telegram/Discord/WhatsApp stubs)
    lib/runtime/              relay-rpc.ts, desktop-rpc.ts, chat-relay-do.ts
    d1/migrations/            D1 registry schema
    schema/                   GraphQL schema (johnnyone-ai.graphql, johnnyone-channels.graphql)
    worker.yaml               legacy `name: hub` (ignored for deploy naming)
  attic/                    Frozen old code (excluded from Nx via .nxignore)
    desktop-thin-client/      Original combined desktop UI — superseded by web + host-app
    mobile-thin-client/       Original mobile Capacitor wrap — deferred
  scripts/build-desktop.sh  Build the desktop binary fresh (`npm run build:desktop`)
  scripts/dev-tmux.sh       Optional local-dev tmux launcher
  lokal.yaml                Local processes, builds, deploy config
```

## Planner and Development Notes

> **Coordinator internals** — the T1/T2 handoff, the `reportAgentResult` API (the
> sole completion/verdict signal; terminal scraping was removed), the
> nudge/escalate + clarify reliability mechanisms, and why scraping was dropped
> (Grok alt-screen scroll-off) live in
> **[`docs/coordinator.md`](docs/coordinator.md)**.

Recent planner/runtime behavior worth knowing:

- Planning and Development tabs use a soft-close model. Closing a run removes it from the active tab strip but keeps the plan row, sessions, and plan path so it can be re-opened from `Existing Plans` if the plan path still exists.
- Planning and Development run titles are renameable from the coordinator UI and the title is persisted in the local SQLite `agent_plans` row, so reopened runs keep the renamed title.
- Development can start from a selected phase in either `continue` mode or `single` mode. `single` runs stop after the selected phase is approved instead of automatically continuing to later phases.
- The shared terminal widget lives in `ui/src/components/terminal-screen/` and is used by the standalone Terminal page plus Planning/Development T1/T2. Terminal affordances such as upload image, mobile input helpers, mermaid detection, and history loading should be implemented there, not forked per page.
- The Terminal page reconciles all persisted/cached state (pane layouts, `closedPaneIds`, screen cache) against the active session list on load, so a deleted/archived session can never pin a pane or hide a live session across a reload. See [`docs/terminal-state-reconcile.md`](docs/terminal-state-reconcile.md).
- The planner event log is enriched by the desktop backend. Events now carry actor/category/summary/status transitions plus derived review reasons. For T2 `NEEDS_CHANGES` or `BLOCKED`, the backend parses `SUMMARY`, `FINDINGS`, and `NEXT_STEPS` from the reviewer footer and promotes the first concrete finding into the event `reason`.
- T2 send-back events are intended to explain the pushback, not only the transport action. If the event log only says something like `sent back` without a concrete reason, that is considered drift from current behavior.

## Tech stack

| Layer | Tech |
|---|---|
| Web client | Ionic 8 / Angular 19, standalone components |
| Tauri window UI | Same stack, separate Nx project (`host-app/`) |
| Shared components | `ui/` library — `JohnnyApiService`, terminal-screen, chat-window, etc. |
| Desktop binary | Tauri 2 + Rust (`async-graphql`, `axum`, `rusqlite`, `tokio-tungstenite`) |
| Agent runtime | tmux + provider CLIs (Claude Code, Codex, Cline, Ollama) |
| Worker / gateway | Cloudflare Workers (TS) |
| Edge data | D1 (registry) + Durable Objects (`ChatRelayDO`) |
| GraphQL | Schema-first, `graphql-yoga` on web, custom resolver wiring in the worker |
| Monorepo | Nx 20 |

## Getting started

### Prerequisites

- Node.js 20+
- Rust / Cargo (the Tauri binary compiles native code)
- tmux 3+ (provider CLIs run inside it)
- Display server for the Tauri window (WSLg, X11, or macOS native)
- For Tauri builds on Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`

### Install

```bash
cd personal/apps/johnnyone
npm install
```

## Running JohnnyOne

Everything goes through **npm**. There are **exactly three scripts** — if you're
looking for any other `npm run …`, it no longer exists; use the commands below.

| Command | What it does |
|---|---|
| `npm run desktop` | **Builds (no cache) and launches** the desktop backend against the live worker. This is the one you normally run. |
| `npm run build:desktop` | **Builds only** — produces `desktop/src-tauri/target/release/johnnyone-desktop` without launching. Wipes `dist/host-app` + `.angular/cache` and skips the Nx cache, so **no stale Ionic/Angular assets** get embedded. |
| `npm run deploy:web` | **Deploys both hosted Cloudflare surfaces** (prod): the **worker** (`johnnyone.ethan-353.workers.dev`) and the **web client** (`johnnyone.pages.dev`). Builds web fresh (no Nx cache) so no stale bundle is uploaded. |

### Normal use — run the backend, use the hosted web client

This is all an end-user needs. The web client is already hosted; you only run
the backend binary on your own machine so the worker has a `desktop_node` to
forward calls to.

```bash
cd personal/apps/johnnyone
npm install        # first time only
npm run desktop    # build (no cache) + launch against the LIVE worker
```

Then open **https://johnnyone.pages.dev** in any browser and log in. Done.

`npm run desktop` builds the binary, frees port `:7788` (kills any previous
instance), then launches the **release** binary in the **background** (via
`nohup`) pointed at the **live worker** `https://johnnyone.ethan-353.workers.dev`
and returns your prompt. Logs stream to `/tmp/johnnyone-desktop.log`. Stop it
with:

```bash
lsof -ti:7788 | xargs kill
```

If your account isn't the seeded dev user, set identity inline:

```bash
JOHNNYONE_USER_ID=<your-user-id-uuid> \
JOHNNYONE_TENANT_ID=<your-tenant-id-uuid> \
npm run desktop
```

On launch the binary:

1. Applies DB migrations under `~/.local/share/johnnyone/johnnyone.db`
2. Registers your machine with the live worker as an **online `desktop_node`**
3. Keeps an outbound WebSocket open for relay-RPC
4. Listens on `127.0.0.1:7788` for the embedded UI's local GraphQL calls
5. Opens a Tauri window with the host-app control panel (needs a display server)

Confirm it came up online:

```bash
grep "Desktop node registered" /tmp/johnnyone-desktop.log
```

Use `npm run build:desktop` when you only want to compile (e.g. CI, or to check
it builds) without starting the app.

### Ship a web / worker change

```bash
npm run deploy:web          # deploys BOTH: worker + web client (prod)
```

This builds the web client fresh and deploys the worker
(`johnnyone.ethan-353.workers.dev`) **and** the Pages site
(`johnnyone.pages.dev`). If you need finer-grained control:

```bash
lokal cf worker deploy --env prod    # worker only
lokal cf pages deploy --env prod     # Pages only (after a fresh nx build web)
lokal cf db migrate --env prod       # apply D1 migrations (not part of deploy:web)
```

### Developing the code (not just using the app)

When you're iterating on source rather than running the shipped app:

- **Web UI only**, against the live worker — fastest feedback loop:

  ```bash
  npx nx serve web          # http://localhost:4200
  ```

  Log in with the seeded admin (`admin@johnnyone.local` / `johnnyone-dev`,
  tenant `default`). On `localhost` the web app defaults to a local worker at
  `http://127.0.0.1:7714`; to point it at the live worker instead, run in the
  browser console:

  ```js
  localStorage.setItem('johnnyone_worker_url', 'https://johnnyone.ethan-353.workers.dev');
  ```

  Your desktop binary must be running (`npm run desktop`) for any host data to
  load.

- **Host-app window UI with hot reload** — only when iterating on the Tauri
  control-panel UI:

  ```bash
  cd desktop/src-tauri && cargo tauri dev
  ```

  Serves host-app on `:4201` and runs the binary in debug, loading the webview
  from the dev server so Angular HMR works.

- **Full local stack** (desktop + local worker simulator + web):

  ```bash
  ./scripts/dev-tmux.sh
  ```

  tmux session with: desktop (`cargo run`, embedded `:7788`), edge
  (`lokal cf worker sim`, `:7714`), web (`nx serve web`, `:4200`).

> **Build gotcha — blank window / "Could not connect to localhost".** Never build
> the production binary with a plain `cargo build --release`. Without
> `--features tauri/custom-protocol` the webview stays in dev mode and tries
> `http://localhost:4201`, so you get a blank window even though `:7788` binds.
> **`npm run desktop` is the correct production build** — it sets the
> production-webview flag and embeds fresh assets. Details: `docs/operations.md`.

## Operational notes (hard-won)

Build/run/debug/deploy gotchas a fresh agent needs (toolchain, the
directory-move build-cache trap, local-dev launch identity, the terminal attach
path, and the real deploy story) live in **[`docs/operations.md`](docs/operations.md)**.

## Live deployments

Only the **worker** and **web client** deploy to Cloudflare. The desktop binary
runs on each user's machine.

```bash
npm run deploy:web                      # web client only → johnnyone.pages.dev
lokal cf worker deploy --env prod       # worker only → johnnyone.ethan-353.workers.dev
lokal cf deploy --env prod              # worker + Pages, both
lokal cf db migrate --env prod          # apply D1 migrations
```

### Account + URLs

- CF account: `elitechance` (configured in `~/.lokal/cf.yaml`)
- Worker deploy name: `johnnyone` (`lokal cf worker deploy --env prod`; dev/qa use
  `johnnyone-dev` / `johnnyone-qa`)
  → `https://johnnyone.ethan-353.workers.dev`
- Pages project: `johnnyone` → `https://johnnyone.pages.dev` (slug from
  `lokal.yaml`; deploy with `lokal cf pages deploy --env prod`)

### Worker secrets

Only one secret is required: `JWT_SECRET` on the prod worker script `johnnyone`.

```bash
lokal cf worker secrets set --env prod --name JWT_SECRET --value '<secret>'
lokal cf worker secrets list --env prod
```

**Gotcha:** deploying to a *new* worker name succeeds even when `JWT_SECRET` is
missing; the worker then returns `error code: 1101` until the secret is set.
Secrets cannot be read back from Cloudflare — when renaming from a legacy script
(e.g. `johnnyone-hub`), you must re-set the **same** value manually. See
[`docs/operations.md`](docs/operations.md) § Deploy gotchas.

### Deploy gotchas

Full list (JWT_SECRET trap, D1 migration idempotency, `nx` on PATH, lokal naming,
`ethan-353` workers.dev subdomain): **[`docs/operations.md`](docs/operations.md)**
§ Deploying.

- **Nx caches builds aggressively.** If you've changed `web/` or `ui/` code
  but `npx nx build web` reports "Nx read the output from the cache instead
  of running the command," force a real rebuild:

  ```bash
  rm -rf .nx/cache dist/web dist/ui
  npx nx build web --skip-nx-cache
  ```

  Always do this before `lokal cf pages deploy --env prod` if you've changed
  components — otherwise the deploy uploads a stale bundle.

- **Lokal's resolver-schema validator infers Query vs Mutation from the
  filename prefix.** Recognized mutation prefixes:

  ```
  create-, update-, upsert-, delete-, remove-,
  assign-, complete-, admin-, refresh-,
  start-, send-, close-, revoke-, report-,
  publish-, make-, request-, share-,
  change-, link-, unlink-, register-, authenticate-, mark-
  ```

  Anything else defaults to Query. If your mutation resolver isn't
  recognized (validator complains `Missing resolver: Mutation.fooBar
  (expected: resolvers/foo-bar.ts)`), rename the file to use a recognized
  prefix and align the GraphQL field. You can keep a friendly public name
  in `JohnnyApiService` by wrapping it.

- **`cargo build` is not the way to build the production binary.** The desktop
  binary embeds `dist/host-app/browser` at compile time *and* needs the Tauri
  runtime built in production-webview mode. `cargo build --release` does
  neither for you. Use **`npm run desktop`** (runs `scripts/build-desktop.sh`)
  — see "Running JohnnyOne" above.

## GraphQL API

The worker exposes ~75 auto-wired resolvers grouped by capability. Every
mutation/query that needs host data is routed via `relayRpc` /
`desktopRpc` to the user's online desktop binary. The same surface (plus
`/api/relay/ws`) is the public Partner API. The integration guide is
served in-app at the public **`/integration`** route
(https://johnnyone.pages.dev/integration); the design copy lives at
[`docs/api-partner/index.html`](docs/api-partner/index.html).

- **Auth** (lokal builtin) — `login`, `loginWithOauth`, `myCompleteFirstLogin`, `adminCreate{User,Tenant}`, `refreshToken`
- **Sessions** — `listAiSessions`, `getAiSession`, `createAiSession`, `updateAiSession{Title,Provider,WorkingDirectory,Archived}`, `deleteAiSession`
- **Chat** — `sendRelayChatMessage`, `cancelAiGeneration`, `listAiMessages`
- **Agent planner** — `listAgentPlans`, `getAgentPlan`, `createAgentPlan`, `startAgentPlan`, `updateAgentPlanAmend`, `updateAgentPlan{Stopped,Blocked}`, `updateAgentPhaseManualPass`, `retryAgentReviewer`, `sendAgentFeedbackToWorker`, `deleteAgentPlan`
- **Workspace / host files** — `browseHostDirectory`, `listWorkspaceFiles`, `readHostFile`, `getWorkspaceFileDiff`, `validateWorkspacePlan`
- **File manager (`files_root`-rooted)** — `filesListDir`, `filesRead`, `filesWrite`, `filesMkdir`, `filesRename`, `filesDelete`, `filesUploadChunk` (chunked upload). Path-guarded + size-capped, scoped by `files:read`/`files:write`. See [`docs/host-transport.md`](docs/host-transport.md)
- **Terminal / shell** — `captureTerminal` (deterministic one-shot pane snapshot; live I/O still rides the WSS `terminal_*` envelopes)
- **Providers / settings** — `listDetectedCliTools`, `listProviderConfigs`, `upsertProviderConfig`, `deleteProviderConfig`, `getSetting`, `setSetting`, `getPlannerPromptSettings`, `updatePlannerPromptSettings`
- **Nodes** — `listDesktopNodes`, `registerDesktopNode`, `updateDesktopNodeStatus`
- **API keys (M2M, optional)** — `createApiKey`, `listApiKeys`, `revokeApiKey`
- **Channels / attachments** — `listChannelBindings`, `link/unlinkChannel`, `createChannelWebhook`, `get/create/deleteChatAttachment`
- **Subscriptions** — `onRelayChatDelta`, `onRelayChatMessage`, `onDesktopNodeStatus`

Schema files:
- [`worker/schema/johnnyone-ai.graphql`](worker/schema/johnnyone-ai.graphql)
- [`worker/schema/johnnyone-api-keys.graphql`](worker/schema/johnnyone-api-keys.graphql)
- [`worker/schema/johnnyone-channels.graphql`](worker/schema/johnnyone-channels.graphql)

## Documentation

Repo-local docs live alongside the code:

- **`README.md`** (this file) — overview, structure, run + deploy commands,
  feature inventory, deploy gotchas
- **`CHANGELOG.md`** — dated log of what shipped, in Keep-a-Changelog format
- **`docs/api-partner/index.html`** — Partner/third-party API guide design copy
  (auth, GraphQL, live WSS terminal envelopes, examples, errors). The live guide
  is served in-app at the public `/integration` route
  (`web/src/app/pages/integration/`); keep the two in sync.
- **`docs/api-partner/runbooks/`** — API versioning and partner service-account
  provisioning runbooks
- **`docs/terminal-state-reconcile.md`** — how the Terminal page prunes stale
  persisted/cached session state on load
- **`docs/host-transport.md`** — the host↔web transport primitives (overhaul P2):
  the `files_root` file-manager surface, shell I/O + `captureTerminal` over the
  relay, and the per-session `StreamEvent` channel. Plumbing + types; the UIs that
  consume them are later phases

## Notable features shipped beyond the multi-user-saas plan

The bullets below track work that happened on top of the multi-user-saas
plan, so a fresh session can know what's already live.

- **Per-plan git history (2026-05-25)** — every approved plan state lands as
  a commit in `plan_path/.git`. Initial + amend + revalidate + phase-pass
  commits. Helpers in `desktop/src-tauri/src/services/git_history.rs`.
- **Plan amendments (2026-05-25)** — `updateAgentPlanAmend(id, brief)`
  mutation re-runs T1 in edit-mode + T2 on the diff. UI: "Amend" button in
  the planner coordinator panel. Storage: `agent_plans.amend_brief` column
  (migration 007).
- **Session kinds (2026-05-25)** — `sessions.kind = 'user' | 'agent'`
  (migration 008). `/terminal` only shows user-created sessions; planner
  T1/T2 sessions are hidden. `list_sessions` defaults to `kind='user'`.
- **Convention paths in planner prompts (2026-05-25)** — `{{conventions_path}}`
  pointing at `{workspace}/common/conventions/` is injected into all four
  default planner prompt templates. T1/T2 read every file under that path
  before producing or validating a plan.
- **Markdown link interceptor in planner (2026-05-25)** — clicks on relative
  links inside a rendered plan markdown (e.g. `../../common/methodology.md`)
  are intercepted via a `document:click` HostListener, resolved against the
  current plan file's directory, and opened in the same preview pane.
- **Mermaid zoom modal (2026-05-25)** — global `<app-mermaid-zoom-modal>`
  reachable via `MermaidZoomService`. Click any rendered mermaid diagram
  (planner preview or chat message) to open full-screen. Pinch + drag +
  wheel + double-tap-reset.
- **All four planner modals are Ionic-native (2026-05-25)** — Files, Setup
  (New Planner), Browser (directory picker), Phase Tasks. Use
  `<ion-modal>` + `<ion-segment>` + `<ion-list>` + `<ion-input>` /
  `<ion-textarea>` per `common/conventions/ui.md`. The legacy
  `.modal-backdrop` + `.setup-modal` markup is gone.
- **Partner / third-party API ("J1 API") (2026-06)** — Authenticated
  GraphQL + WSS (`/api/relay/ws`) for external partners (hosted service
  account). JWT (or `?token=`) on WSS upgrade + server-side node
  resolution from token (never client `nodeId`); session-ownership gate
  (`forbidden_session`); optional scoped M2M API keys (`jk_*`,
  `createApiKey` etc.); public in-app integration guide at `/integration`
  + examples. See `docs/api-partner/`.
- **Mobile-friendly navigation (2026-05-25)** — `<ion-split-pane>` left
  menu collapses to a swipe drawer below 768px. Hamburger menu button +
  page title chip (TERMINAL / PLANNING / DEVELOPMENT) in each page's
  topbar. Tab-switch state reset so terminal screens + file lists clear
  cleanly between plans.
- **Tab autocomplete in shell sessions (2026-05-23)** — xterm `onData`
  wired so Tab, arrow keys, Ctrl-R etc. all reach the shell. Mobile
  textarea Tab is intercepted and flushed to the shell as `\t`.

## Roadmap

| Area | Status |
|---|---|
| Multi-user SaaS — foundation cleanup (Phase 0) | Done |
| Multi-user SaaS — `web/` scaffold + nav (Phase 1) | Done |
| Multi-user SaaS — pairing + relay-RPC end-to-end (Phase 2) | Done |
| Multi-user SaaS — Tauri control panel (Phase 3) | Done — host folded into one binary |
| Multi-user SaaS — Mac binary + R2 hosting (Phase 4) | Pending (needs Mac) |
| Multi-user SaaS — ADRs (Phase 5) | Done |
| Terminal page restored from attic + workspace browser | Done (2026-05-23) |
| Planner / Development modes restored from attic | Done (2026-05-23) |
| Per-plan git history + amend workflow | Done (2026-05-25) |
| Ionic-native modals + mobile responsive nav + mermaid zoom | Done (2026-05-25) |
| Partner / third-party API (J1: authenticated GraphQL + WSS) | Done (2026-06) |
| Channel adapters (Telegram, Discord, WhatsApp) | In progress (resolvers stubbed) |
| Browser automation, cron scheduling, voice input | Planned |

## License

Private / Proprietary. All rights reserved.
