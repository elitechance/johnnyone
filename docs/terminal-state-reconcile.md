# Terminal stale-session reconcile

The web Terminal page (`web/src/app/pages/terminal/terminal.page.ts`) persists
pane layouts and a screen cache in `localStorage` so a workspace survives a
reload. Before this fix, that persisted state was never checked for liveness:
if a cached/persisted session id no longer existed in the active session list
(e.g. the session was deleted or archived elsewhere), the UI stayed pinned to
the dead session and the user's real active session could be hidden.

**Symptom:** the cache pinned a deleted session while a live session never
appeared in the Terminal.

## Fix

On Terminal load, after the authoritative active list is fetched
(`api.listSessions('active')` in `loadSessions`), **all** persisted/cached
Terminal state is reconciled against that active set and any entry whose
session id is not active is dropped — and the cleaned state is written back so
the corruption cannot reload. This runs **before** layout restore and session
selection, so a dead session can never pin a pane or be selected.

### The three reconciled stores

| Store | `localStorage` key | Owner |
|---|---|---|
| Pane layouts (`desktopLayouts` + `mobileLayouts`) | `johnnyone_terminal_pane_workspace` | `terminal.page.ts` |
| `closedPaneIds` | `johnnyone_terminal_pane_workspace` | `terminal.page.ts` |
| Screen cache | `johnnyone_terminal_screen_cache` | `TerminalScreenCacheService` |

All three are now pruned by liveness on load (previously: layouts pruned only
in-memory and never rewritten to storage; `closedPaneIds` restored verbatim;
screen cache pruned only by age/count, never by liveness).

## Implementation

### `reconcilePersistedTerminalState(activeSessions)` — `terminal.page.ts`

Single entry point, wired into `loadSessions` right after `sessions.set(...)`.
It:

1. Prunes the in-memory signals (`terminalScreens`, `paneLayouts`,
   `closedPaneIds`) by reusing the existing `removeInactivePaneState(activeIds)`
   (previously dead code — now wired in here).
2. Rewrites the persisted `johnnyone_terminal_pane_workspace` blob with the
   cleaned state, or removes the key entirely when nothing live remains.
3. Prunes the screen cache via `relayTerminal.retainCachedScreens(activeIds)`,
   keeping the page → relay → cache layering (the page never touches the cache
   service directly).

### Pure helpers — `terminal-state-reconcile.ts`

Angular-free so they unit-test in plain jsdom (the project's Vitest config
omits `@analogjs/vitest-angular`):

- `pruneRecordByActiveIds(record, activeIds)` — returns a copy keeping only keys
  in the active set, plus a `changed` flag. Never mutates the input.
- `reconcilePersistedWorkspaceState(raw, activeIds)` — filters `desktopLayouts`,
  `mobileLayouts`, and `closedPaneIds` against the active set. Returns
  `{ next, changed }`; `next` is `null` when nothing live remains (so the caller
  `removeItem`s rather than persisting a hollow `{}`).
- `chooseSessionToSelect(activeSessions, { targetId, currentId })` — pure
  extraction of the selection decision in `loadSessions`. An active `targetId`
  wins; otherwise a `currentId` is kept only if still active (else `null`);
  otherwise the first active session, or `null` when the list is empty. This
  proves the second half of the bug — after the dead pin is pruned, the live
  session becomes the selected one and a dead `currentId` is cleared rather than
  kept pinned.

### `TerminalScreenCacheService.retainOnly(activeSessionIds)`

Liveness prune for the screen cache: drops every cached session not in the
active set, **cancels any pending debounced write** for the dropped ids (so an
in-flight `remember()` cannot resurrect a dead session after the rewrite), and
rewrites `localStorage` only when at least one entry was removed (idempotent
no-op otherwise). Exposed to the page through
`RelayTerminalService.retainCachedScreens(...)`. The debounce-cancel logic was
also factored into a shared `cancelPendingSave(sessionId)` used by `remove`.

## Scope

Web-only logic-layer fix. No change to the terminal pane layout, sidebar,
segment, the shared `johnny-terminal-screen` widget, the cache eviction policy
(age/count), or any worker/GraphQL/desktop-backend code. The active-session
source of truth (`listSessions('active')`) already existed and is unchanged.

## Tests

Vitest + jsdom (`web/**/*.spec.ts`):

- `terminal-state-reconcile.spec.ts` — pure helpers, including the
  deleted-session prune repro and `chooseSessionToSelect`.
- `terminal-screen-cache.service.spec.ts` — `retainOnly` liveness prune and
  pending-write cancellation.
- `terminal-reconcile-integration.spec.ts` — cross-store reconcile repro: a
  dead id pinned across all three stores is pruned on load and the live session
  becomes selectable.

Build gate: `npx nx build web`.
