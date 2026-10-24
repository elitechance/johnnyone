//! The conventions are a DIRECTORY. Inline its files, not its error.
//!
//! FOUND BY THE LIVE RUN, not by a gate. At 14:02:40 on 2026-10-24 glm reviewed
//! the kart plan over the stream transport and returned PASS_WITH_FOLLOWUPS in
//! 37 seconds with zero `<tool_call>` markers — the material reached it. But its
//! finding 7 reported a limitation of its own review: the CONVENTIONS section it
//! was given read, in full,
//!
//! ```text
//! (this document could not be read from the host filesystem: Is a directory (os error 21))
//! ```
//!
//! 88 characters where the review checklists should have been. `conventions_path`
//! is a directory — every planner prompt in `planner_prompts.rs` says so in
//! words: *"project conventions (every file under {{conventions_path}})"*, and
//! three of them name `{{conventions_path}}/review-lenses.md` as the file the
//! lenses are defined in. `stream_lens_material` calls `read_lens_doc_body` on
//! that directory path, and `read_lens_doc_body` is `std::fs::read_to_string`.
//!
//! So the reviewer that is supposed to judge a plan against the review lenses
//! has never once been shown them. The graceful-degradation design worked —
//! glm said so instead of inventing compliance — which is the only reason this
//! is a bug report and not a false PASS. That is also why it is worth fixing
//! properly rather than papering over: the honest reviewer told us what it could
//! not see, and what it could not see was the checklist.
//!
//! WHAT `c5` IS FOR. Five times on these branches a gate proved a module and
//! said nothing about its call site, and every time the defect shipped. So `c5`
//! does not inspect a formatter: it drives the real fan-out against a socket
//! that keeps the request body, and requires a sentence that exists only inside
//! a file in the conventions DIRECTORY to come out on the wire. A perfect
//! expander that `stream_lens_material` never calls cannot pass it.

