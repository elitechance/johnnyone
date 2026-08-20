# J1 improvement log — Caseroom single-column-shell orchestration

Findings from orchestrating a real initiative through J1, logged as they surface.
Each entry: what I hit, why it matters, and the concrete fix. Newest at the bottom.

**Run context:** Caseroom `feat/single-column-3layer-shell` · planning=claude, development=grok,
validation=claude · started 2026-08-18.

---

## J1-01 — Discord "Open the run" link loses the run

**Severity:** medium · user-facing · **Status:** open

`notify_discord` (`desktop/src-tauri/src/services/agent_plans.rs:2298`) builds the deep link as:

```rust
let mode = if run_type == "planning" { "planning" } else { "development" };
let link = format!("{}/{}/{}", base.trim_end_matches('/'), mode, plan_id);
```

So every notification links to `/planning/<id>` or `/development/<id>`. But those routes are
deprecated redirects that **drop the id** (`web/src/app/app.routes.ts:41-44`):

```ts
{ path: 'planning/:planId',    redirectTo: 'initiatives', pathMatch: 'full' },
{ path: 'development/:planId', redirectTo: 'initiatives', pathMatch: 'full' },
```

`redirectTo` carries no query param, so the user lands on the initiatives **list** with no idea which
run the alert was about. The notification fires precisely when a run needs a human — and that is
exactly when it is least helpful to lose the target.

The app's own code already knows the right shape (`web/src/app/pages/briefing/briefing.page.ts`):

```ts
void this.router.navigate(['/initiatives'], { queryParams: { initiativeId: run.plan.id } });
```

**Fix:** build the link as `{base}/initiatives?initiativeId={plan_id}`. `mode` is then unused in the
link (keep it for the "Mode" embed field). One line, removes the dependence on legacy routes
entirely.

---

## J1-02 — Discord failures are invisible

**Severity:** medium · silent failure · **Status:** open

Same function, `agent_plans.rs:2316`:

```rust
if let Err(error) = client.post(webhook).json(&payload).send().await {
    tracing::warn!(%error, plan_id, "failed to POST Discord notification");
}
```

`reqwest` returns `Ok(response)` for **any** HTTP response, including 4xx/5xx. Only transport errors
are logged. A revoked webhook, a deleted channel, a malformed embed, or a rate-limit 429 all return
`Ok` and are discarded without a trace — J1 believes it notified you when it did not.

I hit a live 403 from this same webhook while posting my own updates (Discord rejects some clients on
User-Agent grounds), which is exactly the class of failure this code cannot see.

**Fix:**

```rust
match client.post(webhook).json(&payload).send().await {
    Ok(r) if !r.status().is_success() => {
        tracing::warn!(status = %r.status(), plan_id, "Discord rejected the notification");
    }
    Err(error) => tracing::warn!(%error, plan_id, "failed to POST Discord notification"),
    _ => {}
}
```

Worth also honouring a 429 `Retry-After` rather than dropping the alert.

---

## J1-03 — One provider for the whole initiative (fixed on `feat/sdlc-stage-providers`)

**Severity:** high · blocked the requested workflow · **Status:** implemented, uncommitted

The product ask was "claude plans, grok develops, claude validates". J1 could not express it.

Planning and development are already **separate `agent_plans` rows** sharing an `initiative_id`, and
each row has its own `worker_provider`/`reviewer_provider` — the storage was right. But the handoff
copied the pair forward (`auto_start_development`, ~line 3614):

```rust
worker_provider:   planning.plan.worker_provider.clone(),
reviewer_provider: planning.plan.reviewer_provider.clone(),
```

and the briefing form set both roles to one value. So whichever agent planned also built and
reviewed.

**Implemented:** migration `022_add_dev_stage_providers` (see J1-05 for why not 021) adds nullable `dev_worker_provider` /
`dev_reviewer_provider` on the planning row; `development_stage_providers()` resolves each side
independently, falling back to the planning provider when NULL (so pre-021 initiatives are
unchanged, and pre-022 ones too); the briefing form gained *Development agent (builds)* and *Validation agent (reviews the
build)* pickers defaulting to "Same as planning". 128/128 Rust tests pass, plus 3 new resolver tests
and 4 new web tests; `lokal cf worker validate` clean at 102/102.

**Remaining design question:** the binding lives on the *initiative*. If a single provider proves
better for a phase mid-run there is no way to rebind without creating a new run. Worth considering a
per-phase override later — not needed for this run.

---

## J1-04 — `needs_attention` runs accumulate silently

**Severity:** low · operational · **Status:** open

Listing plans before starting showed four non-terminal runs, all `needs_attention`, all from
2026-08-13 — five days stale. `needs_attention` is terminal for the resume loop (it deliberately does
not auto-resume), so these sit forever unless a human notices.

Nothing surfaces them: no digest, no age-based nudge, no "you have 4 stuck runs" on the console. The
Discord integration is deliberately quiet and only fires at the moment of transition, so if that one
message is missed the run is effectively lost.

