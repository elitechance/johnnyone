use super::output_parser::parse_claude_code_line;
use super::{CliSpawnConfig, StreamChunk};

/// Every built-in claude tool that can read, write, execute or fetch — denied to the plan agent
/// (master D2). The plan agent's only tools are the `mcp__j1__*` ones in `--allowedTools`; this list
/// is belt-and-braces on top of `--permission-mode default` + `--strict-mcp-config`. Probed against
/// the installed CLI 2.1.330 (phase 02 task 07 `artifacts/deny-list-probe.txt`): `LS`, `MultiEdit`,
/// `NotebookRead` and `SlashCommand` are not tools in that version ("matches no known tool") and
/// were dropped from the master D2 list. `Agent`, `LSP` and `Skill` were added after phase 02 review
/// (probed: all known; `artifacts/deny-list-probe-tools.txt`). This list is the SECOND layer: the
/// primary control is `--tools ""` (no built-in tool exists at all), see [`build_config_with`].
pub const PLAN_AGENT_DISALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "Bash",
    "BashOutput",
    "KillShell",
    "Edit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
    "TodoWrite",
    "Agent",
    "LSP",
    "Skill",
];

/// What turns a plain claude_code chat turn into a plan-agent turn (phase 02 task 07). Built by
/// `services::plan_agent::plan_agent_extras`; it holds the config file PATH, never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchExtras {
    /// `<initiatives_dir>/<id>/plan-agent/mcp.json` (0600 in a 0700 dir).
    pub mcp_config_path: String,
    /// Exactly the `mcp__j1__*` tool names.
    pub allowed_tools: Vec<String>,
    /// [`PLAN_AGENT_DISALLOWED_TOOLS`].
    pub disallowed_tools: Vec<String>,
    /// `--append-system-prompt` text.
    pub append_system_prompt: String,
}

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
    build_config_with(prompt, working_directory, model, cli_path, cli_session_id, None)
}

/// [`build_config`] plus optional plan-agent [`LaunchExtras`]. With extras the turn runs with
/// `--permission-mode default` (NOT `bypassPermissions`, so in `--print` mode any tool outside the
/// allow-list is denied), `--mcp-config <path> --strict-mcp-config` (only the j1 server),
/// `--allowedTools` = the j1 tools, `--disallowedTools` = every built-in file/exec/web tool, and the
/// plan-agent system prompt. The token never appears here: only the config file's path does.
pub fn build_config_with(
    prompt: &str,
    working_directory: &str,
    model: &str,
    cli_path: Option<&str>,
    cli_session_id: Option<&str>,
    extras: Option<&LaunchExtras>,
) -> CliSpawnConfig {
    let command = cli_path.unwrap_or("claude").to_string();
    let mut args = vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--print".to_string(),
        "--permission-mode".to_string(),
        if extras.is_some() { "default" } else { "bypassPermissions" }.to_string(),
    ];

    // Each list is ONE comma-joined value, and the last flag before the prompt takes exactly one
    // value, so the CLI's variadic `--allowedTools`/`--mcp-config` can never swallow the prompt.
    if let Some(x) = extras {
        // `--tools ""` = NO built-in tool is available (an allow-list of none; CLI 2.1.330's init
        // event then reports `tools: []`). It cannot go stale when a CLI release adds a tool, which
        // a deny list can. The j1 tools still arrive through the MCP config below.
        args.push("--tools".to_string());
        args.push(String::new());
        args.push("--mcp-config".to_string());
        args.push(x.mcp_config_path.clone());
        args.push("--strict-mcp-config".to_string());
        args.push("--allowedTools".to_string());
        args.push(x.allowed_tools.join(","));
        args.push("--disallowedTools".to_string());
        args.push(x.disallowed_tools.join(","));
        args.push("--append-system-prompt".to_string());
        args.push(x.append_system_prompt.clone());
    }

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
        stdin: None,
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
    fn plain_chat_args_unchanged() {
        let plain = build_config("hello", "/tmp", "opus", None, Some("sid"));
        let with_none = build_config_with("hello", "/tmp", "opus", None, Some("sid"), None);
        assert_eq!(plain.args, with_none.args);
        assert_eq!(plain.command, with_none.command);
        let i = plain.args.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(plain.args[i + 1], "bypassPermissions", "non-plan sessions keep bypassPermissions");
        for flag in ["--tools", "--mcp-config", "--strict-mcp-config", "--allowedTools", "--disallowedTools", "--append-system-prompt"] {
            assert!(!plain.args.iter().any(|a| a == flag), "{flag} leaked into a plain chat");
        }
        assert_eq!(plain.args.last().unwrap(), "hello");
        assert_eq!(plain.env_vars, vec![("CLAUDECODE".to_string(), String::new())]);
    }

    #[test]
    fn plan_agent_deny_list_matches_the_probe() {
        // artifacts/deny-list-probe.txt (phase 02 task 07), last line.
        assert_eq!(
            PLAN_AGENT_DISALLOWED_TOOLS.join(","),
            "Read,Grep,Glob,Bash,BashOutput,KillShell,Edit,Write,NotebookEdit,WebFetch,WebSearch,Task,TodoWrite,Agent,LSP,Skill"
        );
    }

    #[test]
    fn test_unsets_claudecode_env() {
        let config = build_config("hello", "/tmp", "", None, None);
        assert!(config
            .env_vars
            .iter()
            .any(|(k, v)| k == "CLAUDECODE" && v.is_empty()));
    }

    #[test]
    fn test_parse_assistant_message() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#;
        let chunk = parse_line(line).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Text);
        assert_eq!(chunk.content, "Hello!");
    }

    #[test]
    fn parse_tool_use_and_tool_result_lines() {
        let use_line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"mcp__j1__brief_update","input":{"field":"why","add":"x"}}]},"session_id":"s"}"#;
        let c = parse_line(use_line).unwrap();
        assert_eq!(c.chunk_type, ChunkType::ToolUse);
        assert_eq!(c.tool_name.as_deref(), Some("mcp__j1__brief_update"));
        assert_eq!(c.tool_input.unwrap()["field"], "why");
        assert!(c.content.is_empty(), "no tool plumbing in the streamed text");
        let res_line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"{\"version\":3}"}]}]}}"#;
        let c = parse_line(res_line).unwrap();
        assert_eq!(c.chunk_type, ChunkType::ToolResult);
        assert_eq!(c.tool_output.as_deref(), Some("{\"version\":3}"));
        let plain = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        assert_eq!(parse_line(plain).unwrap().tool_output.as_deref(), Some("ok"));
        // A user line with no tool_result is still ignored.
        assert!(parse_line(r#"{"type":"user","message":{"content":"hi"}}"#).is_none());
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
