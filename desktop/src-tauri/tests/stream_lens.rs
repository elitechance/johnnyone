//! A review lens that is not a terminal agent.
//!
//! THE DEFECT THIS PINS, measured on the live host on 2026-10-23:
//!
//! ```text
//! 05:40:22  planning_review_started
//! 05:40:25  coordinator_failed {"reason":"can't find pane: %2375"}
//! ```
//!
//! The lens fan-out created the session correctly — `GLM-VALIDATOR · e37cd849`,
//! provider `lokalai`, model `glm` — and then spawned it as an INTERACTIVE TMUX
//! PANE. `CliProvider::Lokalai::default_command()` is `"curl"` and
//! `terminal.rs::provider_command` has no arm for it, so the catch-all
//! `_ => (command, Vec::new())` made tmux run a pane whose command was bare
//! `curl`. curl with no arguments prints its usage and exits; the pane closed;
//! the next tmux call against `%2375` found nothing. Three seconds, end to end.
//!
//! It would not have worked with a living pane either: `wait_for_lens_verdict`
//! captures the pane, nudges it when idle, and waits for the lens to run a
//! `reportAgentResult` curl ITSELF. lokalai cannot run a curl — it IS a curl.
//!
//! So a lens needs a SECOND TRANSPORT, not a second HTTP client: drive the
//! provider's own `build_config`/`parse_line` pair through `cli_runner`, collect
//! the assistant text, and read the verdict out of it. No tmux, no pane, no
//! self-reporting curl.
//!
//! WHY THESE TESTS ARE SHAPED THIS WAY. Yesterday I reported that glm was a
//! working review lens because `updateAgentPlanValidationConfig` ACCEPTED
//! `{provider:"lokalai", model:"glm", blocking:true}`. That was the
//! configuration layer. The lens had never run. So the test that carries the
//! weight here (s4) runs a real `curl` against a real `TcpListener` serving a
//! real SSE stream and reads the verdict back out of the returned report — a
//! shape-only test would pass on the code that died in three seconds.

use johnnyone_desktop_lib::providers::CliProvider;
use johnnyone_desktop_lib::services::stream_lens::{
    lens_transport, run_stream_lens, stream_lens_prompt_tail, verdict_from_text, LensTransport,
};

use std::io::{Read, Write};
use std::net::TcpListener;

/// Serve exactly one HTTP request on a throwaway port and reply with `body`
/// (headers included), then drop the connection. Returns the port.
fn serve_once(status_line: &str, content_type: &str, body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a throwaway port");
    let port = listener.local_addr().unwrap().port();
    let status_line = status_line.to_string();
    let content_type = content_type.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the lens must connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let mut buf = [0u8; 16384];
        let _ = stream.read(&mut buf);
        let response = format!(
            "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    port
}

/// An OpenAI SSE stream whose deltas spell a review ending in a verdict token.
const SSE_NEEDS_CHANGES: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"The catalog cards are \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"unreadable in dark mode: \"}}]}\n\n",
    ": keep-alive\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"contrast 1.07:1 at \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"catalog.page.scss:12. \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"VERDICT: NEEDS_CHANGES\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{}}]}\n\n",
    "data: [DONE]\n\n",
);

/// A stream that says plenty and never names a verdict.
const SSE_SILENT: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"I looked at the diff and \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"have some thoughts about it.\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// s1 — every provider must CHOOSE a transport.
///
/// The defect was a catch-all. An exhaustive match is the fix, so this test
/// names every variant: adding a provider later without choosing a transport
/// must break the build here, not spawn a pane that dies in three seconds.
#[test]
fn s1_lokalai_is_a_stream_lens_and_every_terminal_cli_is_a_pane() {
    assert_eq!(
        lens_transport(CliProvider::Lokalai),
        LensTransport::Stream,
        "lokalai is an HTTP endpoint driven by curl — it has no interactive pane \
         and cannot run a reportAgentResult curl of its own"
    );

    for provider in [
        CliProvider::ClaudeCode,
        CliProvider::Codex,
        CliProvider::Cline,
        CliProvider::Ollama,
        CliProvider::Grok,
    ] {
        assert_eq!(
            lens_transport(provider),
            LensTransport::Pane,
            "{provider:?} is an interactive CLI; it keeps today's pane transport"
        );
    }

    // Kloo and Shell are refused as lenses upstream by `reject_non_review_lens`,
    // but they are still variants and must still map to something rather than
    // falling through a wildcard.
    for provider in [CliProvider::Kloo, CliProvider::Shell] {
        let _ = lens_transport(provider);
    }
}

/// s2 — the four verdict tokens are read out of free text.
#[test]
fn s2_verdict_from_text_reads_each_token() {
    assert_eq!(
        verdict_from_text("Everything checks out.\nVERDICT: PASS"),
        Some("PASS".to_string())
    );
    assert_eq!(
        verdict_from_text("Two nits worth filing.\nVERDICT: PASS_WITH_FOLLOWUPS"),
        Some("PASS_WITH_FOLLOWUPS".to_string()),
        "PASS_WITH_FOLLOWUPS must not be truncated to PASS — a longest-token \
         match, or a lens asking for follow-ups silently approves instead"
    );
    assert_eq!(
        verdict_from_text("contrast 1.07:1\nVERDICT: NEEDS_CHANGES"),
        Some("NEEDS_CHANGES".to_string())
    );
    assert_eq!(
        verdict_from_text("I cannot reach the dev server.\nVERDICT: BLOCKED"),
        Some("BLOCKED".to_string())
    );
}

