pub mod cli_runner;
pub mod claude_code;
pub mod codex;
pub mod cline;
pub mod ollama_cli;
pub mod output_parser;

use serde::{Deserialize, Serialize};

/// Supported CLI provider types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliProvider {
    ClaudeCode,
    Codex,
    Cline,
    Ollama,
}

impl CliProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Codex => "codex",
            Self::Cline => "cline",
            Self::Ollama => "ollama",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude_code" | "claude-code" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "cline" => Some(Self::Cline),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }

    pub fn default_command(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Cline => "cline",
            Self::Ollama => "ollama",
        }
    }
}

/// A chunk of output from a CLI subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub chunk_type: ChunkType,
    pub content: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_output: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub is_final: bool,
    /// The CLI tool's own session ID (from init or result events).
    /// Used to resume sessions with --resume.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Types of stream chunks emitted during CLI execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkType {
    /// Text content from the assistant
    Text,
    /// A tool is being invoked
    ToolUse,
    /// Result from a tool invocation
    ToolResult,
    /// Final result summary with usage stats
    Result,
    /// Error from the CLI
    Error,
    /// Raw stderr output (informational)
    Stderr,
    /// System/init event (contains session_id, model info, etc.)
    System,
}

/// Configuration for spawning a CLI subprocess.
#[derive(Debug, Clone)]
pub struct CliSpawnConfig {
    pub command: String,
    pub args: Vec<String>,
    pub working_directory: String,
    /// Environment variables to set (empty value = unset the var).
    pub env_vars: Vec<(String, String)>,
}
