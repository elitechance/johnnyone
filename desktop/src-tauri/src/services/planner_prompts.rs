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
            },
        }
    }
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
Conventions: {{conventions_path}}

Read methodology, project conventions (every file under {{conventions_path}}), plan overview, phase overview, discoveries, and current-phase task files directly from those paths on the host. Work only on this phase. Do not start later phases. Update task status/decisions/artifacts as you work. When the whole phase is ready for review, say READY_FOR_T2_VALIDATION."#;

const DEFAULT_DEVELOPMENT_REVIEWER: &str = r#"JOHNNYONE_RUN_ID: {{run_id}}
JOHNNYONE_PHASE_ID: {{phase_id}}
ROLE: T2_REVIEWER

Workspace: {{workspace_path}}
Plan: {{plan_path}}
Current phase: {{phase_id}}
Phase path: {{phase_path}}
Tasks path: {{tasks_path}}
Conventions: {{conventions_path}}

Read methodology, project conventions (every file under {{conventions_path}}), plan overview, phase overview, discoveries, and current-phase task files/artifacts directly from those paths on the host. Validate only; do not implement app changes. You may update plan status artifacts only. Focus on status files, decisions, tests, E2E artifacts, screenshots, acceptance criteria, and convention compliance.

Before returning, append your phase validation result to {{phase_path}}/status.md under `## Phase Validation`. Include verdict, summary, findings, next steps, and any screenshot/test read-back notes. If {{phase_path}}/status.yml exists, update its validation fields consistently with your verdict. Do not edit application source files.

Verdict rules:
- PASS only when the phase acceptance criteria are satisfied.
- NEEDS_CHANGES when T1 can continue work, add evidence, fix issues, document a waiver/scope decision, or explain why the current evidence satisfies acceptance. Missing or insufficient validation evidence is NEEDS_CHANGES, not BLOCKED.
- BLOCKED only when T2 cannot perform validation because of an external access/runtime problem, such as missing workspace/plan paths, unreadable files, unavailable credentials, unavailable required tools, or a broken validation environment. If you can state what T1 must do to pass, use NEEDS_CHANGES.

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

Create or update a methodology-compliant plan at the plan output path. Read the methodology first, then read every file under the conventions path so the plan honors project conventions (UI, backend, testing, tooling, plan-validation, etc.). Inspect the app/source scope and docs scope enough to ground the plan in the real codebase. Use Mermaid for diagrams and HTML+JS mocks when UI/layout is unclear. Include local and live validation strategy when the feature must be testable by the user. Do not implement application code. Only create/update plan files.

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

Validate the plan only. Do not implement application code. Read methodology, every convention file under the conventions path, the created plan, relevant docs, source scope, artifacts, mocks, and diagrams.

Review for methodology + convention compliance, clear phase boundaries, actionable task prompts, explicit acceptance criteria, local/live testing strategy, UI mocks/diagrams where needed, risky assumptions, and whether the plan can produce something the user can test.

Return this footer exactly:

PLAN: {{plan_output_path}}
VERDICT: PASS | NEEDS_CHANGES | BLOCKED
SUMMARY: <one paragraph>
FINDINGS:
- <finding or none>
NEXT_STEPS:
- <step for T1 or none>"#;
