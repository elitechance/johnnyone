//! Per-plan git history.
//!
//! Each agent plan keeps its own `.git` repository inside its `plan_path`.
//! The repo is initialized lazily (on first `ensure_repo` call) and a commit
//! is made on every T2 PASS event — the user's idea is that "every approved
//! state of the plan is a commit, and amendments are just additional commits
//! on `main` after T1 edits and T2 re-approves."
//!
//! Why a real shelled-out `git` instead of `git2-rs` (libgit2):
//!   - The plan_path may already live inside another git repo (user's
//!     monorepo). Nested-`.git` behavior is well-defined for the git CLI;
//!     libgit2 has occasional surprises with nested repos.
//!   - The CLI is universally installed where we expect users to run plans.
//!   - Commits are infrequent enough (T2-PASS only) that subprocess overhead
//!     doesn't matter.
//!
//! Graceful degradation: if `git` isn't on PATH or any command fails, every
//! function returns a `Result` that callers log + continue. Plans still work,
//! just without per-plan history.

use std::process::Command;
use std::path::Path;

/// Ensure a git repo exists at `plan_path`. Idempotent — running this on an
/// already-initialized plan is a no-op. Creates the directory tree if it
/// doesn't exist yet (the user-provided plan_path may not exist before T1's
/// first run).
///
/// Sets a local `user.name` + `user.email` so subsequent commits don't fail
/// in environments where neither is globally configured.
pub fn ensure_repo(plan_path: &str) -> Result<(), String> {
    let plan = Path::new(plan_path);
    std::fs::create_dir_all(plan).map_err(|e| format!("mkdir -p {}: {}", plan_path, e))?;

    let dot_git = plan.join(".git");
    if dot_git.exists() {
        return Ok(());
    }

    let out = Command::new("git")
        .args(["init", plan_path])
        .output()
        .map_err(|e| format!("spawn `git init`: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // Local config — committed values + email/name fallback. Use the JWT user
    // identity when we have it (host gets it via env var that we set on
    // process spawn), else fall back to a JohnnyOne identity. For now we keep
    // it generic; switching to per-user requires plumbing the JWT subject
    // into this layer.
    let _ = Command::new("git")
        .args(["-C", plan_path, "config", "user.email", "johnnyone@local"])
        .output();
    let _ = Command::new("git")
        .args(["-C", plan_path, "config", "user.name", "JohnnyOne"])
        .output();
    // Avoid signing failures on machines with commit-signing globally enabled.
    let _ = Command::new("git")
        .args(["-C", plan_path, "config", "commit.gpgsign", "false"])
        .output();
    Ok(())
}

/// Stage all changes under `plan_path` and commit with `message`. Returns:
///   - `Ok(Some(sha))` — commit was made
///   - `Ok(None)` — nothing changed (clean working tree)
///   - `Err(...)` — git failed (e.g. not installed)
///
/// Callers should treat `Ok(None)` as success (idempotent re-commit attempts
/// after a re-run should not error).
pub fn commit_all(plan_path: &str, message: &str) -> Result<Option<String>, String> {
    // Stage everything (additions, modifications, deletions).
    let add = Command::new("git")
        .args(["-C", plan_path, "add", "-A"])
        .output()
        .map_err(|e| format!("spawn `git add`: {}", e))?;
    if !add.status.success() {
        return Err(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }

    // Check if anything is actually staged. `diff --cached --quiet` exits 0
    // (success) when there's nothing staged and 1 when there is.
    let staged = Command::new("git")
        .args(["-C", plan_path, "diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| format!("spawn `git diff --cached`: {}", e))?;
    if staged.success() {
        return Ok(None);
    }

    // Make the commit. `--allow-empty-message` not used; we always provide one.
    let commit = Command::new("git")
        .args(["-C", plan_path, "commit", "-m", message])
        .output()
        .map_err(|e| format!("spawn `git commit`: {}", e))?;
    if !commit.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        ));
    }

    // Capture the new HEAD sha.
    let sha = Command::new("git")
        .args(["-C", plan_path, "rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("spawn `git rev-parse`: {}", e))?;
    if !sha.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&sha.stdout).trim().to_string()))
}

/// Return the unified diff between the working tree and HEAD. Used by T2 in
/// amend mode so the reviewer focuses on what T1 just changed instead of
/// re-validating the entire plan from scratch.
///
/// Returns `Ok("")` if the working tree is clean or HEAD doesn't exist yet
/// (a brand-new repo before its first commit).
pub fn diff_head(plan_path: &str) -> Result<String, String> {
    // If HEAD doesn't exist (no commits yet), `git diff HEAD` errors.
    // Detect that and return empty.
    let has_head = Command::new("git")
        .args(["-C", plan_path, "rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|e| format!("spawn `git rev-parse HEAD`: {}", e))?;
    if !has_head.status.success() {
        return Ok(String::new());
    }

    let out = Command::new("git")
        .args(["-C", plan_path, "diff", "HEAD"])
        .output()
        .map_err(|e| format!("spawn `git diff HEAD`: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Returns true if the repo at `plan_path` has at least one commit on HEAD.
/// Used to distinguish "initial commit" vs "subsequent commit" when crafting
/// commit messages.
pub fn has_any_commit(plan_path: &str) -> bool {
    Command::new("git")
        .args(["-C", plan_path, "rev-parse", "--verify", "HEAD"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Format a commit message for an amendment using its brief. Truncated to a
/// reasonable subject-line width so `git log --oneline` stays readable.
pub fn amend_commit_message(brief: &str) -> String {
    let first_line = brief.lines().next().unwrap_or("").trim();
    let trimmed = if first_line.chars().count() > 60 {
        let truncated: String = first_line.chars().take(57).collect();
        format!("{}...", truncated)
    } else {
        first_line.to_string()
    };
    if trimmed.is_empty() {
        "amend: (no brief)".to_string()
    } else {
        format!("amend: {}", trimmed)
    }
}