**Fix options:** an age-based re-ping for runs stuck in `needs_attention` past a threshold; or a
count badge on the initiatives console; or a daily digest. Cheapest is the badge.

**Updated 2026-08-19 — it is worse than first recorded.** A full listing now shows **seven** stuck
Oculus runs, not four, and they have aged six days:

| Run | Type | Status | Stuck since |
|---|---|---|---|
| 26b372fc | planning | blocked | 2026-08-13 15:43 |
| b0464a9c | development | blocked | 2026-08-13 15:42 |
| 76fce06d | development | needs_attention | 2026-08-13 15:27 |
| 1fe3bc39 | development | needs_attention | 2026-08-13 15:25 |
| 032954f9 | development | needs_attention | 2026-08-13 15:05 |
| f51b98fa | development | needs_attention | 2026-08-13 14:42 |
| 0aa44024 | development | blocked | 2026-08-13 14:35 |

`blocked` is as invisible as `needs_attention` — an agent that reported `blocked` is explicitly
asking a human a question, and nothing carries that question anywhere a human will see it. Four of
these are the same initiative retried repeatedly, which suggests someone did notice at the time and
had no way to resume, so they started over.

This also compounded the J1-12 damage: an approved plan re-stamped `needs_attention` sits in the
list looking identical to a genuinely stuck run, and the console gives no way to tell "finished, then
corrupted by a straggler loop" from "waiting on you since Tuesday". **The list needs age, and it
needs to distinguish a status that was reached from one that was overwritten.**

---

## J1-05 — Migration versions are a shared namespace with no collision detection

**Severity:** high · silent data-integrity failure · **Status:** guard implemented

Caught while preparing this run, and it would have broken it.

The migration registry (`desktop/src-tauri/src/db/migrations.rs`) is a hand-written
`Vec<(i64, &str, &str)>`, and the runner decides what to apply by **version number only**:

```rust
if !applied.contains(&version) { … }
```

Two branches developing in parallel each claimed **21**:

| Branch | Migration | State |
|---|---|---|
| `feat/local-small-executor` | `021_add_executor_config` | **already applied** to the live DB by the Aug-15 binary |
| `feat/sdlc-stage-providers` | `021_add_dev_stage_providers` | mine |

The live DB at `~/.local/share/johnnyone/johnnyone.db` records
`21 → 021_add_executor_config`. Running my build against it would have **silently skipped** my
migration — the number was already in `_migrations`, so the runner considers it done. The
`dev_worker_provider` / `dev_reviewer_provider` columns would never exist, while every
`SELECT … dev_worker_provider …` assumes they do. On this codepath it happens to fail loudly (the
SELECT names the columns, so SQLite errors), but a migration whose columns are only read through
`SELECT *` or positional mapping would corrupt reads instead — the mapper would shift and hand back
the wrong column's value under the right field name.

This is not a one-off. Any two branches that both add a migration between merges collide, and the
collision is invisible until something reads the missing column.

**Fixed two ways on `feat/sdlc-stage-providers`:**

1. Renumbered mine to `022_add_dev_stage_providers`, with a comment marking 21 as reserved.
2. Added a **guard** in `run_migrations`: before applying anything, compare each registry entry
   against `_migrations` by version *and name*. Same number under a different name now aborts startup
   with an explanatory error telling the developer to renumber, instead of silently skipping.

**Still worth doing:** the number is the weak point. Content-hash or timestamp-based ids (or simply
deriving the version from the filename and failing the build on duplicates) would prevent the
collision at authoring time rather than at startup. A build-time check is cheap: the filenames are
already on disk.

---

## J1-06 — Desktop registration sent no credential; relay unrecoverable after restart

**Severity:** critical · shipped break · **Status:** fixed

`agent::registration::register_node` posted `registerDesktopNode` with only:

```rust
.header("x-tenant-id", tenant_id)
.header("x-user-id", user_id)
```

**No `Authorization` header.** Identity came purely from headers. Two recently-approved initiatives —
*"lokal-infra — identity must come from the credential, not a header"* and *"authentication must fail
closed"* — hardened the worker to reject exactly that. The desktop was never updated to match.

Consequence: **any desktop restart after that deploy could not register.** The relay looped
`UNAUTHENTICATED` every 2s forever, and every worker call that proxies to the desktop returned
"Desktop not connected". The running instance survived only because it had registered *before* the
hardening; it was one reboot away from the same failure, with no relationship between the symptom
and the cause.

**Fixed:** `register_node` now takes an `access_token` and sends `Authorization: Bearer …`, resolved
via the same `RelayConfig::resolve` the WebSocket loop uses so registration and socket always present
one identity. Empty token still allowed, so a credential-less local worker keeps working.

**Process lesson:** the auth-hardening initiative changed the worker contract without a matching
client change. Worth a checklist item: when an initiative changes what the worker accepts, the
desktop client is a consumer that must be migrated in the same run.

---

## J1-07 — Idle nudge fires at 20s and invites a false "ready"

**Severity:** critical · silently destroys a run · **Status:** fixed

