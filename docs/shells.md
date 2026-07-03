# Raw shell launcher + Shells destination (overhaul P6)

> Two operator-facing things: a **`+ New` launcher** in the app nav, and a
> **Shells destination** at **`/shells`** that lists the raw shells you've
> launched (plus attachable external tmux panes) in one place.
>
> Landed on branch `overhaul/2026-07` (commit `overhaul P6: raw shell launcher +
> Shells destination`). **This phase is web-only.** It ships **no** new agent
> runner, session-spawn path, transport, Rust/worker/relay surface, or npm
> dependency. Both the launcher and the list call the **existing** P2
> `createSession`/`listSessions`/`listTmuxSessions` methods and navigate to the
> **existing** P3 terminal surface. Everything below is web
> (`web/src/app/components/launcher-menu/` + `web/src/app/pages/shells/`) + reuse
> of `@johnnyone/ui`.

## What it delivers

Every backend and session primitive already existed and was already wired into
the terminal page — a raw shell is the existing `CliProvider::Shell` ($SHELL in a
tmux pane), attach is the existing external-tmux path, and the terminal page
already renders any session and accepts `?sessionId=` deep-links. P6 adds only the
**launcher + list UI + routing** on top:

1. **`+ New` launcher.** A shared `LauncherMenuComponent` presented as an
   `ion-popover`, triggered from a **New** entry (`add-outline`) in the app menu
   (and from the Shells page header). Four rows:

   | Row | Icon | Action |
   |---|---|---|
   | **New initiative** — start at briefing | `create-outline` | navigate `/briefing/new` (P4) |
   | **Raw shell** — $SHELL · run commands yourself | `terminal-outline` | `createSession({provider:'shell'})` → open `/terminal?sessionId=` |
   | **Attach to tmux session** — pick an existing pane | `git-network-outline` | `listTmuxSessions()` radio picker → `createSession({tmuxSessionName})` → open |
   | **Open file manager** — browse the host FS | `folder-outline` | navigate `/files` (P5) |

