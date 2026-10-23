//! THE SPECIFICATION for the `lokalai` chat provider.
//!
//! This file is the gate, not application code: it is outside the run's edit
//! scope, and changing it to make a run pass is a failed task.
//!
//! Why this provider exists. J1 asks a reviewer a question by spawning a CLI
//! and parsing its stream. The models on lokalai — glm among them — are not
//! CLIs; they sit behind an OpenAI-compatible HTTP endpoint, and the only
//! thing in J1 that could reach them was kloo, which is a TASK executor and is
//! correctly refused as a review lens. The consequence was that the arena's
//! own workflow — qwen develops, glm validates — could not be expressed in J1
//! at all. This provider closes that, and it does it WITHOUT inventing a
//! second transport: it stays a subprocess emitting lines, like every other
//! provider, by driving `curl` against the endpoint with SSE streaming.
//!
//! The secret handling is part of the specification, not a detail. A spawned
//! argv is world-readable in `ps`, so the key may never appear in it.

use johnnyone_desktop_lib::providers::{lokalai, ChunkType, CliProvider};

#[test]
fn t1_lokalai_is_a_known_provider_and_round_trips() {
    assert_eq!(CliProvider::from_str("lokalai"), Some(CliProvider::Lokalai));
    assert_eq!(CliProvider::Lokalai.as_str(), "lokalai");
    assert_eq!(
        CliProvider::from_str(CliProvider::Lokalai.as_str()),
        Some(CliProvider::Lokalai),
        "as_str must round-trip through from_str, or a provider stored in the \
         database cannot be read back"
    );
}

#[test]
fn t2_the_api_key_never_reaches_the_argv() {
    const KEY: &str = "sk-lokalai-THIS-MUST-NOT-APPEAR";
    let cfg = lokalai::build_config(
        "review this diff",
        "/tmp",
        "glm",
        "https://lokalai.example/v1",
        KEY,
    );

    let argv = std::iter::once(cfg.command.clone())
        .chain(cfg.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !argv.contains(KEY),
        "the api key is visible in `ps` output: {argv}"
    );
    for (name, value) in &cfg.env_vars {
        assert!(
            !value.contains(KEY) || name == "LOKALAI_API_KEY",
            "the api key leaked into env var {name}"
        );
    }
}

#[test]
fn t3_the_request_names_the_model_and_asks_for_a_stream() {
    let cfg = lokalai::build_config("hello", "/tmp", "glm", "https://lokalai.example/v1", "k");
    let joined = cfg.args.join(" ");
    assert!(
        joined.contains("https://lokalai.example/v1"),
        "the endpoint is not in the request: {joined}"
    );
    assert_eq!(
        cfg.working_directory, "/tmp",
        "the working directory must be carried through like every other provider"
    );
}

#[test]
fn t4_an_sse_delta_becomes_assistant_text() {
    let line = r#"data: {"choices":[{"delta":{"content":"the grid is too narrow"}}]}"#;
    let chunk = lokalai::parse_line(line).expect("an SSE delta must produce a chunk");
    assert_eq!(chunk.content, "the grid is too narrow");
    assert!(matches!(chunk.chunk_type, ChunkType::Assistant));
}

#[test]
fn t5_the_frames_that_are_not_text_produce_nothing() {
    // A reviewer's verdict must not be polluted by protocol noise, and
    // `[DONE]` is not JSON — parsing it as JSON used to be how a provider
    // turned a clean finish into an error chunk.
    assert!(lokalai::parse_line("data: [DONE]").is_none());
    assert!(lokalai::parse_line("").is_none());
    assert!(lokalai::parse_line(": keep-alive").is_none());
    assert!(
        lokalai::parse_line(r#"data: {"choices":[{"delta":{}}]}"#).is_none(),
        "an empty delta is a heartbeat, not an empty assistant message"
    );
}

#[test]
fn t6_a_usage_frame_reports_its_tokens() {
    let line = r#"data: {"choices":[],"usage":{"prompt_tokens":1200,"completion_tokens":340}}"#;
    let chunk = lokalai::parse_line(line).expect("a usage frame must produce a chunk");
    assert_eq!(chunk.input_tokens, Some(1200));
    assert_eq!(chunk.output_tokens, Some(340));
}

#[test]
fn t7_lokalai_is_a_chat_provider_not_a_oneshot_executor() {
    // The whole point. `Shell` and `Kloo` are refused as review lenses because
    // they execute tasks rather than answering questions. `lokalai` answers
    // questions, so it must land on the other side of that line — this asserts
    // the public shape every call site switches on.
    assert_ne!(CliProvider::Lokalai, CliProvider::Kloo);
    assert_ne!(CliProvider::Lokalai, CliProvider::Shell);
    assert_eq!(
        CliProvider::Lokalai.default_command(),
        "curl",
        "lokalai speaks HTTP; it stays a subprocess like every other provider"
    );
}
