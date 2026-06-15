use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SCHEMA: &str = "johnnyone-planner-prompts/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerPromptSettings {
    #[serde(default = "default_schema")]
    pub schema: String,
    pub development: PlannerDevelopmentPrompts,
    pub planning: PlannerPlanningPrompts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerDevelopmentPrompts {
    pub worker: String,
    pub reviewer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerPlanningPrompts {
    pub planner: String,
    pub reviewer: String,
    /// Variant of `planner` used when an amendment is in flight. Instead of
    /// scaffolding a plan from scratch, T1 reads the existing plan + the
    /// amendment brief and edits files in place. `serde(default)` so old
    /// `~/.johnnyone/planner-prompts.yml` files (pre-amend) still parse.
    #[serde(default = "default_amend_planning_planner")]
    pub amend_planner: String,
    /// Variant of `reviewer` used during amendment review. Pulls a
    /// `{{git_diff}}` into the prompt so T2 focuses on what changed since
    /// the last approved commit rather than re-reviewing the whole plan.
    #[serde(default = "default_amend_planning_reviewer")]
    pub amend_reviewer: String,
}

impl Default for PlannerPromptSettings {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_string(),
            development: PlannerDevelopmentPrompts {
                worker: DEFAULT_DEVELOPMENT_WORKER.to_string(),
                reviewer: DEFAULT_DEVELOPMENT_REVIEWER.to_string(),
            },
            planning: PlannerPlanningPrompts {
                planner: DEFAULT_PLANNING_PLANNER.to_string(),
                reviewer: DEFAULT_PLANNING_REVIEWER.to_string(),
                amend_planner: DEFAULT_AMEND_PLANNING_PLANNER.to_string(),
                amend_reviewer: DEFAULT_AMEND_PLANNING_REVIEWER.to_string(),
            },
        }
    }
}

fn default_amend_planning_planner() -> String {
    DEFAULT_AMEND_PLANNING_PLANNER.to_string()
}

fn default_amend_planning_reviewer() -> String {
    DEFAULT_AMEND_PLANNING_REVIEWER.to_string()
}

fn default_schema() -> String {
    SCHEMA.to_string()
}

pub fn load_prompt_settings() -> Result<PlannerPromptSettings, String> {
    let path = prompt_settings_path()?;
    if !path.exists() {
        let settings = PlannerPromptSettings::default();
        save_prompt_settings(settings.clone())?;
        return Ok(settings);
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    if raw.trim().is_empty() {
        return Ok(PlannerPromptSettings::default());
    }

    let mut settings: PlannerPromptSettings = serde_yaml::from_str(&raw)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
    if settings.schema.trim().is_empty() {
        settings.schema = SCHEMA.to_string();
    }
    Ok(settings)
}

pub fn save_prompt_settings(
    settings: PlannerPromptSettings,
) -> Result<PlannerPromptSettings, String> {
    let path = prompt_settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let mut normalized = settings;
    normalized.schema = SCHEMA.to_string();
    let raw = serde_yaml::to_string(&normalized)
        .map_err(|e| format!("Failed to serialize planner prompts: {}", e))?;
    fs::write(&path, raw).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(normalized)
}

pub fn prompt_settings_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
    Ok(home.join(".johnnyone").join("planner-prompts.yml"))
}

pub fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in values {
        rendered = rendered.replace(&format!("{{{{{}}}}}", key), value);
    }
    rendered
}

const DEFAULT_DEVELOPMENT_WORKER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
JOHNNYONE_PHASE_ID: {{phase_id}}
ROLE: T1_WORKER

