use super::output_parser::parse_grok_line;
use super::{CliSpawnConfig, StreamChunk};

/// Build a CLI spawn config for xAI Grok CLI (headless streaming-json mode).
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
    cli_session_id: Option<&str>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("grok").to_string();
    let mut args = vec![
        "--output-format".to_string(),
        "streaming-json".to_string(),
        "--always-approve".to_string(),
        "--no-alt-screen".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--cwd".to_string(),
        working_directory.to_string(),
        "--single".to_string(),
        prompt.to_string(),
    ];

    if !model.is_empty() {
        args.push("-m".to_string());
        args.push(model.to_string());
    }

    if let Some(session_id) = cli_session_id {
        args.push("-r".to_string());
        args.push(session_id.to_string());
    }

    CliSpawnConfig {
        command,
        args,
        working_directory: working_directory.to_string(),
        env_vars: vec![],
    }
}

/// Parse a line of Grok streaming-json output.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    parse_grok_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChunkType;

    #[test]
    fn test_build_config_new_session() {
        let config = build_config("hello", "/tmp/ws", "grok-composer-2.5-fast", None, None);
        assert_eq!(config.command, "grok");
        assert!(config.args.contains(&"--output-format".to_string()));
        assert!(config.args.contains(&"streaming-json".to_string()));
        assert!(config.args.contains(&"--single".to_string()));
        assert!(config.args.contains(&"hello".to_string()));
        assert!(config.args.contains(&"--cwd".to_string()));
        assert!(config.args.contains(&"/tmp/ws".to_string()));
        assert!(!config.args.iter().any(|a| a == "-r"));
    }

    #[test]
    fn test_build_config_resume_session() {
        let config = build_config("follow up", "/tmp/ws", "", None, Some("sess-123"));
        assert!(config.args.contains(&"-r".to_string()));
        assert!(config.args.contains(&"sess-123".to_string()));
    }

    #[test]
    fn test_parse_text_line() {
        let chunk = parse_line(r#"{"type":"text","data":"Hello"}"#).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Text);
        assert_eq!(chunk.content, "Hello");
    }

    #[test]
    fn test_parse_end_line() {
        let chunk =
            parse_line(r#"{"type":"end","stopReason":"EndTurn","sessionId":"sess-1"}"#).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Result);
        assert_eq!(chunk.session_id.as_deref(), Some("sess-1"));
        assert!(chunk.is_final);
    }
}