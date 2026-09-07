//! Workspace-repo git helpers for kloo-mode task commits.
//!
//! Distinct from `git_history.rs` (plan-store `.git`). `commit_task` commits only
//! the allowed `files[]`; `commit_phase` is the phase-boundary backstop that commits
//! whatever a T1 worker left behind.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Strip a leading numeric prefix (`01-add` → `add`). Filesystem-safe.
pub fn task_slug(id: &str) -> String {
    let rest = id
        .split_once('-')
        .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, rest)| rest)
        .unwrap_or(id);
    rest.replace(['/', '\\'], "-")
}

fn git(workspace: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git {}: {e}", args.join(" ")))
}

fn ensure_identity(workspace: &Path) {
    let email = git(workspace, &["config", "--get", "user.email"]).ok();
    if email.as_ref().map(|o| !o.status.success()).unwrap_or(true) {
        let _ = git(workspace, &["config", "user.email", "johnnyone@local"]);
    }
    let name = git(workspace, &["config", "--get", "user.name"]).ok();
    if name.as_ref().map(|o| !o.status.success()).unwrap_or(true) {
        let _ = git(workspace, &["config", "user.name", "JohnnyOne"]);
    }
}

/// Stage only `files` and commit. `Ok(None)` if nothing was staged.
pub fn commit_task(
    workspace: &Path,
    phase_id: &str,
    id: &str,
    slug: &str,
    files: &[impl AsRef<Path>],
) -> Result<Option<String>, String> {
    let ws = workspace
        .to_str()
        .ok_or_else(|| "workspace path is not utf-8".to_string())?;
    ensure_identity(workspace);

    let mut add_args: Vec<String> = vec!["add".into(), "--".into()];
    for f in files {
        add_args.push(f.as_ref().to_string_lossy().into_owned());
    }
    let add_refs: Vec<&str> = add_args.iter().map(String::as_str).collect();
    let add = git(workspace, &add_refs)?;
    if !add.status.success() {
        return Err(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }

    let staged = Command::new("git")
        .args(["-C", ws, "diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| format!("spawn git diff --cached: {e}"))?;
    if staged.success() {
        return Ok(None);
    }

    let message = format!("task {phase_id}/{id}: {slug}");
    let commit = git(workspace, &["commit", "-m", &message])?;
    if !commit.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        ));
    }
    let sha = git(workspace, &["rev-parse", "HEAD"])?;
    if !sha.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&sha.stdout).trim().to_string(),
    ))
}

/// Commit everything left in the workspace at a phase boundary. `Ok(None)` if the tree
/// is already clean.
///
/// A T1 tmux worker (claude_code / grok) is never TOLD to commit — no task prompt asks for
/// it and `planner_prompts` injects no such instruction — so whether a phase's work lands in
/// git depends on the agent's habits. One initiative committed per phase; the next reported
/// `approved`/`complete` with all seven phases living only in the working tree, where any
/// checkout or clean would have destroyed them. This is the deterministic backstop: after a
/// phase passes, J1 commits the workspace itself rather than hoping.
///
/// Unlike `commit_task` this stages everything (`git add -A`), because there is no `files[]`
/// contract on the T1 path — the whole point is that we do not know what the agent touched.
pub fn commit_phase(
    workspace: &Path,
    phase_id: &str,
    summary: &str,
) -> Result<Option<String>, String> {
    if !workspace.join(".git").exists() {
        return Ok(None);
    }
    ensure_identity(workspace);

    let add = git(workspace, &["add", "-A"])?;
    if !add.status.success() {
        return Err(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }
    let staged = git(workspace, &["diff", "--cached", "--quiet"])?;
    if staged.status.success() {
        return Ok(None); // nothing left to commit — the worker already did it
    }

    let first_line = summary.lines().next().unwrap_or("").trim();
    let subject = if first_line.is_empty() {
        format!("phase {phase_id}: work committed by JohnnyOne")
    } else {
        let mut s: String = first_line.chars().take(64).collect();
        if first_line.chars().count() > 64 {
            s.push('…');
        }
        format!("phase {phase_id}: {s}")
    };
    let body = "Committed at the phase boundary by JohnnyOne. The worker left changes\n                uncommitted; this backstop keeps a passed phase from living only in the\n                working tree.";
    let commit = git(workspace, &["commit", "-m", &subject, "-m", body])?;
    if !commit.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        ));
    }
    let sha = git(workspace, &["rev-parse", "HEAD"])?;
    if !sha.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&sha.stdout).trim().to_string()))
}

