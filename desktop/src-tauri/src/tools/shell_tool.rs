use super::tool_schema::{RiskLevel, ToolDefinition};
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;

/// Default timeout for shell commands in milliseconds (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Maximum allowed timeout in milliseconds (5 minutes).
const MAX_TIMEOUT_MS: u64 = 300_000;

/// Maximum output size in bytes (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Commands that are blocked from execution for safety.
const BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=/dev/zero",
    "dd if=/dev/random",
    ":(){:|:&};:",
    "fork bomb",
    "chmod -R 777 /",
    "chown -R",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init 0",
    "init 6",
];

/// Commands that trigger a warning but are still allowed.
const WARNED_PREFIXES: &[&str] = &["rm -rf", "sudo", "chmod", "chown", "kill -9", "pkill"];

#[derive(Debug, Deserialize)]
struct ShellInput {
    command: String,
    #[serde(default)]
    working_directory: Option<String>,
    #[serde(default)]
    env: Option<std::collections::HashMap<String, String>>,
}

/// Shell command executor with timeout and safety guardrails.
pub struct ShellTool;

impl ShellTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute a shell command with the given input parameters and optional timeout.
    pub async fn execute(
        &self,
        input: &serde_json::Value,
        timeout_ms: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        let params: ShellInput = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid shell input: {}", e))?;

        // Validate the command against the blocked list
        self.validate_command(&params.command)?;

        // Determine timeout
        let timeout =
            Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS));

        // Log warnings for potentially dangerous commands
        for prefix in WARNED_PREFIXES {
            if params.command.starts_with(prefix) {
                tracing::warn!(
                    command = %params.command,
                    prefix = %prefix,
                    "Executing potentially dangerous command"
                );
                break;
            }
        }

        tracing::info!(command = %params.command, "Executing shell command");

        // Build the command
        let shell = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let shell_flag = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let mut cmd = Command::new(shell);
        cmd.arg(shell_flag).arg(&params.command);

        // Set working directory if specified
        if let Some(ref cwd) = params.working_directory {
            let path = std::path::Path::new(cwd);
            if !path.exists() {
                return Err(format!("Working directory does not exist: {}", cwd));
            }
            if !path.is_dir() {
                return Err(format!("Working directory is not a directory: {}", cwd));
            }
            cmd.current_dir(path);
        }

        // Set environment variables if specified
        if let Some(ref env_vars) = params.env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        // Execute with timeout
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                format!(
                    "Command timed out after {}ms: {}",
                    timeout.as_millis(),
                    params.command
                )
            })?
            .map_err(|e| format!("Failed to execute command: {}", e))?;

        // Collect stdout and stderr, truncating if necessary
        let stdout = Self::truncate_output(&output.stdout);
        let stderr = Self::truncate_output(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        tracing::info!(
            command = %params.command,
            exit_code = exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "Shell command completed"
        );

        Ok(serde_json::json!({
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "success": output.status.success(),
        }))
    }

    /// Validate a command against the blocked command list.
    fn validate_command(&self, command: &str) -> Result<(), String> {
        let normalized = command.trim().to_lowercase();
        for blocked in BLOCKED_COMMANDS {
            if normalized.contains(blocked) {
                return Err(format!(
                    "Command blocked for safety: contains '{}'. This command is not allowed.",
                    blocked
                ));
            }
        }
        Ok(())
    }

    /// Truncate output to MAX_OUTPUT_BYTES and convert to string.
    fn truncate_output(bytes: &[u8]) -> String {
        if bytes.len() > MAX_OUTPUT_BYTES {
            let truncated = &bytes[..MAX_OUTPUT_BYTES];
            let mut s = String::from_utf8_lossy(truncated).to_string();
            s.push_str("\n... [output truncated]");
            s
        } else {
            String::from_utf8_lossy(bytes).to_string()
        }
    }

    /// Return the tool definition for capability reporting.
    pub fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "shell".to_string(),
            description: "Execute shell commands on the desktop system. Supports setting working directory and environment variables. Dangerous commands are blocked.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_directory": {
                        "type": "string",
                        "description": "Working directory for the command"
                    },
                    "env": {
                        "type": "object",
                        "description": "Additional environment variables",
                        "additionalProperties": { "type": "string" }
                    }
                }
            }),
            requires_approval: true,
            risk_level: RiskLevel::High,
        }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}