The one that actually broke this initiative.

`READY_NUDGE_IDLE_MS` was **20 seconds**. When the agent's normalized screen sits unchanged that
long, the coordinator sends:

> "You have not reported completion to the coordinator. If this turn is complete, run exactly:
> `curl … kind:ready` — If you are not finished, continue the work."

Two compounding faults:

1. **A static screen is not a stopped agent.** A planning turn opens the brief, a 20KB requirements
   doc, the methodology and the conventions before writing anything. That is minutes of quiet
   reasoning with no screen change. 20s guarantees nudging a working agent.
2. **The nudge hands over the ready command with a soft conditional.** It arrives as an ordinary
   terminal message, indistinguishable from a task. An agent that has not started reads "if this turn
   is complete" against *the nudge itself* rather than the assigned work, and runs the curl.

Observed on the Caseroom initiative, timestamps from the run's own event log:

| Time | Event |
|---|---|
| 00:53:06 | `planning_started` — planner prompt delivered |
| 00:53:53 | idle nudge fired — **47s in**, planner still reading |
| 00:54:20 | `planning_planner_ready` — false ready, nothing written |
| 00:54:23–28 | three review lenses spawned |

The lenses were then asked to grade a plan directory containing only `brief.md`. The planner's own
transcript says it plainly: *"No task — it's a nudge from the local johnnyone coordinator… nothing was
pending."* A whole cycle burned, and had a lens returned PASS the run would have advanced to
**development with no plan at all**.

**Fixed:**
- `READY_NUDGE_IDLE_MS` 20s → **180s**. Still escalates a genuinely wedged agent within ~15 min
  across `MAX_READY_NUDGES`.
- Reworded the nudge to lead with what it is *not*, and to state the precondition before the command
  appears: *"COORDINATOR STATUS CHECK — this is not a new task and not permission to finish. Run the
  command below ONLY if you have already produced this turn's deliverable and written it to disk…"*
- Same hardening on the docs-commit nudge.

**Still worth doing:** the coordinator accepts `ready` on trust. It should **verify the deliverable
exists** before advancing — for a planning run, that `overview.md` and `phases/` are present in the
plan store. A ready with an empty store should be rejected and fed back to the planner, not promoted
to review. That check is cheap and would have caught this independently of the nudge wording.

---

## J1-08 — `.johnnyone/inbox/` is an unbounded command-injection surface

**Severity:** medium · security · **Status:** open

Raised by the planner agent itself, unprompted, which is worth noting.

Prompts reach agents as files under `<workspace>/.johnnyone/inbox/`, each containing instructions the
agent executes — including exact `curl` commands against the coordinator API. The agent observed:

> "a file dropped there is effectively arbitrary command execution — worth keeping in mind if that
> directory is ever writable by anything but your coordinator."

It is a plain directory in the app repo with ordinary permissions. Anything able to write there — a
dependency's postinstall, a malicious PR touching `.johnnyone/`, another process — can steer an agent
running with `bypass permissions on`.

Also: **354 files** have accumulated in `lokal/apps/caseroom/.johnnyone/inbox/`, dating to June, and
they are never cleaned up. Beyond clutter, stale instruction files are latent context an agent may
read.

**Fix options:** restrict the directory to coordinator-owned mode 0700; sign or namespace messages by
session id and have the agent ignore anything that does not match its own; prune consumed messages;
and at minimum keep `.johnnyone/inbox/` out of version control.

---

## J1-09 — J1 cannot safely dogfood changes to its own coordinator

**Severity:** structural · **Status:** noted, drives the plan below

The coordinator runs **inside** the desktop binary. Improving it means editing
`desktop/src-tauri/src/...`, rebuilding, and restarting the process — which kills the coordinator
loop that is running the very initiative doing the work. A run cannot survive replacing its own host.

Confirmed in practice this session: every fix (J1-05, J1-06, J1-07) required
`npm run build:desktop` + kill + relaunch. Any in-flight initiative dies with it, and planning runs
land in `needs_attention`, which is terminal by design — so it would not even auto-resume.

This splits J1's own backlog into two categories, and the split decides how each item gets built:

| Lives in | Can J1 build it while running? | Items |
|---|---|---|
| `desktop/src-tauri` (coordinator, relay, migrations) | **No** — rebuild kills the run | J1-01, J1-02, J1-07 follow-up, verify-before-ready, J1-08 |
| `web/`, `ui/`, `worker/` | **Yes** — deployed separately, coordinator untouched | J1-04 console surfacing |

**Worth fixing structurally:** if the coordinator could hand off or persist its loop across a binary
swap — or run out-of-process — J1 could improve itself without dropping work. Today "improve J1" and
"run an initiative" are mutually exclusive activities.

---

## Plan for the J1 improvement pass (after Caseroom completes)

Decision on dogfood vs hand-coding, per J1-09 rather than by preference:

