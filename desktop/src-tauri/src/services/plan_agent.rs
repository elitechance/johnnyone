//! The Plans-tab plan agent (initiative 8ad3ae71 phase 02 tasks 07 and 09; master D1, D2, D5).
//!
//! The plan agent IS the plan's chat session (`agent_plans.briefing_session_id`) run by claude_code
//! on the existing chat paths (`chat_host::send_chat_message_blocking`, the relay's
//! `handle_relay_chat`, `commands::chat`): one `claude --print --resume` per turn, run VISIBLY in the
//! session's own `johnnyone_<sid>` tmux pane, its data read only from the turn's stream-json
//! (`plan_agent_tmux`, D1 2026-11-23). This module only decides the EXTRA launch arguments for such
//! a turn — it builds arguments and writes the private MCP config, it never spawns anything:
//!
//! - [`plan_agent_extras`]: `None` for any session that is not a live plan's agent; `Err` once the
//!   plan left briefing (the turn is refused); otherwise ensures a live token for (plan, session),
//!   (re)writes `mcp.json` when it is missing (minting a fresh token, which revokes the old one —
//!   the raw token is only ever needed to write the file), and returns [`LaunchExtras`]: the config
//!   PATH, the ten `mcp__j1__*` tools as the only allowed tools, the built-in deny list and the
//!   plan-agent system prompt.
//! - [`retire_plan_agent_with`]: the PR #75 retire rules for the plan agent — revoke its token
//!   (deleting `mcp.json`), stop an in-flight turn, archive its session and kill its pane. Called on
//!   accept, delete and replacement.

use crate::providers::claude_code::{LaunchExtras, PLAN_AGENT_DISALLOWED_TOOLS};
use crate::services::agent_plans::{self, AgentPanes};
use crate::services::{chat_host, mcp_tokens, plan_mcp, planner_prompts, settings as settings_service};
use crate::state::app_state::AppState;
use rusqlite::{params, OptionalExtension};

/// Refusal for a non-claude provider on a plan-agent session.
pub const PLAN_AGENT_PROVIDER_ONLY: &str = "Plan agents run on claude_code";

/// The live (not closed) plan whose agent `session_id` is: `(initiative_id, initiative_status)`.
pub fn plan_for_agent_session(state: &AppState, session_id: &str) -> Result<Option<(String, String)>, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT id, initiative_status FROM agent_plans WHERE briefing_session_id = ?1 AND status != 'closed' ORDER BY created_at DESC LIMIT 1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
}

/// Is `session_id` a live plan's agent session? FAILS CLOSED: a lookup error counts as "yes" (the
/// callers are guards — refuse input, exclude the pane, refuse a non-claude turn) and is logged.
pub fn is_plan_agent_session(state: &AppState, session_id: &str) -> bool {
    match plan_for_agent_session(state, session_id) {
        Ok(found) => found.is_some(),
        Err(error) => {
            tracing::warn!(%session_id, %error, "plan-agent lookup failed; treating the session as a plan agent");
            true
        }
    }
}

/// The refusal for closing/archiving/deleting a live plan agent from a terminal or session UI.
pub const PLAN_AGENT_TEARDOWN_REFUSED: &str =
    "This is a plan's agent: it ends with its plan (Start planning or Delete plan), not from here.";

/// `Err` when `session_id` is a live plan's agent (its plan not closed and still in briefing): only
/// the retire path (`retire_plan_agent_with` / `retire_agent_session_with`, which revokes the MCP
/// token) may end it. Fails closed on a lookup error.
pub fn refuse_human_teardown(state: &AppState, session_id: &str) -> Result<(), String> {
    let live = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_plans WHERE briefing_session_id = ?1 AND status != 'closed' AND initiative_status = 'briefing')",
            params![session_id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())
    });
    match live {
        Ok(false) => Ok(()),
        Ok(true) => Err(PLAN_AGENT_TEARDOWN_REFUSED.to_string()),
        Err(error) => {
            tracing::warn!(%session_id, %error, "plan-agent lookup failed; refusing the teardown");
            Err(PLAN_AGENT_TEARDOWN_REFUSED.to_string())
        }
    }
}

/// Is `session_id` the agent of ANY plan (live, accepted or closed)? The stricter check for raw
/// pane access: a plan agent's pane is never typed into or screen-scraped, whatever the plan state.
///
/// FAILS CLOSED: a lookup error counts as "yes" (refuse input, never capture the pane) and is logged.
pub fn is_any_plan_agent_session(state: &AppState, session_id: &str) -> bool {
    let found = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_plans WHERE briefing_session_id = ?1)",
            params![session_id],
            |r| r.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())
    });
    match found {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(%session_id, %error, "plan-agent lookup failed; treating the session as a plan agent");
            true
        }
    }
}

/// The refusal once the plan left briefing.
pub fn retired_error(initiative_status: &str) -> String {
    format!("This plan is in {} — the plan agent is retired.", initiative_status)
}

