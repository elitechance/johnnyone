# Coordinator (CO) — T1/T2 handoff, signals, and reliability

> **Gates** — what CO checks before it believes a `ready`, how lens verdicts roll up
> (including the non-blocking `PASS_WITH_FOLLOWUPS`), and the guards that stop a
> non-converging loop live in **[`sdlc-gates.md`](sdlc-gates.md)**. This document covers the
> handoff and transport those gates sit on top of.

How the planner/development **coordinator** drives the two-tier agent loop, how it
detects "phase done" and review verdicts, the reliability mechanisms around that,
and the known-fragile parts. Code lives in
`desktop/src-tauri/src/services/agent_plans.rs` unless noted.

> The CO runs **inside the desktop binary** (`johnnyone-desktop`). On startup
> `resume_active_plan_loops` (called from `main.rs`) re-spawns a `coordinator_loop`
> for every plan that isn't in a terminal state (`approved` / `blocked` / `stopped`
> / `needs_attention`). `needs_attention` is terminal — those plans do **not**
> auto-resume; they need a manual rerun.

## The two-tier loop

A run is **sequential**, not parallel:

1. **T1 (worker / planner)** does the phase work. Status `phase_worker_running`
   (development) or `planning_planner_running` (planning). T2 is idle here — its
   session exists (created at plan start) but has no task yet, so an empty/idle T2
   while T1 works is **expected**, not a bug.
2. T1 signals completion by calling the **`reportAgentResult` API** (`kind:ready`).
3. CO dispatches **T2 (reviewer)**. Status `phase_review_running` /
   `planning_review_running`.
4. T2 reports its **verdict** via the same API (`kind:verdict`). CO routes on it:
   - `PASS` → advance phase / complete plan.
   - `NEEDS_CHANGES` / `BLOCKED` → send feedback back to T1 (status returns to
     worker-running).
   - no report after the idle window → clarify, then escalate (below).

Both session IDs (`worker_session_id`, `reviewer_session_id`) are created and
stored in the `agent_plans` row at plan creation (`create_planning_run` /
`create_development_run`). CO builds and sends every agent prompt via
`send_terminal_input`.

Errors propagate: any `Err` out of `coordinator_loop` is caught by
`spawn_coordinator_loop`, which sets the plan to `needs_attention` with the error
message.

## How CO receives signals — the `reportAgentResult` API (sole source)

As of 2026-06-16 the coordinator gets completion/verdict **only** from a structured
API call. **There is no terminal scraping of markers or verdicts anymore** — the
old `READY_FOR_T2_VALIDATION` / `VERDICT:`-line scraping was removed because it was
fundamentally fragile (see "Why scraping was dropped").

- **No new listener.** Reuses the in-process host GraphQL at
  `127.0.0.1:7788/graphql` (`host/mod.rs`, `MutationRoot`). **Unauthenticated**
  (localhost only) — the session ID is the verification.
- **Mutation** `reportAgentResult(sessionId, kind, verdict)` →
  `agent_plans::record_agent_report`:
  - `plan_role_for_session` looks `sessionId` up in `agent_plans` (worker vs
    reviewer). Unknown → reject. **Spoof guard:** the id is a 128-bit UUID,
    unguessable by a browser CSRF/DNS-rebind caller (the only realistic remote-ish
    threat to localhost); it's not *secret* (shown in UI/screenshots) but
    unguessable is enough.
  - Role check: `kind="ready"` only from worker/planner; `kind="verdict"` only
    from reviewer. Verdict normalized via `normalize_verdict_token`.
  - Stored in `AppState.agent_reports` (`HashMap<sessionId, AgentReport>`).
- **How agents call it:** CO **bakes a complete `curl` into the agent's prompt**
  (`report_command` / `append_worker_report` / `append_reviewer_report`) with the
  session id substituted; only the verdict enum token is left for the reviewer to
  fill — no shell-escaping risk. The curl is also appended to the NEEDS_CHANGES
  feedback prompts so re-work cycles report too. No shim.
  ```
  curl -s 127.0.0.1:7788/graphql -H 'content-type: application/json' \
    -d '{"query":"mutation{reportAgentResult(sessionId:\"<id>\",kind:\"verdict\",verdict:\"PASS\")}"}'
  ```