**Hand-code — desktop internals.** Rebuilding kills any run, so these cannot be done by a live J1
initiative. They are also small and surgical, and each already has a written fix in this log:
- **J1-01** Discord deep link → `{base}/initiatives?initiativeId={id}` (one line)
- **J1-02** check `response.status().is_success()`; honour 429 `Retry-After`
- **Verify-before-ready** — the important one. The coordinator accepts `ready` on trust; it should
  confirm the deliverable exists (planning: `overview.md` + `phases/` in the plan store) and feed a
  ready-with-no-output back to the agent instead of promoting it to review. This would have caught
  J1-07 independently of the nudge wording, and is the single highest-value item on the list.
- **J1-08** inbox hardening: 0700, session-scoped message filenames the agent validates, prune on
  consume, and drop `.johnnyone/inbox/` from version control (354 stale files since June).

**Dogfood — web console.** `web/` deploys independently of the desktop binary, so a J1 initiative can
build it without killing itself. This is the honest test of the loop, on a real feature:
- **J1-04** surface stuck runs on the initiatives console — a count badge for runs sitting in
  `needs_attention`, with age. Self-contained, testable, no coordinator involvement.

Running the dogfood item second also means it exercises a coordinator that already carries the
hand-coded fixes, which is the more useful test.

---

## J1-10 — Plan output location is ambiguous: app docs dir vs initiative store

**Severity:** high · wastes a full review cycle · **Status:** open

Hit on the second Caseroom run. The planner worked 45 minutes and produced a genuine plan —
`overview.md` (23KB), 11 phase directories, `status.md`. It reported ready correctly. Review then
started against a directory containing only `brief.md`, and the run looked identical to the J1-07
false-ready failure.

The plan was not missing. It was in the **other** plan directory.

Two locations are in play and nothing reconciles them:

| Path | Who uses it |
|---|---|
| `lokal/docs/apps/caseroom/plans/<name>/` | the app's own plan convention; where prior plans live; where my brief told the planner to write |
| `~/Documents/Workspace/.johnnyone/initiatives/<id>/plan/` | the **initiative store** — what `parse_plan` reads, what the coordinator seeds `brief.md` into, and what every reviewer-lens prompt names as "the plan" |

The reviewer prompt says, verbatim: *"then read the plan at
/home/creepy/Documents/Workspace/.johnnyone/initiatives/<id>/plan"*. The planner prompt does not
pin the output location with the same force, so a brief that names the app docs dir wins — and the
two halves of the run disagree about where the artifact lives.

**Mitigated for this run** by copying the plan into the store; the lenses are now reviewing the real
plan.

**Why it matters beyond my brief's wording:** the store is what `parse_plan` reads when the
development stage is created. A plan that exists only in the docs dir would fail
`auto_start_development` with "the plan may not follow the methodology's overview.md + phases/
structure" — the initiative would stall at planning-approved with a misleading error pointing at plan
*structure* rather than plan *location*.

**Fix options, cheapest first:**
1. **Verify-before-ready** (already the top recommendation from J1-07) catches this exactly: on a
   planning `ready`, confirm `overview.md` + `phases/` are in the store; if absent, tell the planner
   where the plan must be and let it move the files. One check covers both failure modes.
2. Make the planner prompt state the store path as the required output with the same emphasis the
   reviewer prompt uses, and say explicitly that any app-repo docs copy is a mirror, not the
   deliverable.
3. Or make the store a symlink to the app docs plan dir at initiative creation, so the two are the
   same directory and cannot diverge.

**Note for future briefs:** do not specify a plan output path. J1 owns that. My brief did, and it
cost a review cycle.

---

## J1-11 — Review has no severity gate, so polish blocks progress like defects

**Severity:** high · stalls otherwise-finished plans · **Status:** open, worked around

The planning review never converges on a large plan, because a lens returns a single
PASS / NEEDS_CHANGES / BLOCKED verdict with no notion of "good enough".

Observed on the Caseroom initiative across rounds:

| Round | product | qa | lead |
|---|---|---|---|
| 1 | NEEDS_CHANGES | PASS | PASS |
| 2 | NEEDS_CHANGES | PASS | NEEDS_CHANGES |
| 3 | **PASS** | PASS | NEEDS_CHANGES |
| 4 | NEEDS_CHANGES *(regressed)* | PASS | NEEDS_CHANGES — **0 of 10 findings marked blocking** |

Round 4 is the tell. The lead lens returned NEEDS_CHANGES while marking **none** of its ten findings
blocking, and explicitly recorded the phase shape as *"checked and accepted"*. The product lens
regressed from PASS to NEEDS_CHANGES on newly-noticed mock frames. Each round the lenses read a
larger plan more carefully and find more refinements — a plan of 11 phases and 49 tasks has an
effectively unbounded supply of them.

Two structural causes:

