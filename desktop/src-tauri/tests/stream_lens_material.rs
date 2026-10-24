//! A stream lens has no tools. The material must arrive INSIDE the prompt.
//!
//! THE DEFECT THIS PINS, measured on the live host on 2026-10-24. The transport
//! fix worked: `agent_lens_review_started` at 01:57:23, `agent_lens_verdict` at
//! 02:00:07, 164 seconds, no `coordinator_failed`. glm was reached, ran, and
//! answered. And this is what it said:
//!
//! ```text
//! I'll review this plan along the glm-validator lens. Let me start by reading
//! the brief, methodology, conventions, and the plan itself.
//! <tool_call>Agent tool call blocked: The 'Agent' tool is not available …
//! <tool_call>Read tool call blocked: The 'Read' tool is not available …
//! <tool_call>Bash tool call blocked: … I'll simulate the file reads.
//! VERDICT: BLOCKED
//! ```
//!
//! …and the plan went to `blocked` 32 seconds later.
//!
//! `BLOCKED` is the honest answer to "review this plan" from something that
//! cannot see the plan. The prompt it was handed says, verbatim:
//!
//! > Read first: the ACCEPTED BRIEF … at {brief}; methodology at {methodology};
//! > all conventions under {conventions}; then read the plan at {plan_output}.
//!
//! Four filesystem paths, handed to a single HTTP completion with no shell, no
//! filesystem and no tools. That prompt was written for a PANE lens — a CLI
//! agent with Read and Bash and its own working copy. The transport is sound;
//! the prompt is addressed to the wrong kind of reviewer.
//!
//! I measured both candidate models before writing a line of this, with the
//! material pasted into the prompt by hand:
//!
//! | model               | reply  | `<tool_call>` markers | verdict       |
//! |---------------------|--------|-----------------------|---------------|
//! | glm-5.3-flash       | 3488 c | 0                     | NEEDS_CHANGES |
//! | deepseek-v4.1-flash | 1842 c | 0                     | NEEDS_CHANGES |
//!
//! Both reviewed it properly the moment they could see it. **glm is not the
//! problem; the prompt is.** So this is not a model swap.
//!
//! WHY THE LAST TEST HERE IS SHAPED THE WAY IT IS. Four times in two days a
//! gate proved a module and said nothing about its call site, and every time
//! the defect shipped: `stream_lens` passed seven tests while the fan-out still
//! spawned panes, and `healthReason` went green 8-of-8 while nothing rendered
//! it. So `m6` does not inspect a formatter. It drives the REAL fan-out against
//! a socket that keeps the request body, and asserts that a sentence which
//! exists only inside the plan file came out on the wire. A perfect helper that
//! the fan-out never calls cannot pass it.

