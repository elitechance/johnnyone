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
    // `files_root` is a security boundary, not just a browse root — `report_markdown_roots`
    // confines the `evidence` markdown reader to it. Reject a value that would make that
    // confinement vacuous HERE, on the way in, so the caller gets a real error instead of a
    // silently-ignored setting. `resolve_files_root` re-checks on read because this mutation is
    // reachable remotely with no scope check and the DB may already hold a poisoned value.
    if key == KEY_FILES_ROOT {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            if let Err(reason) = reject_unsafe_confinement_root(Path::new(trimmed)) {
                return Err(format!("Refusing files_root {:?}: it {}", trimmed, reason));
            }
        }
    }
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

/// Absolute run-state directory for one development phase:
/// `<dir>/<initiative_id>/runs/<plan_id>/<phase_id>`.
/// Pure — no filesystem or DB access. Lives *next to* the plan store, never
/// inside it. Phase-keyed so a later phase cannot clobber this phase's
/// `tasks.json`. Not a snapshot helper (Amendment 1 / D7).
pub fn initiative_runs_path(
    initiatives_dir: &Path,
    initiative_id: &str,
    plan_id: &str,
    phase_id: &str,
) -> PathBuf {
    initiatives_dir
        .join(initiative_id)
        .join("runs")
        .join(plan_id)
        .join(phase_id)
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
/// when the setting is unset/empty, or when the stored value is too broad to be a confinement
/// boundary (see [`reject_unsafe_confinement_root`]). Mirrors [`resolve_initiatives_dir`].
///
/// The fallback is not belt-and-braces. `files_root` is settable remotely via `updateSetting`,
/// which performs no scope check, so the DB may ALREADY hold a poisoned value written before this
/// validation existed; rejecting on write alone would honour it forever. Validating on read too
/// means a poisoned `files_root` degrades to the default rather than opening every path on the box.
pub fn resolve_files_root(state: &AppState) -> PathBuf {
    let configured = get_setting_or(state, KEY_FILES_ROOT, DEFAULT_FILES_ROOT);
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return PathBuf::from(DEFAULT_FILES_ROOT);
    }
    if let Err(reason) = reject_unsafe_confinement_root(Path::new(trimmed)) {
        tracing::warn!(
            configured = trimmed,
            %reason,
            "stored files_root is unsafe as a confinement root; falling back to the default"
        );
        return PathBuf::from(DEFAULT_FILES_ROOT);
    }
    PathBuf::from(trimmed)
}

/// Reject a directory that is too broad to serve as a confinement boundary.
///
/// `files_root` began life as a UX browse-root for the file manager, but it is now also a security
/// boundary: `report_markdown_roots` confines the `evidence` markdown reader to it. That promotion
/// means it inherits an existing hole — `worker/resolvers/ai/update-setting.ts` has no
/// `authorizeForAltToken` call, so ANY authenticated identity (an API key with zero scopes
/// included) can call `updateSetting(key:"files_root", value:"/")`. With `/` as a root every
/// `starts_with` check passes and the confinement guard silently becomes a no-op, re-opening the
/// exfiltration of `~/.claude/**/MEMORY.md` it was written to stop. The worker-side scope check is
/// a separate ticket; this is the host-side half, and it is the half that actually has to hold.
///
/// Two rules, both about `$HOME` rather than about any one secret file:
/// 1. The root may not be a filesystem root, `$HOME` itself, or any ancestor of `$HOME` — those
///    all make the user's entire home directory, dotfiles included, "in root".
/// 2. The root may not be a hidden directory directly under `$HOME` (`~/.claude`, `~/.codex`,
///    `~/.ssh`, `~/.config`). A dotfile directory is never a legitimate browse root, and naming
///    one is the precise move an attacker makes to aim the reader at the secrets it holds.
///
/// Enumerating individual secret filenames was deliberately avoided: that list is unmaintainable
/// and the next agent CLI to land would quietly fall outside it. Legitimate roots are unaffected —
/// `/home/creepy/Documents/Workspace` (the real value on this host) passes both rules.
pub fn reject_unsafe_confinement_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("must be an absolute path".to_string());
    }
    // Compare canonically where possible so `/home/creepy/..` or a symlinked `$HOME` cannot dodge
    // the comparison below; a not-yet-created dir falls back to the literal path.
    let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if candidate.parent().is_none() {
        return Err("is a filesystem root".to_string());
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).and_then(|h| {
        let c = h.canonicalize().unwrap_or(h);
        if c.is_absolute() {
            Some(c)
        } else {
            None
        }
    });
    if let Some(home) = home {
        // `$HOME` itself, or an ancestor of it (`/home`, `/`): the whole home dir would be in-root.
        if home.starts_with(&candidate) {
            return Err(format!(
                "is {} or an ancestor of it",
                home.display()
            ));
        }
        // A hidden directory directly under `$HOME` — i.e. exactly where agent CLIs keep state.
        if candidate.parent() == Some(home.as_path())
            && candidate
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
        {
            return Err("is a hidden directory in the home directory".to_string());
        }
    }
    Ok(())
}

