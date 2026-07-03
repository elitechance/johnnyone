# Briefing loop (overhaul P4)

> The **front door** to an Initiative: a real clarification conversation that runs
> *before* planning, ending in an explicit **"✓ Accept brief → Planning"** that
> transitions the same Initiative `briefing → planning` and kicks the existing
> planner with the accepted brief.
>
> Landed on branch `overhaul/2026-07` (commit `overhaul P4: briefing loop`). This
> doc covers new/changed behavior, the GraphQL surface, the host RPC methods, the
> brief-composition rules, and the UI. Plumbing + one new UI page — no new agent
> runner, no new transport, no forked renderer.

## What changed and why

Before P4, a user handed a cold `brief` string straight to `create_planning_run`.
P4 inserts a clarification loop in front of it: the user opens a briefing, the
J1 briefing assistant asks clarifying questions and drafts a concrete brief over
multiple turns, and only when the brief is solid does the user accept it — which
flips the Initiative into planning and starts the planner.

### The one framing fact

There is **no `initiatives` table and no `Initiative` type**. P1 implemented the
Initiative spine as three additive columns on `agent_plans`
(`initiative_id` / `initiative_status` / `health`). **An Initiative *is* an
`agent_plans` row**; its `id` is the `initiative_id`.

So briefing and planning are **one row**, not two. `briefing` was a
declared-but-unreached status until P4; this phase makes it reachable and adds
the single transition out of it. Accept **does not create a second row** — it
flips `initiative_status` on the same row from `briefing` to `planning`.

P4 adds one more additive column, `briefing_session_id` (migration `018`), that
links the Initiative row to its briefing conversation.

## End-to-end flow

```
createBriefingInitiative(input)                         [worker → host]
  └─ host create_briefing_run
       • INSERT agent_plans row, initiative_status='briefing'
       • worker_session_id = NULL (no planner yet)
       • create a kind='user' chat session → briefing_session_id
       • mkdir <initiatives_dir>/<id>/{plan,attachments}
       → returns AgentPlanRun

  ── conversation (existing relay chat path, unchanged) ──
  sendRelayChatMessage on briefingSessionId
    → CHAT_RELAY_DO → host handle_relay_chat
    → claude --print --output-format stream-json --resume   (multi-turn)
  first turn is seeded by composeBriefingSeed() (preamble + raw ask [+ attach/ref blocks])

  ── optional: enrich the brief ──
  initiativeUploadChunk(...)      📎 Attach / ⤒ Upload → <id>/attachments/
  addInitiativeReferencePath(...) ▤ Reference path     → reference_paths column

acceptInitiativeBrief(input)                            [worker → host]
  └─ host accept_brief
       • guard: row must be initiative_status='briefing' (else error)
       • compose_accepted_brief(draft, attachmentPaths, referencePaths)
       • apply_brief_acceptance: SET brief=composed, initiative_status='planning'  (same row)
       • provision the T1 planner session + plan dir
       • start_planning_run()   ← REUSED planner kickoff
       → returns AgentPlanRun
  UI then navigates to /planning/:initiativeId
```

Nothing here is a new subsystem: the conversation rides the **existing relay chat
path** (P2), uploads reuse the **P2 `host_files` chunk engine** (re-rooted),
reference paths reuse the planner's `browseHostDirectory` + `reference_paths`
column, the transcript renders with the **P3 `markdown-render` core** via
`johnny-chat-window` → `message-bubble`, and the kickoff reuses `start_planning_run`.

## GraphQL surface (worker)

New fields on the existing types (`worker/schema/johnnyone-ai.graphql`):

| Type | Field | Notes |
|---|---|---|
| `AgentPlan` | `briefingSessionId: String` | Nullable. Non-null only on a briefing Initiative. Nullable so a worker deploy against a pre-migration-018 desktop degrades to `null` instead of erroring. |
| `AgentPlanRun` | `briefingSessionId: String` | Mirror of `plan.briefingSessionId` for symmetry; the UI reads the link via `plan.briefingSessionId`. |

New mutations (all thin `desktopRpc` pass-throughs — the host owns behavior):

| Mutation | Auth scope | Host RPC | Effect |
|---|---|---|---|
| `createBriefingInitiative(input: CreateBriefingInput!): AgentPlanRun!` | `plans:write` | `create_briefing_run` | Create an Initiative at `briefing` + its `kind='user'` chat session + store dirs. |
| `acceptInitiativeBrief(input: AcceptBriefInput!): AgentPlanRun!` | `plans:write` | `accept_brief` | Flip the same row `briefing → planning`, compose the final brief, start the planner. |
| `initiativeUploadChunk(initiativeId, path, chunkIndex, totalChunks, dataBase64, done): UploadChunkResult!` | `files:write` | `initiative_upload_chunk` | Chunked upload into `<id>/attachments/` via the re-rooted P2 engine. `dataBase64` is never logged. |
| `addInitiativeReferencePath(initiativeId, path): AgentPlanRun!` | `plans:write` | `add_initiative_reference_path` | Record a host path in `reference_paths`. |

Inputs:

```graphql
input CreateBriefingInput {
  title: String
  workspacePath: String!
  brief: String!          # the raw ask (stored as the draft)
  workerProvider: String!
  reviewerProvider: String!
  model: String
}

input AcceptBriefInput {
  initiativeId: ID!
  finalBrief: String      # optional override of the stored draft
}
```

`UploadChunkResult` is reused from the P2 file-manager surface.

## Host methods (desktop — `agent_plans.rs`, `host_files.rs`, `settings.rs`)

Dispatched from the RPC router in `agent/mod.rs`:

- **`create_briefing_run`** — mirrors `create_planning_run` but: the row lands at
  `initiative_status='briefing'` with `worker_session_id = NULL`; its conversation
  is a **`kind='user'`** chat session (routed through the headless
  `claude --print --resume` chat path, **not** a `kind='agent'` planner terminal);
  both `<id>/plan` and `<id>/attachments` store dirs are created. **No agent is
  spawned here** — the first briefing turn is sent later by the UI.
- **`accept_brief`** (async) — guards that the row is still `briefing` (errors
  otherwise), composes the final brief, flips the row to `planning`, provisions the
  T1 planner session + plan dir, and calls the reused `start_planning_run`.
  - **`compose_accepted_brief(draft, attachment_paths, reference_paths)`** — pure,
    unit-tested. Returns the trimmed draft, then an `## Attached files` bullet list
    (omitted when empty), then a `## Referenced host paths` bullet list (omitted
    when empty).
  - **`apply_brief_acceptance`** — DB-only (`UPDATE … SET brief=?, initiative_status='planning'`),
    unit-testable against in-memory SQLite.
- **`add_initiative_reference_path`** — appends a path to the row's
  `reference_paths` and re-normalizes the block with the same
  `normalize_reference_paths` the planner create uses (dedup/validation live there).
- **`initiative_upload_chunk`** — re-roots the P2 `upload_chunk` engine at
  `settings::initiative_attachments_path(dir, id)` = `<initiatives_dir>/<id>/attachments`.
  Reuses the P2 path-guard (`resolve_within_root`, rejects `..`/escape) and size
  caps (**1 MiB/chunk, 50 MiB total**). Base64 payloads are never logged.

### Storage layout

```
<initiatives_dir>/<initiative_id>/
  plan/          plan files (created at briefing time; used after accept)
  attachments/   📎/⤒ uploaded files, path-guarded + capped
```

### Migration

`desktop/src-tauri/migrations/018_add_briefing_session.sql`:

```sql
ALTER TABLE agent_plans ADD COLUMN briefing_session_id TEXT;
```

Nullable, no backfill — legacy rows are never in `briefing`. Registered beside
`017_*` in `src/db/migrations.rs` and exercised against in-memory SQLite in
`cargo test` (the live DB is never touched).

## UI (web + ui)

- **Route** (`web/src/app/app.routes.ts`): lazy `briefing/new` and
  `briefing/:initiativeId`, both loading `BriefingPage`.
- **`johnny-chat-window`** gains one additive input, `@Input() showComposer = true`.
  The briefing page passes `[showComposer]="false"` so the default message composer
  is suppressed and the briefing composer is used instead. No other behavior change.
- **`BriefingComposerComponent`** (`web/.../components/briefing-composer/`) — the
  input row with **📎 Attach / ⤒ Upload** (→ `initiativeUploadChunk` into
  `<id>/attachments/`), **▤ Reference path** (→ `browseHostDirectory` →
  `addInitiativeReferencePath`), and **Send**. Pending attachments and reference
  paths render as removable chips.
- **`BriefingPage`** (`web/.../pages/briefing/`) — hosts the chat window + composer
  + the accept bar (**"✓ Accept brief → Planning"**, **"Keep refining"**, and the
  hint *"Accepting advances the same initiative from briefing to planning"*). Drives
  the conversation via `AiChatService` on `briefingSessionId`; on accept calls
  `acceptInitiativeBrief` then navigates to `/planning/:id`.
- **`briefing-page-logic.ts`** — pure, Angular/Ionic-free, unit-tested
  (`briefing-page-logic.spec.ts`): `BRIEFING_PREAMBLE`, `composeBriefingSeed`
  (first-turn seed = preamble + raw ask [+ `Attached files:` / `Referenced host
  paths:` blocks]), `foldMessages` (raw rows → `AiMessage[]`), `canAccept` (true
  only for a `briefing`-status Initiative), `shouldSeed`, and the
  `pendingAttachmentsReducer` / `referencePathsReducer` chip reducers.

`JohnnyApiService` (`ui/src/services/johnny-api.service.ts`) adds
`briefingSessionId?: string` to the `AgentPlan` interface (and to the field
selection), plus the four briefing methods (`createBriefingInitiative`,
`acceptInitiativeBrief`, `initiativeUploadChunk`, `addInitiativeReferencePath`).

## Security posture

Unchanged from P2 — briefing reuses the shipped primitives, adds no new privilege:

- Attachments go through the path-guarded (`resolve_within_root`, rejects `..`/escape),
  size-capped (1 MiB/chunk, 50 MiB total) upload engine, re-rooted at the initiative
  store. Traversal and cap rejections are unit-tested.
- Base64 chunk payloads are never logged.
- The briefing `claude_code` session reuses the already-shipped chat launch flags
  (reads only — no new privilege).
- Worker mutations are scoped: `plans:write` (create/accept/reference) and
  `files:write` (upload).

## Status / verification

- **Desktop lands as source + migration + `cargo test`** — the running desktop app
  is intentionally **not** rebuilt/relaunched (it hosts the tmux pane the overhaul
  session lives in). The feature activates on the next chosen rebuild.
- **Builds green:** `cargo build` + `cargo test`; `nx build worker` / `web` / `ui`;
  web vitest for `briefing-page-logic.spec.ts`.
- **Visual verification is deferred** (no browser in the runner). The mocks §07 are
  the layout contract; a live end-to-end briefing → accept is a post-rebuild
  operator smoke test, not a phase gate.

## Out of scope (later phases)

Configurable validation lenses (mock §07 right pane), the file-manager UI (P5),
the raw-shell launcher and the rail `+` "New initiative" menu (P6), the
responsive/multi-pane shell, `review`/`done` transitions, and moving the briefing
preamble to a server-side prompt template (client-seeded for now).
