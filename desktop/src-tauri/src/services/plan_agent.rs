//! The Plans-tab plan agent (initiative 8ad3ae71 phase 02 tasks 07 and 09; master D1, D2, D5).
//!
//! The plan agent IS the plan's chat session (`agent_plans.briefing_session_id`) run by claude_code
//! on the existing headless chat paths (`chat_host::send_chat_message_blocking` and the relay's
//! `handle_relay_chat`): one `claude --print --resume` process per turn, never a tmux pane (D1). This
//! module only decides the EXTRA launch arguments for such a turn — it builds arguments and writes
//! the private MCP config, it never spawns anything:
//!
//! - [`plan_agent_extras`]: `None` for any session that is not a live plan's agent; `Err` once the
//!   plan left briefing (the turn is refused); otherwise ensures a live token for (plan, session),
//!   (re)writes `mcp.json` when it is missing (minting a fresh token, which revokes the old one —
//!   the raw token is only ever needed to write the file), and returns [`LaunchExtras`]: the config
//!   PATH, the ten `mcp__j1__*` tools as the only allowed tools, the built-in deny list and the
//!   plan-agent system prompt.
//! - [`retire_plan_agent_with`]: the PR #75 retire rules for the plan agent — revoke its token
//!   (deleting `mcp.json`), stop an in-flight turn, archive its session. Called on accept, delete and
//!   replacement.

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

/// Is `session_id` a live plan's agent session?
pub fn is_plan_agent_session(state: &AppState, session_id: &str) -> bool {
    matches!(plan_for_agent_session(state, session_id), Ok(Some(_)))
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
/// in-flight turn (not running is fine), archive the session with `agent_session_retired {reason}`.
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
    let session: Option<String> = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT briefing_session_id FROM agent_plans WHERE id = ?1",
                params![initiative_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .unwrap_or(None)
        .flatten();
    if let Some(sid) = session {
        retire_agent_session_now(state, panes, &sid, reason).await;
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

    #[tokio::test]
    async fn plan_agent_session_gets_no_terminal_pane() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let e = crate::terminal::refuse_plan_agent_pane_for_test(&state, &p.session).unwrap_err();
        assert!(e.contains("no terminal"), "{e}");
        assert!(crate::terminal::refuse_plan_agent_pane_for_test(&state, "plain-session").is_ok());
    }

    // ── task 09: revoke + retire ──

    #[tokio::test]
    async fn accept_revokes_token_and_retires_session() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        plan_agent_extras(&state, &p.session).unwrap().unwrap();
        assert!(config_path(&p).exists());
        let panes = RecordingPanes::default();
        agent_plans::freeze_flip_and_retire_agent(&state, &panes, &p.id, None).await.unwrap();
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()), "every token revoked");
        assert_eq!(session_status(&state, &p.session), "archived");
        let retired = events(&state, &p.id, "agent_session_retired");
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0]["sessionId"], json!(p.session));
        assert_eq!(retired[0]["role"], "plan_agent");
        assert_eq!(retired[0]["reason"], "plan_accepted");
        assert!(!config_path(&p).exists(), "mcp.json deleted");
        assert!(panes.killed.lock().unwrap().is_empty(), "no pane to kill");
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
        let panes = RecordingPanes::default();
        agent_plans::delete_plan_with(&state, &panes, p.id.clone()).await.unwrap();
        assert!(token_row(&state, &p.id).iter().all(|(_, revoked)| revoked.is_some()));
        assert_eq!(session_status(&state, &p.session), "archived");
        let retired = events(&state, &p.id, "agent_session_retired");
        assert_eq!(retired.last().unwrap()["reason"], "plan_deleted");
        assert!(!config_path(&p).exists());
        assert!(panes.killed.lock().unwrap().is_empty());
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
        assert!(panes.killed.lock().unwrap().is_empty());
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
        assert!(panes.killed.lock().unwrap().is_empty());
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

    #[tokio::test]
    async fn no_tmux_created() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let panes = RecordingPanes::default();
        let a = plan(&state, &store, "A").await;
        let b = plan(&state, &store, "B").await;
        let c = plan(&state, &store, "C").await;
        for p in [&a, &b, &c] {
            plan_agent_extras(&state, &p.session).unwrap().unwrap();
        }
        agent_plans::freeze_flip_and_retire_agent(&state, &panes, &a.id, None).await.unwrap();
        agent_plans::delete_plan_with(&state, &panes, b.id.clone()).await.unwrap();
        agent_plans::retire_agent_session_with(&state, &panes, &c.session, "x").await.unwrap();
        assert!(panes.killed.lock().unwrap().is_empty(), "the pane seam was asked to kill a plan-agent session");
        let attached: i64 = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sessions WHERE id IN (?1, ?2, ?3) AND tmux_session_name IS NOT NULL",
                    params![a.session, b.session, c.session],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(attached, 0, "no plan-agent session ever got a tmux pane");
    }
}
