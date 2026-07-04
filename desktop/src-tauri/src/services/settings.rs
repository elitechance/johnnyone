use crate::state::app_state::AppState;
use rusqlite::params;
use std::path::{Component, Path, PathBuf};

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
/// Refresh token persisted at login so the relay can refresh a short-lived JWT
/// credential without a restart. Empty for durable `jk_` API-key credentials.
pub const KEY_REFRESH_TOKEN: &str = "refresh_token";
/// Global initiatives store: the directory (outside every repo) that holds Initiative
/// plans under `<initiatives_dir>/<initiative_id>/plan/`. Surfaced through the existing
/// `get_setting`/`set_setting` pair — no dedicated command.
pub const KEY_INITIATIVES_DIR: &str = "initiatives_dir";
/// Global browse root for the file manager (design §5). Distinct from `initiatives_dir` (the plan
/// store). Absolute, user-configurable via the existing `get_setting`/`set_setting` surface — no
/// dedicated command.
pub const KEY_FILES_ROOT: &str = "files_root";

pub const DEFAULT_WORKER_URL: &str = "https://johnnyone.ethan-353.workers.dev";
pub const DEFAULT_TENANT_ID: &str = "00000000-0000-0000-0000-000000000001";
pub const DEFAULT_USER_ID: &str = "00000000-0000-0000-0000-000000000002";
pub const DEFAULT_METHODOLOGY_REL: &str = "lokal/agents/common/methodology.md";
pub const DEFAULT_CONVENTIONS_REL: &str = "lokal/agents/common/conventions";
pub const DEFAULT_WEB_CLIENT_URL: &str = "https://johnnyone.pages.dev/";
/// Default global initiatives store — an absolute path at the Workspace root, outside every
/// repo (design §5b). There is no existing "workspace root" constant to reuse.
pub const DEFAULT_INITIATIVES_DIR: &str = "/home/creepy/Documents/Workspace/.johnnyone/initiatives";
/// Default global file-manager browse root — an absolute path at the Workspace root (design §5).
/// Distinct from the plan store (`DEFAULT_INITIATIVES_DIR`, decision D2).
pub const DEFAULT_FILES_ROOT: &str = "/home/creepy/Documents/Workspace";

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

pub fn resolve_worker_url(state: &AppState) -> String {
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

/// Absolute plan directory for an initiative inside the store: `<dir>/<id>/plan`.
/// Pure — no filesystem or DB access, so it is unit-testable.
pub fn initiative_plan_path(initiatives_dir: &Path, initiative_id: &str) -> PathBuf {
    initiatives_dir.join(initiative_id).join("plan")
}

/// Absolute attachments directory for an initiative: `<dir>/<id>/attachments`.
/// Pure — no filesystem or DB access, so it is unit-testable. Briefing uploads
/// (📎 Attach / ⤒ Upload) land here via the re-rooted P2 upload engine (D5).
pub fn initiative_attachments_path(initiatives_dir: &Path, initiative_id: &str) -> PathBuf {
    initiatives_dir.join(initiative_id).join("attachments")
}

/// Resolve the configured global initiatives store dir (absolute).
/// Falls back to `DEFAULT_INITIATIVES_DIR` when the setting is unset/empty.
pub fn resolve_initiatives_dir(state: &AppState) -> PathBuf {
    let configured = get_setting_or(state, KEY_INITIATIVES_DIR, DEFAULT_INITIATIVES_DIR);
    let trimmed = configured.trim();
    PathBuf::from(if trimmed.is_empty() {
        DEFAULT_INITIATIVES_DIR
    } else {
        trimmed
    })
}

/// Resolve the configured global file-manager root (absolute). Falls back to `DEFAULT_FILES_ROOT`
/// when the setting is unset/empty. Mirrors [`resolve_initiatives_dir`].
pub fn resolve_files_root(state: &AppState) -> PathBuf {
    let configured = get_setting_or(state, KEY_FILES_ROOT, DEFAULT_FILES_ROOT);
    let trimmed = configured.trim();
    PathBuf::from(if trimmed.is_empty() {
        DEFAULT_FILES_ROOT
    } else {
        trimmed
    })
}

/// Resolve `rel` under `root`, rejecting traversal above `root` and any `..` segment.
///
/// Pure: canonicalizes the containment prefix (defeating symlink/`.`/`..` escapes) but performs no
/// read/write. `rel` may be relative (joined onto `root`) or absolute (which must still resolve
/// in-root). Handles not-yet-existing targets (write/mkdir/upload) by canonicalizing the deepest
/// existing ancestor and re-appending the remaining components, so the guard runs *before* any
/// directory is created. Reuses the single [`normalize_path`] for the root side.
pub fn resolve_within_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let raw = Path::new(rel);
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Path must not contain '..'".to_string());
    }
    let candidate = if raw.is_absolute() {
        PathBuf::from(rel)
    } else {
        root.join(rel)
    };
    let normalized_root = normalize_path(root)?;
    let normalized = normalize_existing_prefix(&candidate)?;
    if !normalized.starts_with(&normalized_root) {
        return Err("Path is outside the configured files_root".to_string());
    }
    Ok(normalized)
}

