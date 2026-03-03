use super::{ChunkType, StreamChunk};

/// Parse a Claude Code stream-json line into a StreamChunk.
///
/// Actual Claude CLI `--output-format stream-json --verbose` output format:
///
/// Init:    `{"type":"system","subtype":"init","session_id":"...","model":"...", ...}`
/// Text:    `{"type":"assistant","message":{"content":[{"type":"text","text":"..."}],"usage":{...}},"session_id":"..."}`
/// Result:  `{"type":"result","subtype":"success","result":"...","session_id":"...","total_cost_usd":0.027,"usage":{"input_tokens":3,"output_tokens":4,...}}`
/// Error:   `{"type":"error","error":{"message":"..."}}`
pub fn parse_claude_code_line(line: &str) -> Option<StreamChunk> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg_type = v.get("type")?.as_str()?;

    match msg_type {
        "system" => {
            // Init event — carries the CLI's session_id
            let session_id = v.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string());
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
            Some(StreamChunk {
                chunk_type: ChunkType::System,
                content: format!("Session initialized (model: {})", model),
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id,
            })
        }
        "assistant" => {
            let content = extract_assistant_text(&v)?;
            let session_id = v.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string());
            Some(StreamChunk {
                chunk_type: ChunkType::Text,
                content,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id,
            })
        }
        "tool_use" => {
            let tool = v.get("tool");
            let name = tool
                .and_then(|t| t.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            let input = tool.and_then(|t| t.get("input").cloned());
            Some(StreamChunk {
                chunk_type: ChunkType::ToolUse,
                content: format!("Using tool: {}", name),
                tool_name: Some(name),
                tool_input: input,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id: None,
            })
        }
        "tool_result" => {
            let content = v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            Some(StreamChunk {
                chunk_type: ChunkType::ToolResult,
                content: content.clone(),
                tool_name: None,
                tool_input: None,
                tool_output: Some(content),
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id: None,
            })
        }
        "result" => {
            // Check for error results (e.g. failed --resume with invalid session ID)
            let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            if is_error {
                let errors = v.get("errors")
                    .and_then(|e| e.as_array())
                    .map(|arr| arr.iter()
                        .filter_map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join("; "))
                    .unwrap_or_else(|| "Unknown error".to_string());
                return Some(StreamChunk {
                    chunk_type: ChunkType::Error,
                    content: errors,
                    tool_name: None,
                    tool_input: None,
                    tool_output: None,
                    cost_usd: None,
                    input_tokens: None,
                    output_tokens: None,
                    is_final: true,
                    session_id: None,
                });
            }

            // Actual format: total_cost_usd at top level, tokens under usage object
            let cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
            let usage = v.get("usage");
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|t| t.as_i64());
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|t| t.as_i64());
            let session_id = v.get("session_id").and_then(|s| s.as_str()).map(|s| s.to_string());
            let result_text = v
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string();
            Some(StreamChunk {
                chunk_type: ChunkType::Result,
                content: result_text,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: cost,
                input_tokens,
                output_tokens,
                is_final: true,
                session_id,
            })
        }
        "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            Some(StreamChunk {
                chunk_type: ChunkType::Error,
                content: message,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: true,
                session_id: None,
            })
        }
        // Ignore rate_limit_event and other unknown types
        _ => None,
    }
}

/// Extract text content from a Claude Code assistant message.
fn extract_assistant_text(v: &serde_json::Value) -> Option<String> {
    let message = v.get("message")?;
    let content = message.get("content")?;
    if let Some(arr) = content.as_array() {
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str()
                } else {
                    None
                }
            })
            .collect();
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(""))
        }
    } else if let Some(s) = content.as_str() {
        Some(s.to_string())
    } else {
        None
    }
}

/// Parse a Codex JSON output line.
pub fn parse_codex_line(line: &str) -> Option<StreamChunk> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "message" | "" => {
            let content = v
                .get("content")
                .or_else(|| v.get("message").and_then(|m| m.get("content")))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                return None;
            }
            Some(StreamChunk {
                chunk_type: ChunkType::Text,
                content,
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
        _ => None,
    }
}

/// Parse an Ollama streaming JSON line.
pub fn parse_ollama_line(line: &str) -> Option<StreamChunk> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    if content.is_empty() && !done {
        return None;
    }

    let input_tokens = v.get("prompt_eval_count").and_then(|t| t.as_i64());
    let output_tokens = v.get("eval_count").and_then(|t| t.as_i64());

    Some(StreamChunk {
        chunk_type: if done { ChunkType::Result } else { ChunkType::Text },
        content,
        tool_name: None,
        tool_input: None,
        tool_output: None,
        cost_usd: None,
        input_tokens: if done { input_tokens } else { None },
        output_tokens: if done { output_tokens } else { None },
        is_final: done,
        session_id: None,
    })
}

/// Parse a Cline JSON output line.
pub fn parse_cline_line(line: &str) -> Option<StreamChunk> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "text" | "content" | "" => {
            let content = v
                .get("content")
                .or_else(|| v.get("text"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if content.is_empty() {
                return None;
            }
            Some(StreamChunk {
                chunk_type: ChunkType::Text,
                content,
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
        "tool_use" => {
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();
            let input = v.get("input").cloned();
            Some(StreamChunk {
                chunk_type: ChunkType::ToolUse,
                content: format!("Using tool: {}", name),
                tool_name: Some(name),
                tool_input: input,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: false,
                session_id: None,
            })
        }
        "result" => {
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
            Some(StreamChunk {
                chunk_type: ChunkType::Result,
                content,
                tool_name: None,
                tool_input: None,
                tool_output: None,
                cost_usd: None,
                input_tokens: None,
                output_tokens: None,
                is_final: true,
                session_id: None,
            })
        }
        _ => None,
    }
}
