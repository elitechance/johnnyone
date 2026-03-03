use super::{CliSpawnConfig, StreamChunk};
use super::output_parser::parse_ollama_line;

/// Build a CLI spawn config for Ollama CLI.
///
/// Uses `ollama run <model>` which streams by default.
/// Input is sent via stdin.
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("ollama").to_string();
    let model = if model.is_empty() { "llama3.2" } else { model };

    CliSpawnConfig {
        command,
        args: vec![
            "run".to_string(),
            model.to_string(),
            "--format".to_string(),
            "json".to_string(),
            prompt.to_string(),
        ],
        working_directory: working_directory.to_string(),
        env_vars: vec![],
    }
}

/// Parse a line of Ollama output.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    parse_ollama_line(line)
}
