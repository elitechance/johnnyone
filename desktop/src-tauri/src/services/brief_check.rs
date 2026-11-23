//! Grade a structured brief (Plans tab, initiative 8ad3ae71 phase 00 task 06).
//!
//! Named `brief_check` so it does not clash with `plan_check` (which validates the task specs of a
//! WRITTEN plan — different input, different rule ids, D4). The MCP tool keeps the brief's name
//! `plan_check`. The UI, the plan agent and Start planning all read the same verdict from here.
//!
//! - [`check`] is PURE: the brief fields + a pre-gathered [`BriefCheckCtx`] → ordered items.
//! - [`gather_ctx`] is the ONLY function that touches the database or the filesystem: which `where`
//!   items exist inside the workspace, and which other initiatives are running in the same
//!   workspace (with the files they touch).

use crate::services::brief_model::{is_command_line, BriefFields};
use serde::Serialize;
use std::collections::HashSet;

// ───────────────────────────── types ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// One graded rule. `paths` lists the offending/overlapping `where` items; `initiative_ids` (only
/// for `file_conflict`) lists the running initiatives that overlap — what Queue planning waits on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub rule: String,
    pub status: CheckStatus,
    pub detail: String,
    pub paths: Vec<String>,
    pub initiative_ids: Vec<String>,
}

/// Another initiative running in the same workspace and the workspace-relative files it touches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunningInitiative {
    pub id: String,
    pub title: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BriefCheckCtx {
    /// The `where` items (as written) that exist under the workspace.
    pub workspace_exists_paths: HashSet<String>,
    pub running: Vec<RunningInitiative>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CheckSummary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
}

/// Rule ids in evaluation order (the order [`check`] returns them).
pub const RULES: [&str; 5] = [
    "outcome_one_sentence",
    "done_when_runnable",
    "out_of_scope_present",
    "where_exists",
    "file_conflict",
];

// ───────────────────────────── pure rules ─────────────────────────────

/// Words ending in `.` that do not end a sentence.
const ABBREVIATIONS: &[&str] = &["e.g.", "i.e.", "etc.", "vs.", "cf.", "approx.", "incl.", "esp.", "no.", "fig."];

/// Sentence boundaries inside one line: a `.`/`!`/`?` followed by whitespace and then more text,
/// skipping known abbreviations (`e.g.`). `v1.2` never counts (no whitespace after the dot).
pub fn sentence_boundaries(line: &str) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut count = 0;
    for i in 0..chars.len() {
        if !matches!(chars[i], '.' | '!' | '?') {
            continue;
        }
        let mut j = i + 1;
        if j >= chars.len() || !chars[j].is_whitespace() {
            continue;
        }
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() {
            continue;
        }
        if chars[i] == '.' {
            // The word that ends at this dot.
            let start = chars[..i]
                .iter()
                .rposition(|c| c.is_whitespace())
                .map(|p| p + 1)
                .unwrap_or(0);
            let word: String = chars[start..=i].iter().collect::<String>().to_lowercase();
            let word = word.trim_start_matches(|c: char| matches!(c, '(' | '"' | '\''));
            if ABBREVIATIONS.contains(&word) {
                continue;
            }
        }
        count += 1;
    }
    count
}

fn item(rule: &str, status: CheckStatus, detail: impl Into<String>) -> CheckItem {
    CheckItem {
        rule: rule.to_string(),
        status,
        detail: detail.into(),
        paths: Vec::new(),
        initiative_ids: Vec::new(),
    }
}