use johnnyone_desktop_lib::services::stream_lens::{
    inline_material, stream_lens_prompt_tail, LensDoc,
};

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// Serve one request, reply with `body`, and KEEP what was sent. The captured
/// request is the evidence: it is the only thing that proves the material
/// actually reached the provider rather than merely being formatted nicely in
/// a helper nobody calls.
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
        // Read until the peer stops sending: a prompt carrying inlined material
        // is far larger than one read, and a single `read` into a fixed buffer
        // would truncate exactly the thing under test.
        let mut all = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
            if all.windows(4).any(|w| w == b"\r\n\r\n") && all.len() > 64 {
                // Headers are in and a body has started; one more short read
                // settles the rest, then reply so the client is not left open.
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

fn doc(label: &str, path: &str, body: &str) -> LensDoc {
    LensDoc {
        label: label.to_string(),
        path: path.to_string(),
        body: body.to_string(),
    }
}

/// m1 — every document's BODY is in the material, under its own label.
///
/// The presence half. m2 is the absence half; neither alone is the test —
/// "must not contain X" is satisfied by an empty string, and "must contain X"
/// is satisfied by a prompt that also still tells the reviewer to go and read
/// files.
#[test]
fn m1_the_material_carries_every_document_body() {
    let docs = vec![
        doc("BRIEF", "/ws/plan/brief.md", "Build a kart catalog page."),
        doc("PLAN", "/ws/plan/overview.md", "Phase 1: scaffold. Phase 2: catalog."),
        doc("CONVENTIONS", "/ws/conv/review-lenses.md", "Every phase states its acceptance criteria."),
    ];
    let material = inline_material(&docs, 100_000);

    for d in &docs {
        assert!(
            material.contains(&d.body),
            "the {} body is not in the material. A lens with no filesystem can \
             only review what the prompt carries.\n--- material ---\n{material}",
            d.label
        );
        assert!(
            material.contains(&d.label),
            "the {} section is not labelled. An unlabelled wall of text leaves \
             the reviewer unable to say WHICH document a finding is against.",
            d.label
        );
    }
}

/// m2 — the prompt must not order a tool-less reviewer to use tools.
///
/// These are the exact phrasings that produced the live `BLOCKED`. Each one is
/// an instruction to act on the filesystem, and the reviewer answered every one
/// of them with `<tool_call> … blocked`.
#[test]
fn m2_the_prompt_never_tells_a_stream_lens_to_read_or_run_anything() {
    let docs = vec![doc("PLAN", "/ws/plan/overview.md", "Phase 1: scaffold.")];
    let prompt = format!(
        "{}{}",
        inline_material(&docs, 100_000),
        stream_lens_prompt_tail("glm-validator")
    );
    let lower = prompt.to_lowercase();

    // These are PHRASES, not bare command names. The first version of this list
    // held "ls " and "cat ", and `"ls "` is a substring of **"tools "** — so the
    // prompt could not say "you have no tools", which the very next assertion
    // in this test REQUIRES, without failing this one. That contradiction is
    // not a strict test, it is an impossible task: the worker spent 25 verify
    // attempts and a 60-minute budget against an assertion no prompt could
    // satisfy. An absence pattern has to be narrow enough that only the thing
    // it forbids can match it.
    for banned in [
        "read first",
        "read the plan at",
        "then read",
        "run the command",
        "use the read tool",
        "open the file",
        "list the directory",
        "`ls`",
        "`cat`",
    ] {
        assert!(
            !lower.contains(banned),
            "the stream-lens prompt still contains {banned:?}. That is an \
             instruction to use a tool, and on 2026-10-24 at 02:00:07 the \
             reviewer answered it with `<tool_call> … blocked` and returned \
             BLOCKED.\n--- prompt ---\n{prompt}"
        );
    }

    // The absence above is only meaningful next to a positive statement that
    // the reviewer has everything already. Without it, an empty prompt passes.
    assert!(
        lower.contains("no shell") || lower.contains("no tools") || lower.contains("no filesystem"),
        "the prompt never tells the reviewer it has no tools. It must say so: a \
         model that believes it can fetch the plan will try, and spend its \
         whole reply on blocked tool calls.\n--- prompt ---\n{prompt}"
    );
    assert!(
        lower.contains("everything you need")
            || lower.contains("included below")
            || lower.contains("included above")
            || lower.contains("in this prompt"),
        "the prompt never tells the reviewer the material is already here. \
         Saying only 'you have no tools' invites BLOCKED as the honest answer.\
         \n--- prompt ---\n{prompt}"
    );
}

/// m3 — over budget, what is dropped is NAMED, with a count.
///
/// The failure mode this forbids is a silent truncation: a reviewer handed half
/// a plan with no indication, reporting "phase 3 has no acceptance criteria"
/// about a phase whose criteria were cut off above the fold. A named omission
/// turns a wrong finding into an honest BLOCKED.
#[test]
fn m3_truncation_is_named_with_the_label_and_a_count() {
    let big = "x".repeat(50_000);
    let docs = vec![
        doc("BRIEF", "/ws/plan/brief.md", "Build a kart catalog page."),
        doc("PLAN", "/ws/plan/overview.md", &big),
    ];
    let material = inline_material(&docs, 4_000);

    assert!(
        material.len() <= 4_000,
        "the material is {} bytes against a 4000 budget — the rail does not \
         hold, and a prompt over the model's context window is a BLOCKED with \
         extra steps",
        material.len()
    );
    assert!(
        material.contains("BRIEF") && material.contains("Build a kart catalog page."),
        "the small document was dropped to fit the large one. Budget is spent \
         in order; the brief is the user's intent and is never the casualty.\
         \n--- material ---\n{material}"
    );
    assert!(
        material.contains("PLAN"),
        "the truncated document lost its label entirely"
    );
    assert!(
        material.contains("omitted") || material.contains("truncated"),
        "the material was cut and says nothing about it. A reviewer cannot tell \
         a plan with no acceptance criteria from a plan whose criteria were cut \
         off.\n--- material ---\n{material}"
    );
    // A count, not just the word. "truncated" alone does not say how much.
    let digits: String = material
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .collect();
    assert!(
        !digits.is_empty(),
        "the omission notice carries no number. 'truncated' without a count \
         does not tell the reviewer whether it lost a line or a phase.\
         \n--- material ---\n{material}"
    );
}

/// m4 — under budget, nothing is touched.
///
/// The rail must not fire on ordinary input. A truncation notice on a plan that
/// fitted perfectly well would teach the reviewer to distrust a complete plan.
#[test]
fn m4_under_budget_nothing_is_truncated_and_nothing_is_announced() {
    let docs = vec![
        doc("BRIEF", "/ws/plan/brief.md", "Build a kart catalog page."),
        doc("PLAN", "/ws/plan/overview.md", "Phase 1: scaffold. Phase 2: catalog."),
    ];
    let material = inline_material(&docs, 100_000);

    assert!(
        !material.contains("omitted") && !material.contains("truncated"),
        "a complete set of documents was announced as truncated.\n--- material ---\n{material}"
    );
    for d in &docs {
        assert!(material.contains(&d.body), "{} was altered under budget", d.label);
    }
}

/// m5 — an empty document is reported as empty, not as absent.
///
/// `brief.md` missing and `brief.md` empty are different facts and the reviewer
/// must be able to tell them apart: the first is a broken run, the second is a
/// finding about the plan.
#[test]
fn m5_an_empty_document_says_so() {
    let docs = vec![
        doc("BRIEF", "/ws/plan/brief.md", ""),
        doc("PLAN", "/ws/plan/overview.md", "Phase 1: scaffold."),
    ];
    let material = inline_material(&docs, 100_000);

    assert!(
        material.contains("BRIEF"),
        "an empty document vanished from the material entirely, so the reviewer \
         cannot report that the brief is empty.\n--- material ---\n{material}"
    );
    assert!(
        material.to_lowercase().contains("empty") || material.contains("(0 "),
        "the empty document is in the material but is not marked empty, so it \
         reads as a document the reviewer simply failed to find.\
         \n--- material ---\n{material}"
    );
}

/// m6 — THE CALL SITE, ON THE WIRE.
///
/// This is the test that carries the weight, and it is deliberately not a
/// formatter test. It seeds a real plan whose `overview.md` contains a sentence
/// that exists nowhere else, points the lens at a socket that KEEPS the request
/// body, drives the REAL `run_planning_lens_fanout_review`, and asserts that
/// sentence came out on the wire.
///
/// A perfect `inline_material` that the fan-out never calls fails this test.
/// That is the entire point: the last four defects on this branch were all of
/// exactly that shape.
#[tokio::test]
async fn m6_the_fanout_puts_the_plan_body_on_the_wire() {
    use johnnyone_desktop_lib::db::Database;
    use johnnyone_desktop_lib::services::agent_plans;
    use johnnyone_desktop_lib::state::app_state::AppState;

    // A sentence that appears in no source file, no fixture and no prompt
    // template — so finding it in the request body can only mean the plan file
    // itself was read and inlined.
    const CANARY: &str = "The kart catalog must show a dark-mode contrast of at least 4.5:1.";

    let (port, seen) = serve_once_capturing(SSE_REVIEWED);
    let root = std::env::temp_dir().join(format!("m6-material-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    let db = Database::open(&root.join("test.db")).expect("open db");
    let state = AppState::new(db);
    let plan_id = "m6-material-on-the-wire";
    let workspace = root.join("ws");
    std::fs::create_dir_all(workspace.join("plan/phases/00-x")).expect("plan dirs");
    std::fs::write(
        workspace.join("plan/overview.md"),
        format!("# Plan\n\n## Phase 1\n\n{CANARY}\n"),
    )
    .expect("overview");
    std::fs::write(
        workspace.join("plan/brief.md"),
        "Build a kart catalog page.\n",
    )
    .expect("brief");

    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO provider_configs (id, provider, cli_path, api_key, default_model, settings, is_available, updated_at)
                 VALUES ('m6-cfg', 'lokalai', 'curl', 'sk-m6-test', 'glm', ?1, 1, datetime('now'))",
                rusqlite::params![format!("{{\"endpoint\":\"http://127.0.0.1:{port}/v1\"}}")],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status,
                                          worker_session_id, reviewer_session_id, worker_provider,
                                          reviewer_provider, current_phase_index, initiative_id,
                                          initiative_status, health, validation_config)
                 VALUES (?1, 'planning', 'the material', ?2, ?3, 'planning_review_running',
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
        .expect("seed the plan and the provider config");

    let run = agent_plans::get_plan(&state, plan_id).expect("the seeded plan must load");
    let _ = agent_plans::run_planning_lens_fanout_review(&state, &run).await;

    let body = seen.lock().unwrap().clone();
    assert!(
        !body.is_empty(),
        "the socket saw no request at all — the fan-out did not reach the \
         provider, so this test proves nothing about the prompt either way"
    );

    // The plan body travels inside a JSON string. The canary is deliberately
    // free of characters JSON escapes, so a plain substring search over the
    // request body is exact — no unescaping step to get wrong.
    assert!(
        body.contains(CANARY),
        "the plan's own text never reached the provider. The prompt still \
         points at {} and the reviewer has no filesystem to follow it with — \
         this is the 02:00:07 BLOCKED, unfixed.\n--- request body (first 2000) ---\n{}",
        workspace.join("plan").display(),
        &body.chars().take(2000).collect::<String>()
    );
    assert!(
        !body.to_lowercase().contains("read first"),
        "the prompt on the wire still opens with 'Read first:' — the material \
         may now be included, but the reviewer is still being told to go and \
         fetch it, which is what it spent its entire reply doing.\
         \n--- request body (first 2000) ---\n{}",
        &body.chars().take(2000).collect::<String>()
    );
}