use johnnyone_desktop_lib::db::Database;
use johnnyone_desktop_lib::services::agent_plans;
use johnnyone_desktop_lib::services::stream_lens::{expand_lens_docs, LENS_MATERIAL_MAX_FILES};
use johnnyone_desktop_lib::state::app_state::AppState;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn tmp(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("convdir-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// c1 — a directory becomes one labelled document per file, and each one says
/// which file it is.
///
/// A single blob labelled "CONVENTIONS" would technically carry the bytes, but
/// the reviewer is asked to judge against `review-lenses.md` by name; it has to
/// be able to tell which text came from which file.
#[test]
fn c1_a_directory_is_inlined_as_its_files() {
    let root = tmp("c1");
    let conv = root.join("conventions");
    std::fs::create_dir_all(&conv).expect("conventions dir");
    std::fs::write(conv.join("review-lenses.md"), "# Lenses\n\nProduct, QA, Lead.\n").unwrap();
    std::fs::write(conv.join("commit-style.md"), "# Commits\n\nImperative mood.\n").unwrap();

    let docs = expand_lens_docs("CONVENTIONS", &conv, LENS_MATERIAL_MAX_FILES);

    assert_eq!(
        docs.len(),
        2,
        "a conventions directory with two files produced {} document(s). Today it \
         produces one, whose body is 'Is a directory (os error 21)'.",
        docs.len()
    );
    let joined: String = docs
        .iter()
        .map(|d| format!("{}|{}|{}\n", d.label, d.path, d.body))
        .collect();
    assert!(
        joined.contains("Product, QA, Lead."),
        "the review lenses never made it into any document: {joined}"
    );
    assert!(
        joined.contains("Imperative mood."),
        "a second conventions file was dropped: {joined}"
    );
    assert!(
        docs.iter().any(|d| d.path.ends_with("review-lenses.md")),
        "no document names review-lenses.md as its path, so the reviewer cannot \
         tell which text is the checklist it is being measured against: {joined}"
    );
    assert!(
        docs.iter().all(|d| d.label.contains("CONVENTIONS")),
        "a document lost the CONVENTIONS label and will be inlined as something \
         else entirely: {joined}"
    );

    // Order must be SORTED, not merely stable. Two mutations — dropping the
    // sort, and reversing it — both survived a stability-only assertion on
    // 2026-10-24, because `read_dir` happens to be stable within one run on
    // this filesystem. Stability was never the point:
    //
    //   * the provider's prefix cache keys on the prompt's leading bytes, and
    //   * the real conventions store is 66,970 bytes against an 80,000-byte
    //     material budget, so `inline_material` WILL truncate — and the order
    //     decides which conventions the reviewer gets to see. An order nobody
    //     pinned means the file that falls off the end changes with the
    //     filesystem.
    let paths: Vec<String> = docs.iter().map(|d| d.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(
        paths, sorted,
        "the expansion is not in sorted path order, so which conventions survive \
         the byte budget is down to read_dir order"
    );
    let second = expand_lens_docs("CONVENTIONS", &conv, LENS_MATERIAL_MAX_FILES);
    assert_eq!(
        paths,
        second.iter().map(|d| d.path.clone()).collect::<Vec<_>>(),
        "two expansions of the same directory disagreed"
    );
}

/// c2 — nested directories count. `review-lenses.md` may sit beside a
/// subdirectory of per-area conventions, and the prompt says "every file under".
#[test]
fn c2_subdirectories_are_included() {
    let root = tmp("c2");
    let conv = root.join("conventions");
    std::fs::create_dir_all(conv.join("areas")).expect("nested dir");
    std::fs::write(conv.join("review-lenses.md"), "# Lenses\n").unwrap();
    std::fs::write(conv.join("areas/angular.md"), "Zoneless by default.\n").unwrap();

    let docs = expand_lens_docs("CONVENTIONS", &conv, LENS_MATERIAL_MAX_FILES);
    let joined: String = docs.iter().map(|d| d.body.clone()).collect();
    assert!(
        joined.contains("Zoneless by default."),
        "a conventions file one level down was not inlined. The planner prompts \
         say 'every file under {{conventions_path}}', and the real store has \
         subdirectories: {} document(s) found",
        docs.len()
    );
}

/// c3 — the expansion is BOUNDED, and any cut is named with a count.
///
/// This is the same rule `inline_material` already follows for bytes: a silent
/// truncation is worse than a small one, because the reviewer cannot tell that
/// it is judging against a partial checklist. An unbounded expansion is also a
/// way to blow the context window on a store nobody pruned.
#[test]
fn c3_too_many_files_are_cut_and_the_cut_is_named() {
    let root = tmp("c3");
    let conv = root.join("conventions");
    std::fs::create_dir_all(&conv).expect("conventions dir");
    let over = LENS_MATERIAL_MAX_FILES + 7;
    for i in 0..over {
        std::fs::write(conv.join(format!("{i:03}-rule.md")), format!("rule {i}\n")).unwrap();
    }

    let docs = expand_lens_docs("CONVENTIONS", &conv, LENS_MATERIAL_MAX_FILES);
    assert!(
        docs.len() <= LENS_MATERIAL_MAX_FILES + 1,
        "{} files produced {} documents — the expansion is unbounded, so a \
         conventions store nobody pruned becomes the whole prompt",
        over,
        docs.len()
    );
    // WHICH files survive the cut is decided by the order, and this is the only
    // test with enough files to see it. Measured on 2026-10-24: `read_dir` over
    // 30 files returns them in neither creation nor lexicographic order on
    // either this tmpfs or this ext4 — on ext4 the order is hash-scrambled
    // (`019, 021, 008, 004 …`). c1's two-file directory happened to come back
    // sorted, which is why a mutation that DELETED the sort survived it. Here it
    // cannot: the documents shown must be the lexicographically first
    // `LENS_MATERIAL_MAX_FILES`, so the reviewer sees the same conventions on
    // every host.
    let shown: Vec<String> = docs
        .iter()
        .filter(|d| d.path.ends_with("-rule.md"))
        .map(|d| d.path.clone())
        .collect();
    let mut expected: Vec<String> = (0..over)
        .map(|i| conv.join(format!("{i:03}-rule.md")).to_string_lossy().to_string())
        .collect();
    expected.sort();
    expected.truncate(LENS_MATERIAL_MAX_FILES);
    assert_eq!(
        shown, expected,
        "the files kept past the cap are not the lexicographically first \
         {LENS_MATERIAL_MAX_FILES} — so which conventions the reviewer sees is \
         down to read_dir order, which is hash-scrambled on ext4"
    );

    let joined: String = docs.iter().map(|d| d.body.clone()).collect();
    assert!(
        joined.contains("7 more") || joined.contains(&format!("{} more", over - LENS_MATERIAL_MAX_FILES)),
        "the expansion dropped {} files and said nothing about it. A reviewer \
         judging against a silently partial checklist reports a PASS it has not \
         earned — name the count, as inline_material does for bytes.\n{joined}",
        over - LENS_MATERIAL_MAX_FILES
    );
}

/// c4 — the control arm, twice over.
///
/// An absence assertion is satisfied by nothing at all, and the cheapest way to
/// make c1-c3 green is to change what a plain FILE does. BRIEF, PLAN and
/// METHODOLOGY all go through the same material assembly; a path that is one
/// file must still be exactly one document carrying its bytes. And a path that
/// is neither must still degrade the way the live run did — honestly, with the
/// OS error, rather than panicking or vanishing.
#[test]
fn c4_a_plain_file_is_unchanged_and_a_missing_path_still_degrades_honestly() {
    let root = tmp("c4");
    let file = root.join("methodology.md");
    std::fs::write(&file, "# Methodology\n\nPhases, then review.\n").unwrap();

    let docs = expand_lens_docs("METHODOLOGY", &file, LENS_MATERIAL_MAX_FILES);
    assert_eq!(docs.len(), 1, "a single file must stay a single document");
    assert!(
        docs[0].body.contains("Phases, then review."),
        "the file's own bytes are gone: {:?}",
        docs[0].body
    );
    assert_eq!(
        docs[0].label, "METHODOLOGY",
        "the caller's label was rewritten"
    );

    let missing = expand_lens_docs("CONVENTIONS", &root.join("not-here"), LENS_MATERIAL_MAX_FILES);
    assert_eq!(
        missing.len(),
        1,
        "a missing path must still produce exactly one document saying so — an \
         empty vector is a section that silently disappears from the prompt, and \
         the reviewer cannot report a limitation it was never shown"
    );
    assert!(
        missing[0].body.to_lowercase().contains("could not be read"),
        "a missing conventions path no longer explains itself: {:?}. glm's \
         finding 7 existed because the failure was stated in the prompt.",
        missing[0].body
    );
}

/// Serve one request, reply with `body`, and KEEP what was sent.
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

const SSE_REVIEWED: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Phase 2 has no acceptance \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"criteria. VERDICT: NEEDS_CHANGES\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// c5 — THE CALL SITE. A file from the conventions DIRECTORY must be on the wire.
///
/// This is the test the live run would have failed. It drives the real fan-out
/// and looks for a sentence that exists nowhere but inside a file in the
/// conventions directory, and it also requires the os-error-21 string to be
/// gone — because that string is what the reviewer actually received.
#[tokio::test]
async fn c5_a_conventions_file_reaches_the_provider() {
    // Appears in no source file, no fixture and no template.
    const CANARY: &str = "The QA lens must refuse a phase whose acceptance criteria name no file.";

    let (port, seen) = serve_once_capturing(SSE_REVIEWED);
    let root = tmp("c5");
    let db = Database::open(&root.join("test.db")).expect("open db");
    let state = AppState::new(db);
    let plan_id = "c5-conventions-on-the-wire";
    let workspace = root.join("ws");
    std::fs::create_dir_all(workspace.join("plan/phases/00-x")).expect("plan dirs");
    std::fs::write(workspace.join("plan/overview.md"), "# Plan\n\n## Phase 1\n").unwrap();
    std::fs::write(workspace.join("plan/brief.md"), "Build a kart catalog page.\n").unwrap();

    // The conventions store as it really is: a directory, with the lens
    // checklist in a file inside it. `resolve_conventions_path` resolves the
    // setting RELATIVE to the workspace, so it lives inside the workspace and
    // the setting is the relative path — the same shape the live host uses.
    const CONV_REL: &str = "agents/common/conventions";
    let conv = workspace.join(CONV_REL);
    std::fs::create_dir_all(&conv).expect("conventions dir");
    std::fs::write(conv.join("review-lenses.md"), format!("# Lenses\n\n{CANARY}\n")).unwrap();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO provider_configs (id, provider, cli_path, api_key, default_model, settings, is_available, updated_at)
                 VALUES ('c5-cfg', 'lokalai', 'curl', 'sk-c5-test', 'glm', ?1, 1, datetime('now'))",
                rusqlite::params![format!("{{\"endpoint\":\"http://127.0.0.1:{port}/v1\"}}")],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value)
                 VALUES ('planner_conventions_path', ?1)",
                rusqlite::params![CONV_REL],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status,
                                          worker_session_id, reviewer_session_id, worker_provider,
                                          reviewer_provider, current_phase_index, initiative_id,
                                          initiative_status, health, validation_config)
                 VALUES (?1, 'planning', 'the conventions', ?2, ?3, 'planning_review_running',
                         NULL, NULL, 'claude', 'claude', 0, ?1, 'planning', 'in-progress', ?4)",
                rusqlite::params![
                    plan_id,
                    workspace.to_string_lossy(),
                    workspace.join("plan").to_string_lossy(),
                    r#"[{"name":"glm-validator","provider":"lokalai","model":"glm","blocking":true}]"#,
                ],
            )
            .map_err(|e| e.to_string())
        })
        .expect("seed the plan, the provider config and the conventions path");

    let run = agent_plans::get_plan(&state, plan_id).expect("the seeded plan must load");
    let _ = agent_plans::run_planning_lens_fanout_review(&state, &run).await;

    let body = seen.lock().unwrap().clone();
    assert!(
        !body.is_empty(),
        "the socket saw no request at all, so this test proves nothing either way"
    );
    assert!(
        !body.contains("os error 21"),
        "the prompt on the wire still carries 'Is a directory (os error 21)' where \
         the conventions should be — this is the live 14:02:40 prompt, unchanged.\
         \n--- request body (first 1500) ---\n{}",
        &body.chars().take(1500).collect::<String>()
    );
    assert!(
        body.contains(CANARY),
        "no file from the conventions DIRECTORY reached the provider. The reviewer \
         is asked to judge the plan against review-lenses.md and has never been \
         shown it.\n--- request body (first 1500) ---\n{}",
        &body.chars().take(1500).collect::<String>()
    );
}