/// Canonicalize the deepest ancestor of `path` that exists (resolving symlinks) and re-append the
/// remaining, not-yet-created components. Unlike [`normalize_path`], this tolerates an arbitrary
/// number of missing nested segments (e.g. `write_file("a/b/c.txt")` before `a/b` exists) while
/// still resolving symlinks on the existing prefix so containment cannot be escaped.
fn normalize_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| "Path has no file name".to_string())?
            .to_os_string();
        tail.push(name);
        existing = existing
            .parent()
            .ok_or_else(|| "Path has no parent".to_string())?
            .to_path_buf();
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|e| format!("Invalid path: {}", e))?;
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
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
    if Path::new(fallback).is_absolute() {
        return normalize_path(&PathBuf::from(fallback)).map(|path| path.to_string_lossy().to_string());
    }
    // Relative config path (e.g. the default `lokal/agents/common/methodology.md`). The shared
    // methodology/conventions usually live at the WORKSPACE-tree root, not inside each app's own
    // workspace dir — so try the workspace, then walk up its ancestors and return the first place the
    // path actually EXISTS. Without this, a nested workspace (…/personal/hello-e2e) resolves to a
    // non-existent …/hello-e2e/lokal/agents/common/methodology.md and the planner, given a missing
    // methodology, improvises an off-spec plan (flat plan.md instead of overview.md + phases/).
    for ancestor in workspace.ancestors() {
        let candidate = ancestor.join(fallback);
        if candidate.exists() {
            return candidate
                .canonicalize()
                .map(|path| path.to_string_lossy().to_string())
                .map_err(|e| format!("Invalid path: {}", e));
        }
    }
    // Nothing found up the tree — fall back to the workspace-relative path (may not exist yet).
    normalize_path(&workspace.join(fallback)).map(|path| path.to_string_lossy().to_string())
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

pub(crate) fn normalize_path(path: &Path) -> Result<PathBuf, String> {
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

    #[test]
    fn initiative_plan_path_builds_id_plan() {
        assert_eq!(
            initiative_plan_path(Path::new("/store"), "abc"),
            PathBuf::from("/store/abc/plan")
        );
    }

    #[test]
    fn initiative_attachments_path_builds_id_attachments() {
        assert_eq!(
            initiative_attachments_path(Path::new("/store"), "abc"),
            PathBuf::from("/store/abc/attachments")
        );
    }

    /// A fresh, real temp dir so `canonicalize` succeeds for the containment branch. `Date::now`/
    /// random are unavailable here, so make the suffix unique with the pid + a static counter
    /// (mirrors the Phase-1 `tmp_dir` harness).
    fn guard_tmp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "j1-p2-guard-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolve_within_root_rejects_dotdot() {
        let root = guard_tmp_root();
        assert!(resolve_within_root(&root, "../etc/passwd").is_err());
        assert!(resolve_within_root(&root, "a/../../b").is_err());
    }

    #[test]
    fn resolve_within_root_rejects_above_root() {
        let root = guard_tmp_root();
        // Absolute path outside the root escapes containment even without a `..` segment.
        assert!(resolve_within_root(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn resolve_within_root_allows_in_root() {
        let root = guard_tmp_root();
        let canonical_root = root.canonicalize().unwrap();
        // A not-yet-existing nested target still resolves and stays contained.
        let resolved = resolve_within_root(&root, "sub/file.txt").expect("in-root path");
        assert!(resolved.starts_with(&canonical_root));
        assert!(resolved.ends_with("sub/file.txt"));
    }
}