# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- **Grok CLI provider (terminal-mode).** xAI's `grok` TUI is now a selectable
  provider, launched in a tmux pane as `grok --always-approve`. Available in the
  terminal session picker and the shared provider-selector. Chat-mode/planner
  reject it for now (no streaming runner yet). Touches `CliProvider`,
  `detect_cli_tools`, `terminal.rs::provider_command`, and the three chat-mode
  dispatch matches.
- **Agent operations guide.** New [`docs/operations.md`](docs/operations.md)
  collects build/run/debug/deploy gotchas (toolchain + apt deps, the
  directory-move build-cache trap, `npx tauri build` vs `cargo build`, local-dev
  launch identity, the terminal attach path, lokal CLI setup, and deploy creds).
- **Enriched planner/development event reasons.** The desktop planner
  orchestrator now parses reviewer `SUMMARY`, `FINDINGS`, and
  `NEXT_STEPS` blocks from T2 output and promotes the first concrete
  finding into event `reason`.
  - `planning_gate_result` / `agent_phase_gate_result` now store review
    context instead of only verdict/action.
  - `planning_feedback_sent_to_planner` /
    `agent_feedback_sent_to_worker` now carry the same review reason so
    the event row explains why the run was sent back.
- **Planner prompt enforcement for non-pass verdicts.** Default planner
  prompts and the live `~/.johnnyone/planner-prompts.yml` now require
  T2 to provide at least one concrete finding and one concrete next step
  for `NEEDS_CHANGES` or `BLOCKED`.

### Changed
- **Planner event log semantics.** Event rows are expected to represent
  decision facts, not only routing facts. A send-back event without a
  meaningful review reason is now considered incomplete behavior.
- **Project docs updated for current planner behavior.** README and
  app-local conventions now document soft-close/reopen behavior,
  persisted run rename, single-phase execution mode, shared terminal
  ownership, and review-reason expectations in the event log.

### Fixed
- **Directory picker no longer hardcodes a machine path.** `browse_host_directory`
  now resolves empty/`~`/`~/…` against the host home directory and falls back to
  home for any unresolved path, instead of erroring `Invalid parent path`. The
  hardcoded `/home/creepy/documents/workspace` defaults were removed from the
  planner and terminal pages.

## [2026-05-25]

### Added
- **Per-plan git history.** Every agent plan keeps its own `.git` repo
  inside `plan_path`. `ensure_repo` runs lazily on `start_plan`. Every T2
  PASS becomes a commit on `main` — `initial: <title>` for the first PASS,
  `phase {id}: {title} (validated)` for development phase passes,
  `amend: <brief excerpt>` for amendment passes, `revalidate: <title>` for
  re-runs without an amend. Failures (git not installed, permissions)
  silently degrade — plans still work without history.
  - `desktop/src-tauri/src/services/git_history.rs` (new) — `ensure_repo`,
    `commit_all`, `diff_head`, `has_any_commit`, `amend_commit_message`.
  - `services/agent_plans.rs::commit_plan_on_pass` — invoked from both
    planning and development T2 PASS handlers.
- **Plan amendment workflow.** `updateAgentPlanAmend(id, brief): AgentPlanRun!`
  mutation re-runs T1 in "edit mode" + T2 reviews the resulting
  `git diff HEAD`. UI is an "Amend" button in the planner coordinator panel
  with an Ionic textarea modal.
  - New `agent_plans.amend_brief` column (migration 007). Set on amend
    invocation, cleared by `commit_plan_on_pass` when T2 PASSes.
  - New prompt templates `DEFAULT_AMEND_PLANNING_PLANNER` /
    `DEFAULT_AMEND_PLANNING_REVIEWER` with `{{amendment_brief}}` and
    `{{git_diff}}` placeholders.
  - Resolver lives at `worker/resolvers/ai/update-agent-plan-amend.ts`
    (uses an `update-` prefix because lokal's validator infers Query vs
    Mutation from the filename prefix — `amend-` is not a recognized
    mutation prefix).
- **Session kinds.** `sessions.kind` column with values `'user'` |
  `'agent'` (migration 008). `/terminal` only shows user-created sessions;
  planner/development T1+T2 sessions are tagged `'agent'` and filtered
  out. `list_sessions` defaults to `kind='user'`.
- **Convention paths in planner prompts.** All four default planner
  templates inject `{{conventions_path}}` pointing at
  `{workspace}/common/conventions/`. T1 + T2 read every file under that
  path before generating or validating a plan.