/// Pure: the extras for a plan-agent turn, given the config path.
pub fn launch_extras(mcp_config_path: &std::path::Path) -> LaunchExtras {
    LaunchExtras {
        mcp_config_path: mcp_config_path.to_string_lossy().to_string(),
        allowed_tools: plan_mcp::allowed_tool_names(),
        disallowed_tools: PLAN_AGENT_DISALLOWED_TOOLS.iter().map(|s| s.to_string()).collect(),
        append_system_prompt: planner_prompts::PLAN_AGENT_SYSTEM_PROMPT.to_string(),
    }
}

/// The launch extras for `session_id`'s next turn (see the module docs).
pub fn plan_agent_extras(state: &AppState, session_id: &str) -> Result<Option<LaunchExtras>, String> {
    let Some((initiative_id, initiative_status)) = plan_for_agent_session(state, session_id)? else {
        return Ok(None);
    };
    if initiative_status != "briefing" {
        return Err(retired_error(&initiative_status));
    }
    let store = settings_service::resolve_initiatives_dir(state);
    let path = mcp_tokens::mcp_config_path(&store, &initiative_id)?;
    if !path.is_file() || !mcp_tokens::has_live_token(state, &initiative_id, session_id)? {
        // The raw token is needed only to write the file: mint a fresh one (this revokes any older
        // live token of the plan) and write it.
        let raw = mcp_tokens::mint(state, &initiative_id, session_id)?;
        mcp_tokens::write_mcp_config(&store, &initiative_id, &raw)?;
    }
    tracing::debug!(%initiative_id, %session_id, "plan-agent launch extras ready");
    Ok(Some(launch_extras(&path)))
}

/// Retire a plan's agent (PR #75 rules, task 09): revoke its token + delete `mcp.json`, stop an
/// in-flight turn (not running is fine), archive the session with `agent_session_retired {reason}`
/// and kill its `johnnyone_<sid>` pane.
/// Best effort and idempotent: a failure is logged, never returned, so it cannot block the accept or
/// delete that called it (the briefing check already refuses a stale token).
pub(crate) async fn retire_plan_agent_with(state: &AppState, panes: &dyn AgentPanes, initiative_id: &str, reason: &str) {
    let why = match reason {
        "plan_accepted" => "accepted",
        "plan_deleted" => "deleted",
        other => other,
    };
    if let Err(error) = mcp_tokens::revoke_for_initiative(state, initiative_id, why) {
        tracing::warn!(%initiative_id, %error, "plan agent: token revoke failed");
    }
    let session: Result<Option<Option<String>>, String> = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT briefing_session_id FROM agent_plans WHERE id = ?1",
            params![initiative_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    });
    let sessions: Vec<String> = match session {
        Ok(found) => found.flatten().into_iter().collect(),
        Err(error) => {
            // Never skip the session revoke on a lookup error (F-3-39): fall back to every session
            // that ever held a token for this plan, and retire each (which revokes by session).
            tracing::warn!(%initiative_id, %error, "plan agent: briefing_session_id lookup failed; retiring by token holder");
            token_holders(state, initiative_id)
        }
    };
    for sid in sessions {
        retire_agent_session_now(state, panes, &sid, reason).await;
        // Explicit, so the revoke holds even if the retire above failed part-way.
        if let Err(error) = mcp_tokens::revoke_for_session(state, &sid, why) {
            tracing::warn!(%initiative_id, session_id = %sid, %error, "plan agent: session token revoke failed");
        }
    }
    // Retention: the turn transcripts (`plan-agent/turns/`) are kept through accept and removed with
    // the plan on delete — after the pane is killed, so nothing is still writing them.
    if reason == "plan_deleted" {
        remove_turns(state, initiative_id);
    }
}

/// Every session that ever held an MCP token for `initiative_id` (the retire fallback).
fn token_holders(state: &AppState, initiative_id: &str) -> Vec<String> {
    state
        .db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT DISTINCT session_id FROM mcp_tokens WHERE initiative_id = ?1 ORDER BY session_id")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![initiative_id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%initiative_id, %error, "plan agent: token-holder lookup failed");
            Vec::new()
        })
}

fn remove_turns(state: &AppState, initiative_id: &str) {
    let store = settings_service::resolve_initiatives_dir(state);
    let Ok(dir) = crate::services::plan_agent_tmux::turns_dir(&store, initiative_id) else { return };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%initiative_id, %error, "plan agent: removing turns/ failed"),
    }
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent); // only when empty (mcp.json is already gone)
    }
}

async fn retire_agent_session_now(state: &AppState, panes: &dyn AgentPanes, session_id: &str, reason: &str) {
    let _ = chat_host::stop_generation(state, session_id.to_string()).await;
    if let Err(error) = agent_plans::retire_agent_session_with(state, panes, session_id, reason).await {
        tracing::warn!(%session_id, %error, "plan agent: retire failed");
    }
}

/// Point a briefing plan at a NEW plan-agent session (the only code that changes
/// `briefing_session_id`). The old session's token stops working at once (its `SessionReplaced`
/// check, and the revoke here), its in-flight turn is stopped and it is retired
/// (`plan_agent_replaced`). The new session gets its token lazily on its first turn.
pub async fn replace_plan_agent_session(state: &AppState, initiative_id: &str, new_session_id: &str) -> Result<(), String> {
    replace_plan_agent_session_with(state, &agent_plans::LivePanes, initiative_id, new_session_id).await
}

