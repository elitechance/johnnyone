use super::{ChatMessage, ChatResponse, CompletionOptions, UsageInfo};
use serde::{Deserialize, Serialize};

/// Default Ollama base URL.
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama HTTP client for local inference via the Ollama API.
///
/// Ollama runs locally and provides an OpenAI-compatible API for chat completions.
/// No API key is required for local usage.
pub struct OllamaClient {
    base_url: String,
    client: reqwest::Client,
}

/// Ollama API request body for /api/chat.
#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

/// Ollama API response from /api/chat (non-streaming).
#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    model: String,
    message: OllamaResponseMessage,
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    total_duration: Option<u64>,
    #[serde(default)]
    eval_count: Option<u32>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    role: String,
    content: String,
}

/// Ollama model info from /api/tags.
#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelInfo {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: Option<String>,
}

impl OllamaClient {
    /// Create a new Ollama client with the default base URL.
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Create a new Ollama client with a custom base URL.
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Send a chat completion request to Ollama.
    pub async fn chat_completion(
        &self,
        messages: Vec<ChatMessage>,
        options: CompletionOptions,
    ) -> Result<ChatResponse, String> {
        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<OllamaChatMessage> = messages
            .into_iter()
            .map(|m| OllamaChatMessage {
                role: m.role,
                content: m.content,
            })
            .collect();

        let request_body = OllamaChatRequest {
            model: options.model.clone(),
            messages: ollama_messages,
            stream: false, // Non-streaming for now
            options: Some(OllamaOptions {
                temperature: options.temperature,
                num_predict: options.max_tokens,
            }),
        };

        tracing::debug!(
            model = %options.model,
            url = %url,
            "Sending chat completion to Ollama"
        );

        let response = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Ollama request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(format!(
                "Ollama returned HTTP {}: {}",
                status, body
            ));
        }

        let ollama_response: OllamaChatResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

        let usage = match (ollama_response.prompt_eval_count, ollama_response.eval_count) {
            (Some(prompt), Some(completion)) => Some(UsageInfo {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
            }),
            _ => None,
        };

        Ok(ChatResponse {
            content: ollama_response.message.content,
            model: ollama_response.model,
            finish_reason: ollama_response.done_reason,
            usage,
        })
    }

    /// Check if Ollama is running and reachable.
    pub async fn health_check(&self) -> Result<bool, String> {
        let url = format!("{}/api/tags", self.base_url);

        match self.client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::debug!("Ollama health check: OK");
                    Ok(true)
                } else {
                    tracing::warn!(
                        status = %response.status(),
                        "Ollama health check: unhealthy"
                    );
                    Ok(false)
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Ollama health check: unreachable");
                Err(format!("Ollama is not reachable: {}", e))
            }
        }
    }

    /// List available models from Ollama.
    pub async fn list_models(&self) -> Result<Vec<String>, String> {
        let url = format!("{}/api/tags", self.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to list models: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Ollama returned HTTP {}", response.status()));
        }

        let tags: OllamaTagsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse model list: {}", e))?;

        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }

    /// Get the provider name.
    pub fn name(&self) -> &str {
        "ollama"
    }
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_base_url() {
        let client = OllamaClient::new();
        assert_eq!(client.base_url, "http://localhost:11434");
    }

    #[test]
    fn test_custom_base_url_trailing_slash() {
        let client = OllamaClient::with_base_url("http://localhost:11434/");
        assert_eq!(client.base_url, "http://localhost:11434");
    }
}