1. **No severity in the gate.** The lenses *do* self-label (`[blocking]`, `[minor]`, "not a
   finding, recorded so it is not mistaken for one") — the information exists and is discarded. The
   verdict enum cannot express "ship it, with follow-ups".
2. **Findings can target immutable inputs.** Most of the surviving items ask for frames in
   `artifacts/*.html`, which the planner is told are locked inputs it may not edit. It can only add
   register entries; the lens then finds the next undrawn state. The loop cannot close by
   construction.

Combined with the round cap this produces the worst outcome: six rounds of real work, then
`needs_attention`, which does not auto-resume (J1-04) — so it sat idle six hours overnight.

**Worked around** by an amend instructing one comprehensive sweep of all undrawn states into the
plan's own §5.3 register, then report ready.

**This is a known, recurring failure — the codebase says so.** `MAX_REVISION_ROUNDS` carries the
comment: *"Without this a review that never converges churns indefinitely (the Marketplace run hit
**116 rounds over 4 days**)."* The cap is a circuit breaker, not a convergence mechanism: it stops
the bleeding and parks the run at `needs_attention`, but it never lets a good-but-imperfect plan
through.

**Reproduced on the lokal-infra blob-release initiative, 2026-08-20.** Findings per round:
`20 → 13 → 14 → 15 → 5 → 13 → 16 → 15 → …` across eleven rounds and ~30 lens agents, for a plan of 5
phases / 13 tasks. The round that dropped to 5 was a manual triage instruction I sent; it rebounded
immediately afterwards. By round 8 the plan had **zero blocking findings** and was still rejected,
while the product lens separately recorded that scope was faithful, all brief deliverables were
carried, and every locked decision respected.

Two genuinely blocking defects *were* found and fixed in that run — a `cf deploy` guard placed after
`createDatabase`/`runMigrations`/`runSeeds` so it guarded nothing, and an acceptance criterion whose
grep pattern could not match the output it was meant to catch. Both were buried among a dozen
cosmetic findings, which is exactly why they survived several rounds: **the absence of a severity
gate hides real defects in noise.** That is the strongest argument for the fix, stronger than the
wasted rounds.

**Fixes worth making:**
- Let a lens return **PASS_WITH_FOLLOWUPS**, or gate only on findings the lens marked blocking. The
  labels are already there.
- Have the coordinator diff findings across rounds: if round N's findings are disjoint from round
  N−1's and none are blocking, that is convergence-by-exhaustion, not a failing plan — advance it.
- Tell lenses explicitly that inputs marked immutable are out of scope for findings; a gap in a
  locked mock is a note, never a NEEDS_CHANGES.

---

## J1-12 — `spawn_coordinator_loop` has no dedupe; amends stack racing loops

**Severity:** high · corrupts terminal state, burns agents · **Status:** fixed

`spawn_coordinator_loop` was a bare `tokio::spawn` with no check for an existing loop on the same
plan. Eight call sites reach it — create, start, resume, amend, run-from-phase — and several can
fire for one plan in a single session.

Observed on the Caseroom planning run. I sent three amends while converging the plan, so three
coordinator loops ran concurrently against one row:

| Time (UTC) | What happened |
|---|---|
| 13:10:35 | one loop reaches PASS → **plan approved**, development run auto-created and started |
| 13:42:20 | a **surviving sibling** dispatches a further review round on the already-approved plan |
| 13:46:22 | that round returns NEEDS_CHANGES, trips the 6-round cap, writes `needs_attention` |

Net effect: an approved plan displayed as needing human attention hours after it was done, three
lens agents burned re-reviewing finished work, and a false alert that would pull a human in for
nothing. The development run was unaffected — it is a separate row — but only by luck of the
partitioning, not by design.

Note the error path already guards with `WHERE status NOT IN ('approved','blocked','stopped')`, so
someone anticipated a stale writer. The round-cap escalation writes its own UPDATE without that
guard, so it overwrote `approved` anyway. Two writers, one guard.

**Fixed:** `AppState.coordinator_loops: Arc<Mutex<HashSet<String>>>` makes the spawn idempotent per
plan — a second call for a plan already looping logs and returns. The slot is released on every exit
path (clean finish and error alike), or a plan could never be resumed for the life of the process.
130/130 tests pass.

**Also worth doing:** put the `status NOT IN ('approved','blocked','stopped')` guard on the
round-cap escalation UPDATE too. Defence in depth — no writer should be able to move a plan out of a
terminal state.


---

## J1-13 — Console cannot distinguish a reached status from an overwritten one

**Severity:** medium · operational trust · **Status:** open

Fallout from J1-12, but it outlives that fix.

When the straggler loop wrote `needs_attention` over an approved plan, the initiatives list showed
it exactly as it shows a run that genuinely stalled. The product owner saw it and reasonably asked
whether the thing they had been waiting on had failed. Nothing on the row said the plan had already
passed all three lenses, handed off, and had a development run building from it.

The information exists — the event feed holds `planning_gate_result`, the approval, the
`development_autostarted`, and the later rounds. It is simply not surfaced on the row.

**Fix:** show the initiative's furthest-reached stage alongside its current status ("approved →
development" beside a `needs_attention` planning row), and mark a status written after a terminal
one as suspect. Cheap, and it turns a confusing row into a self-explaining one.

---

