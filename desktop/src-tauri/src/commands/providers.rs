use crate::db::models::{ProviderConfig, UpsertProviderConfigInput};
use crate::providers::CliProvider;
use crate::state::app_state::AppState;
use rusqlite::params;
use serde::Serialize;
#[cfg(target_os = "windows")]
use std::env;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};
use tauri::State;
use uuid::Uuid;

/// Detected CLI tool information.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedTool {
    pub provider: String,
    pub command: String,
    pub found: bool,
    pub path: Option<String>,
}

/// List all provider configurations.
#[tauri::command]
pub async fn list_provider_configs(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderConfig>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, provider, cli_path, api_key, default_model, settings, is_available, updated_at FROM provider_configs ORDER BY provider",
            )
            .map_err(|e| e.to_string())?;

        let configs = stmt
            .query_map([], |row| {
                Ok(ProviderConfig {
                    id: row.get(0)?,
                    provider: row.get(1)?,
                    cli_path: row.get(2)?,
                    api_key: row.get(3)?,
                    default_model: row.get(4)?,
                    settings: row.get(5)?,
                    is_available: row.get::<_, i32>(6)? != 0,
                    updated_at: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        Ok(configs)
    })
}

/// Upsert a provider configuration.
#[tauri::command]
pub async fn upsert_provider_config(
    input: UpsertProviderConfigInput,
    state: State<'_, AppState>,
) -> Result<ProviderConfig, String> {
    state.db.with_conn(|conn| {
        // Check if provider config exists
        let existing_id: Option<String> = conn
            .query_row(
                "SELECT id FROM provider_configs WHERE provider = ?1",
                params![input.provider],
                |row| row.get(0),
            )
            .ok();

        let id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        conn.execute(
            "INSERT INTO provider_configs (id, provider, cli_path, api_key, default_model, settings, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(provider) DO UPDATE SET
               cli_path = COALESCE(?3, cli_path),
               api_key = COALESCE(?4, api_key),
               default_model = COALESCE(?5, default_model),
               settings = COALESCE(?6, settings),
               updated_at = datetime('now')",
            params![
                id,
                input.provider,
                input.cli_path.unwrap_or_default(),
                input.api_key.unwrap_or_default(),
                input.default_model.unwrap_or_default(),
                input.settings.unwrap_or_else(|| "{}".to_string()),
            ],
        )
        .map_err(|e| format!("Failed to upsert provider config: {}", e))?;

        // Query back
        conn.query_row(
            "SELECT id, provider, cli_path, api_key, default_model, settings, is_available, updated_at FROM provider_configs WHERE provider = ?1",
            params![input.provider],
            |row| {
                Ok(ProviderConfig {
                    id: row.get(0)?,
                    provider: row.get(1)?,
                    cli_path: row.get(2)?,
                    api_key: row.get(3)?,
                    default_model: row.get(4)?,
                    settings: row.get(5)?,
                    is_available: row.get::<_, i32>(6)? != 0,
                    updated_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| format!("Failed to read back config: {}", e))
    })
}

/// Delete a provider configuration.
#[tauri::command]
pub async fn delete_provider_config(
    provider: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM provider_configs WHERE provider = ?1",
            params![provider],
        )
        .map_err(|e| format!("Failed to delete provider config: {}", e))?;
        Ok(())
    })
}

/// Detect which CLI tools are available on the system.
#[tauri::command]
pub async fn detect_cli_tools(
    state: State<'_, AppState>,
) -> Result<Vec<DetectedTool>, String> {
    let providers = [
        CliProvider::ClaudeCode,
        CliProvider::Codex,
        CliProvider::Cline,
        CliProvider::Ollama,
    ];

    let mut results = Vec::new();

    for provider in &providers {
        let command = provider.default_command();
        let (found, path) = detect_command(command).await;

        results.push(DetectedTool {
            provider: provider.as_str().to_string(),
            command: command.to_string(),
            found,
            path: path.clone(),
        });

        // Update provider config with availability
        let _ = state.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO provider_configs (id, provider, cli_path, is_available, updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))
                 ON CONFLICT(provider) DO UPDATE SET
                   cli_path = CASE WHEN ?3 != '' THEN ?3 ELSE cli_path END,
                   is_available = ?4,
                   updated_at = datetime('now')",
                params![
                    Uuid::new_v4().to_string(),
                    provider.as_str(),
                    path.as_deref().unwrap_or(""),
                    found as i32,
                ],
            )
            .map_err(|e| e.to_string())
        });
    }

    Ok(results)
}

/// Check if a command exists on the system PATH.
async fn detect_command(command: &str) -> (bool, Option<String>) {
    #[cfg(target_os = "windows")]
    {
        return detect_command_windows(command).await;
    }

    #[cfg(not(target_os = "windows"))]
    {
        return detect_command_unix(command).await;
    }
}

#[cfg(not(target_os = "windows"))]
async fn detect_command_unix(command: &str) -> (bool, Option<String>) {
    match tokio::process::Command::new("which")
        .arg(command)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (true, Some(path))
        }
        _ => (false, None),
    }
}

#[cfg(target_os = "windows")]
async fn detect_command_windows(command: &str) -> (bool, Option<String>) {
    if let Some(path) = detect_with_where(command).await {
        return (true, Some(path));
    }

    if let Some(path) = find_windows_command_path(command) {
        return (true, Some(path));
    }

    (false, None)
}

#[cfg(target_os = "windows")]
async fn detect_with_where(command: &str) -> Option<String> {
    let output = tokio::process::Command::new("where")
        .arg(command)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_command_output(&output.stdout)
}

#[cfg(target_os = "windows")]
fn find_windows_command_path(command: &str) -> Option<String> {
    if let Some(found) = resolve_windows_path(PathBuf::from(command)) {
        return Some(found);
    }

    let executable_names = windows_executable_names(command);

    for directory in windows_search_directories() {
        for name in &executable_names {
            let candidate = directory.join(name);
            if let Some(found) = resolve_windows_path(candidate) {
                return Some(found);
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn windows_executable_names(command: &str) -> Vec<String> {
    let mut names = vec![command.to_string()];

    if !has_windows_extension(command) {
        names.push(format!("{command}.exe"));
        names.push(format!("{command}.cmd"));
        names.push(format!("{command}.bat"));
    }

    names
}

#[cfg(target_os = "windows")]
fn has_windows_extension(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.ends_with(".exe") || lower.ends_with(".cmd") || lower.ends_with(".bat")
}

#[cfg(target_os = "windows")]
fn windows_search_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();

    if let Some(path_var) = env::var_os("PATH") {
        directories.extend(env::split_paths(&path_var));
    }

    if let Some(app_data) = env::var_os("APPDATA") {
        directories.push(PathBuf::from(app_data).join("npm"));
    }

    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(local_app_data).join("npm"));
    }

    directories
}

#[cfg(target_os = "windows")]
fn resolve_windows_path(path: PathBuf) -> Option<String> {
    let normalized = Path::new(&path);
    if normalized.is_file() {
        return Some(normalized.to_string_lossy().to_string());
    }

    None
}

#[cfg(target_os = "windows")]
fn parse_command_output(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}