/// Index `task <phase>/<id>:` subjects. Newest commit for a pair wins.
pub fn index_task_commits(
    workspace: &Path,
    phase_id: &str,
    since: Option<&str>,
) -> Result<HashMap<(String, String), String>, String> {
    let ws = workspace
        .to_str()
        .ok_or_else(|| "workspace path is not utf-8".to_string())?;
    let mut args = vec![
        "-C".to_string(),
        ws.to_string(),
        "log".to_string(),
        "--pretty=%H%x09%s".to_string(),
    ];
    if let Some(sha) = since.filter(|s| !s.trim().is_empty()) {
        args.push(format!("{sha}..HEAD"));
    }
    let out = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| format!("spawn git log: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((sha, subject)) = line.split_once('\t') else {
            continue;
        };
        let Some(rest) = subject.strip_prefix("task ") else {
            continue;
        };
        let Some((phase_and_id, _)) = rest.split_once(':') else {
            continue;
        };
        let Some((phase, task_id)) = phase_and_id.split_once('/') else {
            continue;
        };
        if phase != phase_id {
            continue;
        }
        let task_id = task_id.split_whitespace().next().unwrap_or(task_id);
        map.entry((phase.to_string(), task_id.to_string()))
            .or_insert_with(|| sha.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp_repo() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-wsgit-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        let init = Command::new("git").args(["init"]).current_dir(&d).output().unwrap();
        assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
        ensure_identity(&d);
        d
    }

    #[test]
    fn commit_phase_sweeps_everything_a_worker_left_behind() {
        let repo = tmp_repo();
        // a worker that wrote files and never committed -- the initiative-2 failure
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/b.rs"), "fn b() {}\n").unwrap();
        let sha = commit_phase(&repo, "06-loop-metering", "PASS — metering wired")
            .unwrap()
            .expect("a dirty tree must produce a commit");
        assert!(!sha.is_empty());
        let status = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "tree must be clean after the phase commit"
        );
        let log = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "log", "-1", "--pretty=%s"])
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(subject.contains("phase 06-loop-metering:"), "{subject}");
        assert!(subject.contains("PASS"), "{subject}");
    }

    #[test]
    fn commit_phase_is_a_no_op_when_the_worker_already_committed() {
        let repo = tmp_repo();
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
        commit_task(&repo, "00-x", "01-y", "y", &[Path::new("a.rs")])
            .unwrap()
            .expect("commit");
        assert!(
            commit_phase(&repo, "00-x", "PASS").unwrap().is_none(),
            "a clean tree must not produce an empty phase commit"
        );
    }

    #[test]
    fn commit_phase_subject_is_bounded_and_single_line() {
        let repo = tmp_repo();
        std::fs::write(repo.join("a.rs"), "x\n").unwrap();
        let long = "y".repeat(400);
        commit_phase(&repo, "01-p", &format!("{long}\nsecond line")).unwrap().unwrap();
        let log = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "log", "-1", "--pretty=%s"])
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(subject.len() < 120, "subject not bounded: {}", subject.len());
        assert!(!subject.contains("second line"), "{subject}");
    }

    #[test]
    fn commit_task_stages_only_allowed_files() {
        let repo = tmp_repo();
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(repo.join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(repo.join("stray.rs"), "nope\n").unwrap();
        let sha = commit_task(&repo, "00-calc", "01-add", "add", &[Path::new("a.rs"), Path::new("b.rs")])
            .unwrap()
            .expect("commit");
        let log = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "log", "-1", "--pretty=%s"])
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout);
        assert!(subject.contains("task 00-calc/01-add: add"), "{subject}");
        let head = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), sha);
        let untracked = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "ls-files", "--others", "--exclude-standard"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&untracked.stdout).contains("stray.rs"),
            "stray.rs must remain untracked (no git add -A)"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn index_task_commits_is_phase_keyed() {
        let repo = tmp_repo();
        std::fs::write(repo.join("a.rs"), "1\n").unwrap();
        let sha_calc = commit_task(&repo, "00-calc", "01-add", "add", &[Path::new("a.rs")])
            .unwrap()
            .unwrap();
        std::fs::write(repo.join("a.rs"), "2\n").unwrap();
        let _ = commit_task(&repo, "00-other", "01-add", "add", &[Path::new("a.rs")])
            .unwrap()
            .unwrap();
        let idx = index_task_commits(&repo, "00-calc", None).unwrap();
        assert_eq!(idx.get(&("00-calc".into(), "01-add".into())), Some(&sha_calc));
        assert!(!idx.contains_key(&("00-other".into(), "01-add".into())));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn commit_task_clean_tree_is_none() {
        let repo = tmp_repo();
        std::fs::write(repo.join("a.rs"), "x\n").unwrap();
        let _ = commit_task(&repo, "00-x", "01-a", "a", &[Path::new("a.rs")])
            .unwrap()
            .unwrap();
        let again = commit_task(&repo, "00-x", "01-a", "a", &[Path::new("a.rs")]).unwrap();
        assert!(again.is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn task_slug_strips_numeric_prefix() {
        assert_eq!(task_slug("01-add"), "add");
        assert_eq!(task_slug("add"), "add");
    }
}
