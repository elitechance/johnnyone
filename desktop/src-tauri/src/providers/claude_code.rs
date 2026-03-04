use super::{CliSpawnConfig, StreamChunk};
use super::output_parser::parse_claude_code_line;

/// Build a CLI spawn config for Claude Code.
///
/// Uses `claude` CLI with `--output-format stream-json` for real-time JSON streaming.
///
/// Session handling:
/// - First message in a session: uses `--session-id <uuid>` to create a named session
/// - Subsequent messages: uses `--resume <cli_session_id>` to continue the conversation
///   The CLI maintains its own conversation history when resumed.
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
    cli_session_id: Option<&str>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("claude").to_string();
    let mut args = vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--print".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ];

    if !model.is_empty() {
        args.push("--model".to_string());
        args.push(model.to_string());
    }

    // Session continuity: --resume for existing sessions, --session-id for new ones
    if let Some(sid) = cli_session_id {
        args.push("--resume".to_string());
        args.push(sid.to_string());
    }

    // Prompt as the final positional argument
    args.push(prompt.to_string());

    CliSpawnConfig {
        command,
        args,
        working_directory: working_directory.to_string(),
        // Must unset CLAUDECODE to avoid "nested session" error when spawned from within Claude Code
        env_vars: vec![("CLAUDECODE".to_string(), String::new())],
    }
}

/// Parse a line of Claude Code stream-json output.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    parse_claude_code_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChunkType;

    #[test]
    fn test_build_config_new_session() {
        let config = build_config("hello", "/tmp", "", None, None);
        assert_eq!(config.command, "claude");
        assert!(config.args.contains(&"--output-format".to_string()));
        assert!(config.args.contains(&"stream-json".to_string()));
        assert!(config.args.contains(&"--print".to_string()));
        assert!(config.args.contains(&"hello".to_string()));
        // No --resume for new sessions
        assert!(!config.args.contains(&"--resume".to_string()));
    }

    #[test]
    fn test_build_config_resume_session() {
        let config = build_config("follow up", "/tmp", "opus", None, Some("abc-123"));
        assert!(config.args.contains(&"--resume".to_string()));
        assert!(config.args.contains(&"abc-123".to_string()));
        assert!(config.args.contains(&"--model".to_string()));
        assert!(config.args.contains(&"opus".to_string()));
    }

    #[test]
    fn test_unsets_claudecode_env() {
        let config = build_config("hello", "/tmp", "", None, None);
        assert!(config.env_vars.iter().any(|(k, v)| k == "CLAUDECODE" && v.is_empty()));
    }

    #[test]
    fn test_parse_assistant_message() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#;
        let chunk = parse_line(line).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Text);
        assert_eq!(chunk.content, "Hello!");
    }

    #[test]
    fn test_parse_result_actual_format() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1425,"result":"Four","session_id":"abc-123","total_cost_usd":0.027,"usage":{"input_tokens":3,"output_tokens":4}}"#;
        let chunk = parse_line(line).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Result);
        assert!(chunk.is_final);
        assert_eq!(chunk.cost_usd, Some(0.027));
        assert_eq!(chunk.input_tokens, Some(3));
        assert_eq!(chunk.output_tokens, Some(4));
        assert_eq!(chunk.session_id, Some("abc-123".to_string()));
    }
}