- **Mermaid zoom modal.** Global `<app-mermaid-zoom-modal>` reachable via
  `MermaidZoomService`. Click any rendered mermaid (planner preview or
  chat message) to open full-screen. Wheel zoom, pinch zoom, drag pan,
  double-tap-to-reset, +/-/Fit/Close buttons. Fit-to-viewport on open so
  diagrams don't render microscopic on phones.
- **Mermaid inline rendering in chat.** `message-bubble.component.ts`
  detects `` ```mermaid `` code fences during markdown render and swaps in
  the actual SVG. Clicking opens the same zoom modal.
- **Markdown link interceptor in planner.** Clicks on relative links
  inside `.plan-markdown-preview` are intercepted via a
  `@HostListener('document:click')`. Paths like
  `../../common/methodology.md` are resolved against the current plan
  file's directory and opened in the same preview pane. Absolute
  `http(s)://` and `mailto:` links pass through to the browser.
- **Mobile-friendly navigation.** `<ion-split-pane>` left menu collapses
  to a swipe drawer below 768px. Each page topbar gets an
  `<ion-menu-button>` hamburger + a page-title chip (TERMINAL / PLANNING
  / DEVELOPMENT). Side menu is draggable to resize.
- **Workspace picker modal.** Clicking "New Terminal" opens an Ionic
  directory browser instead of creating a session at the default path.
  Choice persisted as `last_working_directory`.
- **Shell provider for terminal sessions.** New `CliProvider::Shell`
  variant in the host — spawns `$SHELL` (or bash) in tmux. Shell sessions
  don't accept chat-mode messages; you type directly into the terminal.
- **xterm.js onData wiring.** Tab autocomplete, arrow-key history,
  Ctrl-R, Ctrl-C etc. now flow through to the shell. Mobile textarea
  intercepts Tab keypresses too.

### Changed
- **All planner modals refactored to Ionic primitives.** Files, Setup
  (New Planner), Browser (host directory picker), Phase Tasks all use
  `<ion-modal>` + `<ion-segment>` + `<ion-list>` + `<ion-item>` +
  `<ion-input>` / `<ion-textarea>` + `<ion-button>` per
  `common/conventions/ui.md`. Legacy `.modal-backdrop` / `.setup-modal` /
  `.files-modal` markup removed.
- **Dark theme forced globally.** Imports
  `@ionic/angular/css/palettes/dark.always.css` + adds a `--jo-*` token
  palette. `<meta name="color-scheme" content="dark">` + `theme-color`
  so browser chrome matches.
- **Plan-tab switch resets state.** `selectPlan` now wipes terminal
  screens + file-modal signals + selections + browse paths before
  swapping `currentRun`. `attachPlanTerminals` prunes stale screens and
  calls `refreshVisual` (not just `subscribeVisual`) so the host
  re-emits the current pane state.
- **Files modal scrolls correctly on mobile.** Replaced the 7752px-tall
  unbounded layout with `<ion-content>` which gives momentum scroll +
  safe-area handling for free.
- **Modal close buttons + safe-area on iOS.** `padding-top:
  env(safe-area-inset-top)` on the modal `::part(content)` so the close
  × stays tappable below iOS notch / URL bar.

### Fixed
- **Terminal last-row clipping.** Removed vertical padding from
  `.terminal-host` + switched to `proposeDimensions()` + `terminal.resize()`
  so xterm doesn't overdraw one row off-screen.
- **Tmux session leaks on kill.** `archive_session` and `delete_session`
  now call `kill_terminal_session` so the tmux pane is dropped with the
  session row.
- **`listProviderConfigs` returning null cliPath.** Added snake_case →
  camelCase conversion in `worker/resolvers/ai/list-provider-configs.ts`
  (and the same for `upsert-provider-config.ts`).
- **`No online desktop node found` on direct page nav.** Tab-switch
  reset ensures the right node lookup runs for the current plan.

## [2026-05-23]

### Added
- **Multi-user SaaS architecture shipped.** Six phases of the
  `multi-user-saas` plan landed: foundation cleanup, web scaffold +
  navigation, multi-tenant pairing + relay-RPC, Tauri control panel
  (consolidated into one binary), ADRs.
- **Single Tauri desktop binary (`johnnyone-desktop`).** The earlier
  split between `johnnyone-host` (headless daemon) and `johnnyone-desktop`
  (Tauri shell) was collapsed. The Tauri binary now opens a window AND
  runs the embedded host's GraphQL listener on `:7788`, registers as a
  desktop node, holds an outbound WebSocket to `ChatRelayDO`. Cleaner
  install story (one binary instead of two).