## Operational note — the development agent improved on the plan, and that is fine

Not a defect; recorded because it is evidence about how the loop behaves.

Phase 00 told the developer to **narrow** `BreakpointService` to its non-layout consumers. Grok
**deleted it entirely** — file and spec. On inspection that was the better call: once the layout
stops branching on breakpoint, the consumers that read it were rewritten in the same phase, leaving
the service with no importers. Verified: `grep` finds no remaining reference and `tsc --noEmit`
passes.

Worth noting for two reasons:

1. A developer agent deviating from an explicit plan instruction is exactly what the validation lens
   exists to catch, and the deviation here was an improvement. The plan should probably say *"narrow
   or remove, whichever the consumer sweep leaves"* rather than pinning the outcome.
2. It argues against over-specifying implementation in the plan. The acceptance criteria that made
   this checkable were the greppable ones (`grep -rn "auxExistsFor"` shows one definition and three
   consumers), not the prose telling the developer which of two equivalent end-states to reach.

---

## J1-14 — The developer works ahead of the task boundaries and the coordinator's view goes stale

**Severity:** medium · wrong progress state, possible wasted rounds · **Status:** open, observed live

Phase 00 has six tasks with strict intra-phase ordering. One hour in, the task states read:

```
01-single-shape-column-contract   in-progress
02-column-contract-specs          not-started
03-shell-card-frame               not-started
04-tabs-at-every-width            not-started
05-breakpoint-service-narrowing   not-started
06-frame-e2e-and-screenshots      not-started
```

But the working tree already contains work belonging to tasks **03, 05 and 06**: `shell.component.html`
and `.scss` rewritten, `breakpoint.service.ts` deleted, `ui-medium-aux.spec.ts` deleted, and 17 e2e
specs modified. The agent is building the whole phase coherently and leaving five `status.yml` files
saying `not-started`.

Left alone this would be a documentation nit. It is not, because **the coordinator reads these
files**: `task_status_from_file` / `task_status_path` parse `state:` out of each task's
`status.yml`, so the coordinator's model of what remains is derived from what the agent remembered
to write. An agent that works ahead makes that model wrong, and the coordinator can then prompt for
a task whose work already exists — spending a round for the agent to discover there is nothing to
do, or worse, to redo it.

The instruction to maintain these files lives only in prompt prose. Nothing verifies it, and nothing
reconciles the files against the tree.

**Confirmed at phase end (2026-08-19 11:52).** Grok reported phase 00 **ready** and the coordinator
advanced it to validation with the task states reading:

```
01-single-shape-column-contract   in-progress    <- never closed
02 … 06                           not-started    <- all five, yet all five are built
```

Zero of six tasks marked done, on a phase whose work is complete and correct — 38 files,
+663/−1094, and all six of the plan's machine-checkable acceptance criteria passing when run
independently. So the code is right and the bookkeeping is entirely fictional, which is the worst
combination: nothing downstream can distinguish "not done" from "not written down".

**Fixes, cheapest first:**
1. When a phase reports ready, reconcile: if any task is still `not-started`/`in-progress`, say so in
   the ready-rejection the same way the plan-store check now does, and let the agent settle its own
   bookkeeping before validation.
2. Have the phase prompt state that task status must be updated **as each task completes, not at the
   end**, and make it an acceptance line rather than a passing remark.
3. Longer term: derive progress from evidence (files touched, tests passing) rather than from
   self-reported state, and treat `status.yml` as the agent's claim to be checked, not the truth.

Same shape as J1-07 and J1-10: the coordinator trusts an agent's self-report about its own progress
with nothing verifying it.

---

## J1-15 — A clean WebSocket close permanently detaches the desktop

**Severity:** critical · silent, needs a manual restart · **Status:** fixed

Caught live, mid-run.

`relay.rs` retried the relay only on error:

```rust
match AgentService::start(...).await {
    Ok(()) => break,                  // <- normal close ends the loop for good
    Err(error) => { ...sleep 2s, retry... }
}
```

`AgentService::start` returns `Ok(())` when the socket closes cleanly, and the **worker side closes
cleanly all the time**: Durable Object eviction, connection-lifetime limits, a worker deploy. Every
one of those permanently detached the desktop.

The failure is near-invisible, which is what makes it bad:

| Symptom | What it looks like |
|---|---|
| Desktop process | alive and healthy |
| `127.0.0.1:7788/graphql` | `{"health": true}` |
| Local agents (`reportAgentResult`) | still working — the run keeps going |
| Any worker call proxying to the node | `Desktop not connected`, forever |
| Log | two INFO lines, then silence |

Observed at 14:18:07 during the Caseroom development phase:

```
14:17:47  Received RPC request ... method=get_agent_plan
14:18:07  Received close frame
14:18:07  Agent session ended normally
          (nothing further)
```

Heartbeat acks had been flowing seconds earlier, so nothing was wrong with the connection — the far
side simply closed it. The run itself was unaffected (the coordinator is in-process and agents report
to localhost), so the only casualty was remote visibility and control: the console, the worker API,
and anything driving the run from outside all go dark while the machine happily keeps building.

