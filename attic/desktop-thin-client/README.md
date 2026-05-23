# desktop-thin-client (archived)

**Archived:** 2026-05-23
**Reason:** Replaced by the new `web/` Nx project (Phase 1 of the multi-user-saas plan). The desktop Ionic/Angular thin client UI is no longer first-class — the canonical client surface is the hosted web app at `johnnyone-dev-web.pages.dev`.
**Plan:** [`personal/docs/johnnyone/plans/multi-user-saas/`](../../../../docs/johnnyone/plans/multi-user-saas/)

## What's here

The full Angular project that used to live at `personal/apps/johnnyone/desktop/`:

- `src/` — pages (chat, planner, terminal, settings, login) + services + main.ts
- `e2e/` — Playwright specs that exercised the desktop frontend
- `package.json`, `tsconfig.json`, `tsconfig.app.json` — Angular project config

## What's NOT here (still active)

- `desktop/src-tauri/` (NOT in this archive) — the Tauri shell + `johnnyone-host` Rust backend. Continues to live at the original path. The Tauri shell is reworked in Phase 3 of the plan to load a separate small control-panel project instead of this archived UI.

## Where the value went

- Reusable components → already in `ui/src/components/` (terminal-screen, message-bubble, chat-window, etc.). The new `web/` (and future mobile / desktop installers) composes from `ui/`.
- Reusable services → `ui/src/services/johnny-api.service.ts` (the canonical GraphQL client) and the auth service/guard which Phase 1 ports into `web/`.
- E2E specs → some patterns may inform new `web/e2e/` specs, but new specs are written fresh against the new UI.

## Reviving

If a future plan reintroduces a standalone desktop or mobile thin client, the right approach is **not** to lift this directory wholesale. Instead, generate a fresh Nx project, compose from `ui/`, and reference these files for historical context only (especially route shapes, env detection logic in `src/main.ts`, and the auth service that's about to be ported to `web/`).
