//! The DEVELOPMENT review fan-out is still pane-only. kart will die on it.
//!
//! WHY THIS IS NOT A TIDY-UP. `run_planning_lens_fanout_review` learned the
//! stream transport on 2026-10-24 and was proved live: glm reviewed the kart
//! plan in 37 seconds with zero `<tool_call>` markers. Its twin,
//! `run_lens_fanout_review` (`agent_plans.rs:6138`, the per-PHASE development
//! review), was never touched. It still does this for every lens, whatever the
//! provider:
//!
//! ```text
//! let sid = spawn_ephemeral_agent(state, &plan_id, &desc.name, &desc.provider, …)
//! ```
//!
//! And `lens_transport(CliProvider::Lokalai)` is `LensTransport::Stream`
//! precisely because *"`lokalai`'s command IS curl: no pane to drive, no shell
//! to report from"* (`stream_lens.rs:64`).
//!
//! So the development fan-out will open a tmux pane whose command is `curl`,
//! wait for a verdict nobody can type into it, and fail the way the planning
//! path failed twice:
//!
//! ```text
//! 01:18:38  coordinator_failed {"reason":"can't find pane: %2424"}
//! ```
//!
//! THE PART THAT MAKES IT URGENT. kart's `validation_config` is
//! `[{"name":"glm-validator","provider":"lokalai","model":"glm","blocking":true}]`,
//! and `auto_start_development` copies `validation_config` onto the development
//! run it creates (`copy_initiative_json_fields`). So the moment the handoff is
//! unblocked and kart's first phase finishes building, its phase review walks
//! into this. The plan review is fixed; the phase review is the same bug waiting
//! one step further down the road.
//!
//! ONE HELPER, NOT A THIRD COPY. This is the sixth instance on these branches of
//! a correct module with an unexamined call site, and the second of this exact
//! pair of fan-outs diverging. The fix is not "teach the development fan-out the
//! same trick" — that leaves two copies to drift. It is one shared per-lens
//! dispatch both fan-outs call. `f4` is the test that fails if a third copy ever
//! appears.

use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::state::app_state::AppState;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const SSE_NEEDS_CHANGES: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Task 02 has no test. \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"VERDICT: NEEDS_CHANGES\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// Serve one request and keep what was sent.
fn serve_once_capturing(body: &'static str) -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a throwaway port");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the lens must connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(1500)))
            .ok();
        let mut all = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
            if all.windows(4).any(|w| w == b"\r\n\r\n") && all.len() > 64 {
                if let Ok(n2) = stream.read(&mut buf) {
                    all.extend_from_slice(&buf[..n2]);
                }
                break;
            }
        }
        *sink.lock().unwrap() = String::from_utf8_lossy(&all).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (port, seen)
}