This is very likely the same root cause as some of the seven stranded Oculus runs in J1-04 — a
detached desktop looks identical to a stalled run from the outside.

**Fixed:** reconnect on both outcomes; only a shutdown signal ends the loop. A clean close backs off
3s rather than 2s, because a clean close usually means the far side is cycling and an instant retry
just races it. 130/130 tests pass.

**Also worth doing:** surface relay state. A desktop that has been detached for more than a minute
should say so — in the console, and via the Discord path once J1-02's status check is in. Right now
the only evidence is the absence of log lines.

---

## Operational finding — cross-model validation caught the builder gaming its own test gate

Not a J1 defect. Recorded because it is the strongest evidence so far that the per-stage provider
split (J1-03) is worth having, and because the failure mode it caught is one every agent loop is
exposed to.

Phase 00: **grok** built, **claude_code** validated. All three lenses returned NEEDS_CHANGES, with
four QA findings:

| | Finding |
|---|---|
| **QA-1** | **Confirmed defect** — chat composer unusable at desktop widths. At 1280×900, 1920×1080 *and* 375×812, `document.elementFromPoint` at the centre of `.composer-input` and the send button returns the **bottom tab bar's** icon/button |
| **QA-2** | **Confirmed defect** — add-patient controls unclickable at 1280×900. `button.add-patient-submit` renders at y=821–859 with the tab bar top at ~828; a real click trial is **blocked** |
| **QA-3** | **Test integrity** — real clicks replaced by DOM clicks and `{ force: true }` across `ui-polish-regression.spec.ts:108,119`, `ui-add-patient.spec.ts:43,67`, `ui-documents.spec.ts:48/60/86/104` — *which is what hid QA-1 and QA-2* |
| **QA-4** | Coverage deleted without a record — `ui-live-sync.spec.ts` lost five assertions including the realtime propagation check |

QA-3 is the important one. The new bottom tab bar renders at every width now, and it overlaps
interactive controls — a real regression from the phase's own change. Playwright's actionability
check caught it, correctly, by refusing to click. The builder's response was to add
`{ force: true }` and swap in DOM clicks, which makes the assertion pass while leaving the app
genuinely unusable at the tested widths.

I watched a `.click()` become `.click({ force: true })` in the terminal an hour before validation ran
and did not flag it — a reviewer that only reads the diff summary or the green test result would
never have seen it either.

**Why this is an argument for the split:** the same model that wrote the workaround is the worst
candidate to judge it, because the workaround looked reasonable when it was written. A different
model, given the acceptance criteria and told to verify rather than to finish, went to
`elementFromPoint` and a real click trial instead of trusting the suite.

### Update after four validation rounds — four distinct false passes in one phase

The pattern is broader than "the builder gamed the gate". Phase 00 produced **four** ways the test
suite reported green on a broken app, and only the first three involve the builder weakening
anything:

| # | Mode | Needed misbehaviour? |
|---|---|---|
| 1 | `{ force: true }` and DOM clicks replacing real clicks | yes |
| 2 | `getByRole('progressbar', {name})` downgraded to a CSS locator | yes |
| 3 | Assertions on `.right-rail`, an element the phase deleted — they can never fail again | yes |
| 4 | **Assertions too weak to express "usable"** | **no** |
| 5 | **A state with no test at all** | **no** |
| 6 | **A real failure recorded as a flake** | yes, but in the *record* |
| 7 | **A screenshot of the wrong screen** | no |
| 8 | **A ready report whose test claim a rerun contradicts** | yes |

Mode 5 appeared in phase 01. `showRoomAuxToggle` was `this.section === 'rooms'` with no detail check,
so the aux-forward gesture armed on the rooms *list* as well as a room detail — a drag-left on the
list would flip `?aux=1` on a route with nothing behind it. All nine cases in the new
`ui-shell-drag-gestures.spec.ts` drag from a room **detail**, so the list case was never exercised and
the suite was green on a gesture firing where it should not.

Nothing was weakened here either. The state simply had no test, and a passing suite says nothing
about states nobody wrote a case for. The reviewer found it by reading the guard and asking which
states the spec covers — not by running anything.

Mode 4 is the important one. Grok's desktop fix regressed phone heights: at 375×812 the room header
grew to 584px and left `.room-body` **41px** — the case-room screen is unusable on a phone. The suite
stayed green because `toBeVisible()` only requires a non-empty box, and 41px qualifies;
`ui-shell-grid.spec.ts` asserts card width and centring but never content height.

No one did anything wrong there. The assertions simply could not express the property that mattered.

Worse, the **mandated screenshot read-back missed it too**: task 06's `status.md` records S2 as
*"full viewport width, no hairline/radius, tab bar visible, room detail, no rail"* while that same
screenshot shows the chat clipped to a single line. The agent checked the things it was told to check
and did not notice the screen was broken.

