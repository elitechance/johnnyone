//! Creation and execution must agree about who builds and who validates.
//!
//! THE DEFECT THIS PINS, measured on the live run.
//!
//! The kart plan `e37cd849-1d8a-4a31-9023-3a8ccaf97c7d` reached `approved` at
//! 14:02:40 on 2026-10-24 — a real glm review, `PASS_WITH_FOLLOWUPS`. The next
//! event on that plan is:
//!
//! ```text
//! development_autostart_failed
//!   {"reason":"Could not auto-start development from the approved plan:
//!     Provider 'kloo' is a oneshot executor, not a chat provider."}
//! ```
//!
//! Its columns are `dev_worker_provider = 'kloo'`, `dev_reviewer_provider =
//! 'kloo'`. `create_plan` puts `worker_provider` and `reviewer_provider`
//! through `reject_oneshot_plan_provider` (`agent_plans.rs:347-349`) and puts
//! the `dev_*` columns through `normalize_stage_provider`, which only trims
//! whitespace. So the write that cannot run was accepted without complaint, and
//! `auto_start_development` — which feeds exactly those columns back in as the
//! live providers via `development_stage_providers` — refused them hours later,
//! on the one transition a human was waiting for.
//!
//! WHY THE ROW CANNOT BE REPAIRED. Of the plan mutations in
//! `worker/schema/johnnyone-ai.graphql`, none writes `dev_worker_provider` or
//! `dev_reviewer_provider`; `devWorkerProvider` appears only on the AgentPlan
//! type and on the two *create* inputs. `CreateAgentPlanInput` also has no
//! `executorConfig`, so a development run made directly through
//! `createAgentPlan` can never be local-small. Autostart from an approved
//! planning row is therefore the ONLY route to a local-small development run —
//! and a bad `dev_*` column makes an approved plan permanently unable to take
//! it. That is what moved this onto the critical path: not tidiness, a dead end.
//!
//! WHY THE PROVIDER NAME WAS WRONG IN THE FIRST PLACE. kloo-mode is not a
//! provider choice. `task_loop::phase_is_kloo_mode` decides it per phase, from
//! the filesystem — a phase is kloo-mode iff some `tasks/<id>/task.yml` exists
//! ("Filesystem gate only — not the plan's worker provider id", task_loop.rs:97)
//! — and kart's `01-foundation` has four of them. So kloo builds that phase
//! whatever the column says, and a column naming kloo is always a mistake,
//! never an expression of intent.
//!
//! WHAT `d6` IS FOR. The lesson of the last four defects on these branches is
//! that proving a layer says nothing about the layer past it. A creation check
//! that rejects `kloo` and a runtime that still refuses something creation
//! accepted would be the same bug with a smaller blast radius. So `d6` asserts
//! the invariant itself, over the real functions: for a planning row that
//! creation ACCEPTED, `create_plan` must accept the pair that
//! `development_stage_providers` derives from it. Creation and execution agree,
//! or `d6` is red.

use johnnyone_desktop_lib::db::models::CreateAgentPlanInput;
use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::state::app_state::AppState;

fn state_in(tag: &str) -> (AppState, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("devprov-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    let db = Database::open(&root.join("test.db")).expect("open db");
    (AppState::new(db), root)
}

/// A workspace holding a methodology-shaped plan: `overview.md` plus one phase
/// directory, which is what `parse_plan` requires of a development create.
fn workspace_with_plan(root: &std::path::Path) -> (String, String) {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("plan/phases/01-foundation")).expect("plan dirs");
    std::fs::write(ws.join("plan/overview.md"), "# Plan\n\n## Phases\n").expect("overview");
    std::fs::write(
        ws.join("plan/phases/01-foundation/overview.md"),
        "# 01 foundation\n",
    )
    .expect("phase overview");
    (
        ws.to_string_lossy().to_string(),
        ws.join("plan").to_string_lossy().to_string(),
    )
}

fn planning_input(ws: &str, plan: &str, dev_worker: Option<&str>, dev_reviewer: Option<&str>)
    -> CreateAgentPlanInput
{
    CreateAgentPlanInput {
        run_type: Some("planning".to_string()),
        title: Some("kart - the visual identity pass".to_string()),
        workspace_path: ws.to_string(),
        plan_path: plan.to_string(),
        worker_provider: "claude".to_string(),
        reviewer_provider: "claude".to_string(),
        brief: Some("a professional ecommerce shop".to_string()),
        app_scope: None,
        docs_scope: None,
        reference_paths: None,
        worker_setup_commands: None,
        reviewer_setup_commands: None,
        dev_worker_provider: dev_worker.map(str::to_string),
        dev_reviewer_provider: dev_reviewer.map(str::to_string),
    }
}