/// A DEVELOPMENT run parked at a phase review, with one lens on `provider`.
/// `endpoint` is `None` for a provider that needs no HTTP config.
fn seed_development(tag: &str, provider: &str, endpoint: Option<u16>) -> (AppState, String, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("devlens-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    let db = Database::open(&root.join("test.db")).expect("open db");
    let state = AppState::new(db);
    let plan_id = format!("devlens-{tag}");
    let workspace = root.join("ws");
    std::fs::create_dir_all(workspace.join("plan/phases/01-foundation")).expect("plan dirs");
    std::fs::write(workspace.join("plan/overview.md"), "# Plan\n\n## Phases\n").unwrap();
    std::fs::write(
        workspace.join("plan/phases/01-foundation/overview.md"),
        "# 01 foundation\n\nThe palette tokens and the type faces.\n",
    )
    .unwrap();

    let pid = plan_id.clone();
    let ws = workspace.to_string_lossy().to_string();
    let pp = workspace.join("plan").to_string_lossy().to_string();
    let cfg = format!(
        r#"[{{"name":"glm-validator","provider":"{provider}","model":"glm","blocking":true}}]"#
    );
    let ep = endpoint.map(|p| format!("{{\"endpoint\":\"http://127.0.0.1:{p}/v1\"}}"));
    state
        .db
        .with_conn(move |conn| {
            if let Some(settings) = ep {
                conn.execute(
                    "INSERT INTO provider_configs (id, provider, cli_path, api_key, default_model, settings, is_available, updated_at)
                     VALUES ('devlens-cfg', 'lokalai', 'curl', 'sk-devlens', 'glm', ?1, 1, datetime('now'))",
                    rusqlite::params![settings],
                )
                .map_err(|e| e.to_string())?;
            }
            conn.execute(
                "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status,
                                          worker_session_id, reviewer_session_id, worker_provider,
                                          reviewer_provider, current_phase_id, current_phase_index,
                                          initiative_id, initiative_status, health, validation_config)
                 VALUES (?1, 'development', 'kart - the visual identity pass', ?2, ?3,
                         'phase_review_running', NULL, NULL, 'claude', 'claude',
                         '01-foundation', 0, ?1, 'development', 'in-progress', ?4)",
                rusqlite::params![pid, ws, pp, cfg],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO agent_plan_phases (id, plan_id, phase_id, phase_title, phase_index, status)
                 VALUES (?1, ?2, '01-foundation', 'foundation', 0, 'review_running')",
                rusqlite::params![format!("{pid}-ph"), pid],
            )
            .map_err(|e| e.to_string())
        })
        .expect("seed a development run at a phase review");

    (state, plan_id, root)
}

fn session_ids(state: &AppState) -> Vec<String> {
    state
        .db
        .with_conn(|conn| {
            let mut st = conn.prepare("SELECT id FROM sessions").map_err(|e| e.to_string())?;
            let rows = st
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
        })
        .unwrap_or_default()
}

/// Sessions created for a given PROVIDER. This, not the total, is the question:
/// see `f1`'s note on the T1 worker.
fn sessions_for_provider(state: &AppState, provider: &str) -> Vec<String> {
    let p = provider.to_string();
    state
        .db
        .with_conn(move |conn| {
            let mut st = conn
                .prepare("SELECT COALESCE(title, id) FROM sessions WHERE provider = ?1")
                .map_err(|e| e.to_string())?;
            let rows = st
                .query_map([p], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
        })
        .unwrap_or_default()
}

fn session_count(state: &AppState) -> i64 {
    state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .map_err(|e| e.to_string())
        })
        .expect("count sessions")
}

