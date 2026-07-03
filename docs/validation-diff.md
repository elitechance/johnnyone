# Configurable validation + Diff tab (overhaul P7)

> Two independent, dogfoodable things landed on branch `overhaul/2026-07`:
>
> 1. **Configurable validation** — the phase-review fan-out (development **and**
>    planning) is now driven by an **ordered, N-lens array persisted on the
>    Initiative** instead of a hardcoded 3-lens set. Each lens carries its own
>    provider/model, a `vision` flag, and a **BLOCK/WARN** gate role. Minimum 1
>    lens, no maximum. The default (an unconfigured Initiative) reproduces
>    **exactly** today's `product` / `qa` / `lead` triad, so nothing regresses.
> 2. **Diff tab** — a fourth tab in the session center pane (beside Transcript /
>    Raw terminal) that renders the **working-tree git diff** of the session's
>    directory: a changed-file list with +/- counts and per-file hunks,
>    syntax-highlighted by reusing the P3 render core (`highlightCode`).
>
> This phase **extends** two mature subsystems (the review orchestration and the
> git-ops-over-relay transport) — it does not create new ones. The net-new
> surface is one JSON column, one whole-tree `git_diff` RPC, one diff-render
> component, one config UI, and their tests. New Rust lands with **deferred
> activation**: the configurable review and the Diff tab's live data go live only
> at an operator-chosen desktop rebuild + relaunch — the running app is not
> relaunched by this phase.

## 1. Configurable validation

### The model

Validation lenses are persisted as a **nullable JSON array on the `agent_plans`
row** (migration `019_add_validation_config.sql` → `validation_config TEXT`).
`NULL`, empty, or malformed JSON resolves to the default template **in code**, so
an Initiative created before this feature behaves exactly as before.

Each lens is a `ValidationLens`
(`desktop/src-tauri/src/db/models.rs`):

| Field | Type | Meaning |
|---|---|---|
| `name` | `String` | Lens label, used in the reviewer prompt + footer roll-up |
| `provider` | `String` | A real `CliProvider` (`claude_code` / `codex` / `cline` / `ollama` / `grok`) — **not** `shell` |
| `model` | `String?` | Free-text model; omitted ⇒ the provider's configured default |
| `prompt` | `String?` | Optional per-lens reviewer prompt override |
| `vision` | `bool` | Design-authority §3: the lens may run on a vision-capable model; a vision lens that cannot read a screenshot approves on functional grounds |
| `blocking` | `bool` | `true` gates phase promotion; `false` **warns/annotates only** |

### Resolve + default

`resolve_validation_lenses(run)` (`services/agent_plans.rs`) is the single
contract the fan-out reads: it parses `validation_config`; on absent / malformed /
empty-array it falls back to `default_validation_config(reviewer_provider, model)`
— the `product` / `qa` / `lead` triad, all `blocking:true`, all `vision:false`, on
the plan's reviewer provider/model. This reproduces the old hardcoded
`REVIEW_LENSES` exactly.

### The fan-out (extended, not rewritten)

`run_lens_fanout_review` (development) and `run_planning_lens_fanout_review`
(planning) were **edited in place**:

- The hand-unrolled `tokio::join!` over exactly three reviewer sessions is now
  `futures_util::future::join_all` over a **`Vec`** of per-lens verdict futures —
  N-arity, order-preserving.
- Each lens spawns its reviewer on **its own** provider/model; the lens `name`
  (and `prompt`, when present) feed the reviewer prompt.
- `merged_verdict` and `count_consecutive_non_pass` are **untouched** (already
  N-agnostic).

### The BLOCK/WARN gate split

The new pure `gate_verdict_over_blocking(outcomes, lenses)` computes the
PASS/FAIL that gates phase promotion from **only the `blocking:true` lenses**
(if there are none, it PASSes). `blocking:false` (warn) lenses:

- **do not** affect the gate, but
- **still fold their findings into the merged reviewer body** (`merge_lens_body`),
  and appear in the footer roll-up tagged `(warn)` (`lens_summary_line`) so a
  reader sees a non-PASS warn lens did not block.

