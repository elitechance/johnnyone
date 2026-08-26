//! Read-only Prompt Library catalog over existing `PlannerPromptSettings` + run history.
//!
//! Metadata only — prompt bodies stay on `get_planner_prompt_settings`. USED credits a
//! slot only when a live engine path reads it (overview D2). Tests must not call
//! `list_prompt_library` (that writes `~/.johnnyone/planner-prompts.yml` when missing).

use crate::services::agent_plans::{executor_mode_is_local_small, select_planning_planner};
use crate::services::planner_prompts::{load_prompt_settings, PlannerPromptSettings};
use crate::state::app_state::AppState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Catalog row matching worker GraphQL `PromptLibraryEntry` (camelCase JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptLibraryEntry {
    pub id: String,
    pub key: String,
    pub name: String,
    pub role: String,
    pub scope: String,
    pub version: String,
    pub used_count: i32,
    pub customised: bool,
    pub read_only: bool,
    pub engine_reads: bool,
}

struct Slot {
    key: &'static str,
    name: &'static str,
    role: &'static str,
    engine_reads: bool,
    optional: bool,
}

/// Names match Settings `COMMERCIAL_ROWS` / `SMALL_MODE_ROWS`.
const SLOTS: &[Slot] = &[
    Slot {
        key: "planning.planner",
        name: "Planning planner",
        role: "planner",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "planning.reviewer",
        name: "Planning reviewer",
        role: "lens",
        engine_reads: false,
        optional: false,
    },
    Slot {
        key: "planning.amendPlanner",
        name: "Planning amend planner",
        role: "planner",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "planning.amendReviewer",
        name: "Planning amend reviewer",
        role: "lens",
        engine_reads: false,
        optional: false,
    },
    Slot {
        key: "development.worker",
        name: "Development worker",
        role: "worker",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "development.reviewer",
        name: "Development reviewer",
        role: "lens",
        engine_reads: false,
        optional: false,
    },
    Slot {
        key: "development.workerNudge",
        name: "Development worker nudge",
        role: "worker",
        engine_reads: false,
        optional: true,
    },
    Slot {
        key: "smallMode.planner",
        name: "Small-mode planner",
        role: "planner",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "smallMode.reviewer",
        name: "Small-mode reviewer",
        role: "lens",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "smallMode.leafWrapper",
        name: "Leaf wrapper",
        role: "worker",
        engine_reads: true,
        optional: false,
    },
    Slot {
        key: "smallMode.amendPlanner",
        name: "Amend planner",
        role: "planner",
        engine_reads: true,
        optional: false,
    },
];

fn stored_value<'a>(settings: &'a PlannerPromptSettings, key: &str) -> Option<&'a str> {
    match key {
        "planning.planner" => Some(settings.planning.planner.as_str()),
        "planning.reviewer" => Some(settings.planning.reviewer.as_str()),
        "planning.amendPlanner" => Some(settings.planning.amend_planner.as_str()),
        "planning.amendReviewer" => Some(settings.planning.amend_reviewer.as_str()),
        "development.worker" => Some(settings.development.worker.as_str()),
        "development.reviewer" => Some(settings.development.reviewer.as_str()),
        "development.workerNudge" => settings
            .development
            .worker_nudge
            .as_deref()
            .filter(|s| !s.trim().is_empty()),
        "smallMode.planner" => Some(settings.small_mode.planner.as_str()),
        "smallMode.reviewer" => Some(settings.small_mode.reviewer.as_str()),
        "smallMode.leafWrapper" => Some(settings.small_mode.leaf_wrapper.as_str()),
        "smallMode.amendPlanner" => Some(settings.small_mode.amend_planner.as_str()),
        _ => None,
    }
}

fn is_customised(settings: &PlannerPromptSettings, defaults: &PlannerPromptSettings, key: &str) -> bool {
    if key == "development.workerNudge" {
        return stored_value(settings, key).is_some();
    }
    stored_value(settings, key) != stored_value(defaults, key)
}

/// Catalog key for the planner field `select_planning_planner` would return.
fn planner_catalog_key(local_small: bool, amending: bool) -> &'static str {
    match (local_small, amending) {
        (true, true) => "smallMode.amendPlanner",
        (true, false) => "smallMode.planner",
        (false, true) => "planning.amendPlanner",
        (false, false) => "planning.planner",
    }
}

