use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SCHEMA: &str = "johnnyone-planner-prompts/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerPromptSettings {
    #[serde(default = "default_schema")]
    pub schema: String,
    pub development: PlannerDevelopmentPrompts,
    pub planning: PlannerPlanningPrompts,
    #[serde(default)]
    pub small_mode: SmallModePrompts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmallModePrompts {
    #[serde(default = "default_small_mode_planner")]
    pub planner: String,
    #[serde(default = "default_small_mode_reviewer")]
    pub reviewer: String,
    #[serde(default = "default_small_mode_leaf_wrapper")]
    pub leaf_wrapper: String,
    #[serde(default = "default_small_mode_amend_planner")]
    pub amend_planner: String,
}

impl Default for SmallModePrompts {
    fn default() -> Self {
        Self {
            planner: DEFAULT_SMALL_MODE_PLANNER.to_string(),
            reviewer: DEFAULT_SMALL_MODE_REVIEWER.to_string(),
            leaf_wrapper: DEFAULT_SMALL_MODE_LEAF_WRAPPER.to_string(),
            amend_planner: DEFAULT_SMALL_MODE_AMEND_PLANNER.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerDevelopmentPrompts {
    pub worker: String,
    pub reviewer: String,
    /// Operator-authored persist-only key. No shipped default. Absent stays
    /// absent; present is preserved verbatim (`skip_serializing_if`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_nudge: Option<String>,
}

/// RPC / host overlay input. Additive `worker_nudge` is `Option`: `None` (key
/// omitted) keeps `current`; `Some` writes. Do **not** deserialize this into
/// `PlannerDevelopmentPrompts` as a substitute — that type is the stored shape.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerDevelopmentOverlay {
    pub worker: String,
    pub reviewer: String,
    #[serde(default)]
    pub worker_nudge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// RPC / host overlay input. Additive amend fields are `Option`: `None` keeps
/// `current` (no `DEFAULT_AMEND_*` fill). Stored YAML may still serde-default
/// those keys onto `PlannerPlanningPrompts`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerPlanningOverlay {
    pub planner: String,
    pub reviewer: String,
    #[serde(default)]
    pub amend_planner: Option<String>,
    #[serde(default)]
    pub amend_reviewer: Option<String>,
}

impl Default for PlannerPromptSettings {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_string(),
            development: PlannerDevelopmentPrompts {
                worker: DEFAULT_DEVELOPMENT_WORKER.to_string(),
                reviewer: DEFAULT_DEVELOPMENT_REVIEWER.to_string(),
                worker_nudge: None,
            },
            planning: PlannerPlanningPrompts {
                planner: DEFAULT_PLANNING_PLANNER.to_string(),
                reviewer: DEFAULT_PLANNING_REVIEWER.to_string(),
                amend_planner: DEFAULT_AMEND_PLANNING_PLANNER.to_string(),
                amend_reviewer: DEFAULT_AMEND_PLANNING_REVIEWER.to_string(),
            },
            small_mode: SmallModePrompts::default(),
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

fn default_small_mode_planner() -> String {
    DEFAULT_SMALL_MODE_PLANNER.to_string()
}

fn default_small_mode_reviewer() -> String {
    DEFAULT_SMALL_MODE_REVIEWER.to_string()
}

fn default_small_mode_leaf_wrapper() -> String {
    DEFAULT_SMALL_MODE_LEAF_WRAPPER.to_string()
}

fn default_small_mode_amend_planner() -> String {
    DEFAULT_SMALL_MODE_AMEND_PLANNER.to_string()
}

/// Built-in plan-check constraint. The default `smallMode.reviewer` contains
/// this sentence; `lens_preamble_from_reviewer` extracts it (or a tuned
/// replacement) without the role / Judge-only / footer blocks.
pub(crate) const PLAN_CHECK_ALREADY_VALIDATED: &str = "The plan-check script has already validated shape, files, DAG, scoped verify, must_contain, bounds, and the UI-task rule. Do not re-count files or re-check those rules.";

/// Overlay a settings update onto `current`.
///
/// Required fields (`worker`, `reviewer`, `planning.planner`, `planning.reviewer`)
/// always write. Additive fields (`worker_nudge`, `amend_planner`, `amend_reviewer`)
/// are `Option` on the input: `Some(s)` writes, `None` keeps `current.*`.
/// `small_mode: None` keeps the already-saved smallMode (do not replace with
/// built-in defaults).
pub fn overlay_prompt_settings(
    mut current: PlannerPromptSettings,
    development: PlannerDevelopmentOverlay,
    planning: PlannerPlanningOverlay,
    small_mode: Option<SmallModePrompts>,
) -> PlannerPromptSettings {
    current.development.worker = development.worker;
    current.development.reviewer = development.reviewer;
    if let Some(nudge) = development.worker_nudge {
        current.development.worker_nudge = Some(nudge);
    }
    current.planning.planner = planning.planner;
    current.planning.reviewer = planning.reviewer;
    if let Some(amend_planner) = planning.amend_planner {
        current.planning.amend_planner = amend_planner;
    }
    if let Some(amend_reviewer) = planning.amend_reviewer {
        current.planning.amend_reviewer = amend_reviewer;
    }
    if let Some(sm) = small_mode {
        current.small_mode = sm;
    }
    current
}

/// Strip role header, "Judge only" scope, and PLAN/VERDICT footer so a
/// per-lens prompt can take a tunable preamble from `smallMode.reviewer`
/// without inheriting that template's dimension or report protocol.
pub(crate) fn lens_preamble_from_reviewer(reviewer: &str) -> String {
    let mut text = reviewer;
    if let Some(i) = reviewer.find("Return this footer exactly") {
        text = &reviewer[..i];
    }
    if let Some(i) = text.find("Judge only:") {
        text = &text[..i];
    }
    let mut kept = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("JOHNNYONE_")
            || t.starts_with("ROLE:")
            || t.starts_with("Workspace")
            || t.starts_with("App/source")
            || t.starts_with("Docs ")
            || t.starts_with("Plan path:")
            || t.starts_with("Plan output")
            || t.starts_with("Methodology:")
            || t.starts_with("Conventions:")
        {
            continue;
        }
        kept.push(t);
    }
    let out = kept.join("\n");
    if out.trim().is_empty() {
        PLAN_CHECK_ALREADY_VALIDATED.to_string()
    } else {
        out
    }
}

/// Prepend a tunable leaf wrapper to `prompt.md` in memory. Empty wrapper is a no-op.
pub fn wrap_leaf_prompt(wrapper: &str, prompt: &str) -> String {
    let w = wrapper.trim();
    if w.is_empty() {
        prompt.to_string()
    } else {
        format!("{w}\n\n{prompt}")
    }
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

pub(crate) const DEFAULT_DEVELOPMENT_WORKER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
JOHNNYONE_PHASE_ID: {{phase_id}}
ROLE: T1_WORKER

Workspace: {{workspace_path}}
Plan: {{plan_path}}
Current phase: {{phase_id}}
Phase path: {{phase_path}}
Tasks path: {{tasks_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

Read methodology, project conventions (every file under {{conventions_path}}), plan overview, phase overview, discoveries, and current-phase task files directly from those paths on the host. Work only on this phase. Do not start later phases. Update task status/decisions/artifacts as you work. The task and phase files under {{phase_path}} are the spec you are being measured against, not working notes — do not edit or delete them. If a requirement is wrong or impossible, record that in the task's decisions file and leave the requirement standing, so the change is a decision someone made rather than one that happened quietly; the coordinator rejects a ready report that removed spec lines.

This phase will be reviewed against the three lenses in {{conventions_path}}/review-lenses.md, so prepare the evidence each needs:
- Product (no code): for every screen in the plan's screens-to-verify inventory, capture a final screenshot of the delivered screen so it can be matched against the agreed mock.
  A screenshot is the one claim the reviewer cannot re-derive, so each one has to carry its own provenance: name the file in the task's decisions or status file and say what it shows — the route or screen it was taken on and the state it is in. Capture a distinct image per claim; the coordinator rejects a ready report whose images are byte-identical to each other or that nothing describes.
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

pub(crate) const DEFAULT_PLANNING_PLANNER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
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
- Give every phase/task EXPLICIT, TESTABLE acceptance criteria + a definition-of-done under an `## Acceptance` (or `## Definition of done`) heading. Every criterion must name the thing that decides it, in backticks: the command to run, the route and element to look at, the file:line to read. "The aux layer traps focus" is not a criterion; "`npm run e2e -- aux-focus` passes" and "on `/patients/:id` with the aux layer open, Tab from the last control reaches `.aux-skip-link`" are. A criterion nobody can run is one the worker cannot finish and the reviewer can only agree with, which is where a false pass starts — the coordinator rejects a ready report whose criteria do not name their check. [enables the QA lens]
- For user-facing work, include a "screens to verify" inventory: each screen/flow → its mock/visual reference → how to navigate to it → its acceptance. [enables the Product lens]
- Prefer reusing existing shared components/widgets over new or forked ones, and note foreseeable security/maintainability concerns. [enables the Lead lens]

When the plan is ready for review, say exactly:

READY_FOR_T2_PLAN_REVIEW"#;

pub(crate) const DEFAULT_PLANNING_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
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

/// Shared with `DEFAULT_SMALL_MODE_PLANNER` so the 3-round amend loop
/// does not burn a round rediscovering the checker's grammar.
macro_rules! small_mode_task_constraints {
    () => {
        r#"Each task dir must contain task.yml + prompt.md. Write them at exactly phases/<phase-id>/tasks/<task-id>/ — the tasks/ segment is required. Plan-check only walks that path; a task.yml at phases/<phase-id>/<task-id>/ is empty_plan.

task.yml fields (only these):
id, files, new, verify, must_contain, depends_on, ctx, cwd
Do not invent other fields.

Field types (serde will reject anything else):
- id: string matching the task directory name
- files: YAML list of workspace-relative paths
- new: optional YAML list of new file paths (subset of files). Omit if none. Never a boolean.
- verify: a SINGLE quoted string, e.g. verify: "cargo test add_works -- --exact". Not a YAML list.
- must_contain: YAML list of strings
- depends_on: YAML list of task ids
- ctx: integer token budget such as 32768. Never prose.
- cwd: optional relative directory

cwd is the directory to run verify in (e.g. desktop/src-tauri). When cwd is set, every files[] and new: path is still workspace-relative and must live under that cwd (files_outside_cwd otherwise).

verify must be an allowlisted AND scoped argv command: cargo test with a filter (bare `cargo test` is verify_not_allowlisted), npx vitest|jest run + a file, npx playwright test + a spec file, npx nx build + a single project, npx ng build (optionally + a project), go test + a package that is not ./..., python -m pytest + a file, node + a test file.

UI tasks (.html/.scss/.css in files[]) ARE allowed — a small model writes the markup/styles and the T2 review lens judges the visual result with vision and guides revisions. Every UI task still needs a mechanical verify so the leaf executor knows when it is done: prefer a structural test (npx vitest run <spec>, or npx playwright test <spec> for an Ionic/Angular app, with must_contain anchoring what it must produce or remove); a task editing ONLY styling with no testable logic may gate on a scoped build (npx nx build <project>, or npx ng build for an Angular app). For every UI task, have it capture a screenshot and list that screen in screens-to-verify so the reviewer can judge it visually.

No two tasks in a phase may claim the same file (file_collision). must_contain must be non-trivial: each needle at least 4 characters and not a stop-needle. depends_on must resolve, form a DAG, and never point forward."#
    };
}

pub fn small_mode_planner_template() -> &'static str {
    DEFAULT_SMALL_MODE_PLANNER
}

pub(crate) const DEFAULT_SMALL_MODE_PLANNER: &str = concat!(
r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T1_PLANNER (LOCAL-SMALL)

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

Emit a methodology-compliant plan that a mechanical plan-check can execute. Every phase must contain at least one task.yml dir; a plan with none is rule empty_plan.

"#
    , small_mode_task_constraints!(), r#"

D12 sizing: at most 150 tasks per phase (MAX_TASKS_PER_PHASE) and 800 tasks total (MAX_TASKS_TOTAL). Keep each task small enough for the leaf executor. Do not write tasks.json. Do not self-claim done.

status.md lists phases, not tasks — one row per phase (id, one-line goal, task count).
phase overview task table is id and one-line goal only — no files[] / verify / prompt dump.

When the plan is ready for review, say exactly:

READY_FOR_T2_PLAN_REVIEW"#
);

pub(crate) const DEFAULT_SMALL_MODE_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T2_PLAN_REVIEWER (LOCAL-SMALL)

Workspace root: {{workspace_path}}
App/source scope: {{app_scope}}
Docs scope: {{docs_scope}}
Plan path: {{plan_output_path}}
Methodology: {{methodology_path}}
Conventions: {{conventions_path}}

The plan-check script has already validated shape, files, DAG, scoped verify, must_contain, bounds, and the UI-task rule. Do not re-count files or re-check those rules.

Judge only: brief coverage, decomposition, phase ordering, and whether each phase's tasks add up to that phase's goal.

Return this footer exactly:

PLAN: {{plan_output_path}}
VERDICT: PASS | NEEDS_CHANGES | BLOCKED
SUMMARY: <one paragraph>
FINDINGS:
- <finding or none>
NEXT_STEPS:
- <step for T1 or none>"#;

pub(crate) const DEFAULT_SMALL_MODE_LEAF_WRAPPER: &str = r#"You are a leaf executor. Edit only the files listed in files[].
Do not touch status.yml or tasks.json. Do not rewrite the plan store.
Stop when verify passes. Do not add extra files or drive-by refactors."#;

pub(crate) const DEFAULT_SMALL_MODE_AMEND_PLANNER: &str = concat!(
    r#"JOHNNYONE_RUN_ID: {{run_id}}
ROLE: T1_PLANNER (LOCAL-SMALL AMEND)

Workspace root: {{workspace_path}}
Existing plan at: {{plan_output_path}}

This is a mid-phase replan. Read amendment.json. Fix or split failed tasks only.
Do not rewrite done task dirs. Do not drop commits. Do not invent mock fields.

"#,
    small_mode_task_constraints!(),
    r#"

When ready, say READY_FOR_T2_PLAN_REVIEW"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_yaml_without_small_mode_still_deserializes() {
        let raw = r#"
schema: johnnyone-planner-prompts/v1
development:
  worker: W
  reviewer: R
planning:
  planner: P
  reviewer: V
"#;
        let s: PlannerPromptSettings = serde_yaml::from_str(raw).unwrap();
        assert_eq!(s.development.worker, "W");
        assert_eq!(s.planning.planner, "P");
        assert_eq!(s.small_mode.planner, DEFAULT_SMALL_MODE_PLANNER);
        assert_eq!(s.small_mode.reviewer, DEFAULT_SMALL_MODE_REVIEWER);
        assert_eq!(s.small_mode.leaf_wrapper, DEFAULT_SMALL_MODE_LEAF_WRAPPER);
        assert_eq!(s.small_mode.amend_planner, DEFAULT_SMALL_MODE_AMEND_PLANNER);
    }

    #[test]
    fn default_reviewer_contains_plan_check_sentence() {
        assert!(DEFAULT_SMALL_MODE_REVIEWER.contains(PLAN_CHECK_ALREADY_VALIDATED));
    }

    #[test]
    fn default_planner_teaches_required_fields_and_rollup() {
        let p = DEFAULT_SMALL_MODE_PLANNER;
        assert!(p.contains("task.yml"));
        assert!(p.contains("must_contain"));
        assert!(p.contains("cwd"));
        assert!(p.contains("empty_plan"));
        assert!(p.contains("status.md lists phases, not tasks"));
        assert!(p.contains("phase overview task table is id and one-line goal only"));
        assert!(p.contains("scoped"));
        assert!(p.contains("bare `cargo test`"));
        assert!(p.contains("file_collision"));
        assert!(p.contains("non-trivial"));
        assert!(p.contains("DAG"));
        assert!(p.contains("never point forward"));
        assert!(p.contains("MAX_TASKS_PER_PHASE"));
        assert!(p.contains("MAX_TASKS_TOTAL"));
        assert!(p.contains("phases/<phase-id>/tasks/<task-id>/"));
        assert!(p.contains("SINGLE quoted string"));
        assert!(p.contains("integer token budget"));
        // UI tasks are now allowed with a scoped build as a fallback verify.
        assert!(p.contains("nx build"));
        assert!(p.contains("UI tasks"));
        let a = DEFAULT_SMALL_MODE_AMEND_PLANNER;
        assert!(a.contains("file_collision"));
        assert!(a.contains("must_contain"));
        assert!(a.contains("verify_not_allowlisted"));
        assert!(a.contains("nx build"));
        let taught = p.split("task.yml fields").nth(1).unwrap_or(p);
        assert!(
            !taught.contains("mock:"),
            "taught-fields section must not include mock:: {taught}"
        );
    }

    fn fnv1a64(s: &str) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for b in s.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    #[test]
    fn commercial_defaults_are_byte_for_byte() {
        let d = PlannerPromptSettings::default();
        assert_eq!(d.planning.planner, DEFAULT_PLANNING_PLANNER);
        assert_eq!(d.planning.reviewer, DEFAULT_PLANNING_REVIEWER);
        assert_eq!(d.development.worker, DEFAULT_DEVELOPMENT_WORKER);
    }

    /// Pinned fingerprints so editing DEFAULT_PLANNING_* does not silently
    /// move both sides of the equality above. Update the constants only when
    /// a commercial-prompt change is intentional.
    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    fn commercial_templates_match_pinned_fingerprints() {
        // Ten shipped DEFAULT_* strings. Not workerNudge (no shipped default).
        assert_eq!(fnv1a64(DEFAULT_PLANNING_PLANNER), 0x57b8ba3c4843e8a7);
        assert_eq!(fnv1a64(DEFAULT_PLANNING_REVIEWER), 0x42e3b3d0e4178fe5);
        assert_eq!(fnv1a64(DEFAULT_DEVELOPMENT_WORKER), 0xe0e0b581211ea700);
        assert_eq!(fnv1a64(DEFAULT_DEVELOPMENT_REVIEWER), 0x882713aa907d5d7f);
        assert_eq!(fnv1a64(DEFAULT_AMEND_PLANNING_PLANNER), 0x7f841cf263c8c761);
        assert_eq!(fnv1a64(DEFAULT_AMEND_PLANNING_REVIEWER), 0xea96c82d6fcfac66);
        assert_eq!(fnv1a64(DEFAULT_SMALL_MODE_PLANNER), 0xea74fb71968a2b4d);
        assert_eq!(fnv1a64(DEFAULT_SMALL_MODE_REVIEWER), 0x37ef8923327e716d);
        assert_eq!(fnv1a64(DEFAULT_SMALL_MODE_LEAF_WRAPPER), 0x464e7f9226edda96);
        assert_eq!(fnv1a64(DEFAULT_SMALL_MODE_AMEND_PLANNER), 0x20caf23012644a43);
    }

    #[test]
    fn lens_preamble_strips_role_judge_and_footer() {
        let from_default = lens_preamble_from_reviewer(DEFAULT_SMALL_MODE_REVIEWER);
        assert_eq!(from_default, PLAN_CHECK_ALREADY_VALIDATED);
        assert!(!from_default.contains("Judge only:"));
        assert!(!from_default.contains("Return this footer"));
        assert_eq!(
            lens_preamble_from_reviewer("SENTINEL-REVIEWER-XYZ"),
            "SENTINEL-REVIEWER-XYZ"
        );
    }

    #[test]
    fn overlay_without_small_mode_keeps_tuned_strings() {
        let mut current = PlannerPromptSettings::default();
        current.small_mode.planner = "TUNED-PLANNER".into();
        current.small_mode.reviewer = "TUNED-REVIEWER".into();
        let merged = overlay_prompt_settings(
            current,
            PlannerDevelopmentOverlay {
                worker: "W".into(),
                reviewer: "R".into(),
                worker_nudge: None,
            },
            PlannerPlanningOverlay {
                planner: "P".into(),
                reviewer: "V".into(),
                amend_planner: None,
                amend_reviewer: None,
            },
            None,
        );
        assert_eq!(merged.development.worker, "W");
        assert_eq!(merged.planning.planner, "P");
        assert_eq!(merged.small_mode.planner, "TUNED-PLANNER");
        assert_eq!(merged.small_mode.reviewer, "TUNED-REVIEWER");
    }

    #[test]
    fn wrap_leaf_prompt_empty_is_identity() {
        assert_eq!(wrap_leaf_prompt("", "body"), "body");
        assert_eq!(wrap_leaf_prompt("  \n", "body"), "body");
        assert_eq!(wrap_leaf_prompt("WRAP", "body"), "WRAP\n\nbody");
    }

    #[test]
    fn leaf_wrapper_is_short() {
        assert!(DEFAULT_SMALL_MODE_LEAF_WRAPPER.lines().count() <= 30);
    }

    #[test]
    fn overlay_partial_planning_json_does_not_clobber_amend() {
        let mut current = PlannerPromptSettings::default();
        current.planning.amend_planner = "TUNED-AMEND-P".into();
        current.planning.amend_reviewer = "TUNED-AMEND-R".into();
        current.development.worker_nudge = Some("TUNED-NUDGE".into());
        let planning: PlannerPlanningOverlay = serde_json::from_value(serde_json::json!({
            "planner": "P",
            "reviewer": "V"
        }))
        .unwrap();
        let development: PlannerDevelopmentOverlay = serde_json::from_value(serde_json::json!({
            "worker": "W",
            "reviewer": "R"
        }))
        .unwrap();
        assert!(planning.amend_planner.is_none());
        assert!(planning.amend_reviewer.is_none());
        assert!(development.worker_nudge.is_none());
        let merged = overlay_prompt_settings(current, development, planning, None);
        assert_eq!(merged.planning.planner, "P");
        assert_eq!(merged.planning.reviewer, "V");
        assert_eq!(merged.planning.amend_planner, "TUNED-AMEND-P");
        assert_eq!(merged.planning.amend_reviewer, "TUNED-AMEND-R");
        assert_eq!(merged.development.worker_nudge.as_deref(), Some("TUNED-NUDGE"));
    }

    #[test]
    fn overlay_omitted_worker_nudge_keeps_none() {
        let current = PlannerPromptSettings::default();
        assert!(current.development.worker_nudge.is_none());
        let development: PlannerDevelopmentOverlay = serde_json::from_value(serde_json::json!({
            "worker": "W",
            "reviewer": "R"
        }))
        .unwrap();
        let planning: PlannerPlanningOverlay = serde_json::from_value(serde_json::json!({
            "planner": "P",
            "reviewer": "V"
        }))
        .unwrap();
        let merged = overlay_prompt_settings(current, development, planning, None);
        assert!(merged.development.worker_nudge.is_none());
    }

    #[test]
    fn yaml_worker_nudge_present_round_trips() {
        let raw = r#"
schema: johnnyone-planner-prompts/v1
development:
  worker: W
  reviewer: R
  workerNudge: CUSTOM-NUDGE
planning:
  planner: P
  reviewer: V
"#;
        let s: PlannerPromptSettings = serde_yaml::from_str(raw).unwrap();
        assert_eq!(s.development.worker_nudge.as_deref(), Some("CUSTOM-NUDGE"));
        let out = serde_yaml::to_string(&s).unwrap();
        assert!(out.contains("workerNudge"));
        let again: PlannerPromptSettings = serde_yaml::from_str(&out).unwrap();
        assert_eq!(again.development.worker_nudge.as_deref(), Some("CUSTOM-NUDGE"));
    }

    #[test]
    fn yaml_without_worker_nudge_omits_the_key() {
        let s = PlannerPromptSettings::default();
        assert!(s.development.worker_nudge.is_none());
        let out = serde_yaml::to_string(&s).unwrap();
        assert!(
            !out.contains("workerNudge"),
            "absent worker_nudge must skip_serializing_if: {out}"
        );
    }

    fn sentinel_settings() -> PlannerPromptSettings {
        PlannerPromptSettings {
            schema: "johnnyone-planner-prompts/v1".into(),
            development: PlannerDevelopmentPrompts {
                worker: "W".into(),
                reviewer: "R".into(),
                worker_nudge: Some("WNUDGE".into()),
            },
            planning: PlannerPlanningPrompts {
                planner: "P".into(),
                reviewer: "V".into(),
                amend_planner: "AMEND-P".into(),
                amend_reviewer: "AMEND-R".into(),
            },
            small_mode: SmallModePrompts {
                planner: "SM-P".into(),
                reviewer: "SM-R".into(),
                leaf_wrapper: "SM-L".into(),
                amend_planner: "SM-AMEND-P".into(),
            },
        }
    }

    /// Amendment 1: load → write unmodified through the same Option input types
    /// as host/RPC → every field including the three extras is identical.
    /// Does not touch ~/.johnnyone/planner-prompts.yml.
    #[test]
    fn prompt_settings_round_trip_is_byte_identical() {
        let loaded = sentinel_settings();
        // Complete write: Some(sentinel) for every additive field (not the
        // stored String structs — that path stays green even if None→keep
        // regresses).
        let development = PlannerDevelopmentOverlay {
            worker: loaded.development.worker.clone(),
            reviewer: loaded.development.reviewer.clone(),
            worker_nudge: loaded.development.worker_nudge.clone(),
        };
        let planning = PlannerPlanningOverlay {
            planner: loaded.planning.planner.clone(),
            reviewer: loaded.planning.reviewer.clone(),
            amend_planner: Some(loaded.planning.amend_planner.clone()),
            amend_reviewer: Some(loaded.planning.amend_reviewer.clone()),
        };
        let small_mode = Some(loaded.small_mode.clone());
        let written = overlay_prompt_settings(
            PlannerPromptSettings::default(),
            development,
            planning,
            small_mode,
        );
        assert_eq!(written.development.worker, "W");
        assert_eq!(written.development.reviewer, "R");
        assert_eq!(written.development.worker_nudge.as_deref(), Some("WNUDGE"));
        assert_eq!(written.planning.planner, "P");
        assert_eq!(written.planning.reviewer, "V");
        assert_eq!(written.planning.amend_planner, "AMEND-P");
        assert_eq!(written.planning.amend_reviewer, "AMEND-R");
        assert_eq!(written.small_mode.planner, "SM-P");
        assert_eq!(written.small_mode.reviewer, "SM-R");
        assert_eq!(written.small_mode.leaf_wrapper, "SM-L");
        assert_eq!(written.small_mode.amend_planner, "SM-AMEND-P");
        assert_ne!(written.planning.amend_planner, DEFAULT_AMEND_PLANNING_PLANNER);
        assert_ne!(written.planning.amend_reviewer, DEFAULT_AMEND_PLANNING_REVIEWER);
        assert_eq!(written, loaded);

        // Serialize path used by save_prompt_settings — a present worker_nudge
        // must not be dropped. No operator-home write.
        let yaml = serde_yaml::to_string(&written).unwrap();
        assert!(yaml.contains("workerNudge"));
        assert!(yaml.contains("WNUDGE"));
        let deserialized: PlannerPromptSettings = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.development.worker_nudge.as_deref(), Some("WNUDGE"));
        assert_eq!(deserialized, loaded);

        // Reverse-skew: omitted additive keys on the input types keep current.
        let mut current = sentinel_settings();
        current.development.worker_nudge = Some("TUNED-NUDGE".into());
        current.planning.amend_planner = "TUNED-AMEND-P".into();
        current.planning.amend_reviewer = "TUNED-AMEND-R".into();
        let partial_dev: PlannerDevelopmentOverlay = serde_json::from_value(serde_json::json!({
            "worker": "W2",
            "reviewer": "R2"
        }))
        .unwrap();
        let partial_plan: PlannerPlanningOverlay = serde_json::from_value(serde_json::json!({
            "planner": "P2",
            "reviewer": "V2"
        }))
        .unwrap();
        let kept = overlay_prompt_settings(current.clone(), partial_dev, partial_plan, None);
        assert_eq!(kept.development.worker, "W2");
        assert_eq!(kept.development.reviewer, "R2");
        assert_eq!(kept.development.worker_nudge.as_deref(), Some("TUNED-NUDGE"));
        assert_eq!(kept.planning.planner, "P2");
        assert_eq!(kept.planning.reviewer, "V2");
        assert_eq!(kept.planning.amend_planner, "TUNED-AMEND-P");
        assert_eq!(kept.planning.amend_reviewer, "TUNED-AMEND-R");
        assert_eq!(kept.small_mode, current.small_mode);
    }
}
