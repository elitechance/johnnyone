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

/// t8 — the one that matters, and the one t2 cannot cover.
///
/// t2 is an ABSENCE assertion: "the key is not in the argv". A request that
/// carries no key at all satisfies it perfectly. The first implementation of
/// this provider passed t2 and every other test here while putting
/// `header = "Authorization: Bearer %{env:LOKALAI_API_KEY}"` in curl's config
/// file — and curl does NOT expand `%{env:…}` there. Measured on curl 8.18.0:
/// the header goes out as the literal text `Bearer %{env:LOKALAI_API_KEY}`,
/// so every lokalai call would have 401'd with a green suite.
///
/// So this test runs the real curl against a real socket and reads what
/// actually arrives. The working form is `variable = %NAME` plus
/// `expand-header = "… {{NAME}}"`.
#[test]
fn t8_the_key_actually_arrives_in_the_authorization_header() {
    use std::io::Read;
    use std::net::TcpListener;

    const KEY: &str = "sk-lokalai-ARRIVES-INTACT";

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a throwaway port");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("curl must connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).to_string()
    });

    let cfg = lokalai::build_config(
        "review this",
        "/tmp",
        "glm",
        &format!("http://127.0.0.1:{port}/v1"),
        KEY,
    );

    let mut cmd = std::process::Command::new(&cfg.command);
    cmd.args(&cfg.args).current_dir(&cfg.working_directory);
    for (name, value) in &cfg.env_vars {
        cmd.env(name, value);
    }
    let _ = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let request = server.join().expect("the server thread must not panic");

    assert!(
        request.contains(&format!("Bearer {KEY}")),
        "the Authorization header did not carry the key. What arrived:\n{request}"
    );
    assert!(
        !request.contains("%{env:"),
        "curl sent an UNEXPANDED %{{env:…}} placeholder — it does not expand those \
         in a config file. Use `variable = %NAME` + `expand-header`. Request:\n{request}"
    );
    assert!(
        !request.contains("{{"),
        "an unexpanded {{{{…}}}} template reached the wire. Request:\n{request}"
    );
}

/// t9 — building a request must not leave anything behind.
///
/// The first implementation wrote a mode-0600 curl config file per request and
/// never deleted it. Twenty of them were sitting in the temp directory after a
/// single afternoon of test runs; a long-running host leaks one per chat turn.
///
/// The config file is not needed at all. Neither of the two lines it carried
/// holds a secret — `variable = %NAME` names an environment variable and
/// `expand-header` is a template — so both belong in argv, where curl takes
/// them as `--variable` and `--expand-header`. The key itself stays in the
/// environment, which is what t2 and t8 pin.
#[test]
fn t9_building_a_request_leaves_no_file_behind() {
    let before = leaked_config_files();

    let cfg = lokalai::build_config(
        "review this",
        "/tmp",
        "glm",
        "https://lokalai.example/v1",
        "sk-whatever",
    );

    let after = leaked_config_files();
    assert_eq!(
        after, before,
        "build_config left {} new file(s) in the temp directory",
        after.saturating_sub(before)
    );

    let argv = cfg.args.join(" ");
    assert!(
        !argv.contains("--config") && !argv.contains("-K"),
        "the request still goes through a config file: {argv}"
    );
}

fn leaked_config_files() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("lokalai-curl-")
                })
                .count()
        })
        .unwrap_or(0)
}
