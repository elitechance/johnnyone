use crate::state::app_state::AppState;
use rusqlite::params;
use std::path::{Path, PathBuf};

pub const KEY_WORKER_URL: &str = "worker_url";
pub const KEY_TENANT_ID: &str = "tenant_id";
pub const KEY_USER_ID: &str = "user_id";
pub const KEY_PLANNER_METHODOLOGY_PATH: &str = "planner_methodology_path";
pub const KEY_PLANNER_CONVENTIONS_PATH: &str = "planner_conventions_path";
pub const KEY_WEB_CLIENT_URL: &str = "web_client_url";
/// Discord incoming-webhook URL for attention alerts (blocked / needs-human).
/// Empty = alerts disabled. Read by the coordinator's notifier.
pub const KEY_DISCORD_WEBHOOK_URL: &str = "discord_webhook_url";
pub const KEY_ACCESS_TOKEN: &str = "access_token";

pub const DEFAULT_WORKER_URL: &str = "https://johnnyone.ethan-353.workers.dev";
pub const DEFAULT_TENANT_ID: &str = "00000000-0000-0000-0000-000000000001";
pub const DEFAULT_USER_ID: &str = "00000000-0000-0000-0000-000000000002";
pub const DEFAULT_METHODOLOGY_REL: &str = "lokal/agents/common/methodology.md";
pub const DEFAULT_CONVENTIONS_REL: &str = "lokal/agents/common/conventions";
pub const DEFAULT_WEB_CLIENT_URL: &str = "https://johnnyone.pages.dev/";

#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub worker_url: String,
    pub user_id: String,
    pub tenant_id: String,
    pub access_token: String,
}

#[derive(Debug, Clone)]
pub struct HostSettings {
    pub worker_url: String,
    pub tenant_id: String,
    pub user_id: String,
    pub access_token: String,
    pub planner_methodology_path: String,
    pub planner_conventions_path: String,
    pub web_client_url: String,
    pub discord_webhook_url: String,
}

pub fn get_setting(state: &AppState, key: String) -> Result<String, String> {
    Ok(get_setting_or(state, &key, ""))
}

pub fn get_setting_or(state: &AppState, key: &str, default: &str) -> String {
    state
        .db
        .with_conn(|conn| {
            let result = conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            );
            Ok(result.unwrap_or_else(|_| default.to_string()))
        })
        .unwrap_or_else(|_| default.to_string())
}

pub fn set_setting(state: &AppState, key: String, value: String) -> Result<(), String> {
    state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set setting: {}", e))?;
        Ok(())
    })
}

pub fn load_host_settings(state: &AppState) -> HostSettings {
    HostSettings {
        worker_url: get_setting_or(state, KEY_WORKER_URL, DEFAULT_WORKER_URL),
        tenant_id: get_setting_or(state, KEY_TENANT_ID, DEFAULT_TENANT_ID),
        user_id: get_setting_or(state, KEY_USER_ID, ""),
        access_token: get_setting_or(state, KEY_ACCESS_TOKEN, ""),
        planner_methodology_path: get_setting_or(
            state,
            KEY_PLANNER_METHODOLOGY_PATH,
            DEFAULT_METHODOLOGY_REL,
        ),
        planner_conventions_path: get_setting_or(
            state,
            KEY_PLANNER_CONVENTIONS_PATH,
            DEFAULT_CONVENTIONS_REL,
        ),
        web_client_url: get_setting_or(state, KEY_WEB_CLIENT_URL, DEFAULT_WEB_CLIENT_URL),
        discord_webhook_url: get_setting_or(state, KEY_DISCORD_WEBHOOK_URL, ""),
    }
}

impl RelayConfig {
    pub fn resolve(state: &AppState) -> Option<Self> {
        let worker_url = resolve_worker_url(state);
        let user_id = resolve_user_id(state);
        let tenant_id = resolve_tenant_id(state);
        let access_token = resolve_access_token(state);

        if worker_url.trim().is_empty() || user_id.trim().is_empty() {
            return None;
        }

        Some(Self {
            worker_url,
            user_id,
            tenant_id,
            access_token,
        })
    }
}

fn resolve_worker_url(state: &AppState) -> String {
    std::env::var("JOHNNYONE_WORKER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| get_setting_or(state, KEY_WORKER_URL, DEFAULT_WORKER_URL))
}

fn resolve_user_id(state: &AppState) -> String {
    std::env::var("JOHNNYONE_USER_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| get_setting_or(state, KEY_USER_ID, ""))
}

fn resolve_tenant_id(state: &AppState) -> String {
    std::env::var("JOHNNYONE_TENANT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| get_setting_or(state, KEY_TENANT_ID, DEFAULT_TENANT_ID))
}

fn resolve_access_token(state: &AppState) -> String {
    std::env::var("JOHNNYONE_ACCESS_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| get_setting_or(state, KEY_ACCESS_TOKEN, ""))
}

/// Resolve a configured host path against a planner workspace root.
/// Relative paths are joined to `workspace_path`; absolute paths are used as-is.
pub fn resolve_workspace_host_path(
    workspace_path: &str,
    configured_path: &str,
) -> Result<String, String> {
    let trimmed = configured_path.trim();
    let fallback = if trimmed.is_empty() {
        return Err("Configured path is empty".to_string());
    } else {
        trimmed
    };

    let workspace = normalize_existing_dir(Path::new(workspace_path))?;
    let candidate = if Path::new(fallback).is_absolute() {
        PathBuf::from(fallback)
    } else {
        workspace.join(fallback)
    };

    normalize_path(&candidate).map(|path| path.to_string_lossy().to_string())
}

pub fn resolve_methodology_path(state: &AppState, workspace_path: &str) -> Result<String, String> {
    let configured = get_setting_or(
        state,
        KEY_PLANNER_METHODOLOGY_PATH,
        DEFAULT_METHODOLOGY_REL,
    );
    resolve_workspace_host_path(workspace_path, &configured).or_else(|_| {
        resolve_workspace_host_path(workspace_path, DEFAULT_METHODOLOGY_REL)
    })
}

pub fn resolve_conventions_path(state: &AppState, workspace_path: &str) -> Result<String, String> {
    let configured = get_setting_or(
        state,
        KEY_PLANNER_CONVENTIONS_PATH,
        DEFAULT_CONVENTIONS_REL,
    );
    resolve_workspace_host_path(workspace_path, &configured).or_else(|_| {
        resolve_workspace_host_path(workspace_path, DEFAULT_CONVENTIONS_REL)
    })
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.is_dir() {
        return Err(format!(
            "Workspace path is not a directory: {}",
            path.display()
        ));
    }
    path.canonicalize()
        .map_err(|e| format!("Invalid workspace path: {}", e))
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Invalid path: {}", e))
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| "Path has no parent".to_string())?
            .canonicalize()
            .map_err(|e| format!("Invalid parent path: {}", e))?;
        Ok(parent.join(
            path.file_name()
                .ok_or_else(|| "Path has no file name".to_string())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_path_against_workspace() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../..")
            .canonicalize()
            .expect("workspace root");
        let resolved = resolve_workspace_host_path(
            workspace.to_string_lossy().as_ref(),
            "lokal/agents/common/methodology.md",
        )
        .expect("relative path should resolve");
        assert!(resolved.contains("methodology.md"));
        assert!(Path::new(&resolved).is_file());
    }
}