/// d1 — the exact live write must be refused at creation.
///
/// `dev_worker_provider = 'kloo'` on a planning create is the column that cost
/// the kart plan its handoff. Creation accepting it is the defect.
#[test]
fn d1_creation_refuses_a_oneshot_dev_worker() {
    let (state, root) = state_in("d1");
    let (ws, plan) = workspace_with_plan(&root);

    let err = agent_plans::create_plan(&state, planning_input(&ws, &plan, Some("kloo"), None))
        .err()
        .expect(
            "creation accepted dev_worker_provider='kloo'. This is the live defect: the kart \
             plan carried exactly this and `auto_start_development` refused it at 14:02:40 on \
             2026-10-24, after planning review had already passed.",
        );

    assert!(
        err.contains("kloo"),
        "the refusal must name the provider the caller actually sent, or the console \
         cannot say which field is wrong: {err}"
    );
    assert!(
        err.contains("oneshot"),
        "the refusal must say WHY kloo cannot be a development provider, in the same words \
         the run path uses ('is a oneshot executor, not a chat provider'); a bare \
         'invalid provider' sends the user looking for a typo: {err}"
    );
}

/// d2 — and the validator half, and `shell` as well as `kloo`.
///
/// `reject_oneshot_plan_provider` refuses both names. A creation check that
/// learned only the one name from the live failure would let `shell` through and
/// the next approved plan would die the same way.
#[test]
fn d2_creation_refuses_every_oneshot_name_on_either_side() {
    let (state, root) = state_in("d2");
    let (ws, plan) = workspace_with_plan(&root);

    for name in ["kloo", "shell"] {
        for (worker, reviewer) in [(Some(name), None), (None, Some(name))] {
            let got = agent_plans::create_plan(
                &state,
                planning_input(&ws, &plan, worker, reviewer),
            );
            assert!(
                got.is_err(),
                "creation accepted a oneshot provider {name:?} as \
                 dev_worker={worker:?} dev_reviewer={reviewer:?}. Both columns are fed back \
                 to create_plan by development_stage_providers, so both sides are the same \
                 dead end."
            );
        }
    }
}

/// d3 — one rule, not two copies of it.
///
/// The failure mode this forecloses: a second, hand-written check beside
/// `reject_oneshot_plan_provider` that drifts from it. So this does not inspect
/// the implementation — it compares behaviour. For every provider name, whether
/// creation accepts it in `dev_worker_provider` must match whether it accepts
/// it in `worker_provider`. A divergence in either direction is red: a dev
/// check stricter than the run path rejects configurations that would have run,
/// and a looser one re-opens the live defect for whichever name it missed.
///
/// Unknown names are in the list deliberately: `reject_oneshot_plan_provider`
/// lets them through (`_ => Ok(())`), so a dev check that validates against a
/// hard-coded allowlist of known providers fails here rather than in six weeks.
#[test]
fn d3_the_dev_columns_obey_the_same_rule_as_the_run_providers() {
    let (state, root) = state_in("d3");
    let (ws, plan) = workspace_with_plan(&root);

    for name in [
        "kloo", "shell", "claude", "codex", "cline", "ollama", "grok", "lokalai", "no-such-agent",
    ] {
        let as_run_provider = {
            let mut input = planning_input(&ws, &plan, None, None);
            input.worker_provider = name.to_string();
            agent_plans::create_plan(&state, input).is_ok()
        };
        let as_dev_provider =
            agent_plans::create_plan(&state, planning_input(&ws, &plan, Some(name), None)).is_ok();

        assert_eq!(
            as_dev_provider, as_run_provider,
            "provider {name:?} is treated differently in dev_worker_provider \
             (accepted={as_dev_provider}) than in worker_provider \
             (accepted={as_run_provider}). The two must be the same rule applied twice, \
             not two rules that will drift."
        );
    }
}

/// d4 — the check must not be satisfied by refusing everything.
///
/// An absence assertion is satisfied by nothing at all (the 2026-10-23 lesson),
/// and the cheapest way to make d1–d3 green is to reject every `dev_*` value.
/// So this demands the working configuration still works, and that the stored
/// columns are the ones the caller sent.
#[test]
fn d4_a_valid_dev_pair_is_still_accepted_and_round_trips() {
    let (state, root) = state_in("d4");
    let (ws, plan) = workspace_with_plan(&root);

    let run = agent_plans::create_plan(&state, planning_input(&ws, &plan, Some("grok"), Some("claude")))
        .expect("a valid chat-provider pair must still create a planning run");

    assert_eq!(
        run.plan.dev_worker_provider.as_deref(),
        Some("grok"),
        "the builder binding did not survive creation"
    );
    assert_eq!(
        run.plan.dev_reviewer_provider.as_deref(),
        Some("claude"),
        "the validator binding did not survive creation"
    );

    let blank = agent_plans::create_plan(&state, planning_input(&ws, &plan, Some("  "), None))
        .expect("a blank field means 'unbound' and must still create");
    assert!(
        blank.plan.dev_worker_provider.is_none(),
        "an empty form field became a provider named {:?} instead of SQL NULL",
        blank.plan.dev_worker_provider
    );
}