Verified by curl: unknown id rejected, role mismatches rejected, bad verdict
rejected, valid `ready`/`verdict` accepted (logged `recorded structured agent
report`).

## The wait loops (how CO consumes reports + escalates)

The capture (`tmux capture-pane -p -e -N -J -S -<n>`, fresh, throttle-independent)
is still used — but **only as an idle detector** (`snapshot_idle_key` over
`normalize_terminal_snapshot_for_idle`) to tell *when* an agent has stopped, so CO
doesn't nudge mid-work. It is no longer a signal source.

- **Worker/planner ready** — `wait_for_agent_ready_report` returns when a
  `kind:ready` report arrives. If the session goes **idle ~`READY_NUDGE_IDLE_MS`
  (20s) without reporting**, CO **re-sends the exact `curl`**; after
  `MAX_READY_NUDGES` (5) it returns `Err` → `needs_attention`.
- **Reviewer verdict** — `wait_for_reviewer_idle_or_verdict` returns
  `Some(snapshot)` on a `kind:verdict` report (it appends an authoritative
  `VERDICT: <v>` line so `handle_reviewer_output` reads the reported verdict while
  SUMMARY/FINDINGS still come from the pane). It returns `None` after ~20s idle
  with no report → caller runs `clarify_or_needs_attention` /
  `clarify_planning_or_needs_attention`, which **re-send the verdict `curl`** and
  escalate after `CLARIFICATION_LIMIT` (5).
- Each wait clears any stale report for its session at start, so a leftover report
  can't trigger the next turn.
- **Revision-round cap (`MAX_REVISION_ROUNDS = 6`, added 2026-06-16).** A review that
  never converges (T2 keeps returning non-PASS → back to T1 → re-review …) must not
  churn forever. `count_consecutive_non_pass` (used by
  `consecutive_non_pass_planning_rounds` / `consecutive_non_pass_phase_rounds`) counts
  the tail of the gate-result log, stopping at the most recent PASS so an amend cycle
  / later phase starts fresh. Once a non-PASS round is the Nth in a row,
  `handle_planning_reviewer_output` / `handle_reviewer_output` escalate to
  `needs_attention` (→ Discord attention) with a reason instead of looping back to T1.
  **Root cause it fixes:** the Marketplace planning run looped **116 NEEDS_CHANGES
  rounds over 4 days** (QA voted NEEDS_CHANGES 113/113; every merged review had
  `findings: ["none"]`, so T1 got no actionable feedback) with no cap → it never
  escalated, just silently burned.
- **Lens reasons come through the API, not scraping (2026-06-16).** First attempt
  scraped the SUMMARY/FINDINGS footer off the terminal — fragile: `marker_search_text`
  only dropped the ESC byte (leaving visible `[38;2;…m` codes), and when no footer was
  captured the fallback grabbed Grok's status/shortcut bar as the "summary". **Correct
  fix:** the lens reports verdict **and** summary + findings via `reportAgentResult`
  (the mutation already had `summary`/`findings`/`reason`; `AgentReport` stores them).
  To keep the agent's free text out of the shell, `lens_report_instruction` has it
  write a GraphQL request (fixed `query`, only `variables` filled) to a per-session
  body file `review_body_path` and POST with `curl -d @file`. `wait_for_lens_verdict`
  now returns the `AgentReport`; `insights_from_report` builds `ReviewInsights` from
  its fields (no terminal parse). `lens_verdict_payload` puts the reason on the
  `agent_lens_verdict` event so the web T2 panel shows *why* each lens voted.
  (`strip_ansi` + `is_terminal_chrome_line` were added to harden the few remaining
  scrape paths — e.g. the idle detector and the merged-footer parse.)