### Persisting a config

`update_plan_validation_config(state, id, config)` (mirrors
`update_plan_app_scope`) validates the JSON (parse + provider against the real
`CliProvider` set) before writing; a `null`/empty `config` clears the column
(→ resolve returns the default). It is reachable end-to-end:

`updateAgentPlanValidationConfig(id, config)` mutation
→ worker resolver (`resolvers/ai/update-agent-plan-validation-config.ts`,
gated on the `plans:write` scope)
→ `desktopRpc('update_agent_plan_validation_config', …)`
→ `rpc_update_agent_plan_validation_config` → the store fn above.

### The config UI

Route **`/initiatives/:id/validation`** (lazy, `authGuard`; reached from the
planner run settings) renders the mock §07 "Validation · configure" surface:

- an `ion-reorder-group` of lenses (drag to reorder → `moveLens`),
- per-lens **provider** `ion-select` (the real registry minus `shell`) + free-text
  **model**, a **vision** toggle, and a **BLOCK/WARN** toggle,
- **Add lens** appends; **remove** is a no-op while a single lens remains (min 1),
- **Save** persists `toConfigJson(lenses)` via `updateAgentPlanValidationConfig`.

All list decisions live in the pure, Angular/Ionic-free
`validation-config-logic.ts` (`defaultLenses`, `addLens`, `removeLens` (min-1),
`moveLens`, `providerOptions`, `toConfigJson`, `fromConfigJson`), unit-tested under
the plugin-less web vitest. `fromConfigJson` is legacy-safe: a wire lens missing
`vision` → `false`, missing `model` → `''`, missing `blocking` → `true`.

## 2. Diff tab

### The transport — one whole-tree `git_diff` RPC

Before P7 the only diff op was the per-file, `planId`-keyed
`get_workspace_file_diff`. P7 adds **one** whole-tree, **path-keyed** op
(`git_diff(path)` in `services/agent_plans.rs`):

- resolves the repo via `git rev-parse --show-toplevel` on `path` (reusing
  `git_root_for_path`),
- runs `git diff HEAD --numstat` for per-file +/- counts and `git diff HEAD` for
  hunks, and parses them into a `GitDiffView`,
- is path-guarded (`resolve_workspace_file_path`, rejecting traversal outside the
  configured root) and **read-only** — it only ever runs `git diff`,
- a non-repo / no-cwd path returns a benign empty view (no error), and
- **untracked files are excluded** (v1 diffs tracked changes vs HEAD; showing
  untracked files would require `git add -N`, which mutates the index).

`GitDiffView { repoRoot?, branch?, clean, files[] }` where each
`GitDiffFile { path, oldPath?, additions, deletions, binary, diff }` (camelCase on
the wire; the Rust structs use `snake_case` + serde rename).

Reachable end-to-end:
`gitDiff(path): GitDiffView!` query
→ worker resolver (`resolvers/ai/git-diff.ts`, gated on the `files:read` scope —
same posture as `filesListDir`/`filesRead`)
→ `desktopRpc('git_diff', { path })` → `rpc_git_diff` → the service above.
Client method: `JohnnyApiService.gitDiff(path)`.

### The render — `johnny-diff-view` + pure `diff-parse.ts`

`ui/src/components/diff-view/` ships a standalone `johnny-diff-view`
(`@Input() view: GitDiffView`) that renders the changed-file list (per-file +/-)
and the selected file's hunks. Each code line is classified by the pure
`diff-parse.ts` and highlighted with the **P3 render core**
(`highlightCode(text, langForPath(path))`) — **no second highlighter**.