/// d5 — an existing row must be repairable.
///
/// Validation at creation does nothing for the plan that is already wrong, and
/// the live kart plan is already wrong. No mutation writes these columns, so
/// today the only repair is a hand-written UPDATE against the running host's
/// database. `rebind_development_providers` is that missing repair, and it obeys
/// the same rule: it refuses a oneshot name rather than writing a second dead
/// end, and it refuses on a development row, which has nothing to hand off.
#[test]
fn d5_an_existing_planning_row_can_be_rebound_under_the_same_rule() {
    let (state, root) = state_in("d5");
    let (ws, plan) = workspace_with_plan(&root);

    let run = agent_plans::create_plan(&state, planning_input(&ws, &plan, Some("grok"), Some("grok")))
        .expect("seed a planning run");
    let id = run.plan.id.clone();

    let rebound = agent_plans::rebind_development_providers(
        &state,
        id.clone(),
        Some("claude".to_string()),
        Some("claude".to_string()),
    )
    .expect("rebinding a planning row to a valid chat pair must succeed");
    assert_eq!(rebound.plan.dev_worker_provider.as_deref(), Some("claude"));
    assert_eq!(rebound.plan.dev_reviewer_provider.as_deref(), Some("claude"));

    let err = agent_plans::rebind_development_providers(
        &state,
        id.clone(),
        Some("kloo".to_string()),
        None,
    )
    .err()
    .expect("the repair must not be a way around the rule it exists to repair");
    assert!(
        err.contains("kloo") && err.contains("oneshot"),
        "the rebind refusal must read like the creation refusal: {err}"
    );

    // Clearing is how a row goes back to inheriting the planning providers.
    let cleared = agent_plans::rebind_development_providers(&state, id.clone(), None, None)
        .expect("clearing both bindings must be allowed");
    assert!(
        cleared.plan.dev_worker_provider.is_none()
            && cleared.plan.dev_reviewer_provider.is_none(),
        "clearing left {:?}/{:?} behind",
        cleared.plan.dev_worker_provider,
        cleared.plan.dev_reviewer_provider
    );

    let dev = agent_plans::create_plan(&state, {
        let mut input = planning_input(&ws, &plan, None, None);
        input.run_type = Some("development".to_string());
        input
    })
    .expect("seed a development run");
    let wrong_stage = agent_plans::rebind_development_providers(
        &state,
        dev.plan.id.clone(),
        Some("claude".to_string()),
        None,
    );
    assert!(
        wrong_stage.is_err(),
        "a development row was allowed to carry a development-stage binding it will never \
         read; auto_start_development sets both columns NULL on the row it creates because \
         a development run is terminal in the stage chain."
    );
}

/// d6 — THE INVARIANT. Creation and execution must not disagree.
///
/// This is the assertion the live failure actually violated, stated over the
/// real functions rather than over either side's wording. `create_plan` accepted
/// the planning row; `development_stage_providers` derives the development pair
/// from exactly that row; `create_plan` must then accept that pair. If this is
/// green and d1 is red, nothing has been fixed — and if a future change adds a
/// third provider column, this is the test that notices.
#[test]
fn d6_a_row_creation_accepted_can_always_start_development() {
    let (state, root) = state_in("d6");
    let (ws, plan) = workspace_with_plan(&root);

    // Every name creation might accept, bound both ways round, plus the
    // inherit-from-planning case that pre-migration-021 initiatives use.
    let candidates = [
        None,
        Some("claude"),
        Some("grok"),
        Some("codex"),
        Some("kloo"),
        Some("shell"),
        Some("  "),
    ];

    for worker in candidates {
        for reviewer in candidates {
            let created =
                match agent_plans::create_plan(&state, planning_input(&ws, &plan, worker, reviewer)) {
                    // Creation refused it: nothing downstream can be surprised.
                    Err(_) => continue,
                    Ok(run) => run,
                };

            let (dev_worker, dev_reviewer) =
                agent_plans::development_stage_providers(&created.plan);

            let mut dev = planning_input(&ws, &plan, None, None);
            dev.run_type = Some("development".to_string());
            dev.worker_provider = dev_worker.clone();
            dev.reviewer_provider = dev_reviewer.clone();

            if let Err(err) = agent_plans::create_plan(&state, dev) {
                panic!(
                    "creation ACCEPTED a planning row bound to dev_worker={worker:?} \
                     dev_reviewer={reviewer:?}, and the development stage then refused the \
                     pair it derives from that same row ({dev_worker:?}, {dev_reviewer:?}): \
                     {err}\n\nThis is the kart failure exactly: planning review passed at \
                     14:02:40 on 2026-10-24 and the handoff died on a configuration the \
                     write path had already blessed."
                );
            }
        }
    }
}
