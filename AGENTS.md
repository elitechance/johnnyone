# AGENTS.md — how to dogfood JohnnyOne (J1)

> For an agent dropped into this repo with **zero prior context**. Read this top-to-bottom once
> and you can drive a real J1 build end-to-end.

## What is J1?

**JohnnyOne (J1)** is an AI coding **orchestrator**. You give it a *brief*; it runs a full software
lifecycle autonomously — it **plans** (a T1 planner agent drafts a phased plan), gets that plan
**reviewed** by a configurable panel of "lens" reviewers, then **develops** it phase by phase, with
each phase's output **reviewed** by the same lenses (a loop-back on `NEEDS_CHANGES`) until the whole
thing is **done**. The agents are real CLI coding agents (Claude Code, etc.) running in `tmux`.

## What is "dogfooding"?

"**Eating your own dog food**" — using your own product to build your own product. **Dogfooding J1**
means using the running J1 system to build the *next* improvement to J1 itself (this repo). You author
a brief describing a J1 feature, hand it to J1, and J1's own planner + worker + lens reviewers build
it. You (the overseer agent) **monitor**, unstick it if it stalls, verify the gates, and ship the PR.
It's the fastest way to both build features *and* surface J1's own weaknesses.

## Architecture (what talks to what)

```
  Web app (Angular/Ionic)            Desktop "engine" (Tauri/Rust)
  johnnyone.pages.dev                the ACTUAL orchestrator + owns tmux/agents/SQLite
        │                                   ▲   ▲
        │ GraphQL                relay-RPC  │   │ local HTTP 127.0.0.1:7788  (host API)
        ▼                        (WSS)      │   │ local endpoint 127.0.0.1:7714
  CF Worker (GraphQL gateway) ──────────────┘   │
  johnnyone.ethan-353.workers.dev               └─ agents run as `tmux` session `johnnyone_<sessionId>`
```

- **Desktop engine** (`desktop/src-tauri/`) is the brain: spawns agents, runs the coordinator loop,
  owns the SQLite DB (`~/.local/share/johnnyone/johnnyone.db`) and every `tmux` agent session. Must be
  running (`pgrep -af johnnyone-desktop`; heartbeats in `/tmp/johnnyone-desktop.log`).
- **CF Worker** (`worker/`) is a thin GraphQL gateway that **relays** calls to the desktop over a WSS
  relay. This is how you drive J1 **headlessly** (no UI). Endpoint:
  `https://johnnyone.ethan-353.workers.dev/graphql`.
- **Web** (`web/`) + **ui lib** (`ui/`) = the operator console at `johnnyone.pages.dev/initiatives`.
- **Plan store** lives OUTSIDE this repo: `/home/creepy/Documents/Workspace/.johnnyone/initiatives/<id>/plan`
  (configurable). Only *code edits* touch this repo; all J1-generated docs/plans/artifacts go there.

An **Initiative** = ONE logical unit of work made of **two plan-runs** sharing an `initiativeId`: a
`planning` run (its id *is* the initiativeId) and a `development` run. Lifecycle status flows
`planning → development → review → done` with an orthogonal health (`in-progress | needs-attention | blocked`).

## The headless dogfood recipe

You drive planning + development through the **Worker GraphQL**, which relays to the desktop. (You can
*also* click through the web console, but headless is scriptable and what monitoring relies on.)

### 0. Auth (every call)

- **Endpoint:** `https://johnnyone.ethan-353.workers.dev/graphql`
- **Headers:** `content-type: application/json`, `x-tenant-id: 00000000-0000-0000-0000-000000000001`,
  and a **browser-like `User-Agent`** (a blank/non-browser UA gets Cloudflare **HTTP 403 error 1010** —
  not an auth error).
- **Login** (dev-seed creds, from `worker/d1/seeds/01-default-tenant.sql`) →
  `Authorization: Bearer <accessToken>`. **JWT expires in ~15 min** — re-mint for long runs.

```graphql
mutation { login(input:{ email:"admin@johnnyone.local", password:"johnnyone-dev",
  tenantId:"00000000-0000-0000-0000-000000000001" }) { accessToken } }
```

