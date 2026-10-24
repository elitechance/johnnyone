//! A review lens that is NOT a terminal agent.
//!
//! THE DEFECT THIS FIXES, measured on the live host:
//!
//! ```text
//! 05:40:22  planning_review_started
//! 05:40:25  coordinator_failed {"reason":"can't find pane: %2375"}
//! ```
//!
//! The planning lens fan-out registered `GLM-VALIDATOR · e37cd849`
//! (provider `lokalai`, model `glm`) and then spawned it as an INTERACTIVE TMUX
//! PANE. `CliProvider::Lokalai::default_command()` is `"curl"`, and
//! `terminal.rs::provider_command` has no arm for it, so the catch-all
//! `_ => (command, Vec::new())` made tmux run a pane whose command was bare
//! `curl`. curl with no arguments prints its usage and exits; the pane closed;
//! the next tmux call against `%2375` found nothing.
//!
//! It would not have worked with a living pane either: `wait_for_lens_verdict`
//! captures the pane, nudges it when idle, and waits for the lens to run a
//! `reportAgentResult` curl ITSELF. lokalai cannot run a curl — it IS a curl.
//!
//! THE INVARIANT:
//!
//! > A review lens produces a verdict over whatever transport its provider has.
//! > An HTTP provider is driven as a stream, not as a terminal pane, and a lens
//! > that named no verdict is an error — never a defaulted PASS.
//!
//! So this module adds a SECOND TRANSPORT, not a second HTTP client: it drives
//! the provider's own `build_config`/`parse_line` pair through `cli_runner`,
//! concatenates the assistant deltas in arrival order, and reads the verdict
//! token out of that text. No tmux, no pane, no self-reporting curl, and no
//! entry in `agent_plan_review_sessions` because there is nothing to dispose.

use crate::providers::cli_runner::spawn_cli;
use crate::providers::{lokalai, ChunkType, CliProvider};
use crate::state::app_state::AgentReport;

/// How a lens's provider is driven to produce a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensTransport {
    /// An interactive CLI in a tmux pane that runs its own `reportAgentResult`
    /// curl — today's path, unchanged.
    Pane,
    /// An HTTP endpoint driven as a stream; its verdict is read out of its text.
    Stream,
}

/// The transport a lens's provider must use.
///
/// EXHAUSTIVE on purpose — no `_` arm. The defect WAS a catch-all
/// (`terminal.rs::provider_command`'s `_ => (command, Vec::new())` handed tmux a
/// bare `curl`), so a provider added later must be forced to CHOOSE here rather
/// than fall through into a pane that dies in three seconds.
pub fn lens_transport(provider: CliProvider) -> LensTransport {
    match provider {
        // Interactive CLIs: they have a TUI to drive and a shell to report from.
        CliProvider::ClaudeCode => LensTransport::Pane,
        CliProvider::Codex => LensTransport::Pane,
        CliProvider::Cline => LensTransport::Pane,
        CliProvider::Ollama => LensTransport::Pane,
        CliProvider::Grok => LensTransport::Pane,
        CliProvider::Shell => LensTransport::Pane,
        CliProvider::Kloo => LensTransport::Pane,
        // `lokalai`'s command IS curl: no pane to drive, no shell to report from.
        CliProvider::Lokalai => LensTransport::Stream,
    }
}

/// The four verdict tokens a lens may name, LONGEST FIRST.
///
/// Order is load-bearing: `verdict_from_text` returns the first token it finds,
/// so a shorter token listed first would let `PASS_WITH_FOLLOWUPS` be read as
/// `PASS` — a lens asking for follow-ups would silently approve instead.
const VERDICT_TOKENS: [&str; 4] = [
    "PASS_WITH_FOLLOWUPS",
    "NEEDS_CHANGES",
    "BLOCKED",
    "PASS",
];

/// Read a verdict token out of a lens's free text. `None` when it named none.
///
/// `None` is NOT a pass — the caller must treat it as an error. A defaulted PASS
/// would turn "the reviewer said nothing" into "the reviewer approved", which is
/// the worst possible failure for a blocking gate.
pub fn verdict_from_text(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    VERDICT_TOKENS
        .iter()
        .find(|token| upper.contains(*token))
        .map(|token| (*token).to_string())
}

/// The tail appended to a stream lens's prompt.
///
/// A stream lens has no shell, so it gets NO `reportAgentResult` curl and no
/// `127.0.0.1:7788` host pointer — the pane protocol's self-reporting mechanism
/// is unavailable to it. Instead it is SHOWN the four tokens (it cannot emit one
/// it was never shown) and asked to end with `VERDICT: <token>`, which
/// [`verdict_from_text`] reads back out of its text.
pub fn stream_lens_prompt_tail(lens_name: &str) -> String {
    format!(
        "\n\n---\n\
         You are the `{lens_name}` review lens, running as a single streamed \
         response. You have no shell and no terminal: you CANNOT post your result \
         anywhere, and nothing else will read your output except this response \
         stream. Your review IS the report.\n\n\
         Write your review as free text: what you checked, what you found, and \
         file:line evidence for every finding. Then end your response with \
         EXACTLY one final line naming your verdict, in this form:\n\n\
         \x20   VERDICT: <token>\n\n\
         Where <token> is exactly one of these four, spelled exactly:\n\
         \x20   PASS — ship it as-is.\n\
         \x20   PASS_WITH_FOLLOWUPS — acceptable now, but follow-ups must be filed.\n\
         \x20   NEEDS_CHANGES — blocking problems found; name each one.\n\
         \x20   BLOCKED — you could not review it, and why.\n\n\
         Name exactly one token on that final line. If you cannot decide, say \
         BLOCKED — do not stay silent, and do not assume approval: a review that \
         names no verdict is recorded as an error, never as a PASS.\n"
    )
}