fn amending_from_brief(amend_brief: Option<&str>) -> bool {
    amend_brief.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Build catalog rows from settings. `used_count` is always 0; overlay via `combine`.
pub fn catalog_from_settings(settings: &PlannerPromptSettings) -> Vec<PromptLibraryEntry> {
    let defaults = PlannerPromptSettings::default();
    SLOTS
        .iter()
        .filter(|slot| {
            if slot.optional {
                stored_value(settings, slot.key).is_some()
            } else {
                true
            }
        })
        .map(|slot| {
            let customised = is_customised(settings, &defaults, slot.key);
            PromptLibraryEntry {
                id: slot.key.to_string(),
                key: slot.key.to_string(),
                name: slot.name.to_string(),
                role: slot.role.to_string(),
                scope: if customised {
                    "workspace".to_string()
                } else {
                    "builtin".to_string()
                },
                version: "v1".to_string(),
                used_count: 0,
                customised,
                read_only: !customised,
                engine_reads: slot.engine_reads,
            }
        })
        .collect()
}

struct PlanRow {
    id: String,
    run_type: String,
    executor_config: Option<String>,
    amend_brief: Option<String>,
}

/// USED counts from existing `agent_plans` / `agent_plan_events`. Role-based: credit
/// only slots the live engine reads (`engine_reads`). Inert lens slots stay 0.
pub fn usage_counts(state: &AppState) -> Result<HashMap<String, i32>, String> {
    let plans: Vec<PlanRow> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, run_type, executor_config, amend_brief FROM agent_plans",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PlanRow {
                    id: row.get(0)?,
                    run_type: row.get(1)?,
                    executor_config: row.get(2)?,
                    amend_brief: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })?;

    let lens_plan_ids: Vec<String> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT plan_id FROM agent_plan_events WHERE type = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params!["agent_lens_review_started"], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })?;

    let mut counts: HashMap<String, i32> = HashMap::new();
    let mut by_id: HashMap<String, &PlanRow> = HashMap::new();

    for plan in &plans {
        by_id.insert(plan.id.clone(), plan);
        let local_small = executor_mode_is_local_small(plan.executor_config.as_deref());
        let amending = amending_from_brief(plan.amend_brief.as_deref());
        match plan.run_type.as_str() {
            "planning" => {
                let key = planner_catalog_key(local_small, amending);
                *counts.entry(key.to_string()).or_insert(0) += 1;
            }
            "development" => {
                let key = if local_small {
                    "smallMode.leafWrapper"
                } else {
                    "development.worker"
                };
                *counts.entry(key.to_string()).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    for plan_id in lens_plan_ids {
        if let Some(plan) = by_id.get(&plan_id) {
            let local_small = executor_mode_is_local_small(plan.executor_config.as_deref());
            if plan.run_type == "planning" && local_small {
                *counts.entry("smallMode.reviewer".to_string()).or_insert(0) += 1;
            }
        }
    }

    Ok(counts)
}

pub fn combine(
    settings: &PlannerPromptSettings,
    counts: &HashMap<String, i32>,
) -> Vec<PromptLibraryEntry> {
    catalog_from_settings(settings)
        .into_iter()
        .map(|mut entry| {
            entry.used_count = if entry.engine_reads {
                counts.get(&entry.key).copied().unwrap_or(0)
            } else {
                0
            };
            entry
        })
        .collect()
}

pub fn list_prompt_library(state: &AppState) -> Result<Vec<PromptLibraryEntry>, String> {
    let settings = load_prompt_settings()?;
    let counts = usage_counts(state)?;
    Ok(combine(&settings, &counts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_state;

    fn by_key(entries: &[PromptLibraryEntry]) -> HashMap<String, PromptLibraryEntry> {
        entries
            .iter()
            .map(|e| (e.key.clone(), e.clone()))
            .collect()
    }

    fn insert_plan(
        state: &AppState,
        id: &str,
        run_type: &str,
        local_small: bool,
        amend: Option<&str>,
    ) {
        let exec = if local_small {
            Some(r#"{"mode":"local-small"}"#)
        } else {
            None
        };
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, worker_provider, reviewer_provider, executor_config, amend_brief)
                     VALUES (?1, ?2, 't', '/w', '/p', 'claude_code', 'claude_code', ?3, ?4)",
                    params![id, run_type, exec, amend],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
    }

    fn insert_lens_events(state: &AppState, plan_id: &str, n: usize) {
        state
            .db
            .with_conn(|conn| {
                for i in 0..n {
                    conn.execute(
                        "INSERT INTO agent_plan_events (id, plan_id, type, payload_json) VALUES (?1, ?2, 'agent_lens_review_started', '{}')",
                        params![format!("{plan_id}-lens-{i}"), plan_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn used(entries: &[PromptLibraryEntry], key: &str) -> i32 {
        entries
            .iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("missing catalog key {key}"))
            .used_count
    }

    fn engine_reads(entries: &[PromptLibraryEntry], key: &str) -> bool {
        entries
            .iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("missing catalog key {key}"))
            .engine_reads
    }

    #[test]
    fn default_settings_ten_builtin_keys() {
        let entries = catalog_from_settings(&PlannerPromptSettings::default());
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "planning.planner",
                "planning.reviewer",
                "planning.amendPlanner",
                "planning.amendReviewer",
                "development.worker",
                "development.reviewer",
                "smallMode.planner",
                "smallMode.reviewer",
                "smallMode.leafWrapper",
                "smallMode.amendPlanner",
            ]
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Planning planner",
                "Planning reviewer",
                "Planning amend planner",
                "Planning amend reviewer",
                "Development worker",
                "Development reviewer",
                "Small-mode planner",
                "Small-mode reviewer",
                "Leaf wrapper",
                "Amend planner",
            ]
        );
        for e in &entries {
            assert_eq!(e.id, e.key);
            assert_eq!(e.scope, "builtin");
            assert!(!e.customised);
            assert!(e.read_only);
            assert_eq!(e.version, "v1");
            assert_eq!(e.used_count, 0);
            println!("{} {} {} {} {}", e.key, e.name, e.role, e.scope, e.version);
        }
        assert!(entries.iter().all(|e| e.key != "development.workerNudge"));
    }

    #[test]
    fn overlay_planning_planner_is_workspace() {
        let mut settings = PlannerPromptSettings::default();
        settings.planning.planner = "custom planner overlay".to_string();
        let map = by_key(&catalog_from_settings(&settings));
        let row = map.get("planning.planner").unwrap();
        assert!(row.customised);
        assert_eq!(row.scope, "workspace");
        assert!(!row.read_only);
        for key in [
            "planning.reviewer",
            "planning.amendPlanner",
            "development.worker",
            "smallMode.planner",
        ] {
            let other = map.get(key).unwrap();
            assert!(!other.customised, "{key} should stay built-in");
            assert_eq!(other.scope, "builtin");
            assert!(other.read_only);
        }
    }

    #[test]
    fn worker_nudge_optional_row() {
        let mut settings = PlannerPromptSettings::default();
        assert!(catalog_from_settings(&settings)
            .iter()
            .all(|e| e.key != "development.workerNudge"));

        settings.development.worker_nudge = Some("x".to_string());
        let map = by_key(&catalog_from_settings(&settings));
        let row = map.get("development.workerNudge").expect("nudge row");
        assert!(row.customised);
        assert_eq!(row.scope, "workspace");
        assert!(!row.read_only);
        assert!(!row.engine_reads);
        assert_eq!(row.used_count, 0);
        assert_eq!(row.role, "worker");
        assert_eq!(row.name, "Development worker nudge");

        settings.development.worker_nudge = Some(String::new());
        assert!(catalog_from_settings(&settings)
            .iter()
            .all(|e| e.key != "development.workerNudge"));
    }

    #[test]
    fn engine_reads_flags_match_d2() {
        let entries = catalog_from_settings(&PlannerPromptSettings::default());
        assert!(!engine_reads(&entries, "planning.reviewer"));
        assert!(!engine_reads(&entries, "planning.amendReviewer"));
        assert!(!engine_reads(&entries, "development.reviewer"));
        assert!(engine_reads(&entries, "planning.planner"));
        assert!(engine_reads(&entries, "planning.amendPlanner"));
        assert!(engine_reads(&entries, "development.worker"));
        assert!(engine_reads(&entries, "smallMode.planner"));
        assert!(engine_reads(&entries, "smallMode.reviewer"));
        assert!(engine_reads(&entries, "smallMode.leafWrapper"));
        assert!(engine_reads(&entries, "smallMode.amendPlanner"));
    }

    #[test]
    fn planner_used_slot_matches_select_planning_planner() {
        let settings = PlannerPromptSettings::default();
        for local_small in [false, true] {
            for amending in [false, true] {
                let key = planner_catalog_key(local_small, amending);
                let selected = select_planning_planner(&settings, local_small, amending);
                let field = match key {
                    "planning.planner" => settings.planning.planner.as_str(),
                    "planning.amendPlanner" => settings.planning.amend_planner.as_str(),
                    "smallMode.planner" => settings.small_mode.planner.as_str(),
                    "smallMode.amendPlanner" => settings.small_mode.amend_planner.as_str(),
                    other => panic!("unexpected planner key {other}"),
                };
                assert!(
                    std::ptr::eq(selected, field),
                    "local_small={local_small} amending={amending} key={key}"
                );
            }
        }
    }

    #[test]
    fn usage_commercial_planning_lenses_do_not_credit_inert_reviewer() {
        let (state, root) = test_state();
        insert_plan(&state, "p1", "planning", false, None);
        insert_lens_events(&state, "p1", 2);
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert_eq!(used(&entries, "planning.planner"), 1);
        assert_eq!(used(&entries, "planning.reviewer"), 0);
        assert!(!engine_reads(&entries, "planning.reviewer"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_commercial_amend_planning() {
        let (state, root) = test_state();
        insert_plan(&state, "p1", "planning", false, Some("please amend"));
        insert_lens_events(&state, "p1", 1);
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert_eq!(used(&entries, "planning.amendPlanner"), 1);
        assert_eq!(used(&entries, "planning.amendReviewer"), 0);
        assert_eq!(used(&entries, "planning.planner"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_local_small_planning_credits_small_mode_reviewer() {
        let (state, root) = test_state();
        insert_plan(&state, "p1", "planning", true, None);
        insert_lens_events(&state, "p1", 3);
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert_eq!(used(&entries, "smallMode.planner"), 1);
        assert_eq!(used(&entries, "smallMode.reviewer"), 3);
        assert_eq!(used(&entries, "planning.reviewer"), 0);
        assert_eq!(used(&entries, "planning.planner"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_local_small_development() {
        let (state, root) = test_state();
        insert_plan(&state, "d1", "development", true, None);
        insert_lens_events(&state, "d1", 2);
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert_eq!(used(&entries, "smallMode.leafWrapper"), 1);
        assert_eq!(used(&entries, "development.reviewer"), 0);
        assert_eq!(used(&entries, "development.worker"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_commercial_development() {
        let (state, root) = test_state();
        insert_plan(&state, "d1", "development", false, None);
        insert_lens_events(&state, "d1", 2);
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert_eq!(used(&entries, "development.worker"), 1);
        assert_eq!(used(&entries, "development.reviewer"), 0);
        assert_eq!(used(&entries, "smallMode.leafWrapper"), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_empty_db_is_all_zero() {
        let (state, root) = test_state();
        let entries = combine(
            &PlannerPromptSettings::default(),
            &usage_counts(&state).unwrap(),
        );
        assert!(entries.iter().all(|e| e.used_count == 0));
        assert_eq!(entries.len(), 10);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn entry_serializes_camel_case() {
        let entry = PromptLibraryEntry {
            id: "planning.planner".into(),
            key: "planning.planner".into(),
            name: "Planning planner".into(),
            role: "planner".into(),
            scope: "builtin".into(),
            version: "v1".into(),
            used_count: 3,
            customised: false,
            read_only: true,
            engine_reads: true,
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["usedCount"], 3);
        assert!(v.get("used_count").is_none());
        assert_eq!(v["engineReads"], true);
        assert_eq!(v["readOnly"], true);
        assert_eq!(v["customised"], false);
    }
}