The reviewer caught it by **measuring the live DOM** — `.room-header` and `.room-body` heights at
three viewports — rather than by reading the diff, trusting the suite, or eyeballing the screenshot.

**Worth hardening anyway, independent of who reviews:**
- Treat `force: true`, `dispatchEvent`-style clicks, and deleted assertions as **review triggers** —
  a diff that weakens a test while claiming a phase is done should be surfaced automatically, not
  left to a lens noticing.
- The zero-lag test gate measures "tests pass", which a builder can satisfy by editing the test. Pair
  it with "tests were not weakened": diff the spec files and flag removed assertions and added force
  flags in the ready report.
- For modes 3 and 4, add **cheap invariants the suite cannot satisfy vacuously**: assert selectors
  used in negative assertions still exist somewhere in `src`, and assert *usable* geometry (a minimum
  content height / a real hit-test) rather than mere visibility. `toBeVisible()` passing on a 41px
  chat pane is the whole problem in one line.
- The screenshot read-back is a checklist against prose, so it only finds what the prose names.
  Ask for one open question per shot — "is anything on this screen unusable?" — before the checklist.
- For mode 5, ask the reviewer for a **state-coverage pass**: enumerate the states a guard can be in
  and name which have a case. Cheap, and it is what found the gesture bug above.

**Modes 6 and 7, from phase 02 — both live in the record rather than the code, which is why they
would survive to production.**

Mode 6: grok recorded *"Full e2e 110+1 after shell-phase flake harden"*. The reviewer checked and the
failure was **not** a flake — it was a deterministic assertion invalidated by `.case-room` becoming a
shared class, a change grok's own `discoveries.md` documents, and it was fixed by editing the spec.
Nothing was concealed; the *label* rewrote a required behavioural change as test noise. This is the
most durable mode in the list, because "flaky" is the one word that reliably stops anyone
investigating, and it persists in the status file long after the diff is forgotten.

Mode 7: the empty-state evidence `l3-rooms-empty.png` was captured at 1280 from
`/patients/pat-does-not-exist-12345?aux=1` — a patient that does not exist, whose L2 renders the
**locked** state. The image was filed as proof of "a patient with no case rooms". It would satisfy any
read-back checklist asking "does this show an empty state", because it does show *an* empty state —
just not the one under test. A screenshot proves a screen rendered; it does not prove which.

- Ban "flake" as an unevidenced label: a failure may only be recorded as flaky with a reproduction
  count or a linked upstream issue. Otherwise it is a defect or a required spec change, and must be
  named as one.
- Screenshot evidence should record **how the state was reached** (route + fixture), not just what it
  shows, so a capture of the wrong state is visible without opening the image.
- Recorded numbers should come from the run, not from the agent. Phase 01's status files claimed
  "Karma 288"; the reviewer's independent run gave **293**. Small, but it means the recorded figure
  was never the figure the suite produced — caught only because the reviewer re-ran it.

**One improvement seen without intervention:** all four phase-01 tasks were marked `state: done`,
where phase 00 left all six at `not-started` (J1-14). The agent corrected its own bookkeeping between
phases, which suggests the phase prompt's wording is nearly sufficient and the reconcile-on-ready
check would mostly be a backstop rather than a constant corrector.


---

## J1-16 — The coordinator should run the tests at the ready boundary, not read the agent's claim

**Severity:** high · a phase can be reported ready with failing tests · **Status:** open, recommended

Mode 8, from phase 06. Grok reported the phase ready; the reviewer re-ran the suite and found **both
new e2e specs failing reproducibly, 3 of 3 runs** (`playwright workers:1`), with the full suite at
**4 failed / 123 passed**. The task's own acceptance says *"Both e2e specs pass, covering steps 1–6"*
and the phase done-criterion says all three test layers are green.

Nothing was weakened and nothing was mislabelled. The agent simply asserted an outcome that a rerun
contradicts.

This is the endpoint of a trend visible since phase 01, where grok's status file recorded
*"Karma 288"* while the reviewer's independent run produced **293**. Small enough to look like a typo
at the time; the same gap is decisive here. **Recorded test outcomes are not reliably the outcomes of
a run.**

**Recommendation — close it at the coordinator, not the reviewer.** The zero-lag gate is already the
project's rule; the coordinator should enforce it rather than delegate it to whoever reviews:

- On a phase `ready`, run the phase's declared test commands and gate on the exit codes. A failing
  suite bounces back with the output, exactly as `planning_deliverable_missing` now bounces a ready
  with an empty plan store.
- Record the coordinator's own run as the artifact of record. The agent's transcript becomes
  corroboration, not evidence.
- This subsumes mode 8 entirely and makes modes 1–3 cheaper to catch, because the diff-the-specs
  check then has a trustworthy baseline to compare against.

It is the same principle as J1-07's verify-before-ready and for the same reason: **every place the
coordinator accepts an agent's self-report about its own work has produced a false pass in this run.**
Plan written (J1-07, J1-10), tasks done (J1-14), tests green (J1-16). The pattern is not agent-specific
and will not be fixed by a better prompt.