> If a relay-routed call returns **"No online backend app found"** with a token that worked before, the
> token almost certainly **expired** — re-mint first before suspecting the desktop. Prove the node is up
> with the non-relay `query{ listDesktopNodes{ id status lastHeartbeatAt } }`.

### 1. Create the planning run

`createAgentPlan(input: CreateAgentPlanInput!)` returns `AgentPlanRun` — note it **wraps** the plan:
query `{ plan { id initiativeId status } }`, not `{ id }`.

```graphql
mutation($i: CreateAgentPlanInput!){ createAgentPlan(input:$i){ plan{ id initiativeId status planPath } } }
```

Input that actually works (learned the hard way):

| field | value | gotcha |
|---|---|---|
| `runType` | `"planning"` | dev auto-starts after plan approval — you do NOT create the dev run |
| `title` | short string | |
| `workspacePath` | `/home/creepy/Documents/Workspace/personal/apps/johnnyone` | the target repo (this one, for dogfood) |
| `planPath` | `<anything valid, no ".."` | **advisory** — a planning run derives its OWN store path `<initiatives_dir>/<newUuid>/plan` and creates it. Your value is ignored except for the `..` guard. |
| `workerProvider` / `reviewerProvider` | `"claude_code"` | |
| `brief` | the task (see below) | this is the whole job |
| `appScope` / `docsScope` | **omit unless a real subdir** | these are validated as **filesystem PATHS relative to the workspace**, NOT prose — passing a description makes create fail with `Invalid parent path`. |
| `referencePaths` | **omit or single path** | comma-joining multiple paths into one string fails path validation. Put reference material in the brief instead. |

### 2. Set the validation lenses (the review panel)

Separate call. `config` is a **JSON string** — an array of lenses:

```graphql
mutation($id:ID!,$c:String!){ updateAgentPlanValidationConfig(id:$id, config:$c){ plan{ validationConfig } } }
```

```json
[{"name":"qa","provider":"claude_code","vision":false,"blocking":true},
 {"name":"pr","provider":"claude_code","vision":false,"blocking":true},
 {"name":"le","provider":"claude_code","vision":false,"blocking":true}]
```

- Lens `name` is free-form (`qa`/`pr`/`le`, or the defaults `product`/`qa`/`lead`).
- **`vision:false`** = validate FUNCTIONALLY (build + tests + code reading). Use it — the reviewer worker
  has **no browser**, so a screenshot/e2e gate (`vision:true`) will stall a UI phase. Say so in the brief.

### 3. Start planning

```graphql
mutation($id:ID!){ startAgentPlan(id:$id){ plan{ status } } }   # → planning_planner_running
```

That's it. From here the lifecycle runs itself: planning → plan-review (the lenses review the *plan*) →
on approval the **development run auto-starts** → each phase: worker builds → lenses review → `PASS`
advances / `NEEDS_CHANGES` loops back → last phase pass → **done**.

## Writing a brief that doesn't churn

The single biggest lever on run quality. A vague brief causes fabrication + loop-backs. Bake decisions in:

- **State the outcome + the exact surfaces/files** (this repo: `web/src/app/pages/terminal/*`,
  `desktop/src-tauri/src/services/agent_plans.rs`, `worker/schema/johnnyone-ai.graphql`,
  `ui/src/services/johnny-api.service.ts`).
- **Name the conventions to follow.** This repo uses a **pure-seam pattern**: DOM-free logic modules +
  `vitest` specs (see `web/src/app/pages/terminal/console-logic.ts` / `*-logic.ts` + their `.spec.ts`).
  Tell the worker to mirror it.
- **Reuse, don't reinvent:** existing GraphQL/relay/`desktopRpc` idioms; one resolver per file under
  `worker/resolvers/ai/` (auto-wired by filename → the leading verb decides Query vs Mutation).
- **Define validation the worker CAN do** (no screenshots): `cargo check`, `vitest`, `nx build web`,
  `lokal cf worker validate`. Put these in the brief as the acceptance gates.
- **List what's OUT of scope** explicitly.
- Keep the plan/mocks OUT of this repo — in `.johnnyone/overhaul/` at the Workspace root.

## Monitoring a run