2. **Raw shell launch.** "Raw shell" spawns a `CliProvider::Shell` session via the
   existing `createSession` path and opens it at `/terminal?sessionId=<id>` so you
   can type commands — reusing the terminal page's rendering verbatim. It spawns
   with the persisted `last_working_directory` (via `getSetting`, falling back to
   the host default when unset); there is **no** working-directory picker on the
   launcher (that stays on the terminal page's New-session flow).

3. **Shells destination + `/shells` route.** A `ShellsPage` with two groups:
   **active shells** (`listSessions('active')` filtered to `provider === 'shell'`
   or `attachedTmux === true`, newest-first) and **attachable tmux**
   (`listTmuxSessions()` minus any pane already attached). A shell row navigates to
   the terminal surface; an attachable row attaches (`createSession`) then opens.
   `/shells` is an `authGuard`, lazily-loaded route with a **Shells** entry
   (`code-working-outline`) in the app menu. The list refreshes on
   `ionViewWillEnter`, so it's current when you return from the terminal.

## Where the code lives

```
web/src/app/components/launcher-menu/
  launcher-menu.component.ts     LauncherMenuComponent (selector app-launcher-menu, standalone) — the §06 popover
  launcher-menu.component.html / .scss
  launcher-logic.ts              pure, Angular/Ionic-free launcher decisions (+ .spec.ts)

web/src/app/pages/shells/
  shells.page.ts                 ShellsPage (selector app-shells, standalone) — the list destination
  shells.page.html / .scss
  shells-page-logic.ts           pure, Angular/Ionic-free filter/partition/dedupe/label decisions (+ .spec.ts)
```

Both surfaces are **thin wiring**: every decision (which entries, what each does,
the create inputs, the terminal nav shape, the list filter/dedupe/labels) lives in
the two **pure** modules so they are unit-tested under the plugin-less web vitest
(`web/vitest.config.ts`) without pulling in Ionic or the API service. Reused
verbatim from `@johnnyone/ui`: `JohnnyApiService` (`createSession` /
`listSessions` / `listTmuxSessions` / `getSetting`) and — as a navigation target
only — the P3 terminal page (`?sessionId=`). No `@johnnyone/ui` code was modified,
and the terminal renderer is **not** forked; opening a shell navigates, it does not
embed a second terminal.

Wiring touch points: `app.routes.ts` gains the lazy `/shells` route;
`app.component.html`/`.ts` gain the **New** launcher trigger + the **Shells** nav
entry and their icon registrations.

## The pure logic seams

### `launcher-logic.ts` — launcher decisions

| Export | Purpose |
|---|---|
| `LAUNCHER_ENTRIES` | the four §06 rows (`kind`/`title`/`subtitle`/`icon`), in mock order — asserted by the spec |
| `launcherIntent(kind)` | row → descriptor: `initiative`/`files` → `{type:'navigate',path}`; `shell` → `{type:'createShell'}`; `attach` → `{type:'attachPicker'}` |
| `terminalRoute(sessionId)` | the shared open-in-terminal nav shape `{path:'/terminal', queryParams:{sessionId}}` (also reused by the Shells page) |
| `shellCreateInput(cwd?)` | create-input for a raw shell — `{provider:'shell'}`, adding `workingDirectory` **only** when a cwd is given (absent key ⇒ host default) |
| `attachTmuxInput(name)` | create-input for attaching an external pane — `{tmuxSessionName:name, title:name}` (attach instead of spawn). The `title` was **added by the console fix-forward** so the attached pane matches the `attachableTmux` de-dupe join and leaves the "attachable" list — see [`console-fixes.md`](console-fixes.md) §4 |

### `shells-page-logic.ts` — list decisions

| Export | Purpose |
|---|---|
| `isShellSession(s)` | `provider === 'shell' \|\| attachedTmux === true` — the whole basis of the filter (agent sessions excluded) |
| `partitionShells(sessions)` | the shell/attached subset, sorted newest-first by `updatedAt ?? createdAt` |
| `attachableTmux(tmux, sessions)` | external panes minus any already attached (join by name = an attached session's `title`); best-effort UX dedupe — the host prevents a real double-attach |
| `shellSessionLabel(s)` | row label `{title, tag:'shell'\|'tmux'}` (`title \|\| 'Shell'`) — a trivial label, not a fork of the terminal page's `providerLabel` |
| `formatRelTime(iso, nowIso)` | coarse "just now / Nm / Nh / Nd ago" — `nowIso` is a **parameter** (no `Date.now()` inside) so the spec is deterministic |
| `openIntent` | re-exports `plainTerminalRoute` — the `/terminal?sessionId=&surface=shell` shape, defined once. The console fix-forward switched this from `terminalRoute` so a shell opens as a **plain terminal surface** (no initiative chrome) — see [`console-fixes.md`](console-fixes.md) §4 |

## Security posture

- **No new privilege or attack surface.** A raw shell runs the host's `$SHELL`
  with the operator's own privileges — identical to the terminal page's existing
  New-session flow. The launcher merely triggers the already-reviewed P2 create
  path.
- **Attach only surfaces what the host allows.** The picker lists exactly what
  `listTmuxSessions()` returns, which already excludes internal `johnnyone_<id>`
  panes.
- **Never log session ids as secrets.** Create/list failures surface via an Ionic
  alert; they are not logged as sensitive data.

## Scope notes

- **Web-only, reuse-only.** No new agent runner, re-implemented session
  spawn/transport, forked terminal renderer, Rust/worker/relay surface, or npm
  dependency. The only new code is the `components/launcher-menu/` component + its
  pure module, the `pages/shells/` page + its pure module, their specs, a route,
  and nav entries. The running desktop app is **not** rebuilt — because P6 touches
  no Rust, the live host already serves the shell spawn/attach ops, so the feature
  is exercisable against it without a rebuild.
- **Open/attach only.** Detaching/closing/renaming a shell from the list is out of
  scope; those lifecycle controls already live on the terminal surface (per-session
  tab).
- **Visual verification is deferred** (no browser in the runner). Acceptance is
  `nx build web`/`ui` + the pure specs (`launcher-logic.spec.ts`,
  `shells-page-logic.spec.ts`), with mock §06 (launcher) and
  `artifacts/shells-destination-mock.html` (list) as the layout contracts.
- **Deferred to Phase 8:** the full Work/Files/Shells/Settings nav rail and its
  widescreen three-pane shell. P6 ships the launcher popover + a single `/shells`
  nav entry, mirroring P5's reachability approach.