pub(crate) async fn replace_plan_agent_session_with(
    state: &AppState,
    panes: &dyn AgentPanes,
    initiative_id: &str,
    new_session_id: &str,
) -> Result<(), String> {
    let old: Option<Option<String>> = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT briefing_session_id FROM agent_plans WHERE id = ?1 AND initiative_status = 'briefing' AND status != 'closed'",
            params![initiative_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })?;
    let Some(old) = old else {
        return Err("only a draft plan (in briefing) can get a new plan agent".to_string());
    };
    // Revoke first: from here on the old token is dead whatever happens next.
    mcp_tokens::revoke_for_initiative(state, initiative_id, "replaced")?;
    // Retire the old session while it is still the plan's agent, so its `agent_session_retired`
    // event is attributed to this plan.
    if let Some(old) = old.as_deref().filter(|o| *o != new_session_id) {
        retire_agent_session_now(state, panes, old, "plan_agent_replaced").await;
    }
    state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE agent_plans SET briefing_session_id = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![initiative_id, new_session_id],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::mcp::handle_mcp;
    use crate::providers::claude_code::build_config_with;
    use crate::services::agent_plans::PaneKillFuture;
    use crate::services::plan_mcp::fixture::{capture_logs, events, plan, use_store, Plan};
    use crate::test_support::test_state;
    use serde_json::json;
    use std::sync::Mutex;

    /// Records every pane kill; spawns nothing.
    #[derive(Default)]
    struct RecordingPanes {
        killed: Mutex<Vec<String>>,
    }

    impl AgentPanes for RecordingPanes {
        fn kill<'a>(&'a self, _state: &'a AppState, session_id: &'a str) -> PaneKillFuture<'a> {
            self.killed.lock().unwrap().push(session_id.to_string());
            Box::pin(async move { Ok(false) })
        }
    }

    fn flag_value<'a>(args: &'a [String], flag: &str) -> &'a str {
        let i = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag} missing"));
        &args[i + 1]
    }

    fn plan_args(extras: &LaunchExtras) -> Vec<String> {
        build_config_with("hello", "/ws", "", None, Some("cli-sid"), Some(extras)).args
    }

    fn token_row(state: &AppState, id: &str) -> Vec<(String, Option<String>)> {
        state
            .db
            .with_conn(|c| {
                let mut s = c
                    .prepare("SELECT session_id, revoked_at FROM mcp_tokens WHERE initiative_id = ?1 ORDER BY created_at, rowid")
                    .unwrap();
                let rows = s
                    .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
                Ok(rows)
            })
            .unwrap()
    }

    fn session_status(state: &AppState, sid: &str) -> String {
        state
            .db
            .with_conn(|c| c.query_row("SELECT status FROM sessions WHERE id = ?1", params![sid], |r| r.get(0)).map_err(|e| e.to_string()))
            .unwrap()
    }

    fn config_path(p: &Plan) -> std::path::PathBuf {
        mcp_tokens::mcp_config_path(&p.store, &p.id).unwrap()
    }

    fn write_call() -> Vec<u8> {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "brief_update", "arguments": { "field": "why", "add": "x" } } })
            .to_string()
            .into_bytes()
    }

    // ── task 07: launch extras ──

    #[test]
    fn plan_agent_args_least_privilege() {
        let args = plan_args(&launch_extras(std::path::Path::new("/store/P/plan-agent/mcp.json")));
        let allowed: Vec<&str> = flag_value(&args, "--allowedTools").split(',').collect();
        assert!(!allowed.is_empty());
        assert!(allowed.iter().all(|t| t.starts_with("mcp__j1__")), "{allowed:?}");
        assert_eq!(allowed, plan_mcp::allowed_tool_names().iter().map(String::as_str).collect::<Vec<_>>());
        let denied: Vec<&str> = flag_value(&args, "--disallowedTools").split(',').collect();
        for t in ["Read", "Grep", "Glob", "Bash", "Write", "Edit"] {
            assert!(denied.contains(&t), "{t} not denied");
        }
        // Primary control: `--tools ""` — no built-in tool exists for the plan agent.
        assert_eq!(args.iter().filter(|a| *a == "--tools").count(), 1);
        assert_eq!(flag_value(&args, "--tools"), "", "--tools must carry an EMPTY value");
        for t in ["Agent", "LSP", "Skill"] {
            assert!(denied.contains(&t), "{t} not denied");
        }
        // Ordinary claude_code chats are unchanged: no --tools at all.
        let plain = crate::providers::claude_code::build_config("hello", "/ws", "", None, Some("cli-sid")).args;
        assert!(!plain.iter().any(|a| a == "--tools"));
        assert_eq!(flag_value(&plain, "--permission-mode"), "bypassPermissions");
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert_eq!(flag_value(&args, "--permission-mode"), "default");
        assert!(!args.iter().any(|a| a.contains("bypassPermissions")));
        assert_eq!(flag_value(&args, "--mcp-config"), "/store/P/plan-agent/mcp.json");
        assert_eq!(flag_value(&args, "--append-system-prompt"), planner_prompts::PLAN_AGENT_SYSTEM_PROMPT);
        // The prompt stays the final positional, after a single-value flag.
        assert_eq!(args.last().unwrap(), "hello");
        assert_eq!(flag_value(&args, "--resume"), "cli-sid");
    }

    #[test]
    fn allowed_tools_are_mcp_j1_only() {
        let args = plan_args(&launch_extras(std::path::Path::new("/x/mcp.json")));
        let allowed: Vec<&str> = flag_value(&args, "--allowedTools").split(',').collect();
        assert_eq!(allowed.len(), 10);
        assert!(allowed.iter().all(|t| t.starts_with("mcp__j1__")));
        let registry: std::collections::BTreeSet<String> =
            plan_mcp::TOOLS.iter().map(|t| format!("mcp__j1__{}", t.name)).collect();
        let got: std::collections::BTreeSet<String> = allowed.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, registry);
        for builtin in ["Read", "Grep", "Glob", "LS", "Bash", "Write", "Edit", "MultiEdit", "NotebookEdit", "WebFetch", "WebSearch", "Task"] {
            assert!(!allowed.contains(&builtin), "{builtin} allowed");
        }
    }

    #[tokio::test]
    async fn launch_args_never_contain_token() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        mcp_tokens::revoke_for_initiative(&state, &p.id, "test").unwrap();
        let extras = plan_agent_extras(&state, &p.session).unwrap().expect("plan session");
        // The file now holds a NEW token; read it to know what must not leak.
        let text = std::fs::read_to_string(&extras.mcp_config_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let token = v["mcpServers"]["j1"]["headers"]["Authorization"].as_str().unwrap().trim_start_matches("Bearer ").to_string();
        let hash = mcp_tokens::sha256_hex(&token);
        assert_eq!(mcp_tokens::verify(&state, &token).unwrap().session_id, p.session);
        let cfg = build_config_with("hello", &p.ws.to_string_lossy(), "", None, None, Some(&extras));
        let joined = format!("{} {}", cfg.command, cfg.args.join(" "));
        let env: String = cfg.env_vars.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for secret in [&token, &hash] {
            assert!(!joined.contains(secret.as_str()), "argv holds token material");
            assert!(!env.contains(secret.as_str()), "env holds token material");
            assert!(!extras.mcp_config_path.contains(secret.as_str()), "path holds token material");
        }
        assert_eq!(cfg.env_vars, vec![("CLAUDECODE".to_string(), String::new())]);
        // The same turn as it really runs: inside the plan agent's tmux pane. Neither the tmux
        // command string nor the generated turn script holds token material (fake tmux; no CLI).
        let tmux = std::sync::Arc::new(crate::services::plan_agent_tmux::fake::FakeTmux::default());
        let (_proc, _rx) = crate::services::plan_agent_tmux::spawn_plan_agent_turn_with(
            &state,
            tmux.clone(),
            cfg.clone(),
            p.session.clone(),
            crate::providers::claude_code::parse_line,
            None,
        )
        .await
        .unwrap();
        let tmux_cmd: String = tmux.calls().iter().map(|c| c.join(" ")).collect::<Vec<_>>().join("\n");
        assert!(tmux_cmd.contains("new-session"), "the turn ran in tmux");
        let script = std::fs::read_to_string(
            crate::services::plan_agent_tmux::turns_dir(&store, &p.id).unwrap().join("1.sh"),
        )
        .unwrap();
        for secret in [&token, &hash] {
            assert!(!tmux_cmd.contains(secret.as_str()), "tmux command string holds token material");
            assert!(!script.contains(secret.as_str()), "turn script holds token material");
        }
        assert!(script.contains(&extras.mcp_config_path), "the pane command carries the config PATH");
    }

    #[test]
    fn logged_launch_args_contain_no_token() {
        let (state, _r) = test_state();
        let ((token, hash), text) = capture_logs(|rt| {
            rt.block_on(async {
                let store = use_store(&state);
                let p = plan(&state, &store, "A").await;
                mcp_tokens::revoke_for_initiative(&state, &p.id, "test").unwrap();
                let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
                let cfg = build_config_with("hello", &p.ws.to_string_lossy(), "", None, None, Some(&extras));
                // The exact line the CLI runner logs before every spawn.
                crate::providers::cli_runner::log_spawn(&cfg, &cfg.working_directory);
                let text = std::fs::read_to_string(&extras.mcp_config_path).unwrap();
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                let t = v["mcpServers"]["j1"]["headers"]["Authorization"].as_str().unwrap().trim_start_matches("Bearer ").to_string();
                let h = mcp_tokens::sha256_hex(&t);
                (t, h)
            })
        });
        assert!(text.contains("Spawning CLI subprocess"), "capture works:\n{text}");
        assert!(text.contains("--mcp-config"), "the args were logged:\n{text}");
        assert!(!text.contains(&token), "a log line holds the token");
        assert!(!text.contains(&hash), "a log line holds the hash");
    }

    #[tokio::test]
    async fn extras_none_for_non_plan_session() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let plain = crate::services::sessions::create_session(
            &state,
            crate::db::models::CreateSessionInput {
                provider: Some("claude_code".into()),
                model: None,
                working_directory: Some(p.ws.to_string_lossy().to_string()),
                title: Some("plain".into()),
                kind: Some("user".into()),
                setup_commands: None,
                tmux_session_name: None,
            },
        )
        .unwrap();
        assert_eq!(plan_agent_extras(&state, &plain.id).unwrap(), None);
        assert_eq!(plan_agent_extras(&state, "no-such-session").unwrap(), None);
        assert!(!is_plan_agent_session(&state, &plain.id));
        assert!(is_plan_agent_session(&state, &p.session));
        // A closed (deleted) plan's session is no longer a plan agent.
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET status='closed' WHERE id=?1", params![p.id]).map_err(|e| e.to_string()))
            .unwrap();
        assert_eq!(plan_agent_extras(&state, &p.session).unwrap(), None);
    }

    #[tokio::test]
    async fn extras_err_after_accept() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        state
            .db
            .with_conn(|c| agent_plans::apply_brief_acceptance_at(c, &p.id, "brief", Some(1)))
            .unwrap();
        let e = plan_agent_extras(&state, &p.session).unwrap_err();
        assert_eq!(e, "This plan is in planning — the plan agent is retired.");
        assert!(!config_path(&p).exists(), "no config written for a retired agent");
    }

    #[tokio::test]
    async fn extras_rewrite_config_when_missing_mints_new_token_and_revokes_old() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        // The fixture minted a token but wrote no file → the first turn mints + writes.
        let first = plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let path = config_path(&p);
        assert_eq!(first.mcp_config_path, path.to_string_lossy());
        let read_token = || {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            v["mcpServers"]["j1"]["headers"]["Authorization"].as_str().unwrap().trim_start_matches("Bearer ").to_string()
        };
        let t1 = read_token();
        assert!(matches!(mcp_tokens::verify(&state, &p.token), Err(mcp_tokens::Reject::Revoked { .. })), "fixture token revoked");
        // A second turn reuses the live token and the file.
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        assert_eq!(read_token(), t1);
        assert_eq!(token_row(&state, &p.id).iter().filter(|(_, r)| r.is_none()).count(), 1);
        // File gone → a fresh token, the old one revoked.
        std::fs::remove_file(&path).unwrap();
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let t2 = read_token();
        assert_ne!(t1, t2);
        assert!(matches!(mcp_tokens::verify(&state, &t1), Err(mcp_tokens::Reject::Revoked { .. })));
        assert_eq!(mcp_tokens::verify(&state, &t2).unwrap().initiative_id, p.id);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn system_prompt_mentions_tools_only_and_choices_line() {
        let p = planner_prompts::PLAN_AGENT_SYSTEM_PROMPT;
        assert!(p.contains("planning partner"));
        assert!(p.contains("ONLY through the j1 tools"));
        assert!(p.contains("clarifying questions"));
        assert!(p.contains("Never write code"));
        let last = p.lines().last().unwrap();
        assert!(last.starts_with("CHOICES: "), "{last}");
        assert_eq!(last.trim_start_matches("CHOICES: ").split(" | ").count(), 3);
        for t in plan_mcp::TOOLS.iter().filter(|t| t.name != "brief_versions") {
            assert!(p.contains(&format!("mcp__j1__{}", t.name)), "{} not mentioned", t.name);
        }
    }

    /// D1 (2026-11-23): the plan agent HAS a visible pane, but only its chat turns create it; the
    /// terminal paths may view an existing pane, never create one (that would start an unrestricted
    /// interactive CLI) and never type into it (data flows only through stream-json).
    #[tokio::test]
    async fn plan_agent_pane_is_view_only_and_created_by_turns() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let access = |sid: &str, exists: bool, input: bool| crate::terminal::plan_agent_pane_access_for_test(&state, sid, exists, input);
        assert!(access(&p.session, true, false).is_ok(), "viewing the live pane is allowed");
        let e = access(&p.session, false, false).unwrap_err();
        assert!(e.contains("first chat message"), "{e}");
        let e = access(&p.session, true, true).unwrap_err();
        assert!(e.contains("view-only"), "{e}");
        // Ordinary sessions are unaffected.
        for (exists, input) in [(false, false), (true, true), (false, true)] {
            assert!(access("plain-session", exists, input).is_ok());
        }
    }

    /// F-1-11: the guard follows the RESOLVED tmux target, not just the session id. A row attached
    /// to `johnnyone_<planSid>` (inserted directly — `create_session` refuses it, see below) cannot
    /// type into the plan agent's pane; an attachment to the user's own tmux is unaffected.
    #[tokio::test]
    async fn attached_session_targeting_plan_pane_cannot_type() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let insert = |id: &str, target: &str| {
            state
                .db
                .with_conn(|c| {
                    c.execute(
                        "INSERT INTO sessions (id, title, provider, model, working_directory, kind, tmux_session_name, attached_tmux) VALUES (?1, 't', 'shell', '', '/', 'user', ?2, 1)",
                        params![id, target],
                    )
                    .map_err(|e| e.to_string())
                })
                .unwrap();
        };
        let sneaky = uuid::Uuid::new_v4().to_string();
        insert(&sneaky, &format!("johnnyone_{}", p.session));
        let e = crate::terminal::plan_agent_input_check_for_test(&state, &sneaky).unwrap_err();
        assert!(e.contains("view-only"), "{e}");
        let mine = uuid::Uuid::new_v4().to_string();
        insert(&mine, "kord");
        assert!(crate::terminal::plan_agent_input_check_for_test(&state, &mine).is_ok());
        // The plan agent's own row is refused too.
        assert!(crate::terminal::plan_agent_input_check_for_test(&state, &p.session).is_err());
    }

    #[test]
    fn create_session_refuses_attaching_johnnyone_names() {
        let (state, _r) = test_state();
        let mk = |name: &str| {
            crate::services::sessions::create_session(
                &state,
                crate::db::models::CreateSessionInput {
                    provider: None,
                    model: None,
                    working_directory: Some("/".into()),
                    title: None,
                    kind: None,
                    setup_commands: None,
                    tmux_session_name: Some(name.into()),
                },
            )
        };
        let e = mk(&format!("johnnyone_{}", uuid::Uuid::new_v4())).unwrap_err();
        assert!(e.contains("J1-owned"), "{e}");
        assert!(mk("  johnnyone_anything").is_err(), "trimmed before the check");
        assert!(mk("kord").is_ok(), "the user's own tmux can still be attached");
    }

    /// F-2-24: a live plan agent can't be archived, deleted or have its pane killed from a terminal
    /// or the session UI (that would leave a briefing plan with a dead agent and a live token). Only
    /// the retire path ends it — and that still works and revokes the token.
    #[tokio::test]
    async fn plan_agent_cannot_be_closed_from_a_terminal_only_retired() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        // The guards refuse BEFORE touching tmux or the row.
        let e = crate::services::sessions::archive_session(&state, p.session.clone()).await.unwrap_err();
        assert_eq!(e, PLAN_AGENT_TEARDOWN_REFUSED);
        let e = crate::services::sessions::delete_session(&state, p.session.clone()).await.unwrap_err();
        assert_eq!(e, PLAN_AGENT_TEARDOWN_REFUSED);
        let e = crate::terminal::kill_terminal_session_human(&state, &p.session).await.unwrap_err();
        assert_eq!(e, PLAN_AGENT_TEARDOWN_REFUSED);
        assert_eq!(session_status(&state, &p.session), "active");
        assert!(mcp_tokens::has_live_token(&state, &p.id, &p.session).unwrap(), "token untouched");
        // An ordinary session is unaffected by the guard.
        assert!(refuse_human_teardown(&state, "plain-session").is_ok());
        // The retire path still ends it (and revokes the token).
        let panes = RecordingPanes::default();
        agent_plans::freeze_flip_and_retire_agent(&state, &panes, &p.id, None).await.unwrap();
        assert_eq!(session_status(&state, &p.session), "archived");
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()));
        assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()]);
        // Once the plan left briefing the guard no longer applies.
        assert!(refuse_human_teardown(&state, &p.session).is_ok());
    }

    /// F-2-25: the engine computes `planAgent` on the session (get + list), so the web never guesses
    /// from a title a rename could change.
    #[tokio::test]
    async fn session_plan_agent_flag_is_engine_computed() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let plain = crate::services::sessions::create_session(
            &state,
            crate::db::models::CreateSessionInput {
                provider: Some("claude_code".into()),
                model: None,
                working_directory: Some(p.ws.to_string_lossy().to_string()),
                title: Some("Plan agent - looks like one".into()),
                kind: None,
                setup_commands: None,
                tmux_session_name: None,
            },
        )
        .unwrap();
        assert!(crate::services::sessions::get_session(&state, p.session.clone()).unwrap().plan_agent);
        assert!(!crate::services::sessions::get_session(&state, plain.id.clone()).unwrap().plan_agent, "title is not the signal");
        let listed = crate::services::sessions::list_sessions_filtered(&state, None, None).unwrap();
        assert!(listed.iter().any(|s| s.id == p.session && s.plan_agent));
        assert!(listed.iter().any(|s| s.id == plain.id && !s.plan_agent));
        // On the wire (relay RPC and host GraphQL both serialise it as `planAgent`).
        let v = serde_json::to_value(crate::services::sessions::get_session(&state, p.session.clone()).unwrap()).unwrap();
        assert_eq!(v["plan_agent"], json!(true));
    }

    /// F-2-22: every plan-agent guard FAILS CLOSED when the lookup itself fails (here: the
    /// `agent_plans` table is unreadable). Input is refused, the pane is excluded from capture,
    /// teardown is refused and a non-claude turn is refused.
    #[tokio::test]
    async fn plan_agent_guards_fail_closed_on_lookup_error() {
        let (state, _r) = test_state();
        state
            .db
            .with_conn(|c| c.execute_batch("ALTER TABLE agent_plans RENAME TO agent_plans_unreadable").map_err(|e| e.to_string()))
            .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        assert!(is_plan_agent_session(&state, &sid));
        assert!(is_any_plan_agent_session(&state, &sid));
        assert!(crate::terminal::plan_agent_pane_access_for_test(&state, &sid, true, true).is_err(), "input refused");
        let listing = format!("johnnyone_{sid}\t%9\n");
        assert!(
            crate::services::question_watch::watchable_pane_ids(&listing, |s| is_any_plan_agent_session(&state, s)).is_empty(),
            "pane excluded from capture"
        );
        assert!(refuse_human_teardown(&state, &sid).is_err(), "teardown refused");
        assert!(crate::services::chat_host::claim_plan_agent_turn(&state, &sid, "codex").is_err(), "non-claude turn refused");
    }

    // ── task 09: revoke + retire ──

    #[tokio::test]
    async fn accept_revokes_token_and_retires_session() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        assert!(config_path(&p).exists());
        let turns = crate::services::plan_agent_tmux::turns_dir(&store, &p.id).unwrap();
        std::fs::create_dir_all(&turns).unwrap();
        std::fs::write(turns.join("1.jsonl"), "{}\n").unwrap();
        let panes = RecordingPanes::default();
        agent_plans::freeze_flip_and_retire_agent(&state, &panes, &p.id, None).await.unwrap();
        assert!(turns.join("1.jsonl").exists(), "F-1-15: accept keeps the transcript (deleted only with the plan)");
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()), "every token revoked");
        assert_eq!(session_status(&state, &p.session), "archived");
        let retired = events(&state, &p.id, "agent_session_retired");
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0]["sessionId"], json!(p.session));
        assert_eq!(retired[0]["role"], "plan_agent");
        assert_eq!(retired[0]["reason"], "plan_accepted");
        assert!(!config_path(&p).exists(), "mcp.json deleted");
        assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()], "its johnnyone_<sid> pane is killed");
        let run = agent_plans::get_plan(&state, &p.id).unwrap();
        assert_eq!(run.plan.initiative_status, "planning");
        assert!(run.events.iter().any(|e| e.summary.contains("plan accepted")), "timeline text");
    }

    #[tokio::test]
    async fn delete_revokes_token_and_retires_session() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let turns = crate::services::plan_agent_tmux::turns_dir(&store, &p.id).unwrap();
        std::fs::create_dir_all(&turns).unwrap();
        std::fs::write(turns.join("1.jsonl"), "{}\n").unwrap();
        let panes = RecordingPanes::default();
        agent_plans::delete_plan_with(&state, &panes, p.id.clone()).await.unwrap();
        assert!(!turns.exists(), "F-1-15: the turn transcripts are deleted with the plan");
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()));
        assert_eq!(session_status(&state, &p.session), "archived");
        let retired = events(&state, &p.id, "agent_session_retired");
        assert_eq!(retired.last().unwrap()["reason"], "plan_deleted");
        assert!(!config_path(&p).exists());
        assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()]);
        let r = handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        assert_eq!(r.status, 401);
    }

    #[tokio::test]
    async fn retire_revokes_token() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let panes = RecordingPanes::default();
        // Any retire path (incl. the reaper) revokes the session's token.
        assert!(agent_plans::retire_agent_session_with(&state, &panes, &p.session, "orphan_sweep:session_archived").await.unwrap());
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()));
        assert!(!std::path::Path::new(&extras.mcp_config_path).exists());
        assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()]);
        // Idempotent.
        assert!(!agent_plans::retire_agent_session_with(&state, &panes, &p.session, "again").await.unwrap());
    }

    #[tokio::test]
    async fn new_session_token_revokes_old() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let new_raw = mcp_tokens::mint(&state, &p.id, "S-NEW").unwrap();
        assert!(matches!(mcp_tokens::verify(&state, &p.token), Err(mcp_tokens::Reject::Revoked { .. })));
        assert_eq!(mcp_tokens::verify(&state, new_raw.expose()).unwrap().session_id, "S-NEW");
        let r = handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        assert_eq!(r.status, 401);
    }

    #[tokio::test]
    async fn replacement_stops_and_retires_old_session() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let fresh = crate::services::sessions::create_session(
            &state,
            crate::db::models::CreateSessionInput {
                provider: Some("claude_code".into()),
                model: None,
                working_directory: Some(p.ws.to_string_lossy().to_string()),
                title: Some("Plan agent - A (2)".into()),
                kind: Some("agent".into()),
                setup_commands: None,
                tmux_session_name: None,
            },
        )
        .unwrap();
        let panes = RecordingPanes::default();
        replace_plan_agent_session_with(&state, &panes, &p.id, &fresh.id).await.unwrap();
        assert_eq!(session_status(&state, &p.session), "archived");
        let retired = events(&state, &p.id, "agent_session_retired");
        assert_eq!(retired.last().unwrap()["reason"], "plan_agent_replaced");
        assert_eq!(retired.last().unwrap()["sessionId"], json!(p.session));
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()));
        assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()], "the old pane is killed");
        // The old session is no longer the plan's agent; the new one gets a fresh token on its first turn.
        assert_eq!(plan_agent_extras(&state, &p.session).unwrap(), None);
        assert!(plan_agent_extras(&state, &fresh.id).unwrap().is_some());
        assert!(mcp_tokens::has_live_token(&state, &p.id, &fresh.id).unwrap());
        // Replacement is only for draft plans.
        state
            .db
            .with_conn(|c| agent_plans::apply_brief_acceptance_at(c, &p.id, "b", Some(1)))
            .unwrap();
        assert!(replace_plan_agent_session_with(&state, &panes, &p.id, "x").await.is_err());
    }

    /// G (F-3-39): a DB error on the `briefing_session_id` lookup is logged and the session is
    /// STILL retired + revoked (found through its token row), not silently skipped.
    #[test]
    fn retire_survives_a_briefing_lookup_error() {
        let ((), logs) = capture_logs(|rt| {
            rt.block_on(async {
                let (state, _r) = test_state();
                let store = use_store(&state);
                let p = plan(&state, &store, "A").await;
                assert!(mcp_tokens::has_live_token(&state, &p.id, &p.session).unwrap());
                // Make the lookup fail (the column is gone); the token table still answers.
                state
                    .db
                    .with_conn(|c| {
                        c.execute("ALTER TABLE agent_plans RENAME COLUMN briefing_session_id TO briefing_gone", [])
                            .map_err(|e| e.to_string())
                    })
                    .unwrap();
                let panes = RecordingPanes::default();
                retire_plan_agent_with(&state, &panes, &p.id, "plan_deleted").await;
                assert_eq!(*panes.killed.lock().unwrap(), vec![p.session.clone()], "the agent's pane is still killed");
                let status: String = state
                    .db
                    .with_conn(|c| {
                        c.query_row("SELECT status FROM sessions WHERE id = ?1", params![p.session], |r| r.get(0))
                            .map_err(|e| e.to_string())
                    })
                    .unwrap();
                assert_eq!(status, "archived");
                assert!(!mcp_tokens::has_live_token(&state, &p.id, &p.session).unwrap());
            })
        });
        assert!(logs.contains("briefing_session_id lookup failed"), "{logs}");
    }

    #[tokio::test]
    async fn retired_session_cannot_call_mcp() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let before = crate::services::brief_store::version_count(&state, &p.id).unwrap();
        let ok = handle_mcp(&state, Some(&p.bearer()), &write_call()).await;
        assert_eq!(ok.status, 200);
        let panes = RecordingPanes::default();
        agent_plans::retire_agent_session_with(&state, &panes, &p.session, "plan_agent_replaced").await.unwrap();
        for method in ["initialize", "tools/list"] {
            let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method }).to_string().into_bytes();
            assert_eq!(handle_mcp(&state, Some(&p.bearer()), &body).await.status, 401);
        }
        assert_eq!(handle_mcp(&state, Some(&p.bearer()), &write_call()).await.status, 401);
        assert_eq!(crate::services::brief_store::version_count(&state, &p.id).unwrap(), before + 1);
    }

    /// Every way a plan agent ends (accept, delete, any retire) kills exactly its own
    /// `johnnyone_<sid>` pane through the pane seam, and a first turn creates that pane under the
    /// J1-owned name (fake tmux — no tmux server, no CLI).
    #[tokio::test]
    async fn plan_agent_pane_created_on_first_turn_and_killed_on_every_retire() {
        use crate::services::plan_agent_tmux::{fake::FakeTmux, spawn_plan_agent_turn_with};
        let (state, _r) = test_state();
        let store = use_store(&state);
        let panes = RecordingPanes::default();
        let a = plan(&state, &store, "A").await;
        let b = plan(&state, &store, "B").await;
        let c = plan(&state, &store, "C").await;
        for p in [&a, &b, &c] {
            let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
            let cfg = build_config_with("hi", &p.ws.to_string_lossy(), "", None, None, Some(&extras));
            let tmux = std::sync::Arc::new(FakeTmux::default());
            let _turn = spawn_plan_agent_turn_with(&state, tmux.clone(), cfg, p.session.clone(), crate::providers::claude_code::parse_line, None)
                .await
                .unwrap();
            let calls = tmux.calls();
            let new = calls.iter().find(|c| c[0] == "new-session").expect("first turn creates the pane");
            assert_eq!(new[3], format!("johnnyone_{}", p.session));
            assert!(uuid::Uuid::parse_str(&p.session).is_ok(), "johnnyone_<uuid>");
            assert_eq!(crate::terminal::owned_session_id_from_tmux_name(&new[3]), Some(p.session.clone()));
        }
        agent_plans::freeze_flip_and_retire_agent(&state, &panes, &a.id, None).await.unwrap();
        agent_plans::delete_plan_with(&state, &panes, b.id.clone()).await.unwrap();
        agent_plans::retire_agent_session_with(&state, &panes, &c.session, "x").await.unwrap();
        assert_eq!(*panes.killed.lock().unwrap(), vec![a.session.clone(), b.session.clone(), c.session.clone()]);
    }
}