`parse_verdict` is still used — but now only to read the authoritative `VERDICT:`
line CO itself appends from a report. It is hardened to ignore any line listing
more than one option (so an echoed instruction can't be misread). `parse_review_insights`
still reads SUMMARY/FINDINGS from the pane for the event log — best-effort prose,
not a gating signal; it degrades to empty if the pane scrolled.

Constants: `READY_NUDGE_IDLE_MS = 20000`, `MAX_READY_NUDGES = 5`,
`CLARIFICATION_LIMIT = 5`. Tests: `cargo test --lib -- parse_verdict idle_key`.

## Why scraping was dropped

Grok ("Grok Build Beta", formerly "Grok Composer 2.5 Fast") is a **full-screen /
alternate-screen TUI**. `tmux capture-pane` can only read the *visible viewport* of
an alt-screen app — there is **no scrollback even with `-S -`**. On a long review
the `VERDICT:` line scrolled out of the captured frame, so the verdict couldn't be
parsed even though the agent emitted it (root cause of `Marketplace`'s
"unknown verdict"). Combined with format drift (the 3-lens output) and a
clarification race that once fired 5 re-asks in seconds, scraping was too brittle to
keep even as a fallback. The API removes the whole class: the agent reports the fact
directly, and if it doesn't, CO re-sends the command and ultimately escalates —
the failure is visible, not silently masked.

Historical note: the earlier "reviewer verdict-aware wait" fix (replacing
`wait_for_idle`) addressed the fast-clarification race while scraping was still the
source; the API supersedes it. `normalize_terminal_snapshot_for_idle` still carries
Grok-version-specific filters (e.g. `"Grok Composer"` vs the current
`"Grok Build · always-approve"`) used by the idle detector — re-audit when Grok's
UI changes.

## Next-gen coordinator — ephemeral agents, lens fan-out, update triggers, docs-commit (ALL BUILT as of 2026-06-17)

Design agreed in-session (2026-06-16). All four share one foundation; build the
foundation once, then the features plug in.

### Shared foundation — ephemeral agents + structured signals  ✅ BUILT (2026-06-16)
Migration `013_agent_plan_review_sessions`; helpers `register/unregister_review_session`,
`spawn_ephemeral_agent`/`dispose_ephemeral_agent` (currently `#[allow(dead_code)]`
until a feature wires them); `plan_role_for_session` now checks the registration
table first; `record_agent_report` + the `reportAgentResult` mutation take
`kind` (ready|verdict|done|update) + `findings`/`summary`/`severity`/`reason`/`evidence`.
Verified by curl: unknown rejected, reviewer verdict w/ fields accepted, `update`
accepted, and a registered lens session attributed `role=qa`.

- CO can spawn **ephemeral** agents for a single step (spawn → task → report →
  dispose), in addition to the persistent worker/reviewer.
- Each ephemeral agent is registered — `agent_plan_review_sessions(plan_id, role_or_lens, session_id)`
  — so `reportAgentResult`'s spoof guard trusts its reports. **Identity is derived
  from this registration, never self-declared** by the agent (the agent's baked
  `curl` only carries `sessionId`; CO resolves which lens/role it is). Extend
  `plan_role_for_session` to return the lens for review sessions.
- Extend `reportAgentResult`: add `kind` (`ready` | `verdict` | `update` | `done`),
  optional `findings`/`summary`, and for updates `severity` (`info`|`warn`|`attention`)
  + `reason` + optional `evidence` pointer. CO stamps sender identity from the
  registration. `append_event` → `publish_plan_update` keeps the UI live.

### 3-lens review fan-out (development + planning)  ✅ BUILT (2026-06-16)
`run_lens_fanout_review` (the `phase_review_running` arm calls it):
- Spawns **three ephemeral reviewers — Product, QA, Lead — in parallel**
  (`spawn_ephemeral_agent` per `REVIEW_LENSES`), each with a self-contained
  single-lens prompt (`lens_reviewer_prompt`) + the verdict report curl. Per-phase
  scope keeps 3× affordable.
- Collects verdicts concurrently (`tokio::join!` of `wait_for_lens_verdict`, which
  nudges/escalates per lens), disposes all three, then merges
  (`merged_verdict`: BLOCKED > NEEDS_CHANGES > PASS; PASS iff all pass) into a
  reviewer footer with findings labeled `[Product]/[QA]/[Lead]`, routed through the
  existing `handle_reviewer_output` (one merged to-do, not three round-trips).
- `dispatch_review` no longer prompts the persistent reviewer session (it just
  transitions to review); that session is unused for dev fan-out. The
  single-reviewer `reviewer_phase_prompt` is kept `#[allow(dead_code)]` as a
  fallback.
- The plan-repo per-phase commit (`pass_phase` → `git_history::commit_all`) is
  unchanged.

**Planning also fans out (added 2026-06-16):** `run_planning_lens_fanout_review`
(the `planning_review_running` arm calls it) mirrors the dev path against the whole
plan — spawns the same Product/QA/Lead ephemeral reviewers with
`planning_lens_reviewer_prompt` (planning-review checklist, judges the *plan*:
scope/mocks/screens-inventory, testable acceptance criteria, sound approach),
merges via `merge_planning_lens_outcomes` (no `PHASE:` header), routes through
`handle_planning_reviewer_output`. `dispatch_planning_review` is now
transition-only; the single-reviewer `planning_reviewer_prompt` +
`wait_for_reviewer_idle_or_verdict` are kept `#[allow(dead_code)]` as reference.
Planning's review surface is the whole plan (bigger than a dev phase), so its 3×
costs more than dev's per-phase fan-out — accepted by design.

### Update triggers + attention alerts (CO + author observability)
**Built (2026-06-16):**
- **Per-lens activity events** — `run_lens_fanout_review` /
  `run_planning_lens_fanout_review` emit `agent_lens_review_started` /
  `agent_lens_verdict` (payload `{ lens, verdict }`) so the planner event feed shows
  Product/QA/Lead activity.
- **Web run-stage observability (built 2026-06-16)** — answers "what stage is this
  run in" without reading raw status strings. `runStage` computed (`planner.page.ts`)
  maps `plan.status` (+ mode/phases/lens summary) to `{badge, title, detail, tone,
  steps, activeIndex}`. Surfaced as: (a) a **stage badge** in every panel header
  (T1/T2/CO — replaces the raw status / `unknown` gate verdict); (b) a **stage
  stepper** row in the CO panel (Draft › Review › Approved for planning; Work ›
  Review › Done + "Phase X/N" for dev) with a current-stage title + detail line (the
  detail shows the live lens summary during review); (c) a **T2 placeholder**
  (`t2Placeholder`) that explains the state when there's no lens review yet (e.g.
  "Review has not started — T1 is still working") instead of the old idle reviewer
  terminal. Tone colors: active=blue, done=green, attention=red. Probe:
  `scripts/stage-observability-probe.cjs` (compiles real SCSS).