Poll `getAgentPlan(id)` and `listAgentPlans` (re-mint the JWT each ~10 min):

```graphql
query($id:ID!){ getAgentPlan(id:$id){ plan{ status initiativeStatus health error currentPhaseId workerSessionId } } }
```

Key transitions to watch: `planning_planner_running → planning_review_running → approved` (dev run
appears) `→ phase_worker_running ⇄ phase_review_running` (per phase; `initiativeStatus` pulses to
`review` during the lens fan-out) `→ done`.

**Ground truth for liveness is the `tmux` pane**, not the DB: `tmux capture-pane -p -t johnnyone_<sessionId>`.
A spinner / climbing token count = working (a long e2e can write no files for 20–40 min — not a stall).
The worker signals completion by POSTing `reportAgentResult(sessionId, kind:"ready")` to the **local**
host `127.0.0.1:7788` (not the cloud worker).

**A `NEEDS_CHANGES` loop-back is healthy** — the lens found something and the worker is fixing it. Read
the feedback in `<workspace>/.johnnyone/inbox/<uuid>.md` to see exactly what was flagged.

### If a run gets stuck (`needs_attention` / `blocked`)

- Prefer a **directed message to the worker** over editing code yourself: `tmux send-keys -t johnnyone_<sid> -l "…"` then send `Enter`. (The cloud chat mutation does NOT reach the tmux process.)
- If the worker finished but the coordinator stopped listening, `retryAgentReviewer(id)` re-enters the
  review. See `docs/` and the coordinator notes for the deeper recovery paths.

## Shipping (verify → PR → deploy)

Run the gates **yourself** before trusting the run's own green:

```bash
cd desktop/src-tauri && cargo check --features tauri/custom-protocol   # engine compiles
cd - && npx vitest run --config web/vitest.config.ts web/src/app/pages/terminal/   # jsdom-scoped
npx nx build web --skip-nx-cache                                        # prod web build
lokal cf worker validate                                               # schema↔resolver parity
```

> **vitest gotcha:** run web specs through **`--config web/vitest.config.ts`** — it sets
> `environment:'jsdom'` + `include:['web/**/*.spec.ts']`. Running bare `npx vitest` from the repo root
> uses the `node` env and any `localStorage`/jsdom spec fails with a false negative.

Then PR + merge (via `gh` or the GitHub API) and deploy:

```bash
npm run deploy:web    # nx build web && lokal cf worker deploy --env prod && lokal cf pages deploy --env prod
```

> Deploy **via the npm script**, not by calling `lokal cf pages deploy` directly — the script puts
> `node_modules/.bin` on PATH; a bare `lokal` invocation fails its internal `nx build` with `nx: not found`.

## Safety rules & sharp edges

- **Only ever kill `johnnyone_*` tmux sessions** — NEVER the user's own named sessions.
- Plan store (`.johnnyone/initiatives/`) and design docs (`.johnnyone/overhaul/`) live at the **Workspace
  root**, not in this repo, and are gitignored here — don't commit them.
- A resolver's filename **verb prefix** decides Query vs Mutation in the lokal CLI. A new verb (e.g.
  `run-`) may need adding to `MUTATION_PREFIXES` in the shared `lokal` CLI
  (`lokal/apps/lokal-infra/packages/cli/cloudflare/worker.ts`) — a **separate repo**. Without it, the
  resolver falls back to `type Query` and `lokal cf worker validate` breaks.
- Restart the engine with `npm run desktop` (rebuilds release engine, frees `:7788`, relaunches). This
  **kills any tmux agent pane**, so do it at a chosen boundary, not mid-phase.

## A worked example (this session shipped these)

- **johnnyone PR #22** — console unified Run/Resume (`runInitiativeFromPhase` mutation) + mobile terminal
  fixes: 3 phases / 11 tasks, built by J1 under `qa/pr/le` lenses, 2 healthy loop-backs (one caught a
  real backend no-op), all gates green, deployed.
- **johnnyone PR #23** — collapsed the console's per-run duplicate rows into one row per initiative.
- **lokal-infra PR #2** — added the `run-` mutation prefix (the cross-repo deploy prerequisite above).
