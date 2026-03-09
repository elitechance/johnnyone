pub fn host_simulator_enabled() -> bool {
    std::env::var("JOHNNYONE_HOST_SIMULATOR")
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

pub fn simulated_cli_path(command: &str) -> String {
    format!("/simulator/bin/{}", command)
}

pub fn simulated_chat_response(provider: &str, model: &str, content: &str) -> String {
    let trimmed = content.trim();
    let reply_body = if trimmed.is_empty() {
        "No prompt content was provided.".to_string()
    } else {
        format!("Echo: {}", trimmed)
    };

    format!(
        "Simulated {} response via {}.\n\n{}",
        provider,
        if model.is_empty() { "default-model" } else { model },
        reply_body
    )
}
