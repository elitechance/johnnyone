# Console fix-forward — per-initiative tabs, the Plan tab, per-initiative validation, plain shells, resizable dividers

> A fix-forward pass over the shipped 2026-07 overhaul, fixing four findings from
> real use of the initiative console. **Fix-forward in the new UI/backend —
> nothing is reverted.** It is **web + `ui` Angular only**: no Rust/worker/relay
> edit, no new GraphQL op, no new route, no new npm dependency, no migration
> (finding #2's validation runtime was verified to already be per-initiative — see
> §3). Every surface reuses shipped, tested components. `nx build web`/`ui` green;
> 225 web vitest specs pass.
>
> Each fix follows the shipped codebase's **pure-seam** pattern: the decision logic
> lives in an Angular/Ionic/DOM-free `*-logic.ts` module unit-tested under the
> plugin-less web vitest (`web/vitest.config.ts`), with template/DOM behavior
> proven by a clean `nx build web` plus the orchestrator's deferred Playwright pass
> (no browser in the runner).

## 1. Console tabs are the primary per-initiative surface

Before this pass the four tabs (**Transcript · Raw terminal · Plan · Diff**) lived
*inside* the per-session pane block, keyed by session id. An initiative with no
live terminal fell through to the `workspace-empty` state with **no tabs at all**.
The fix drives the tab shell from the **selected initiative** and resolves *its*
session, reusing every existing pane component rather than forking them — so the
tabs render whether or not a terminal is attached.

`console-tabs-logic.ts` (pure) owns the decisions; it reuses the `PaneTab` tab-id
union + `paneTabOf` from `terminal-transcript-tab.ts` rather than defining a second
tab type:

| Export | Purpose |
|---|---|
| `resolvePrimarySessionId(init)` | The session whose Transcript/Raw/Diff the initiative's tabs display. Precedence **worker → reviewer → briefing**; `null` for a null initiative or one with no session yet (see `artifacts/tab-dataflow.md`). |
| `initiativeTabOf(tabs, initiativeId)` | Active tab for an initiative (defaults to Transcript). A **distinct** map from the page's session-keyed `paneTabs`, but reuses `paneTabOf` so the default/lookup rule stays in one place. |
| `rawAttachNeeded(primarySessionId, hasScreen)` | Whether the Raw tab shows the inline attach / **New Terminal** affordance instead of a live screen: true when there is no primary session yet, or one exists but no attached `terminalScreens()[id]`. Attaching from inside the tab brings the live pane up in the same tab. |

## 2. The Plan tab (built)

The Plan pane tab was a reserved literal only (`docs/validation-diff.md` §2 noted
it as "reserved for its own phase"). It is now a real document view — reusing the
P3 markdown/mermaid core (`johnny-markdown-view`), `getAgentPlan(id)` for
structured phases/tasks, and `readFile(path)` for the markdown bodies. **No new
GraphQL, no new renderer.**

`plan-tab-logic.ts` (pure) is a projection over the existing `getAgentPlan` result:

| Export | Purpose |
|---|---|
| `docNavModel(run)` | Doc-navigator entries: fixed `overview.md`, `status.md`, then one expandable entry per phase (host order), each carrying its task rows. |
| `phaseCards(run)` | Per-phase cards with `done/total` and task rows, in host order. Null-safe → `[]`. |
| `planCounts(run)` | Header counts → `{ phases, tasks, done }`, rendered "N phases · M tasks · K done". |
| `taskStatusLabel(status)` | Task-row status chip `{ label, token }`. |
| `planDocPath(planPath, sel)` | Builds a file path from the **server-provided** `planPath` + a **fixed** suffix (`overview.md`, `status.md`, or `phases/<id>/overview.md`) — never a user-typed path. Returns `null` for an empty `planPath`; a phase id containing `/`, `\`, or `..` is rejected (defense-in-depth traversal guard even though the id is server data). |

Status classification is normalized the way the host does it (`normStatus`: trim,
lowercase, `_`→`-`) and treats `passed`/`complete`/`completed` as `done`, so a raw
or un-normalized value (deploy skew, direct markdown) still classifies correctly.

## 3. Validation config is strictly per-initiative (verify + clarity badge)

This was a **verify-and-clarify** finding, not an architectural one. Tracing proved
the config is already per-initiative in storage (`agent_plans.validation_config`),
GraphQL, the console read, and the desktop review runtime (`resolve_validation_lenses`
reads the per-row config) — so **no Rust/runtime change was needed**. The "reads as
global" symptom was purely the shared **default triad** fallback that every
*unconfigured* initiative resolves to.

The clarity fix adds the missing "is this the default template or this initiative's
own config?" signal:

- `lensSourceOf(json)` in `validation-config-logic.ts` → `'default' | 'custom'`.
  `'custom'` exactly when `fromConfigJson` would **not** fall back to
  `defaultLenses()` — i.e. a non-empty string that JSON-parses to a non-empty
  array. This is the single owner of that parse boundary.
- `console-logic.ts` gains `lensSource(validationConfig)` which **delegates** to
  `lensSourceOf`, so the console right-column panel and the Configure page
  (`/initiatives/:id/validation`) share one rule and can't drift. Each surface
  shows a **"default template"** badge (distinct from **"configured"**) so an
  unconfigured initiative is visibly using the shared default rather than looking
  like a global setting.

## 4. Shells: attach fix + plain terminal surface

**Attach fix (finding #3a).** `attachTmuxInput(name)` in `launcher-logic.ts`
previously returned `{ tmuxSessionName: name }`, omitting `title`. The Shells list's
`attachableTmux` de-dupe joins an attached session to a tmux pane by
`title === tmux.name`, so an attached pane never left the "attachable" list. The fix
carries the title through — `{ tmuxSessionName: name, title: name }` — mirroring the
working terminal-page attach.

**Plain terminal surface (findings #3b / #4).** A raw shell or attached tmux now
opens a **plain terminal surface** — terminal I/O + status bar only, with no rail,
lifecycle bar, validation column, files, or dividers:

- `plainTerminalRoute(sessionId)` (`launcher-logic.ts`) — the same
  `/terminal?sessionId=` deep-link plus an explicit `surface=shell` marker the
  terminal page reads **synchronously** so the plain surface paints from first
  frame (no console-chrome flash). `terminalRoute` is left unchanged for non-shell
  callers.
- `isPlainShellSurface(surfaceParam, session)` — true when `surface==='shell'`
  (drives first paint) **or** the resolved session is a shell / attached-tmux pane
  (`provider === 'shell' || attachedTmux === true`, the backstop once the session
  loads). Pure/total, null-safe.
- `shells-page-logic.ts` `openIntent` now re-exports `plainTerminalRoute` (was
  `terminalRoute`), so every Shells-list open uses the plain surface.

Opening a shell still **navigates** to the existing terminal surface — no second
terminal implementation.

## 5. Resizable console dividers

Two drag handles — **rail ↔ center** and **center ↔ validation** — with widths
persisted to `localStorage` (same convention as the shipped
`loadSidebarWidth`/`saveSidebarWidth`). The page owns the pointer handlers +
signals and delegates the math to `console-layout-logic.ts` (pure), which follows
the planner's signal-driven `grid-template-columns` idiom:

| Export | Purpose |
|---|---|
| `clampRailWidth(px)` | Clamp to `[180, 420]`; non-finite → default `216` (the shipped `.console` first track). |
| `clampRightWidth(px)` | Clamp to `[220, 520]`; non-finite → default `262` (the shipped third track). |
| `consoleColumns(rail, right, dividerPx=6)` | The `grid-template-columns` for the 3-pane console **including** the two 6px divider tracks: `rail | divider | center | divider | right`, center = `minmax(0, 1fr)` so it flexes. |
| `parseStoredWidth(raw, clamp)` | Parse a `localStorage` width string and clamp it; empty/null/junk → the clamp fn's default (via `NaN`). |

Below the ~760px container width the console still collapses to a single column
with no stray handles.

## Where the code lives

New pure modules (each with a `.spec.ts` sibling under `web/`):

```
web/src/app/pages/terminal/
  console-tabs-logic.ts     per-initiative primary session + tab state + Raw-attach affordance (§1)
  plan-tab-logic.ts         doc-nav model, phase cards, counts, safe plan-doc path (§2)
  console-layout-logic.ts   divider clamps, grid-template, stored-width parse (§5)
```

Extended in place:

```
web/src/app/pages/terminal/
  console-logic.ts                       + lensSource (§3, delegates to lensSourceOf)
  terminal.page.{ts,html,scss}           tab shell lifted to the selected initiative, Plan tab render,
                                         plain-shell surface, divider handles + persisted widths
web/src/app/pages/validation-config/
  validation-config-logic.ts             + lensSourceOf → 'default' | 'custom' (§3)
  validation-config.page.{ts,html,scss}  default-template vs configured badge
web/src/app/components/launcher-menu/
  launcher-logic.ts                      + plainTerminalRoute / isPlainShellSurface; attachTmuxInput
                                         now carries title (§4)
  launcher-menu.component.ts             plain-surface open wiring
web/src/app/pages/shells/
  shells-page-logic.ts                   openIntent → plainTerminalRoute (§4)
```

Reference material (already in-repo): mock §02/§03/§04
(`.overhaul-refs/overhaul-2026-07-mocks.html`); the net-new visual states drawn in
`artifacts/console-states.html` (tabs with no terminal, the Raw-tab inline attach,
the plain-shell surface, the two divider handles) and `artifacts/tab-dataflow.md`
(selected initiative → primary session → tab content).

## Scope notes

- **Web + `ui` only.** No Rust/worker/relay edit, no new GraphQL op, no new route,
  no new npm dependency, no migration. §3 confirmed the validation runtime is
  already per-initiative, so nothing gated on a desktop rebuild.
- **Reuse, not forks.** Markdown/mermaid render (`johnny-markdown-view`), transcript
  (`johnny-transcript-view`), diff (`johnny-diff-view`), terminal I/O
  (`johnny-terminal-screen`), structured phases/tasks (`getAgentPlan`), markdown
  bytes (`readFile`), the P7 lens parser (`fromConfigJson`/`defaultLenses`), and the
  planner grid-resize/width-persistence idioms are all wrapped, never re-authored.
- **Visual verification deferred** — the runner has no browser. Acceptance is the
  mock/`artifacts` contract + `nx build web`/`ui` + the pure specs; the orchestrator
  captures the Playwright screenshots afterward (screens S1–S7 in the plan
  `overview.md`).
</content>
</invoke>
