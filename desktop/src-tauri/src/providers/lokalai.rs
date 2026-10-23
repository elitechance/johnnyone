//! `lokalai` — an OpenAI-compatible HTTP endpoint (glm among them) driven as a
//! subprocess, exactly like every other provider here: spawn `curl`, parse its
//! stdout lines into `StreamChunk`s. No second transport is invented inside J1.
//!
//! The secret handling is part of the contract, not a detail. `cli_runner`
//! spawns argv directly (no shell), and `ps` shows argv to every user on the
//! machine — so the API key may NEVER appear in it. It lives in the child's
//! environment (`LOKALAI_API_KEY`) and stays there: argv carries only the
//! *name* of that variable. curl imports it with `--variable %LOKALAI_API_KEY`
//! and substitutes it into the header with
//! `--expand-header "Authorization: Bearer {{LOKALAI_API_KEY}}"`, so the
//! secret never touches argv nor the disk. NOTE: curl does **not** expand
//! `%{env:…}` — that form goes out on the wire verbatim and 401s every call.
//! The expanding pair is `--variable %NAME` + `--expand-header` + `{{NAME}}`.
//! Nothing is written to disk at all: an earlier revision wrote a mode-0600
//! curl config file per request and leaked one per chat turn. Never log the
//! key.

use super::{ChunkType, CliSpawnConfig, StreamChunk};
use rusqlite::params;

use crate::state::app_state::AppState;

/// Env var the key is exported under, so argv can reference it by name without
/// the secret ever appearing in `ps` output.
pub const API_KEY_ENV: &str = "LOKALAI_API_KEY";

/// Directories kloo searches for its profile, in kloo's own search order.
pub fn kloo_profile_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim().trim_end_matches('/').to_string();
        if !home.is_empty() {
            let home = std::path::PathBuf::from(home);
            dirs.push(home.join(".kloo"));
            dirs.push(home.join(".config").join("kloo"));
            dirs.push(home.join("etc"));
        }
    }
    dirs.push(std::path::PathBuf::from("/etc"));
    dirs
}

/// The first kloo profile we can read (`kloo.json` or `profiles.json`), as text.
fn read_kloo_profile() -> Option<String> {
    for dir in kloo_profile_dirs() {
        for name in ["kloo.json", "profiles.json"] {
            let path = dir.join(name);
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// `providers.lokalai.endpoint` from a kloo profile JSON. Trailing slash trimmed.
pub fn endpoint_from_profile_json(profile_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(profile_json).ok()?;
    let endpoint = value
        .get("providers")?
        .get("lokalai")?
        .get("endpoint")?
        .as_str()?;
    let trimmed = endpoint.trim().trim_end_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `providers.lokalai.apiKey` from a kloo profile JSON. A `${VAR}` reference is
/// expanded through the environment (kloo expands it at launch); a placeholder
/// naming an unset variable is None, never the literal text. The key is a
/// secret: returned to the caller, never logged.
pub fn api_key_from_profile_json(profile_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(profile_json).ok()?;
    let raw = value
        .get("providers")?
        .get("lokalai")?
        .get("apiKey")
        .or_else(|| value.get("providers")?.get("lokalai")?.get("api_key"))?
        .as_str()?;
    expand_key_reference(raw)
}

/// Resolve a stored `apiKey` value: `${NAME}` is an environment reference and
/// must be expanded (sending the literal placeholder is a guaranteed 401);
/// anything else is a literal key, trimmed. Blank/broken → None.
pub fn expand_key_reference(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if let Some(name) = trimmed
        .strip_prefix("${")
        .and_then(|inner| inner.strip_suffix('}'))
        .map(str::trim)
    {
        if name.is_empty() {
            return None;
        }
        return std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    // A bare `$NAME` names a variable rather than holding a key: not a token.
    if trimmed.starts_with('$') {
        return None;
    }
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The endpoint + key for a `lokalai` chat request.
///
/// Order (first hit wins): the provider config row (`ProviderConfig.apiKey`,
/// endpoint under the `endpoint` key of its `settings` JSON), then the kloo
/// profile's `providers.lokalai.{endpoint,apiKey}`. Returns `None` only when
/// neither source has anything at all.
pub fn resolve_endpoint_and_key(state: &AppState) -> Option<(String, String)> {
    let (mut endpoint, mut api_key) = from_provider_config(state)
        .map(|(endpoint, key)| (endpoint.unwrap_or_default(), key.unwrap_or_default()))
        .unwrap_or_default();

    if endpoint.trim().is_empty() || api_key.trim().is_empty() {
        if let Some(profile) = read_kloo_profile() {
            if endpoint.trim().is_empty() {
                endpoint = endpoint_from_profile_json(&profile).unwrap_or_default();
            }
            if api_key.trim().is_empty() {
                api_key = api_key_from_profile_json(&profile).unwrap_or_default();
            }
        }
    }

    if endpoint.trim().is_empty() && api_key.trim().is_empty() {
        return None;
    }
    Some((endpoint, api_key))
}

/// `ProviderConfig.apiKey` + `settings.endpoint` of the `lokalai` row.
fn from_provider_config(state: &AppState) -> Option<(Option<String>, Option<String>)> {
    let row = state.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT api_key, settings FROM provider_configs WHERE provider = ?1",
            params!["lokalai"],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .ok())
    });
    let (api_key, settings) = match row {
        Ok(Some(row)) => row,
        _ => return None,
    };
    let endpoint = settings
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value
                .get("endpoint")
                .and_then(|e| e.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        });
    let api_key = api_key
        .as_deref()
        .and_then(expand_key_reference)
        .or(api_key.filter(|k| !k.trim().is_empty()));
    if endpoint.is_none() && api_key.is_none() {
        return None;
    }
    Some((endpoint, api_key))
}

/// Build a spawn config for a `lokalai` chat completion.
///
/// `endpoint` is the OpenAI-compatible base (e.g. `https://lokalai.example/v1`);
/// `/chat/completions` is appended. The request body asks for `"stream": true`
/// so the response is SSE, which [`parse_line`] reads frame by frame.
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    endpoint: &str,
    api_key: &str,
) -> CliSpawnConfig {
    let model = if model.is_empty() { "glm" } else { model };
    let url = chat_completions_url(endpoint);
    let body = request_body(prompt, model);

    // Everything curl needs goes in argv as flags: none of it is a secret. The
    // header is a TEMPLATE referencing the env var by name, and `--variable`
    // imports that name from the environment at request time, so the key itself
    // is never in argv (t2) and never on disk (t9). `--data` carries the prompt
    // body, which is not a secret either.
    let mut args: Vec<String> = vec![
        "--silent".to_string(),
        "--show-error".to_string(),
        "--no-buffer".to_string(),
        "--request".to_string(),
        "POST".to_string(),
        "--variable".to_string(),
        format!("%{API_KEY_ENV}"),
        "--expand-header".to_string(),
        format!("Authorization: Bearer {{{{{API_KEY_ENV}}}}}"),
        "--header".to_string(),
        "content-type: application/json".to_string(),
        "--data".to_string(),
        body,
    ];
    args.push(url);

    let mut env_vars = Vec::new();
    if !api_key.is_empty() {
        env_vars.push((API_KEY_ENV.to_string(), api_key.to_string()));
    }

    CliSpawnConfig {
        command: "curl".to_string(),
        args,
        working_directory: working_directory.to_string(),
        env_vars,
    }
}

/// The full chat-completions URL for a base endpoint.
pub fn chat_completions_url(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        return "http://127.0.0.1:8000/v1/chat/completions".to_string();
    }
    if endpoint.ends_with("/chat/completions") {
        return endpoint.to_string();
    }
    format!("{endpoint}/chat/completions")
}

