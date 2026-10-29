//! Watchdog host slice tests.
//!
//! Proves that:
//! • watchdog_interval_clamp — both ends of the clamp
//! • watchdog_prompt_reject — control chars and over-600 chars
//! • watchdog_row_role — a session_reports row with role='watchdog' is read back
//! • watchdog_does_not_consume_brief — watchdog does NOT burn the user's brief
//! • watchdog_rpc_param — RPC accepts {sessionId: …} and errors on missing it
//! • host_schema — SDL includes updateSessionWatchdogPrompt, NOT updateSessionWatchdog

use crate::agent::message_types::RpcRequest;
use crate::agent::AgentService;
use crate::db::Database;
use crate::services::sessions;
use crate::state::app_state::AppState;
use std::sync::Arc;

fn test_state(sid: &str) -> AppState {
    let root = std::env::temp_dir().join(format!("wg-{sid}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Database::open(&root.join("test.db")).unwrap();
    let state = AppState::new(db);
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO sessions (id, title, provider, attached_tmux, tmux_session_name, status, watchdog_enabled)
             VALUES (?1, 'test', 'shell', 1, ?1, 'active', 0)",
            rusqlite::params![sid],
        ).map_err(|e| e.to_string())
    }).unwrap();
    state
}

// ===================================================================
// watchdog_interval_clamp
// ===================================================================
#[test]
fn watchdog_interval_clamp() {
    let state = test_state("clamp");
    // 1 clamps to 60
    sessions::set_session_watchdog(&state, "clamp", true, 1).unwrap();
    let row: (i64, i64) = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT watchdog_enabled, watchdog_interval_secs FROM sessions WHERE id = ?1",
            rusqlite::params!["clamp"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|e| e.to_string())
    }).unwrap();
    assert_eq!(row.0, 1);
    assert_eq!(row.1, 60);
    // 99999 clamps to 3600
    sessions::set_session_watchdog(&state, "clamp", true, 99999).unwrap();
    let iv: i64 = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT watchdog_interval_secs FROM sessions WHERE id = ?1",
            rusqlite::params!["clamp"],
            |r| r.get(0),
        ).map_err(|e| e.to_string())
    }).unwrap();
    assert_eq!(iv, 3600);
}

// ===================================================================
// watchdog_prompt_reject
// ===================================================================
#[test]
fn watchdog_prompt_reject() {
    let state = test_state("prej");
    // 601 chars exceeds cap
    let long = "x".repeat(601);
    let e = sessions::set_session_watchdog_prompt(&state, "prej", &long).unwrap_err();
    assert!(e.contains("600") || e.contains("exceed"));
    // embedded \r rejected
    let e2 = sessions::set_session_watchdog_prompt(&state, "prej", "a\rb").unwrap_err();
    assert!(e2.contains("control"));
    // embedded \n rejected
    let e3 = sessions::set_session_watchdog_prompt(&state, "prej", "a\nb").unwrap_err();
    assert!(e3.contains("control"));
    // \t is ok
    sessions::set_session_watchdog_prompt(&state, "prej", "a\tb").unwrap();
}

// ===================================================================
// watchdog_row_role
// ===================================================================
#[tokio::test]
async fn watchdog_row_role() {
    let state = Arc::new(test_state("wrr"));
    sessions::set_session_watchdog(&state, "wrr", true, 120).unwrap();
    sessions::set_session_watchdog_prompt(&state, "wrr", "still there?").unwrap();
    AgentService::deliver_chat_message(&state, "wrr", "still there?", "watchdog").await.unwrap();
    let collected_rows: Vec<(String, String)> = state.db.with_conn(|conn| {
        let mut s = conn.prepare(
            "SELECT session_id, role FROM session_reports WHERE session_id = ?1 ORDER BY created_at"
        ).map_err(|e| e.to_string())?;
        let data = s.query_map(rusqlite::params!["wrr"], |r| {
            Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))
        }).map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
        Ok(data)
    }).unwrap();
    let wr = collected_rows.iter().find(|(_, role)| role == "watchdog");
    assert!(wr.is_some(), "expected watchdog row, got {collected_rows:?}");
    assert_eq!(wr.unwrap().0, "wrr");
}

// ===================================================================
// watchdog_does_not_consume_brief
// ===================================================================
#[tokio::test]
async fn watchdog_does_not_consume_brief() {
    let state = Arc::new(test_state("noburn"));
    sessions::set_session_watchdog(&state, "noburn", true, 120).unwrap();
    sessions::set_session_watchdog_prompt(&state, "noburn", "still there?").unwrap();
    // watchdog first — should NOT consume the brief slot
    AgentService::deliver_chat_message(&state, "noburn", "still there?", "watchdog").await.unwrap();
    {
        let briefed = state.shell_briefed.lock().await;
        assert!(!briefed.contains("noburn"),
            "watchdog delivery must NOT add session to shell_briefed");
    }
    // user after watchdog — MUST consume the brief slot
    let _ = AgentService::deliver_chat_message(&state, "noburn", "hello", "user").await;
    {
        let briefed = state.shell_briefed.lock().await;
        assert!(briefed.contains("noburn"),
            "user delivery must add session to shell_briefed");
    }
}

// ===================================================================
// watchdog_rpc_param
// ===================================================================
#[test]
fn watchdog_rpc_param() {
    let state = test_state("rpc");
    sessions::set_session_watchdog(&state, "rpc", true, 60).unwrap();
    sessions::set_session_watchdog_prompt(&state, "rpc", "hello").unwrap();

    // Build a req that the worker actually sends: {sessionId, enabled, intervalSecs}
    let ok_req = RpcRequest {
        request_id: "t1".into(),
        method: "update_session_watchdog".into(),
        params: serde_json::json!({"sessionId": "rpc", "enabled": true, "intervalSecs": 300}),
    };
    let state = Arc::new(state);
    let resp = tokio::runtime::Runtime::new().unwrap().block_on(
        AgentService::handle_rpc(&ok_req, &state)
    );
    assert!(resp.success, "valid sessionId must succeed: {:?}", resp.error);
    // Verify the row actually changed
    let iv: i64 = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT watchdog_interval_secs FROM sessions WHERE id = 'rpc'",
            [],
            |r| r.get(0),
        ).map_err(|e| e.to_string())
    }).unwrap();
    assert_eq!(iv, 300, "watchdog interval must have been updated in DB");

    // The same payload with sessionId renamed to id must ERROR — that catches
    // a regression back to params["id"].
    let bad_req = RpcRequest {
        request_id: "t2".into(),
        method: "update_session_watchdog".into(),
        params: serde_json::json!({"id": "rpc", "enabled": true, "intervalSecs": 300}),
    };
    let resp2 = tokio::runtime::Runtime::new().unwrap().block_on(
        AgentService::handle_rpc(&bad_req, &state)
    );
    assert!(!resp2.success, "missing sessionId must fail, got: {:?}", resp2.error);
    assert!(resp2.error.as_ref().unwrap().contains("sessionId"),
        "error must mention sessionId, got: {:?}", resp2.error);
}

// ===================================================================
// host_schema
// ===================================================================
#[test]
fn host_schema() {
    let sdl = crate::host::schema_sdl();
    assert!(sdl.contains("updateSessionWatchdogPrompt("),
        "SDL must contain updateSessionWatchdogPrompt( mutation, got SDL:\n{sdl}");
    assert!(!sdl.contains("updateSessionWatchdog("),
        "SDL must NOT contain updateSessionWatchdog( mutation (interval is RPC-only), got SDL:\n{sdl}");
}