/// Workspace-relative path in a comparable form: no `./` prefix, no trailing `/`, no empty segments.
pub fn normalize_rel(p: &str) -> String {
    let t = p.trim().trim_matches('`');
    let t = t.strip_prefix("./").unwrap_or(t);
    t.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Two workspace-relative paths overlap when equal or when one is a directory prefix of the other
/// at a segment boundary (`src/a` overlaps `src/a/b.rs`, not `src/ab`).
pub fn overlaps(a: &str, b: &str) -> bool {
    let a = normalize_rel(a);
    let b = normalize_rel(b);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || b.starts_with(&format!("{}/", a)) || a.starts_with(&format!("{}/", b))
}

/// Running initiatives whose files overlap this brief's `where`, with our overlapping items.
pub fn conflicts(fields: &BriefFields, ctx: &BriefCheckCtx) -> Vec<(RunningInitiative, Vec<String>)> {
    let mut out = Vec::new();
    for r in &ctx.running {
        let mine: Vec<String> = fields
            .where_
            .iter()
            .filter(|w| r.files.iter().any(|f| overlaps(w, f)))
            .cloned()
            .collect();
        if !mine.is_empty() {
            out.push((r.clone(), mine));
        }
    }
    out
}

/// Grade a brief. Always returns one item per rule, in [`RULES`] order.
pub fn check(fields: &BriefFields, ctx: &BriefCheckCtx) -> Vec<CheckItem> {
    let mut items = Vec::with_capacity(RULES.len());

    // 1. outcome_one_sentence
    let outcome_lines: Vec<&str> = fields.outcome.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    items.push(match outcome_lines.as_slice() {
        [] => item(RULES[0], CheckStatus::Fail, "Outcome is empty"),
        [line] if sentence_boundaries(line) == 0 => item(RULES[0], CheckStatus::Ok, "Outcome is one sentence"),
        [_] => item(RULES[0], CheckStatus::Fail, "Outcome is more than one sentence"),
        many => item(
            RULES[0],
            CheckStatus::Fail,
            format!("Outcome spans {} lines; keep it to one sentence", many.len()),
        ),
    });

    // 2. done_when_runnable
    let commands = fields.done_when.lines().filter(|l| !l.trim().is_empty() && is_command_line(l)).count();
    items.push(if commands > 0 {
        item(RULES[1], CheckStatus::Ok, format!("{} runnable check(s) in Done when", commands))
    } else {
        item(
            RULES[1],
            CheckStatus::Fail,
            "Done when has no runnable command (start a line with a command such as `cargo test`, or wrap it in backticks)",
        )
    });

    // 3. out_of_scope_present
    items.push(if fields.out_of_scope.trim().is_empty() {
        item(RULES[2], CheckStatus::Fail, "Out of scope is empty")
    } else {
        item(RULES[2], CheckStatus::Ok, "Out of scope is stated")
    });

    // 4. where_exists
    let wheres: Vec<&String> = fields.where_.iter().filter(|w| !w.trim().is_empty()).collect();
    items.push(if wheres.is_empty() {
        item(RULES[3], CheckStatus::Fail, "Where lists no files or folders")
    } else {
        let missing: Vec<String> = wheres
            .iter()
            .filter(|w| !ctx.workspace_exists_paths.contains(w.as_str()))
            .map(|w| w.to_string())
            .collect();
        if missing.is_empty() {
            item(RULES[3], CheckStatus::Ok, format!("All {} Where path(s) exist", wheres.len()))
        } else {
            let mut it = item(
                RULES[3],
                CheckStatus::Fail,
                format!("Not found in the workspace: {}", missing.join(", ")),
            );
            it.paths = missing;
            it
        }
    });

    // 5. file_conflict
    let found = conflicts(fields, ctx);
    items.push(if found.is_empty() {
        item(RULES[4], CheckStatus::Ok, "No running initiative touches these files")
    } else {
        let detail = found
            .iter()
            .map(|(r, paths)| format!("{} touches {}", r.title, paths.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        let mut it = item(RULES[4], CheckStatus::Warn, detail);
        let mut paths: Vec<String> = Vec::new();
        for (_, ps) in &found {
            for p in ps {
                if !paths.contains(p) {
                    paths.push(p.clone());
                }
            }
        }
        it.paths = paths;
        it.initiative_ids = found.iter().map(|(r, _)| r.id.clone()).collect();
        it
    });

    items
}

pub fn check_summary(items: &[CheckItem]) -> CheckSummary {
    let mut s = CheckSummary::default();
    for i in items {
        match i.status {
            CheckStatus::Ok => s.ok += 1,
            CheckStatus::Warn => s.warn += 1,
            CheckStatus::Fail => s.fail += 1,
        }
    }
    s
}

/// The `fail` rule ids (recorded in `brief_frozen.checkFailures`, D11).
pub fn failed_rules(items: &[CheckItem]) -> Vec<String> {
    items
        .iter()
        .filter(|i| i.status == CheckStatus::Fail)
        .map(|i| i.rule.clone())
        .collect()
}

// ───────────────────────────── context (the only I/O) ─────────────────────────────

/// Gather the context [`check`] grades against:
/// - which `where` items exist: `normalize_path(workspace.join(item))` must stay under the
///   canonical workspace AND exist (an escaping item such as `../x` is reported missing);
/// - which OTHER initiatives run in the SAME `workspace_path` (`status != 'closed'`,
///   `initiative_status IN ('planning','development','review')`), one entry per initiative, with
///   their files = the `where` of their accepted brief version ∪ `files[]` of the task specs under
///   their `plan_path` (unreadable specs are ignored).
pub fn gather_ctx(
    state: &crate::state::app_state::AppState,
    initiative_id: &str,
    fields: &BriefFields,
) -> Result<BriefCheckCtx, String> {
    use crate::services::agent_plans::normalize_path;
    use crate::services::task_spec::load_task_spec;
    use rusqlite::{params, OptionalExtension};
    use std::path::Path;

    let workspace: String = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT workspace_path FROM agent_plans WHERE id = ?1",
            params![initiative_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("plan {} not found", initiative_id))
    })?;

    let mut ctx = BriefCheckCtx::default();
    if let Ok(ws) = normalize_path(Path::new(&workspace)) {
        for w in &fields.where_ {
            let rel = w.trim().trim_matches('`');
            if rel.is_empty() {
                continue;
            }
            if let Ok(p) = normalize_path(&ws.join(rel)) {
                if p.starts_with(&ws) && p.exists() {
                    ctx.workspace_exists_paths.insert(w.clone());
                }
            }
        }
    }

    // (initiative_id, title, plan_path, accepted where) per running row of another initiative.
    let rows: Vec<(String, String, String, Option<String>)> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT p.initiative_id, p.title, p.plan_path,
                        (SELECT bv.fields_json FROM brief_versions bv
                          WHERE bv.initiative_id = p.initiative_id AND bv.version = p.accepted_brief_version)
                   FROM agent_plans p
                  WHERE p.workspace_path = ?1 AND p.initiative_id != ?2 AND p.status != 'closed'
                    AND p.initiative_status IN ('planning','development','review')
                  ORDER BY p.created_at, p.id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![workspace, initiative_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    })?;

    for (id, title, plan_path, accepted_fields) in rows {
        let mut files: Vec<String> = Vec::new();
        let mut push = |f: &str| {
            let n = normalize_rel(f);
            if !n.is_empty() && !files.contains(&n) {
                files.push(n);
            }
        };
        if let Some(json) = accepted_fields {
            if let Ok(f) = serde_json::from_str::<BriefFields>(&json) {
                for w in &f.where_ {
                    push(w);
                }
            }
        }
        let phases_dir = Path::new(&plan_path).join("phases");
        let mut task_dirs = Vec::new();
        if let Ok(phases) = std::fs::read_dir(&phases_dir) {
            for phase in phases.flatten() {
                if let Ok(tasks) = std::fs::read_dir(phase.path().join("tasks")) {
                    for t in tasks.flatten() {
                        task_dirs.push(t.path());
                    }
                }
            }
        }
        task_dirs.sort();
        for dir in task_dirs {
            if let Ok(spec) = load_task_spec(&dir) {
                for f in &spec.files {
                    push(f);
                }
            }
        }
        // Planning + development rows share an initiative id: merge them into one entry.
        match ctx.running.iter_mut().find(|r| r.id == id) {
            Some(existing) => {
                for f in files {
                    if !existing.files.contains(&f) {
                        existing.files.push(f);
                    }
                }
            }
            None => ctx.running.push(RunningInitiative { id, title, files }),
        }
    }
    Ok(ctx)
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app_state::AppState;
    use crate::test_support::{test_state, tmp_dir};
    use rusqlite::params;

    fn good() -> BriefFields {
        BriefFields {
            outcome: "Ship the Plans tab.".into(),
            why: "w".into(),
            where_: vec!["src".into()],
            done_when: "cargo test -j 8 --lib".into(),
            out_of_scope: "Billing".into(),
            decided: String::new(),
        }
    }

    fn ctx_with(paths: &[&str]) -> BriefCheckCtx {
        BriefCheckCtx {
            workspace_exists_paths: paths.iter().map(|s| s.to_string()).collect(),
            running: Vec::new(),
        }
    }

    fn status_of(items: &[CheckItem], rule: &str) -> CheckStatus {
        items.iter().find(|i| i.rule == rule).unwrap().status
    }

    #[test]
    fn check_table() {
        use CheckStatus::*;
        type Edit = fn(&mut BriefFields);
        // (name, edit, rule, expected status)
        let rows: Vec<(&str, Edit, &str, CheckStatus)> = vec![
            ("one sentence", |_| {}, "outcome_one_sentence", Ok),
            ("empty outcome", |f| f.outcome = "  ".into(), "outcome_one_sentence", Fail),
            ("two sentences", |f| f.outcome = "Ship it. Then test it.".into(), "outcome_one_sentence", Fail),
            ("question then more", |f| f.outcome = "Why not? Do it".into(), "outcome_one_sentence", Fail),
            ("two lines", |f| f.outcome = "Ship it\nand more".into(), "outcome_one_sentence", Fail),
            ("e.g. is not a sentence end", |f| f.outcome = "Add checks, e.g. a lint gate.".into(), "outcome_one_sentence", Ok),
            ("i.e. is not a sentence end", |f| f.outcome = "One gate, i.e. the test run.".into(), "outcome_one_sentence", Ok),
            ("v1.2 is not a sentence end", |f| f.outcome = "Upgrade to v1.2 of the API.".into(), "outcome_one_sentence", Ok),
            ("trailing ! is fine", |f| f.outcome = "Ship it!".into(), "outcome_one_sentence", Ok),
            ("bare command", |_| {}, "done_when_runnable", Ok),
            ("backticked command", |f| f.done_when = "`./run-all.sh`".into(), "done_when_runnable", Ok),
            ("bulleted command", |f| f.done_when = "- npx vitest run".into(), "done_when_runnable", Ok),
            ("prose done_when", |f| f.done_when = "It works well\nUsers are happy".into(), "done_when_runnable", Fail),
            ("command word must be exact", |f| f.done_when = "golang is installed".into(), "done_when_runnable", Fail),
            ("empty done_when", |f| f.done_when = String::new(), "done_when_runnable", Fail),
            ("out of scope present", |_| {}, "out_of_scope_present", Ok),
            ("out of scope empty", |f| f.out_of_scope = " \n".into(), "out_of_scope_present", Fail),
            ("where exists", |_| {}, "where_exists", Ok),
            ("where missing", |f| f.where_ = vec!["src".into(), "nope".into()], "where_exists", Fail),
            ("where empty", |f| f.where_ = Vec::new(), "where_exists", Fail),
            ("no conflict", |_| {}, "file_conflict", Ok),
        ];
        assert!(rows.len() >= 16);
        for (name, edit, rule, want) in rows {
            let mut f = good();
            edit(&mut f);
            let items = check(&f, &ctx_with(&["src"]));
            assert_eq!(items.iter().map(|i| i.rule.as_str()).collect::<Vec<_>>(), RULES.to_vec(), "{name}: rule order");
            assert_eq!(status_of(&items, rule), want, "{name}");
        }
        // Missing where items are listed in `paths`, and the summary counts statuses.
        let mut f = good();
        f.where_ = vec!["src".into(), "nope".into()];
        let items = check(&f, &ctx_with(&["src"]));
        let w = items.iter().find(|i| i.rule == "where_exists").unwrap();
        assert_eq!(w.paths, vec!["nope"]);
        assert_eq!(check_summary(&items), CheckSummary { ok: 4, warn: 0, fail: 1 });
        assert_eq!(failed_rules(&items), vec!["where_exists"]);
        // A conflict is a WARN that names the initiative.
        let mut ctx = ctx_with(&["src"]);
        ctx.running.push(RunningInitiative { id: "I2".into(), title: "Other".into(), files: vec!["src/main.rs".into()] });
        let items = check(&good(), &ctx);
        let c = items.iter().find(|i| i.rule == "file_conflict").unwrap();
        assert_eq!((c.status, c.initiative_ids.clone(), c.paths.clone()), (Warn, vec!["I2".to_string()], vec!["src".to_string()]));
        assert!(c.detail.contains("Other"), "{}", c.detail);
        let s = serde_json::to_value(c).unwrap();
        assert_eq!(s["status"], "warn");
        assert_eq!(s["initiativeIds"][0], "I2");
    }

    #[test]
    fn where_escape_is_missing() {
        let (state, _root) = test_state();
        let ws = tmp_dir("bc-ws").canonicalize().unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        // A sibling that really exists OUTSIDE the workspace.
        let outside = ws.parent().unwrap().join("bc-outside-x");
        std::fs::create_dir_all(&outside).unwrap();
        crate::services::brief_store::tests::briefing_row(&state, "P", &ws.to_string_lossy());
        let mut f = good();
        f.where_ = vec!["src".into(), "../bc-outside-x".into(), outside.to_string_lossy().to_string(), "ghost".into()];
        let ctx = gather_ctx(&state, "P", &f).unwrap();
        assert!(ctx.workspace_exists_paths.contains("src"));
        assert!(!ctx.workspace_exists_paths.contains("../bc-outside-x"), "`..` escape must be missing");
        assert_eq!(ctx.workspace_exists_paths.len(), 1, "absolute outside path and ghost are missing too");
        let items = check(&f, &ctx);
        let w = items.iter().find(|i| i.rule == "where_exists").unwrap();
        assert_eq!(w.status, CheckStatus::Fail);
        assert_eq!(w.paths.len(), 3);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn conflict_overlap_rules() {
        assert!(overlaps("src/a.rs", "src/a.rs"), "file == file");
        assert!(overlaps("./src/a.rs", "src/a.rs/"), "normalized forms");
        assert!(overlaps("src", "src/a/b.rs"), "dir prefix");
        assert!(overlaps("src/a/b.rs", "src/"), "dir prefix, either side");
        assert!(!overlaps("src/a", "src/ab"), "segment boundary");
        assert!(!overlaps("src/ab", "src/a"));
        assert!(!overlaps("web", "worker"));
        assert!(!overlaps("", "src"));
        assert_eq!(normalize_rel("`./web//src/`"), "web/src");
    }

    fn row(state: &AppState, id: &str, initiative: &str, ws: &str, plan_path: &str, status: &str, istatus: &str, accepted: Option<i64>) {
        state
            .db
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, initiative_id, initiative_status, accepted_brief_version) \
                     VALUES (?1, 'planning', ?1, ?2, ?3, ?4, 'claude_code', 'claude_code', ?5, ?6, ?7)",
                    params![id, ws, plan_path, status, initiative, istatus, accepted],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
    }

    #[test]
    fn gather_ctx_ignores_closed_and_other_workspaces() {
        let (state, _root) = test_state();
        let ws = tmp_dir("bc-ws2").canonicalize().unwrap();
        let ws_s = ws.to_string_lossy().to_string();
        let other_ws = tmp_dir("bc-ws3").canonicalize().unwrap().to_string_lossy().to_string();
        // A running initiative's plan with one task spec.
        let plan = tmp_dir("bc-plan");
        let task_dir = plan.join("phases/00-x/tasks/01-y");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("task.yml"), "id: 01-y\nfiles: [\"web/src/app.ts\"]\nverify: \"true\"\n").unwrap();
        // A malformed spec is ignored.
        let bad = plan.join("phases/00-x/tasks/02-bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("task.yml"), "not: [valid").unwrap();

        crate::services::brief_store::tests::briefing_row(&state, "ME", &ws_s);
        row(&state, "RUN", "RUN", &ws_s, &plan.to_string_lossy(), "phase_worker_running", "planning", Some(1));
        row(&state, "RUN-DEV", "RUN", &ws_s, "/none", "phase_worker_running", "development", None);
        row(&state, "CLOSED", "CLOSED", &ws_s, "/none", "closed", "development", None);
        row(&state, "ELSEWHERE", "ELSEWHERE", &other_ws, "/none", "draft", "development", None);
        row(&state, "DRAFTING", "DRAFTING", &ws_s, "/none", "draft", "briefing", None);
        row(&state, "FINISHED", "FINISHED", &ws_s, "/none", "done", "done", None);
        // RUN's accepted brief lists `desktop`.
        state
            .db
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json) VALUES ('RUN', 1, 'you', ?1, '{}')",
                    params![serde_json::to_string(&BriefFields { where_: vec!["desktop".into()], ..Default::default() }).unwrap()],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();

        let mut f = good();
        f.where_ = vec!["web".into(), "worker".into()];
        let ctx = gather_ctx(&state, "ME", &f).unwrap();
        assert_eq!(ctx.running.len(), 1, "only RUN (its two rows merged): {:?}", ctx.running);
        assert_eq!(ctx.running[0].id, "RUN");
        assert_eq!(ctx.running[0].files, vec!["desktop", "web/src/app.ts"]);
        let items = check(&f, &ctx);
        let c = items.iter().find(|i| i.rule == "file_conflict").unwrap();
        assert_eq!((c.status, c.paths.clone()), (CheckStatus::Warn, vec!["web".to_string()]));
    }
}