/// The OpenAI-compatible request body. Contains no secret.
pub fn request_body(prompt: &str, model: &str) -> String {
    let prompt = serde_json::to_string(prompt).unwrap_or_else(|_| "\"\"".to_string());
    let model = serde_json::to_string(model).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "{{\"model\":{model},\"stream\":true,\"messages\":[{{\"role\":\"user\",\"content\":{prompt}}}]}}"
    )
}

/// Parse one line of OpenAI SSE output into a `StreamChunk`.
///
/// - `data: {"choices":[{"delta":{"content":"…"}}]}` → an `Assistant` chunk.
/// - `data: [DONE]` → `None`. It is **not** JSON; parsing it as JSON is how a
///   clean finish becomes an error chunk in a reviewer's verdict.
/// - a blank line, a `:` SSE keep-alive comment, and an **empty delta** →
///   `None`. An empty delta is a heartbeat, not an empty assistant message.
/// - a frame carrying `usage` → a chunk with `input_tokens`/`output_tokens`
///   from `prompt_tokens`/`completion_tokens`.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // SSE keep-alive comment.
    if line.starts_with(':') {
        return None;
    }
    // Only `data:` frames carry payload; `event:`/`id:`/`retry:` are protocol
    // noise and must not pollute a reviewer's verdict.
    let payload = line.strip_prefix("data:")?.trim();
    // The terminal frame is not JSON.
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;

    // A usage frame reports tokens; it carries no text.
    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        let input_tokens = usage
            .get("prompt_tokens")
            .and_then(|t| t.as_i64())
            .or_else(|| usage.get("input_tokens").and_then(|t| t.as_i64()));
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(|t| t.as_i64())
            .or_else(|| usage.get("output_tokens").and_then(|t| t.as_i64()));
        if input_tokens.is_some() || output_tokens.is_some() {
            return Some(StreamChunk {
                chunk_type: ChunkType::Result,
                content: String::new(),
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens,
                output_tokens,
                is_final: true,
                session_id: None,
            });
        }
    }

    let delta_text = v
        .get("choices")
        .and_then(|choices| choices.as_array())?
        .first()?
        .get("delta")
        .and_then(|delta| delta.get("content"))
        .and_then(|content| content.as_str())
        .unwrap_or("");

    if delta_text.is_empty() {
        // Heartbeat / role-only / finish_reason-only frame.
        return None;
    }

    Some(StreamChunk {
        chunk_type: ChunkType::Assistant,
        content: delta_text.to_string(),
        tool_name: None,
        tool_input: None,
        tool_output: None,
        cost_usd: None,
        input_tokens: None,
        output_tokens: None,
        is_final: false,
        session_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "sk-secret-DO-NOT-LEAK";

    #[test]
    fn key_never_reaches_argv() {
        let cfg = build_config("hi", "/tmp", "glm", "https://lokalai.example/v1", KEY);
        let joined = std::iter::once(cfg.command.clone())
            .chain(cfg.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!joined.contains(KEY), "key visible in argv: {joined}");
    }

    #[test]
    fn endpoint_and_cwd_are_carried_through() {
        let cfg = build_config("hi", "/tmp", "glm", "https://lokalai.example/v1", "k");
        assert!(cfg.args.join(" ").contains("https://lokalai.example/v1"));
        assert_eq!(cfg.working_directory, "/tmp");
    }

    #[test]
    fn url_is_not_duplicated_when_already_full() {
        assert_eq!(
            chat_completions_url("https://x.example/v1/chat/completions"),
            "https://x.example/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://x.example/v1/"),
            "https://x.example/v1/chat/completions"
        );
    }

    #[test]
    fn request_body_asks_for_a_stream() {
        let body = request_body("hello \"world\"", "glm");
        assert!(body.contains("\"stream\":true"));
        assert!(body.contains("\"model\":\"glm\""));
        assert!(body.contains("hello \\\"world\\\""));
    }

    #[test]
    fn header_and_body_are_argv_flags_not_a_config_file() {
        let cfg = build_config("hi", "/tmp", "glm", "https://lokalai.example/v1", "k");
        let joined = cfg.args.join(" ");
        assert!(cfg.args.iter().any(|a| a == "--variable"));
        assert!(cfg
            .args
            .iter()
            .any(|a| a == "Authorization: Bearer {{LOKALAI_API_KEY}}"));
        assert!(cfg.args.iter().any(|a| a == "--data"));
        assert!(joined.contains("\"stream\":true"));
        assert!(!joined.contains("--config") && !joined.contains("-K"));
    }

    #[test]
    fn building_a_request_writes_nothing_to_disk() {
        // The marker name matches the prefix the leak test in the specification
        // counts (`lokalai-curl-…`); creating it here would make that test
        // falsely report a leak, so build_config must not create it either.
        let marker = format!("lokalai-curl-{}-argv-check", std::process::id());
        let path = std::env::temp_dir().join(&marker);
        let before = std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with("lokalai-curl-"))
                    .count()
            })
            .unwrap_or(0);
        let _ = build_config("hi", "/tmp", "glm", "https://lokalai.example/v1", "k");
        let after = std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with("lokalai-curl-"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(after, before, "build_config leaked a temp file");
        assert!(!path.exists());
    }

    #[test]
    fn parses_delta_and_ignores_noise() {
        let chunk = parse_line(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        assert_eq!(chunk.content, "hi");
        assert!(matches!(chunk.chunk_type, ChunkType::Assistant));
        assert!(parse_line("data: [DONE]").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line(": keep-alive").is_none());
        assert!(parse_line(r#"data: {"choices":[{"delta":{}}]}"#).is_none());
    }

    #[test]
    fn profile_dirs_follow_kloos_search_order() {
        let joined: Vec<String> = kloo_profile_dirs()
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        assert_eq!(joined.last().map(String::as_str), Some("/etc"));
        assert!(joined.iter().any(|d| d.ends_with("/.kloo")), "{joined:?}");
    }

    #[test]
    fn profile_json_supplies_endpoint_and_key() {
        let profile = r#"{"defaultProvider":"lokalai","providers":{"lokalai":{"endpoint":"https://lokalai.example/v1/","apiKey":"sk-in-profile"}}}"#;
        assert_eq!(
            endpoint_from_profile_json(profile).as_deref(),
            Some("https://lokalai.example/v1")
        );
        assert_eq!(
            api_key_from_profile_json(profile).as_deref(),
            Some("sk-in-profile")
        );
        assert_eq!(endpoint_from_profile_json("not json", ), None);
        assert_eq!(api_key_from_profile_json("{}", ), None);
    }

    #[test]
    fn placeholder_key_is_expanded_never_forwarded_verbatim() {
        // Names a variable that cannot be set in this process's environment.
        assert_eq!(expand_key_reference("${LOKALAI_DEFINITELY_UNSET_VAR}"), None);
        assert_eq!(expand_key_reference("${}"), None);
        assert_eq!(expand_key_reference("$PLAIN"), None);
        assert_eq!(expand_key_reference("  sk-literal  "), Some("sk-literal".into()));
        assert_eq!(expand_key_reference("   "), None);
    }

    #[test]
    fn parses_usage_frame() {
        let chunk = parse_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        )
        .unwrap();
        assert_eq!(chunk.input_tokens, Some(7));
        assert_eq!(chunk.output_tokens, Some(3));
    }
}