/// Every directory a report's `evidence` markdown may legitimately live in.
///
/// The confinement roots for `agent_plans::read_report_markdown`. That reader had NO directory
/// confinement at all, and the host's GraphQL on 127.0.0.1:7788 is unauthenticated by design:
/// any local process could name `~/.claude/**/MEMORY.md`, a repo's `CLAUDE.md` or private notes
/// as `evidence`, and the host would read it and persist it into `session_reports.markdown` —
/// which `listSessionReports` then serves back THROUGH the Cloudflare worker on only
/// `sessions:read`. That is a local-to-remote exfiltration path, and this list is what closes it.
///
/// Both roots are load-bearing; measured against this host's `session_reports` table, every
/// historical evidence path sits under one of them, and both are still in active use:
/// - `files_root` — the Workspace plan store (`.johnnyone/reports/`, `.johnnyone/replies/`) and
///   every plan workspace. Defensible as a root because a *same-uid local* caller, which is who
///   can reach the unauthenticated host port at all, could already read those bytes directly off
///   disk; admitting them here adds no new capability for that caller. Note this is NOT an
///   authorization-equivalence claim, and must not be read as one: the `files_root`-rooted reader
///   is `filesRead` → `host_files::read_file`, whose resolver requires
///   `authorizeForAltToken(ctx, 'files:read')`, whereas `reportAgentResult` requires nothing at
///   all. (`hostReadFile` is a third thing again — rooted at the plan's `workspace_path`.) So the
///   two surfaces are equivalent in *reachable bytes for a local caller*, not in authorization.
/// - `/tmp` — the agent scratchpads (`/tmp/claude-<uid>/<session>/scratchpad/…`) where the
///   majority of reports are written, and occasionally a plan workspace itself.
///
/// Why `/tmp` is pinned on unix rather than taken from `env::temp_dir()`: `temp_dir()` returns
/// `$TMPDIR` when it is set, and the host inherits `$TMPDIR` from whatever launched it, so
/// `TMPDIR=/home/creepy` would have silently admitted all of `$HOME` — `~/.claude` included — and
/// turned the guard off. The churn this root has to survive (`claude-<uid>/<session>/`) is all
/// *inside* the path, not in the root itself, so reading the root from the environment bought
/// nothing and cost the guarantee.
///
/// Every root is then passed through [`reject_unsafe_confinement_root`], so neither a poisoned
/// `files_root` nor a hostile `$TMPDIR` on a non-unix host can widen the boundary to `$HOME` or
/// its dotfiles — precisely the read the guard exists to stop. A root that fails is dropped rather
/// than clamped: the result is always narrower, never wider. Kept here, beside
/// `resolve_files_root`, so the roots have one home rather than being hardcoded at the call site.
pub fn report_markdown_roots(state: &AppState) -> Vec<PathBuf> {
    // `resolve_files_root` already validates (and falls back to the default on a poisoned value).
    let mut roots = vec![resolve_files_root(state)];
    let temp = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    match reject_unsafe_confinement_root(&temp) {
        Ok(()) => roots.push(temp),
        Err(reason) => tracing::warn!(
            temp = %temp.display(),
            %reason,
            "temp dir is unsafe as a confinement root; dropping it from the report markdown roots"
        ),
    }
    roots
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

    #[test]
    fn initiative_runs_path_is_phase_keyed() {
        let path = initiative_runs_path(
            Path::new("/store"),
            "init-1",
            "plan-1",
            "00-atomic-plan-store",
        );
        assert_eq!(
            path,
            PathBuf::from("/store/init-1/runs/plan-1/00-atomic-plan-store")
        );
        let rendered = path.to_string_lossy();
        assert!(
            !rendered.contains("snapshots"),
            "runs path must not include a snapshots segment: {}",
            rendered
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

    // ── files_root as a security boundary ───────────────────────────────────────────────────
    // `report_markdown_roots` confines the evidence-markdown reader to `files_root`, and
    // `updateSetting` is remotely callable with no scope check, so a hostile value here turns the
    // confinement guard into a no-op. These pin the values that must never be accepted.

    #[test]
    fn unsafe_confinement_roots_are_rejected() {
        // The exact attack: files_root = "/" makes every `starts_with` check pass.
        assert!(reject_unsafe_confinement_root(Path::new("/")).is_err(), "/");
        assert!(
            reject_unsafe_confinement_root(Path::new("relative/path")).is_err(),
            "not absolute"
        );
        let home = std::env::var("HOME").expect("HOME is set in the test env");
        assert!(
            reject_unsafe_confinement_root(Path::new(&home)).is_err(),
            "$HOME itself would put every dotfile in root"
        );
        assert!(
            reject_unsafe_confinement_root(Path::new(
                Path::new(&home).parent().unwrap().to_str().unwrap()
            ))
            .is_err(),
            "an ancestor of $HOME"
        );
        // Aiming the root straight at an agent CLI's state dir is the other obvious move.
        for hidden in [".claude", ".codex", ".ssh", ".config"] {
            let p = Path::new(&home).join(hidden);
            assert!(
                reject_unsafe_confinement_root(&p).is_err(),
                "hidden dir under $HOME: {}",
                p.display()
            );
        }
    }

    #[test]
    fn a_legitimate_files_root_is_still_accepted() {
        // The real value on this host must keep working — the guard is not allowed to be so tight
        // that it breaks the production browse root (or the report paths confined to it).
        assert!(reject_unsafe_confinement_root(Path::new(
            "/home/creepy/Documents/Workspace"
        ))
        .is_ok());
        // A deep, not-yet-existing dir under $HOME is fine: it is neither $HOME nor hidden there.
        let home = std::env::var("HOME").expect("HOME is set in the test env");
        assert!(reject_unsafe_confinement_root(
            &Path::new(&home).join("Documents/Workspace/nested/does-not-exist-yet")
        )
        .is_ok());
        // A non-hidden sibling of the home dir is a user choice, not an escalation.
        assert!(reject_unsafe_confinement_root(Path::new("/srv/projects")).is_ok());
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