# Web-app shell — nav rail, initiative console, lifecycle bar, amber theme (overhaul P8)

Phase 8 of the 2026-07 self-overhaul arc turns "old shell + new pages" into the
approved mock (`.overhaul-refs/overhaul-2026-07-mocks.html`). It is **web + `ui`
Angular only** — the desktop Rust/Tauri engine and the worker GraphQL relay are
**untouched**, no new GraphQL op, no new route, no new npm dependency, no
migration. Every surface it arranges already shipped in P1–P7; P8 is **layout,
theme, and assembly**, not new features.

What it delivers:

1. A global **icon nav rail** (J1 logo · `+ New` · Work / Files / Shells /
   Settings) replacing the old `ion-menu` text sidebar, plus a **mobile
   bottom-nav** rendered from the same list.
2. A unified **initiative console** at the existing `terminal` route ("Work"):
   a master-detail grid wrapping the existing pane-tab shell (Transcript / Raw
   terminal / Plan / Diff) with an initiative master-list on the left and a
   validation-summary + host-files column on the right.
3. A **lifecycle bar** + **status pills** rendering the P1 `initiativeStatus` +
   `health` in the mock's per-stage semantic colors.
4. The mock's **amber accent** applied app-wide via the token layer, with both a
   light and a dark palette authored to the mock's exact hexes (dark stays the
   active theme).
5. **Responsive** phone (single column + segment switcher + bottom nav) ↔
   widescreen (multi-pane console) behaviour, driven by container queries.

> **Reuse contract.** The transcript view (P3), diff view (P7), markdown/files
> view (P5), launcher menu (P6), validation-config logic (P7), pane-tab shell
> (P3), and the P1 lifecycle fields on `AgentPlan` are all *wrapped*, never
> re-authored. The only net-new code is one token retarget, one pure status map
> + two presentational primitives, the nav-rail shell + bottom-nav, and the
> console layout + one pure projection module (plus their specs).

## 1. Amber theme tokens (phase 01)