/// s3 — prose with no token is None, and that is the whole point.
///
/// Paired with s2 so this is not an absence assertion satisfied by nothing:
/// s2 proves the reader finds tokens that are there, s3 proves it does not
/// invent one that is not.
#[test]
fn s3_verdict_from_text_is_none_when_no_token_is_present() {
    assert_eq!(
        verdict_from_text("I read the plan and it looks broadly reasonable to me."),
        None,
        "a reviewer that named no verdict must not be read as having passed it"
    );
    assert_eq!(verdict_from_text(""), None);
}

/// s4 — THE TEST THAT CARRIES THE WEIGHT.
///
/// A real `curl` against a real socket serving a real SSE stream, and the
/// verdict read back out of the returned report. No tmux session exists for
/// this session id and none is created: that is the defect, inverted.
#[tokio::test]
async fn s4_a_stream_lens_gets_its_verdict_from_a_real_socket() {
    let port = serve_once("HTTP/1.1 200 OK", "text/event-stream", SSE_NEEDS_CHANGES);
    let session_id = "stream-lens-no-pane-exists-for-this-id";

    let report = run_stream_lens(
        &format!("http://127.0.0.1:{port}/v1"),
        "sk-test-key-not-in-argv",
        "glm",
        "Review the kart catalog page.",
        "/tmp",
        session_id,
        "glm-validator",
    )
    .await
    .expect("a lens served a complete SSE stream must return a report");

    assert_eq!(report.kind, "verdict", "a lens reports a verdict, not an update");
    assert_eq!(
        report.verdict.as_deref(),
        Some("NEEDS_CHANGES"),
        "the verdict in the stream did not reach the report"
    );
    assert_eq!(
        report.role.as_deref(),
        Some("glm-validator"),
        "the role is CO-stamped from the lens name, never self-declared"
    );

    let findings = report
        .findings
        .as_deref()
        .expect("the lens's own text is the findings — dropping it loses the review");
    assert!(
        findings.contains("1.07:1") && findings.contains("catalog.page.scss:12"),
        "the assistant deltas were not reassembled. findings were:\n{findings}"
    );
    assert!(
        !findings.contains("[DONE]") && !findings.contains("keep-alive"),
        "SSE control frames leaked into the findings:\n{findings}"
    );

    // The inversion of the defect: this path must not have gone near tmux.
    let panes = std::process::Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{session_name}"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    assert!(
        !panes.contains(session_id),
        "a stream lens created a tmux session — that is the pane transport, \
         which is exactly what died on %2375"
    );
}

/// s5 — a lens that named no verdict is an ERROR, never a pass.
///
/// `gate_verdict_over_blocking` treats a blocking lens's verdict as a gate. A
/// defaulted PASS here would turn "the reviewer said nothing" into "the
/// reviewer approved", which is the worst possible failure for a gate.
#[tokio::test]
async fn s5_a_silent_stream_is_an_error_not_a_pass() {
    let port = serve_once("HTTP/1.1 200 OK", "text/event-stream", SSE_SILENT);

    let result = run_stream_lens(
        &format!("http://127.0.0.1:{port}/v1"),
        "sk-test",
        "glm",
        "Review it.",
        "/tmp",
        "silent-lens",
        "glm-validator",
    )
    .await;

    let error = result.expect_err(
        "a stream with no verdict token must be an error — defaulting it to PASS \
         makes a silent reviewer into an approving one",
    );
    assert!(
        error.to_lowercase().contains("verdict"),
        "the error must say what was missing. It said: {error}"
    );
}

/// s6 — an HTTP error is an error, with the status in it.
///
/// The 401 that PR #48's first round would have produced on every call was
/// invisible because nothing ever read the response. It must not be invisible
/// again, and it must not look like a verdict.
#[tokio::test]
async fn s6_an_http_error_reaches_the_caller_as_an_error() {
    let port = serve_once(
        "HTTP/1.1 401 Unauthorized",
        "application/json",
        "{\"error\":{\"message\":\"invalid api key\"}}",
    );

    let result = run_stream_lens(
        &format!("http://127.0.0.1:{port}/v1"),
        "sk-wrong",
        "glm",
        "Review it.",
        "/tmp",
        "unauthorized-lens",
        "glm-validator",
    )
    .await;

    let error = result.expect_err("a 401 must not come back as a verdict");
    assert!(
        error.contains("401") || error.to_lowercase().contains("unauthor"),
        "the error must name the HTTP failure. It said: {error}"
    );
}

/// s7 — the stream lens is told how to end, and is NOT told to run a curl.
///
/// The pane protocol hands the agent a baked `reportAgentResult` curl. A stream
/// lens has no shell to run it in; the verdict must come out of its text. The
/// presence half of this assertion keeps the absence half honest.
#[test]
fn s7_the_stream_lens_prompt_asks_for_a_token_not_a_curl() {
    let tail = stream_lens_prompt_tail("glm-validator");

    for token in [
        "PASS",
        "PASS_WITH_FOLLOWUPS",
        "NEEDS_CHANGES",
        "BLOCKED",
        "VERDICT:",
    ] {
        assert!(
            tail.contains(token),
            "the prompt must name {token} — a lens cannot emit a token it was \
             never shown. Prompt tail was:\n{tail}"
        );
    }
    assert!(
        !tail.contains("reportAgentResult") && !tail.contains("127.0.0.1:7788"),
        "the stream lens was handed the pane protocol's self-reporting curl, \
         which it has no shell to run. Prompt tail was:\n{tail}"
    );
}