- **Web client at `johnnyone-dev.pages.dev`.** Ionic 8 / Angular 19
  routes: `/login`, `/terminal`, `/planning`, `/development`,
  `/settings`, `/install`. Side menu + page-level navigation.
- **Worker on elitechance CF account.** Deployed as `johnnyone-dev-hub`
  at `https://johnnyone-dev-hub.ethan-353.workers.dev`. D1 +
  ChatRelayDO. ~75 auto-wired resolvers.
- **Terminal page restored from `attic/`.** Brought the chat /
  terminal-session UX (xterm.js + tmux-relay + provider picker) into the
  new web client. Replaced the placeholder echo stub.
- **Planner / Development pages restored from `attic/`.** T1 + T2
  side-by-side panes with the coordinator, phase navigation, file
  browser, prompt-settings drawer. Two modes (`/planning` and
  `/development`) backed by the same component with route data.
- **Per-plan tmux runtime.** Each session gets its own tmux pane. The
  provider CLI (Claude Code / Codex / Cline / Ollama / Shell) is the
  command tmux runs.

### Changed
- **Worker renamed.** `johnnyone-dev-johnnyone-hub` → `johnnyone-dev-hub`
  (worker.yaml `name: hub` so lokal's `{project}-{env}-{worker.name}`
  prefix doesn't double up the `johnnyone-` segment). Old worker deleted
  via API after the cutover.
- **Wrangler 3 → 4.** Bumped `package.json` devDeps `wrangler ^3.80.0 →
  ^4.94.0` to resolve a stale project→account binding issue that was
  routing dev deploys to the wrong CF account.
- **`config.amend_brief` lifecycle clear.** `commit_plan_on_pass` now
  clears `amend_brief` to NULL after the amend commit lands so future
  PASSes don't keep using the old amendment label.

### Removed
- **`johnnyone-host` binary.** Folded into `johnnyone-desktop`. The
  `Cargo.toml` `[[bin]]` entry for `johnnyone-host` is gone.
- **Legacy forward-path AI resolvers.** Phase 2 of the multi-user-saas
  plan deleted `worker/lib/runtime/host-graphql.ts` +
  `sendAiChatMessage` / `onAiChatDelta` / `onAiChatComplete` — replaced
  by `sendRelayChatMessage` / `onRelayChatDelta` / `onRelayChatMessage`
  via `ChatRelayDO`.

## [Initial scaffold]

Original architecture before the multi-user-saas pivot. Many specifics
listed below have been replaced by the entries above — kept for
historical reference only.

### Added
- Nx monorepo scaffold with 7 project areas (163 files)
- Worker: D1 schema with 8 AI tables (ai_sessions, ai_messages,
  ai_tool_definitions, ai_tool_executions, ai_provider_configs,
  desktop_nodes, channel_bindings, ai_usage_log) + auth tables
- Worker: Full GraphQL schema -- 6 queries, 11 mutations, 4 subscriptions
- Worker: 21 resolver files covering session CRUD, message handling,
  tool approval, provider config, desktop node management
- Worker: LLM provider abstraction with Claude (Anthropic Messages API),
  OpenAI (Chat Completions), and Ollama implementations
- Worker: AgentSessionDO Durable Object -- agent loop, context assembly,
  tool dispatch, stream relay
- Worker: Tool schema with 7 built-in tools (shell_execute, file_read,
  file_write, file_list, file_search, process_list, system_info)
- Worker: Channel adapter placeholders for Telegram, Discord, WhatsApp
- Desktop: Ionic/Angular frontend with chat, sessions, tools, and
  settings pages
- Desktop: Tauri 2 Rust backend -- agent WebSocket client, reconnection
  logic, heartbeat
- Desktop: Tool executors -- shell (with timeout + blocked commands),
  filesystem (with path sandboxing), process (via sysinfo)
- Desktop: Ollama local provider for offline mode
- Desktop: Offline message queue for sync when back online
- Mobile: Ionic/Angular + Capacitor app (Android) with chat, sessions,
  settings pages
- Mobile: Push notification service
- UI: 8 reusable components -- chat-window, message-bubble,
  message-composer, tool-execution-card, session-list, provider-selector,
  node-status, streaming-text
- UI: GraphQL client service (Observable-based, fetch + graphql-ws)
- UI: JohnnyAPI service with full CRUD operations
- UI: AI chat state management service with signals
- Shared: WebSocket protocol types (tool_call, tool_result, heartbeat,
  session lifecycle)
- Shared: Tool definition constants and JSON parameter schemas
- Shared: Utility functions (ID generation, date formatting)