The house token layer is `--jo-*` (not the mock's bare `--accent` / `--st-*`).
Rather than rename ~10 component stylesheets, P8 **retargets the existing
`--jo-accent`** from the old blue `#5096ff` to the mock's amber, so every current
`var(--jo-accent)` consumer flips app-wide with no consumer edit (D2). It also
adds an on-accent ink token and a semantic per-stage status palette:

```
--jo-accent:      #e8a33d   (was #5096ff)
--jo-accent-hover:#f2b556
--jo-accent-ink:  #1a130a   (on-accent text, e.g. .primary-action)

--jo-st-briefing:    #2fbcd8
--jo-st-planning:    #5d8bf7
--jo-st-development: #9a7ef0
--jo-st-review:      #e070a8
--jo-st-done:        #4bbd74
```

The semantic status palette is **distinct** from the existing health tokens
(`--jo-warn` / `--jo-bad` / `--jo-good`), which are kept.

**Dark stays the only rendered theme** (D1). The app imports
`@ionic/angular/.../dark.always.css` and every component is dark-tuned; removing
dark-only would need a full ad-hoc-color re-audit. So P8 authors a **dormant
light palette** under `:root[data-theme="light"]` — the mock's light hexes are
ready for a future light↔dark toggle, but nothing activates it today.

The plugin-less web vitest cannot resolve compiled CSS custom properties, so the
SCSS `:root` blocks and `web/src/app/pages/terminal/theme-tokens.ts` are kept as
**two views of one source of truth**; `theme-tokens.spec.ts` pins every hex so
the two can't drift.

## 2. Lifecycle status map + primitives (phase 02)

`ui/src/lib/lifecycle-status.ts` is the pure, framework-free map that every
lifecycle surface consumes, so the bar, the pills, and the console master-list
stay visually identical and unit-testable without a browser (mirroring the
`diff-parse.ts` pure-module pattern). It exports **token names only** — the hex
values live in the P01 token layer.

- `LIFECYCLE_STAGES` — the five stages in order: `briefing`, `planning`,
  `development`, `review`, `done`.
- `statusMeta(status)` / `healthMeta(health)` → `{ cssVar, label, className }`;
  input is trimmed + lowercased and tolerates `null`/`undefined`. An unknown
  value returns a benign fallback (`--jo-fg-muted` / `st-unknown`) — **never
  throws**.
- `stageIndex(status)` — index within `LIFECYCLE_STAGES` (`-1` if unknown),
  driving the bar's complete / active / inert fill.

Two thin presentational primitives (both `OnPush`, both colouring themselves from
the map via `--jo-*` tokens, neither fetching data):

- **`johnny-status-pill`** (`ui/src/components/status-pill/`) — `[kind]` selects
  the status vs. health map, `[value]` is the raw `initiativeStatus` / `health`
  string.
- **`johnny-lifecycle-bar`** (`ui/src/components/lifecycle-bar/`) — renders the
  five stages with the active one `.on` in its semantic colour (earlier complete,
  later inert) plus a health pill on the axis below. Per-stage copy mirrors mock
  §01.

`lifecycle-status.spec.ts` (under `web/`) pins the status/health/index mapping,
including the unknown-value fallback.

## 3. Nav rail + responsive nav (phase 03)

`web/src/app/nav-items.ts` is the **single source** for the four global
destinations, so the widescreen icon rail (`.gnav`) and the mobile bottom nav
(`.botnav`) render from one list and can't drift (D4/D12):

| id | label | icon | path (existing route) |
|---|---|---|---|
| `work` | Work | `apps-outline` | `/terminal` |
| `files` | Files | `folder-outline` | `/files` |
| `shells` | Shells | `code-working-outline` | `/shells` |
| `settings` | Settings | `settings-outline` | `/settings` |

`NAV_ITEMS` is pure (no Angular import) so `nav-items.spec.ts` can pin it. Every
`path` targets an **existing** route — no new routes are added; "Work" simply
lands on the `terminal` route, which is where the console lives (D6). Active state
is derived by `routerLinkActive`, highlighted in amber. The `+` button **reuses
the P6 `openLauncher` handler verbatim** — the same `LauncherMenuComponent`
popover with its four entries. The old `ion-menu` text sidebar is removed from
`app.component`.

## 4. Initiative console (phase 04)

"Work" is the console at the `terminal` route: a master-detail CSS grid wrapping
the pane-tab shell the terminal page already owns. `console-logic.ts` is the pure
decision layer behind it — Angular/Ionic/DOM-free, every transform a **projection
over existing client data** (`listAgentPlans`, the already-loaded `gitDiff`, the
Initiative's `validationConfig`); it re-implements no colour or lens logic,
delegating to P2 `lifecycle-status` and P7 `validation-config-logic` (D5/D7/D8):

- `initiativeRows(plans, selectedId, nowIso)` — the left master-list. `nowIso` is
  **injected** (no `Date.now()`) so the spec is deterministic; each row carries
  status/health meta, a `showHealth` flag (a health pill shows only when health
  is noteworthy, i.e. not baseline `in-progress`), and a relative "… ago" string.
- `lensSummary(validationConfig)` — the right-column read-only validation summary,
  **reusing P7's `fromConfigJson`**, which already returns the default triad
  (`product`/`qa`/`lead`) for null/empty/invalid config. No second parser, no
  default re-implementation; a **Configure** link deep-links to
  `/initiatives/:id/validation`.
- `touchedFiles(view)` — the right-column host-files panel, mapping the
  **already-loaded** `gitDiff` view (P7's `diffs` signal — no new data call) to
  compact `+/-` rows; `badge:'new'` when a file only added lines, else `'edited'`.
  A null / clean / empty view → `[]` (the benign empty state P7 already produces).
- `consolePaneFor(segment)` — the §08 mobile segment switcher
  (`transcript` / `files` / `validation`); an unknown/untrusted id falls back to
  `transcript` (the mobile default).

On phone, the grid collapses to one column showing the Transcript surface plus the
segment switcher and the bottom nav; on widescreen it expands to the four-region
console — all via container queries (D12). `console-logic.spec.ts` pins the row,
summary, file, and segment mappings; the render wiring is build-checked and its
visual confirmation deferred (see below).

> **Fix-forward (post-P8).** A later console fix-forward makes the tab shell
> (Transcript · Raw terminal · Plan · Diff) the **primary per-initiative surface**
> — rendered for the selected initiative whether or not a terminal is attached
> (P8 wrapped the session-keyed pane shell, so a no-terminal initiative showed no
> tabs) — **builds the Plan tab**, adds a **default-template vs configured** badge
> to the validation column, and adds **resizable rail↔center↔validation dividers**
> with persisted widths. See [`console-fixes.md`](console-fixes.md).

## Where the code lives

New:

- `web/src/app/pages/terminal/theme-tokens.ts` (+ `.spec.ts`) — pure mirror of the
  SCSS amber palette.
- `web/src/styles.scss` — retargeted `--jo-accent`, new `--jo-accent-ink` +
  `--jo-st-*`, dormant `:root[data-theme="light"]` palette.
- `ui/src/lib/lifecycle-status.ts` — pure status/health → token/label/class map
  (spec: `web/src/app/pages/terminal/lifecycle-status.spec.ts`).
- `ui/src/components/status-pill/` + `ui/src/components/lifecycle-bar/` — the two
  presentational primitives (exported from `ui/src/index.ts`).
- `web/src/app/nav-items.ts` (+ `.spec.ts`) — the shared nav destination list.
- `web/src/app/pages/terminal/console-logic.ts` (+ `.spec.ts`) — the pure console
  projection layer.

Assembled into (existing files, wrapped not rewritten):

- `web/src/app/app.component.{html,scss,ts}` — nav rail + bottom nav replace the
  `ion-menu` sidebar.
- `web/src/app/pages/terminal/terminal.page.{html,scss,ts}` — the console layout
  around the existing pane-tab shell.
- Per-page cleanups (`briefing` / `files` / `planner` / `shells` / `settings`)
  where the old sidebar markup/handlers were removed.

## Scope notes

- **No Rust / worker / relay edit, no new GraphQL op, no new route, no new npm
  dependency, no migration** — P8 is pure web+ui assembly.
- **Dark remains the active theme**; the light palette is authored but dormant
  (D1). Activating a light↔dark toggle is a separate initiative.
- **Visual verification is deferred** — the runner has no browser (D11), so
  acceptance is the mock contract + `nx build web`/`ui` + the pure specs
  (`theme-tokens` / `lifecycle-status` / `nav-items` / `console-logic`). The
  orchestrator captures Playwright screenshots afterward.
- **The running desktop is not rebuilt or relaunched** (D10) — there are no Rust
  changes, so `cargo build`/`test` are green by construction.
