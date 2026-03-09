use super::output_parser::parse_cline_line;
use super::{CliSpawnConfig, StreamChunk};

/// Build a CLI spawn config for Cline.
pub fn build_config(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("cline").to_string();
    let mut args = vec!["--json-output".to_string()];

    if !model.is_empty() {
        args.push("--model".to_string());
        args.push(model.to_string());
    }

    args.push(prompt.to_string());

    CliSpawnConfig {
        command,
        args,
        working_directory: working_directory.to_string(),
        env_vars: vec![],
    }
}

/// Parse a line of Cline JSON output.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    parse_cline_line(line)
}