- **Web lens-review panel (built 2026-06-16, follows the lens-activity mock)** — the
  `web` planner page renders a full panel via the `lensReview` computed
  (`planner.page.ts`), scoped to the most recent review round (events at/after the
  last `*review_started`). **It is the content of the T2 (reviewer) panel** — the
  3 lens reviewers are ephemeral, so the persistent T2 terminal is idle; the panel
  replaces it (terminal shown only as a fallback when there's no lens activity). This
  is why the mock titles it "T2 · Lens review". (First placed it in the CO widget;
  moved to T2 on user feedback — the lens review *is* T2's work.)
  - **Header** — "T2 · Lens review" tag + scope ("Phase <id> review" / "Plan review"
    — 3 lenses).
  - **Pills** — Product/QA/Lead, each a horizontal pill with a status dot +
    right-aligned verdict (reviewing… blue / PASS green / NEEDS_CHANGES amber /
    BLOCKED red). Latest event per lens wins, so a new round flips chips back to
    reviewing.
  - **Activity feed** — one row per round event: time + colored actor badge
    (CO teal / PR blue / QA gold / LE pink, mapped from the payload `lens`) + colored
    verdict keyword + text; non-PASS rows get an orange attention left-stripe. Covers
    `*review_started` (CO "spawning…"), `agent_lens_review_started` (per lens),
    `agent_lens_verdict`, and the merged `*gate_result` (CO "Merged verdict — …").
  - **Legend** — info/pass/needs-changes/attention.
  - All actor/verdict/time derivation is **web-side** from `payloadJson` (no backend
    change). Works for both run types. Verified vs the mock with
    `scripts/lens-strip-probe.cjs` (compiles the component SCSS via `sass`).
  - **Events relocated to T2 + latest-first (2026-06-16).** The generic event log was
    removed from the CO panel and folded into this T2 feed: `activity` is now built
    from the WHOLE run (not just the current round), **newest-first** (cap 80), via
    `lensActivityItem` which has a generic fallback (`event.summary`) for non-lens
    events + a small noise denylist (nudges/clarifications). So T2 = condensed pills
    on top + the "why" feed (lens reasons inline) latest-first; CO panel is now just
    stage stepper + phases + run settings.
  - **Verdict display rename (2026-06-16):** `verdictLabel()` maps the token
    `NEEDS_CHANGES` → **FAILED** in the UI (pills, activity chips, legend). Backend
    tokens are unchanged — display only.
- **Discord notifier, single chokepoint** — `append_event` is the one place that
  fires Discord: `maybe_notify_discord` → `discord_message_for` maps an event type
  to a colored embed, `notify_discord` POSTs it (reads `discord_webhook_url`; no-op
  if unset; always includes a deep link). **Deliberately quiet (updated 2026-06-16):
  alerts fire ONLY on a block/needs-human or a phase/plan being DONE — not on starts,
  and not on every T2 result.** Specifically:
  - **Done (✅/🎉 success):** `agent_phase_gate_result` *with verdict PASS* (phase
    done), `planning_gate_result` *with verdict PASS* (plan approved),
    `agent_plan_completed` (all dev phases done).
  - **Block/attention (🔴):** `agent_phase_needs_attention`, `planning_needs_attention`,
    `agent_blocked`, `agent_unblocked`, `coordinator_failed`.
  - **Silent:** all `*_started` events, and any **non-PASS** `*_gate_result` (a T2
    change-request just loops back to T1 — visible in the web lens panel, no ping).
    Routine events (worker idle, lens started) were never notified.
  - The coordinator catch emits a `coordinator_failed` event (so escalations show in
    the feed AND alert). Mapping is unit-tested; webhook + embed format verified by curl.
- **Discord webhook setting** in **both** UIs: web Settings → Alerts (generic
  get/set) and the desktop (host-app) Settings → Alerts (threaded through
  `HostSettings`/`GqlHostSettings`/`hostSettings` query + per-key `setSetting`).

**4b — blocked-needs-human + reply-to-unblock (built 2026-06-16):**
- The worker/planner prompt is given a **`blocked` report curl** (alongside `ready`):
  if it's genuinely stuck on a human decision, it states the question in its output,
  runs the blocked curl, and waits.
- `wait_for_agent_ready_report` detects the `blocked` report → emits an
  `agent_blocked` event (→ 🔴 Discord attention + feed) **once**, **flips the plan
  status to `blocked`** (see below), and **pauses nudging/escalation** (it no longer
  treats the idle agent as stuck — it's waiting on you). It resumes normally when the
  `ready` report arrives.
- **Status honesty (fixed 2026-08-11).** The pause above is *unbounded*, so the run
  must not keep a `*_running` status while it waits. On block the coordinator records
  the pre-block status, sets the plan to `blocked` (health `blocked`, error = the
  reason); on the `ready` report it restores the exact previous status and emits
  `agent_unblocked`. The restore is guarded on the plan still being `blocked`, so a
  human who stops or closes the run mid-block is never overridden.
  *Why:* run `4187e055` reported `blocked` and then sat at `phase_worker_running` for
  three days — the event log recorded a `None -> None` transition, and every status
  reader (`listAgentPlans`, `getAgentPlan`, the console) said "running" while nothing
  was happening. A Discord ping alone is best-effort: it is a no-op when no webhook is
  configured, and it cannot be re-checked later. The status is the durable signal.
- **Reply-to-unblock reuses the existing message bar**: the worker/planner session is
  persistent, so you open the run (via the alert deep link), read the agent's
  question in its terminal, and type the answer in the T1 message input — the agent
  resumes and eventually reports `ready`. No new reply UI needed.
- Scope: blocked is **worker/planner-only** (lens reviewers don't get the blocked
  curl — a lens that can't decide returns NEEDS_CHANGES/BLOCKED instead).

**Status strip — done (2026-06-16).** The at-a-glance Product/QA/Lead strip from the
mock is now in the `web` planner page (see "Web lens status strip" above). Remaining
nice-to-have: finer per-event severity styling in the raw event feed (cosmetic).

### Docs-commit agent (development only)  ✅ BUILT (2026-06-17)
`run_docs_commit_agent` — called from `pass_phase` right after `agent_plan_completed`
(dev full completion only; planning has no phases so it never fires there):
- **App repo path = the dev run's `app_scope`** (the explicit field; now persisted on
  dev create — `create_plan` normalizes + stores `input.app_scope`, and the web dev
  setup form has an "App repo path (docs commit on completion)" field). If `app_scope`
  is unset → emits `agent_docs_commit_skipped` and returns (no-op). The plan's own git
  repo is separate/intentional; this commits to the **app/code** repo.
- Spawns an ephemeral **"docs"** agent (`spawn_ephemeral_agent`, role `docs`) in the
  app repo, using the worker provider's model (`grok-build`). Prompt
  (`docs_commit_prompt`): read the README first; **don't bloat it** — put detail in a
  `docs/` dir (create if missing); update only the relevant linked docs; base content
  on the plan (`plan_path` overview/status) + actual code; **commit ONLY the doc files
  it changed** (`git add <doc paths>`, never `-A`); **no push**; then report via the
  `done` curl (`report_command(.., "done")`).
- `wait_for_done_report` waits for the `kind:done` API report (nudge/escalate like the
  lens waiter). On success → `agent_docs_committed`; on failure → `agent_docs_commit_failed`
  (→ Discord attention) but the plan stays **approved** (docs failure never un-approves).
- Why an agent (not hardcoded): adapts to each repo's existing doc conventions.

## Related planner UI behaviors (web)

`web/src/app/pages/planner/planner.page.ts`,
`ui/src/components/terminal-screen/terminal-screen.component.ts`:

- **Mobile panel auto-follow** — on mobile only the active panel's terminal is
  subscribed (desktop is status-driven). `syncMobilePanelToRunningPhase` switches
  the visible panel to T1/T2 on a phase transition so T2 updates show; respects a
  manual tab switch within a phase (`lastFollowedRunStatus`).
- **Idle pane keeps its last screen** — `syncPlanTerminalSubscriptions` no longer
  deletes the non-streaming pane's frame, and `seedIdlePlanTerminals` hydrates both
  panes from the persisted cache (`terminal-screen-cache.service`, localStorage,
  30 sessions / 7 days) so a refresh while T1 works still shows T2's last review.
  **Cache-only on purpose:** a live subscribe/unsubscribe here races with and tears
  down the active stream (a `snapshotOnce` attempt did exactly that and broke live
  refresh — do not reintroduce it without solving the refcount race).
- **Desktop scrollbar reserve** — `colsWithScrollbarReserve` subtracts one column
  on desktop so a full-width line (e.g. Grok's box border) can't wrap under the
  vertical scrollbar. Deterministic (no post-hoc clamp) to avoid oscillation.
- **Grok mobile reflow** — `sanitizeGrokMobileSnapshot` strips box-drawing →
  space; `splitLineToDisplayCols` / `splitAnsiLineToTerminalCols` are word-aware
  (no mid-word breaks). The 2s DO publish throttle
  (`MIN_TERMINAL_SCREEN_PUBLISH_MS`) must not be undermined.