Workspace: {{workspace_path}}
Plan: {{plan_path}}
Current phase: {{phase_id}}
Phase path: {{phase_path}}
Tasks path: {{tasks_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

Read methodology, project conventions (every file under {{conventions_path}}), plan overview, phase overview, discoveries, and current-phase task files directly from those paths on the host. Work only on this phase. Do not start later phases. Update task status/decisions/artifacts as you work.

This phase will be reviewed against the three lenses in {{conventions_path}}/review-lenses.md, so prepare the evidence each needs:
- Product (no code): for every screen in the plan's screens-to-verify inventory, capture a final screenshot of the delivered screen so it can be matched against the agreed mock.
- QA (behavioral): make sure each acceptance criterion is actually verifiable, every feature is really wired (not stubbed), and the phase's tests were run green with output captured.
- Lead (structural): reuse shared widgets/services instead of forking them, follow conventions (record any deviation in decisions.md), keep tenancy/authorization scoping and input/path validation intact, and don't leak secrets or creep out of scope.

When the whole phase is ready for review, say READY_FOR_T2_VALIDATION."#;

const DEFAULT_DEVELOPMENT_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
JOHNNYONE_PHASE_ID: {{phase_id}}
ROLE: T2_REVIEWER

Workspace: {{workspace_path}}
Plan: {{plan_path}}
Current phase: {{phase_id}}
Phase path: {{phase_path}}
Tasks path: {{tasks_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

Read methodology, project conventions (every file under {{conventions_path}}), app-local conventions if present under {{workspace_path}}/conventions or {{workspace_path}}/plans/conventions, plan overview, phase overview, discoveries, and current-phase task files/artifacts directly from those paths on the host. Validate only; do not implement app changes. You may update plan status artifacts only.

Run all THREE review lenses defined in {{conventions_path}}/review-lenses.md (development-review checklists) and merge them into one verdict:
- Product (no code): screenshot the delivered screens and match them against the agreed mocks + the plan's screens-to-verify inventory at the level of intent (layout, content, states, affordances), not pixels.
- QA (behavioral): prove/disprove each acceptance criterion against the actual code and running app with file:line + observed behavior; confirm every mocked feature is really wired (not stubbed); validate independently — do not just rerun T1's own e2e.
- Lead (structural): code quality, security (authorization/tenancy scoping, secrets, input/path validation, least privilege), and maintainability (reuse over forking shared widgets, convention compliance, no out-of-scope creep).
PASS only when all three lenses pass. If any fails, return NEEDS_CHANGES with findings labeled by lens (`[Product]`/`[QA]`/`[Lead]`). A Product failure caused by a missing mock or screens-to-verify inventory is a planning gap.

If the phase changes user-facing UI, verify that T1 produced or referenced the required mocks/screenshots/artifacts, that they cover the relevant responsive states, and that the result still matches the current app source. Missing UI evidence is NEEDS_CHANGES.

Before returning, append your phase validation result to {{phase_path}}/status.md under `## Phase Validation`. Include verdict, summary, findings, next steps, and any screenshot/test read-back notes. If {{phase_path}}/status.yml exists, update its validation fields consistently with your verdict. Do not edit application source files.

Verdict rules:
- PASS only when the phase acceptance criteria are satisfied.
- NEEDS_CHANGES when T1 can continue work, add evidence, fix issues, document a waiver/scope decision, or explain why the current evidence satisfies acceptance. Missing or insufficient validation evidence is NEEDS_CHANGES, not BLOCKED.
- BLOCKED only when T2 cannot perform validation because of an external access/runtime problem, such as missing workspace/plan paths, unreadable files, unavailable credentials, unavailable required tools, or a broken validation environment. If you can state what T1 must do to pass, use NEEDS_CHANGES.
- If verdict is NEEDS_CHANGES or BLOCKED, FINDINGS must contain at least one concrete reason and NEXT_STEPS must contain at least one concrete action for T1. Use `none` only when verdict is PASS and there is truly nothing to add.

Return this footer exactly:

PHASE: {{phase_id}}
VERDICT: PASS | NEEDS_CHANGES | BLOCKED
SUMMARY: <one paragraph>
FINDINGS:
- <finding or none>
NEXT_STEPS:
- <step for T1 or none>"#;

const DEFAULT_PLANNING_PLANNER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T1_PLANNER

Workspace root: {{workspace_path}}
App/source scope: {{app_scope}}
Docs scope: {{docs_scope}}
Plan output path: {{plan_output_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

User brief:
{{user_brief}}

Reference paths:
{{reference_paths}}

Create or update a methodology-compliant plan at the plan output path. Read the methodology first, then read every file under the conventions path and any app-local conventions under the app/source scope so the plan honors project conventions (UI, backend, testing, tooling, plan-validation, etc.). Inspect the app/source scope and docs scope enough to ground the plan in the real codebase. Use Mermaid for diagrams and HTML+JS mocks when UI/layout is unclear.

For UI-related work, include concrete mocks or visual references unless the UI impact is trivial and documented in a decision. The plan must name the existing routes/components/widgets/styles/tests it expects to touch so T2 can verify the plan is synced with the current source code. Include local and live validation strategy when the feature must be testable by the user. Do not implement application code. Only create/update plan files.

The plan will be reviewed against the three lenses in {{conventions_path}}/review-lenses.md, so make it reviewable:
- Give every phase/task EXPLICIT, TESTABLE acceptance criteria + a definition-of-done (observable; never "works"/"looks good"). [enables the QA lens]
- For user-facing work, include a "screens to verify" inventory: each screen/flow → its mock/visual reference → how to navigate to it → its acceptance. [enables the Product lens]
- Prefer reusing existing shared components/widgets over new or forked ones, and note foreseeable security/maintainability concerns. [enables the Lead lens]

When the plan is ready for review, say exactly:

READY_FOR_T2_PLAN_REVIEW"#;

const DEFAULT_PLANNING_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T2_PLAN_REVIEWER

Workspace root: {{workspace_path}}
App/source scope: {{app_scope}}
Docs scope: {{docs_scope}}
Plan path: {{plan_output_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

Validate the plan only. Do not implement application code. Read methodology, every convention file under the conventions path, app-local conventions if present under the app/source scope, the created plan, relevant docs, source scope, artifacts, mocks, and diagrams.

Review for methodology + convention compliance, clear phase boundaries, actionable task prompts, explicit acceptance criteria, local/live testing strategy, UI mocks/diagrams where needed, risky assumptions, and whether the plan can produce something the user can test.

Run all THREE review lenses defined in {{conventions_path}}/review-lenses.md (planning-review checklists) and merge them into one verdict:
- Product: the plan has mocks/visual references for every user-facing change AND a "screens to verify" inventory (each screen → its mock → how to navigate to it → its acceptance).
- QA: every phase/task has explicit, TESTABLE acceptance criteria + definition-of-done, and a runnable validation strategy; the plan is not narrower than the original intent.
- Lead: the approach/architecture is technically sound and decomposed sensibly, reuses existing components rather than reinventing, and acknowledges foreseeable security/maintainability concerns.
PASS only when all three pass; otherwise NEEDS_CHANGES with findings labeled `[Product]`/`[QA]`/`[Lead]`.

Before PASS, perform these gates:
- UI mock gate: if the brief or plan changes user-facing UI, verify mocks/screenshots/wireframes or explicit existing-screen references exist and are useful enough for implementation. Missing or superficial UI mocks are NEEDS_CHANGES unless a decision explains why the visual impact is trivial.
- Source-sync gate: inspect the current app/source scope enough to confirm the plan matches the real routes, components, shared widgets, styling conventions, backend/schema boundaries, and test setup. If the plan is not synced with the current source code, return NEEDS_CHANGES.
- If verdict is NEEDS_CHANGES or BLOCKED, FINDINGS must contain at least one concrete reason and NEXT_STEPS must contain at least one concrete action for T1. Use `none` only when verdict is PASS and there is truly nothing to add.

Return this footer exactly:

PLAN: {{plan_output_path}}
VERDICT: PASS | NEEDS_CHANGES | BLOCKED
SUMMARY: <one paragraph>
FINDINGS:
- <finding or none>
NEXT_STEPS:
- <step for T1 or none>"#;

// === Amend variants (Slice 2 / 2026-05-25) ============================
// Used when the user clicks "Amend" on an approved planning run. T1 reads
// the existing plan + applies the amendment in place; T2 reviews the diff
// rather than re-validating from scratch.

const DEFAULT_AMEND_PLANNING_PLANNER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T1_PLANNER (AMEND MODE)

Workspace root: {{workspace_path}}
App/source scope: {{app_scope}}
Docs scope: {{docs_scope}}
Existing plan at: {{plan_output_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

User's amendment brief:
{{amendment_brief}}

Original brief (for context, do not re-implement it from scratch):
{{user_brief}}

Reference paths:
{{reference_paths}}

A plan already lives at {{plan_output_path}}. Read it first (overview.md, status.md, phases/, decisions/, artifacts/), along with the methodology, project conventions, and app-local conventions if present under the app/source scope. The plan also has a per-plan git repo at {{plan_output_path}}/.git — run `git log --oneline` and `git diff` against earlier commits if you want context on past decisions.

Apply the user's amendment by editing files IN PLACE. Add new phases or tasks where the amendment requires them. Update overview.md and status.md to reflect the change. Preserve completed phase status entries unless the amendment explicitly invalidates them. If the amendment changes user-facing UI, update or add mocks/visual references and name the affected source files/components. Do NOT recreate the plan from scratch — that throws away all prior approval history.

When the amendment is applied and the plan is ready for review, say exactly:

READY_FOR_T2_PLAN_REVIEW"#;

const DEFAULT_AMEND_PLANNING_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T2_PLAN_REVIEWER (AMEND MODE)

Workspace root: {{workspace_path}}
App/source scope: {{app_scope}}
Docs scope: {{docs_scope}}
Plan path: {{plan_output_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

User's amendment brief:
{{amendment_brief}}

T1 modified files in place. The diff against the previously-approved commit (`git diff HEAD` in the plan repo) is below. Focus your review on these changes — you do not need to re-validate sections that haven't been touched.

--- BEGIN DIFF ---
{{git_diff}}
--- END DIFF ---

Validate:
1. The diff implements the amendment brief correctly.
2. The amendment doesn't break methodology compliance (phase structure, acceptance criteria, plan validation strategy).
3. New phases/tasks follow the project's conventions (read every file under {{conventions_path}}).
4. Existing approved sections that the amendment SHOULD have touched have been updated (e.g. overview.md if new phases are added, status.md if scope changed).
5. Diagrams + mocks were updated where the amendment changes UI/architecture.
6. UI amendments still match the current app/source scope. Missing or stale UI mocks, or a plan that no longer matches the app, is NEEDS_CHANGES.
7. Run the three review lenses (planning-review checklists) from {{conventions_path}}/review-lenses.md against the amended plan and merge into one verdict: Product (the amendment keeps/adds the needed mocks + screens-to-verify inventory), QA (new/changed phases have explicit, testable acceptance criteria), Lead (the amended approach is sound, reuses existing components, and acknowledges security/maintainability concerns). Label findings by lens (`[Product]`/`[QA]`/`[Lead]`).
8. If verdict is NEEDS_CHANGES or BLOCKED, FINDINGS must contain at least one concrete reason and NEXT_STEPS must contain at least one concrete action for T1. Use `none` only when verdict is PASS and there is truly nothing to add.

Return this footer exactly:

PLAN: {{plan_output_path}}
VERDICT: PASS | NEEDS_CHANGES | BLOCKED
SUMMARY: <one paragraph>
FINDINGS:
- <finding or none>
NEXT_STEPS:
- <step for T1 or none>"#;
