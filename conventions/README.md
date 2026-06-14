# JohnnyOne Project Conventions

These conventions are app-local context for planning and validation. They sit below `common/conventions/` and make T1/T2 aware of JohnnyOne's actual source layout and runtime boundaries.

## App Shape

JohnnyOne is a hosted Ionic/Angular web client plus a local Tauri/Rust desktop backend. The Cloudflare Worker is a GraphQL and relay-RPC gateway; it should stay thin. User data, terminal sessions, planner state, provider subprocesses, and local filesystem access belong to the desktop backend.

Important paths:

- `web/src/app/pages/planner/` — Planning and Development UI, plan files, git/file explorer, phase controls, T1/T2 terminals.
- `web/src/app/pages/terminal/` — Standalone terminal UI.
- `web/src/app/components/` — Web-only shared UI, including Mermaid and attachment helpers.
- `ui/src/components/terminal-screen/` — Shared terminal widget used by Terminal, Planning, Development, T1, and T2. Do not fork terminal behavior into page-local copies.
- `ui/src/components/` — Shared Angular widgets used by web and host-app.
- `ui/src/services/johnny-api.service.ts` — Shared GraphQL client surface.
- `worker/schema/johnnyone-ai.graphql` — Schema-first API contract for AI/planner/files/git features.
- `worker/resolvers/ai/` — Worker resolvers that route most host-owned operations to desktop relay-RPC.
- `desktop/src-tauri/src/services/agent_plans.rs` — Planner/development orchestration, phase execution, T1/T2 prompt rendering.
- `desktop/src-tauri/src/services/planner_prompts.rs` — Default planner prompt templates.
- `desktop/src-tauri/src/terminal.rs` — tmux terminal runtime.
- `desktop/src-tauri/src/db/` and `desktop/src-tauri/migrations/` — Local SQLite state.

## UI Planning Expectations

Planner and terminal surfaces are mobile-web critical. Any plan that changes planner, development, terminal, file browser, Mermaid viewer, modals, chat input, or attachment behavior must include visual artifacts or explicit references to the current screen/component.

For UI work, T1 should identify:

- The route/page and shared widget paths affected.
- Whether the change belongs in `ui/src/components/terminal-screen/` instead of a page-specific terminal copy.
- The Ionic components expected for modals, headers, footers, inputs, toolbars, lists, buttons, and safe-area behavior.
- Mobile-web states, especially bottom controls near browser/tablet system bars.
- Any screenshots, HTML mocks, or existing screens that define the desired layout.

T2 should return `NEEDS_CHANGES` when a UI plan omits mocks or visual references for a meaningful layout/workflow change, or when the plan describes a UI that does not match these source paths.

For review output quality, T2 must give the orchestrator enough structure to produce meaningful event logs. When verdict is `NEEDS_CHANGES` or `BLOCKED`, reviewer output must include:

- A concrete reason in `FINDINGS`, not only a status label.
- At least one actionable `NEXT_STEPS` item for T1.
- Enough specificity that the backend can explain why the run or phase was sent back.

Avoid vague findings like "needs more work" or "not ready yet". Prefer findings that name the missing mock, stale source assumption, failing evidence, missing test, or mismatched component/screen.

The planner event log is expected to communicate decision facts, not only transport facts. "Sent back to T1" is incomplete unless the associated review reason explains what T1 must fix.

## Validation And Deploy

Common commands:

- Web build: `npx nx build web`
- UI library build: `npx nx build ui`
- Prod deploy: `npm run deploy:worker` / `lokal cf deploy --env prod` (see `docs/operations.md` § Deploy gotchas)
- Desktop backend build: `npm run build:desktop` (wipes `dist/host-app` then rebuilds) or `cd desktop/src-tauri && npx tauri build --no-bundle`. **Do not use plain `cargo build --release`** without `--features tauri/custom-protocol` — blank window (`Could not connect to localhost`). See `docs/operations.md`. Dev with HMR: `cargo tauri dev` (`host-app` on :4201).

For deployed web changes, verify the stable dev site is serving the new bundle with:

```bash
curl -sL https://johnnyone.pages.dev/ | grep -o "main-[A-Z0-9]*\\.js" | head -1
```

Planning and development validation should not claim live behavior is current unless the relevant local backend, deployed Worker, and Pages bundle were actually checked.