/// f1 — a development phase review on a stream provider must NOT open a pane.
///
/// The exact assertion that caught the planning path twice, moved one step down
/// the road: the verdict reaches the plan's events, and no session row exists,
/// because a session row IS `spawn_ephemeral_agent` and its pane is what died as
/// `%2375` and `%2424`.
#[tokio::test]
async fn f1_a_development_phase_review_streams_and_creates_no_pane() {
    let (port, _seen) = serve_once_capturing(SSE_NEEDS_CHANGES);
    let (state, id, root) = seed_development("stream", "lokalai", Some(port));

    let run = agent_plans::get_plan(&state, &id).expect("load the seeded run");
    let _ = agent_plans::run_lens_fanout_review_for(&state, &run).await;

    let after = agent_plans::get_plan(&state, &id).expect("reload");
    let verdicts: Vec<&str> = after
        .events
        .iter()
        .filter(|e| e.event_type == "agent_lens_verdict")
        .map(|e| e.payload_json.as_str())
        .collect();
    assert!(
        verdicts.iter().any(|p| p.contains("NEEDS_CHANGES")),
        "the development fan-out produced no verdict for a stream lens — it is \
         still driving a curl command as a tmux pane. Events: {:?}",
        after.events.iter().map(|e| e.event_type.as_str()).collect::<Vec<_>>()
    );
    // WHAT THIS ASKS, AND WHY IT IS NOT `sessions == 0`.
    //
    // s8 asks exactly that of the PLANNING fan-out, and it is right there: a
    // planning review has no builder to talk to, so any session at all is the
    // lens's pane. A development phase review is different, and I had this
    // wrong at first — the assertion failed on a correct fix. Probed it rather
    // than guessing: with the socket serving a real `VERDICT: NEEDS_CHANGES`,
    // one session exists and it is
    //
    //     ("claude", "T1 Worker - t")   via agent_feedback_sent_to_worker
    //
    // i.e. the phase review did its job and sent the findings back to the
    // BUILDER. That session is the feature, not the defect. The lens is on
    // provider `lokalai`, so the honest question is whether a session exists
    // for THAT provider: a lokalai session row is `spawn_ephemeral_agent`
    // opening a tmux pane whose command is `curl`, and nothing else creates one.
    let panes = sessions_for_provider(&state, "lokalai");
    assert!(
        panes.is_empty(),
        "the development fan-out created {} session(s) on provider lokalai: \
         {panes:?}. That is spawn_ephemeral_agent, and a lokalai session is a \
         tmux pane whose command is `curl` waiting for a verdict nobody can type \
         into it. kart's own validation_config names provider lokalai, so this \
         is the pane that would fail its first phase review.",
        panes.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// f2 — THE CONTROL ARM. A pane provider must still get a pane.
///
/// The cheapest way to make `f1` green is to stop spawning reviewers at all.
/// `claude`/`codex`/`grok` lenses are interactive CLIs with a TUI to drive and a
/// shell to report from; they are the existing, working path and must not be
/// collateral damage.
#[tokio::test]
async fn f2_a_pane_provider_still_gets_a_pane() {
    let (state, id, root) = seed_development("pane", "claude", None);

    let run = agent_plans::get_plan(&state, &id).expect("load the seeded run");
    // The spawn itself will not complete in a test environment — no CLI, no
    // tmux server to attach to — so the outcome is irrelevant. What matters is
    // that the fan-out went down the SESSION path for a pane provider.
    let _ = agent_plans::run_lens_fanout_review_for(&state, &run).await;

    let n = session_count(&state);

    // CLEAN UP THE PANES THIS TEST CREATED, before asserting anything, so a
    // failing assertion cannot leak them either. Measured on 2026-10-24: this
    // test left two live `claude` processes in tmux sessions behind, and the
    // same leak in production had accumulated 242 orphaned sessions holding
    // 27 GB — J1 had deleted their `sessions` rows and never killed the panes.
    // Only `johnnyone_<uuid>` names are touched; nothing else is ours to kill.
    for sid in session_ids(&state) {
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &format!("johnnyone_{sid}")])
            .output();
    }

    assert!(
        n >= 1,
        "a `claude` lens created no session at all. The stream path was applied \
         to a provider that has a TUI to drive and a shell to report from, which \
         breaks every lens that works today."
    );
    // And it must be the LENS's session, not the T1 worker's. Without this, the
    // stream path could be applied to `claude` and f2 would still pass on the
    // feedback-to-worker session f1 documents. `spawn_ephemeral_agent` titles a
    // session "<ROLE> · <short>", and the role here is the lens name.
    let titles = sessions_for_provider(&state, "claude");
    assert!(
        titles.iter().any(|t| t.to_lowercase().contains("glm-validator")),
        "no session belongs to the LENS — sessions on provider claude are \
         {titles:?}. A pane lens must get its own ephemeral agent; a session \
         that only exists because findings were sent to the builder is not one."
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// f3 — the development prompt on the wire must be a real review request.
///
/// m6 and c5 proved the planning prompt carries its material, the no-tools
/// statement and a VERDICT request. None of that says anything about the
/// development prompt, which is built by a different function
/// (`lens_reviewer_prompt`, not `planning_lens_reviewer_prompt`). A development
/// lens that is handed paths it cannot read is the 02:00:07 BLOCKED again, just
/// about a phase.
#[tokio::test]
async fn f3_the_development_prompt_on_the_wire_is_reviewable() {
    let (port, seen) = serve_once_capturing(SSE_NEEDS_CHANGES);
    let (state, id, root) = seed_development("wire", "lokalai", Some(port));

    let run = agent_plans::get_plan(&state, &id).expect("load the seeded run");
    let _ = agent_plans::run_lens_fanout_review_for(&state, &run).await;

    let body = seen.lock().unwrap().clone();
    assert!(
        !body.is_empty(),
        "the socket saw no request, so this test proves nothing either way — the \
         fan-out never reached the provider"
    );
    let wire = body.to_lowercase();
    assert!(
        wire.contains("no shell") || wire.contains("no tools") || wire.contains("no filesystem"),
        "the development prompt on the wire never tells the reviewer it has no \
         tools. That statement is what stopped glm spending its whole reply on \
         blocked tool calls at 02:00:07.\n--- first 1200 ---\n{}",
        &body.chars().take(1200).collect::<String>()
    );
    assert!(
        wire.contains("verdict:"),
        "the development prompt never asks for a VERDICT line, so nothing the \
         reviewer returns can be read back and the review is recorded as an \
         error.\n--- first 1200 ---\n{}",
        &body.chars().take(1200).collect::<String>()
    );
    assert!(
        body.contains("The palette tokens and the type faces."),
        "the PHASE's own overview never reached the provider. A tool-less \
         reviewer asked to judge phase 01-foundation was sent a path to it \
         instead of its text.\n--- first 1200 ---\n{}",
        &body.chars().take(1200).collect::<String>()
    );
    assert!(
        body.contains("01-foundation"),
        "the prompt does not name the phase under review"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// f4 — ONE place decides the transport.
///
/// This is a source-text assertion, deliberately. Both fan-outs diverging is the
/// defect; "teach the second one the same trick" leaves two copies to drift, and
/// the next lens caller would make three. The only durable shape is one shared
/// per-lens dispatch, so this counts the places that branch on the transport and
/// requires exactly one. It is the cheapest possible guard against the mistake
/// that has now cost two live runs, and it fails loudly the moment someone
/// copy-pastes the branch instead of calling the helper.
#[test]
fn f4_only_one_call_site_chooses_the_lens_transport() {
    let src = include_str!("../src/services/agent_plans.rs");
    // Exactly one branch on the transport. This already holds today — the
    // planning fan-out owns the only one — and it must STILL hold after the
    // development fan-out learns to stream. The failure this forecloses is the
    // obvious fix: paste the same `if lens_transport(..) == Stream` block into
    // the second fan-out, leaving two copies to drift the way these two already
    // drifted for two days.
    let decisions = src.matches("lens_transport(").count();
    assert_eq!(
        decisions, 1,
        "agent_plans.rs branches on lens_transport() in {decisions} place(s). \
         Exactly one shared per-lens dispatch may decide it; a second copy is \
         the same bug again, one refactor later."
    );

    // MEASURED, so this is satisfiable: `spawn_ephemeral_agent(` appears four
    // times on this branch's base commit — its own definition (:5410), the
    // development fan-out (:6153), the planning fan-out (:6436), and the
    // docs-commit agent (:6648), which is not a lens and is out of scope. The
    // two lens call sites must collapse into one shared dispatch, so three is
    // the number: definition, shared dispatch, docs-commit.
    let spawns = src.matches("spawn_ephemeral_agent(").count();
    assert_eq!(
        spawns, 3,
        "spawn_ephemeral_agent( appears {spawns} times, expected 3: its own \
         definition, the ONE shared lens dispatch, and the docs-commit agent \
         (which is not a lens — leave it alone). Four means the two fan-outs \
         still each spawn their own reviewers."
    );
}