/// Drive a review lens over its provider's HTTP stream and return the report the
/// pane path would have produced. No tmux, no pane, no self-reporting curl.
///
/// Assembled from the pieces that already exist — `lokalai::build_config` +
/// `cli_runner::spawn_cli` + `lokalai::parse_line` — so there is exactly one
/// HTTP client in this crate.
///
/// Every failure mode is an `Err` that says what happened: a missing `curl`, a
/// non-zero curl exit, an HTTP error status in place of a stream (the 401 that
/// would have hit every call in PR #48's first round was invisible because
/// nothing read the response — it must not be invisible again), and a stream
/// that named no verdict. None of them are a PASS.
pub async fn run_stream_lens(
    endpoint: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    working_directory: &str,
    session_id: &str,
    lens_name: &str,
) -> Result<AgentReport, String> {
    let config = lokalai::build_config(prompt, working_directory, model, endpoint, api_key);
    let (mut process, mut rx) =
        spawn_cli(config, session_id.to_string(), lokalai::parse_line).await?;

    // Drain the stream, concatenating the Assistant deltas in arrival order.
    // That concatenation IS the lens's review. Everything else — heartbeats,
    // usage frames, curl's stderr — is not the review and must not leak into it.
    let mut review = String::new();
    let mut stderr_lines: Vec<String> = Vec::new();
    while let Some(chunk) = rx.recv().await {
        match chunk.chunk_type {
            ChunkType::Assistant => review.push_str(&chunk.content),
            ChunkType::Stderr => {
                if !chunk.content.trim().is_empty() {
                    stderr_lines.push(chunk.content);
                }
            }
            _ => {}
        }
    }

    let status = process.wait().await?;
    if !status.success() {
        // `--fail-with-body` makes curl exit non-zero on an HTTP error status,
        // and `--show-error` puts the status in its stderr, so the status
        // reaches this string rather than becoming an empty review.
        let detail = stderr_lines.join(" ");
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("stream lens `{lens_name}`: curl exited {status} (no stderr)")
        } else {
            format!("stream lens `{lens_name}`: curl exited {status}: {detail}")
        });
    }

    let token = verdict_from_text(&review).ok_or_else(|| {
        format!(
            "stream lens `{lens_name}` named no verdict \
             (expected one of PASS | PASS_WITH_FOLLOWUPS | NEEDS_CHANGES | BLOCKED); \
             treating a silent reviewer as an error, never as a PASS. \
             Its review was {} bytes long.",
            review.len()
        )
    })?;

    Ok(AgentReport {
        kind: "verdict".to_string(),
        verdict: Some(token),
        // CO-stamped from the lens name — never taken from the model's output.
        role: Some(lens_name.to_string()),
        findings: Some(review),
        summary: None,
        severity: None,
        reason: None,
        evidence: None,
    })
}

/// Resolve a `lokalai` lens's endpoint + key, or explain why there is none.
pub fn resolve_lokalai_credentials(
    state: &crate::state::app_state::AppState,
) -> Result<(String, String), String> {
    lokalai::resolve_endpoint_and_key(state).ok_or_else(|| {
        "lokalai: no endpoint and no API key — set them in the provider config or a kloo profile"
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_token_wins() {
        assert_eq!(
            verdict_from_text("VERDICT: PASS_WITH_FOLLOWUPS"),
            Some("PASS_WITH_FOLLOWUPS".to_string())
        );
        assert_eq!(
            verdict_from_text("verdict: needs_changes, then pass later"),
            Some("NEEDS_CHANGES".to_string())
        );
    }

    #[test]
    fn prompt_tail_shows_tokens_and_hides_the_curl() {
        let tail = stream_lens_prompt_tail("glm-validator");
        assert!(tail.contains("glm-validator"));
        for token in [
            "PASS",
            "PASS_WITH_FOLLOWUPS",
            "NEEDS_CHANGES",
            "BLOCKED",
            "VERDICT:",
        ] {
            assert!(tail.contains(token), "missing {token}");
        }
        assert!(!tail.contains("reportAgentResult"));
        assert!(!tail.contains("127.0.0.1:7788"));
    }

    #[test]
    fn every_provider_chooses_a_transport() {
        assert_eq!(lens_transport(CliProvider::Lokalai), LensTransport::Stream);
        assert_eq!(lens_transport(CliProvider::ClaudeCode), LensTransport::Pane);
    }
}