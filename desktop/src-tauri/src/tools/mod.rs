pub mod browser_tool;
pub mod filesystem_tool;
pub mod process_tool;
pub mod shell_tool;
pub mod tool_schema;

use crate::agent::message_types::{ToolCall, ToolResult};
use std::time::Instant;
use tracing;

/// ToolDispatcher receives ToolCall messages and routes them to the
/// appropriate tool executor. It collects the result and returns a ToolResult.
pub struct ToolDispatcher {
    shell: shell_tool::ShellTool,
    filesystem: filesystem_tool::FilesystemTool,
    process: process_tool::ProcessTool,
    browser: browser_tool::BrowserTool,
}

impl ToolDispatcher {
    /// Create a new ToolDispatcher with all tool executors initialized.
    pub fn new() -> Self {
        Self {
            shell: shell_tool::ShellTool::new(),
            filesystem: filesystem_tool::FilesystemTool::new(),
            process: process_tool::ProcessTool::new(),
            browser: browser_tool::BrowserTool::new(),
        }
    }

    /// Dispatch a tool call to the appropriate executor and return the result.
    pub async fn dispatch(&self, tool_call: &ToolCall) -> ToolResult {
        let start = Instant::now();

        tracing::info!(
            tool = %tool_call.tool_name,
            call_id = %tool_call.call_id,
            "Dispatching tool call"
        );

        let result = match tool_call.tool_name.as_str() {
            "shell" => self.shell.execute(&tool_call.input, tool_call.timeout_ms).await,
            "filesystem" => self.filesystem.execute(&tool_call.input).await,
            "process" => self.process.execute(&tool_call.input).await,
            "browser" => self.browser.execute(&tool_call.input).await,
            unknown => {
                Err(format!("Unknown tool: {}. Available tools: shell, filesystem, process, browser", unknown))
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                tracing::info!(
                    tool = %tool_call.tool_name,
                    call_id = %tool_call.call_id,
                    duration_ms = duration_ms,
                    "Tool execution succeeded"
                );
                ToolResult::success(tool_call.call_id.clone(), output, duration_ms)
            }
            Err(error) => {
                tracing::warn!(
                    tool = %tool_call.tool_name,
                    call_id = %tool_call.call_id,
                    duration_ms = duration_ms,
                    error = %error,
                    "Tool execution failed"
                );
                ToolResult::failure(tool_call.call_id.clone(), error, duration_ms)
            }
        }
    }

    /// Get the list of available tool definitions for capability reporting.
    pub fn get_tool_definitions(&self) -> Vec<tool_schema::ToolDefinition> {
        vec![
            self.shell.definition(),
            self.filesystem.definition(),
            self.process.definition(),
            self.browser.definition(),
        ]
    }
}

impl Default for ToolDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
