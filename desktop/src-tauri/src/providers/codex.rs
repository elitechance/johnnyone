use super::{CliSpawnConfig, StreamChunk};
use super::output_parser::parse_codex_line;

/// Build a CLI spawn config for OpenAI Codex CLI.
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
    cli_session_id: Option<&str>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("codex").to_string();
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "--sandbox".to_string(),
        "danger-full-access".to_string(),
    ];

    if !model.is_empty() {
        args.push("-m".to_string());
        args.push(model.to_string());
    }

    if let Some(session_id) = cli_session_id {
        args.push("resume".to_string());
        args.push(session_id.to_string());
    }

    args.push(prompt.to_string());

    CliSpawnConfig {
        command,
        args,
        working_directory: working_directory.to_string(),
        env_vars: vec![],
    }
}

/// Parse a line of Codex JSON output.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    parse_codex_line(line)
}
