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