`diff-parse.ts` (pure, spec'd under `web/`):

- `parseUnifiedDiff(diff): DiffLine[]` — classifies each line `add` / `del` /
  `hunk` / `meta` / `context` (headers checked before bare `+`/`-` so `+++`/`---`
  file headers aren't mistaken for content),
- `langForPath(path)` — extension → highlight.js language id (unknown → auto),
- `fileTotals(files)` — sums count/additions/deletions for the header.

### The tab

`terminal.page` gains a **Diff** pane-tab button (`setPaneTab(id,'diff')`) and an
`@if paneTab==='diff'` branch rendering `<johnny-diff-view [view]="diffFor(id)">`.
`loadDiff(session)` fires on tab activation and calls
`gitDiff(session.workingDirectory)`, refetching each activation. Best-effort: no
cwd / non-repo / failed RPC resolves to a benign empty view (the component shows
its own empty state, no toast). The `Plan` pane tab stays reserved for its own
phase; only `Diff` is wired here.

The planner page's existing inline diff (`planner.page.ts` `diffLines` +
`.diff-table`) is intentionally left as-is — migrating it onto `johnny-diff-view`
is a separate cleanup.

## Where the code lives

```
desktop/src-tauri/
  migrations/019_add_validation_config.sql   agent_plans.validation_config TEXT (nullable JSON)
  src/db/migrations.rs                        migration 019 registered
  src/db/models.rs                            AgentPlan.validation_config + ValidationLens struct
  src/services/agent_plans.rs
    default_validation_config / resolve_validation_lenses   default = product/qa/lead
    run_lens_fanout_review / run_planning_lens_fanout_review dynamic join_all fan-out
    gate_verdict_over_blocking / lens_summary_line          BLOCK/WARN split + roll-up
    update_plan_validation_config                           persist/clear/validate
    git_diff / GitDiffView / GitDiffFile                    whole-tree diff service + structs
  src/agent/mod.rs                            rpc_git_diff + rpc_update_agent_plan_validation_config + dispatch arms

worker/
  schema/johnnyone-ai.graphql                 AgentPlan.validationConfig; gitDiff query; GitDiffView/GitDiffFile;
                                              updateAgentPlanValidationConfig mutation
  resolvers/ai/git-diff.ts                     files:read scope → desktopRpc('git_diff')
  resolvers/ai/update-agent-plan-validation-config.ts  plans:write scope → desktopRpc(...)

ui/src/
  services/johnny-api.service.ts               gitDiff, updateAgentPlanValidationConfig, validationConfig typing,
                                              GitDiffView/GitDiffFile interfaces
  components/diff-view/                         johnny-diff-view component + pure diff-parse.ts

web/src/app/
  pages/terminal/terminal.page.{ts,html}       Diff pane-tab button + branch + loadDiff
  pages/terminal/diff-parse.spec.ts            diff-parse unit spec
  pages/validation-config/                      config page + pure validation-config-logic.ts (+ spec)
  app.routes.ts                                 lazy /initiatives/:id/validation route
```

## Security posture

- **`git_diff` is a global path op** with the same posture as
  `filesListDir`/`filesRead`: gated on the `files:read` scope at the worker, and
  path-guarded on the host (`resolve_workspace_file_path` rejects traversal
  outside the configured root). It is **read-only** — it never mutates the repo or
  the index (untracked files are excluded precisely to avoid `git add -N`).
- **The validation config stores provider/model *names*** (non-secret). The host
  validates every per-lens `provider` against the real `CliProvider` set before
  spawning a reviewer, so a config cannot smuggle in an unknown runner.
- **`updateAgentPlanValidationConfig`** is gated on the `plans:write` scope.

## Scope notes

- **Deferred activation.** P7 touches Rust (migration + orchestration +
  `git_diff`); DoD is `cargo build`/`cargo test` green, but the running desktop is
  **not** relaunched (that would kill the delivering session). The configurable
  review and the Diff tab's live data go live at an operator-chosen rebuild +
  relaunch (`npm run desktop`).
- **Visual verification deferred** (no browser in the runner). Acceptance is
  `nx build worker`/`web`/`ui` + `cargo test` + the pure specs
  (`diff-parse.spec.ts`, `validation-config-logic.spec.ts`), with mock §07
  (validation) and §04 (Diff tab) as the layout contracts.
- **Out of scope:** the `Plan` pane tab (its own phase); migrating the planner's
  inline diff onto `johnny-diff-view`; a model catalog / provider-config picker
  (model is free-text); untracked-file diffs; any new npm dependency or second
  render core.
