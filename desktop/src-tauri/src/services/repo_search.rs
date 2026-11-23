//! `repo_search` — the plan agent's ONLY read path into code (Plans tab, initiative 8ad3ae71 phase 02
//! task 05; brief §"MCP security" 6; master D6).
//!
//! A bounded, in-process Rust walker (no `rg`, no subprocess: `rg` is not on the host, and spawning
//! a binary from an MCP call is an injection surface). Read-only and confined:
//!
//! - **Jail.** A `path` argument that is absolute or has a `..` component is refused BEFORE the
//!   filesystem is touched. `root = canonicalize(workspace)`; `start = canonicalize(root/path)` must
//!   stay under `root`. The walk never follows a symlinked directory; a symlinked file is
//!   canonicalized and searched only when its target is under `root` (and not excluded).
//! - **Hard exclusions** (checked on the CANONICAL path of every directory and file before it is
//!   opened; no argument can override them): anything under the canonicalized initiatives dir (other
//!   plans' stores and their `mcp.json` tokens), any path with a `.johnnyone` or `plan-agent`
//!   component, any file named `mcp.json`, any file whose name starts with `.env`. A `path` pointing
//!   into an excluded area returns an empty result with `excluded: true` and no echo of contents.
//! - **Skips** the shared workspace ignore list (`.git`, `node_modules`, `target`, …), files over
//!   2 MiB and non-UTF-8 files.
//! - **Caps.** ≤ `max_results` matches (default 50, max 200), each `text` ≤ 240 chars, total output
//!   ≤ 64 KiB (`truncated: true`), and a wall-clock deadline (3 s in production; `timedOut: true`).
//!
//! The match is a literal, case-insensitive substring match.

use crate::services::agent_plans::WORKSPACE_IGNORED_DIRS;
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Default number of matches when the caller gives none.
pub const DEFAULT_MAX_RESULTS: usize = 50;
/// Hard ceiling on `max_results`.
pub const MAX_RESULTS_CAP: usize = 200;
/// Max chars of one match's `text`.
pub const MAX_LINE_CHARS: usize = 240;
/// Max serialized size of the whole result.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Files larger than this are skipped.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Production wall-clock budget.
pub const TIME_BUDGET: Duration = Duration::from_secs(3);
/// Max chars of the query (mirrors the tool schema).
pub const MAX_QUERY_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    pub timed_out: bool,
    pub excluded: bool,
}

/// Search with the production 3 s budget.
pub fn repo_search(
    workspace: &Path,
    initiatives_dir: &Path,
    query: &str,
    path: Option<&str>,
    max_results: Option<usize>,
) -> Result<SearchResult, String> {
    repo_search_until(
        workspace,
        initiatives_dir,
        query,
        path,
        max_results,
        Instant::now() + TIME_BUDGET,
    )
}

/// Refuse an absolute path or any `..` component — pure, before any filesystem access.
pub fn check_relative(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if path.starts_with('/') || path.starts_with('\\') || p.is_absolute() || p.has_root() {
        return Err("path must be relative to the workspace".to_string());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_))) {
        return Err("path must not contain `..`".to_string());
    }
    Ok(())
}

/// The hard exclusion set, on a CANONICAL path. `root` is the canonical workspace, `init_dir` the
/// canonical initiatives dir (if it exists).
fn is_excluded(canon: &Path, root: &Path, init_dir: Option<&Path>, is_file: bool) -> bool {
    if let Some(d) = init_dir {
        if canon.starts_with(d) {
            return true;
        }
    }
    let rel = canon.strip_prefix(root).unwrap_or(canon);
    let bad_component = rel.components().any(|c| match c {
        Component::Normal(n) => {
            let n = n.to_string_lossy();
            n == ".johnnyone" || n == "plan-agent"
        }
        _ => false,
    });
    if bad_component {
        return true;
    }
    if is_file {
        if let Some(name) = canon.file_name().map(|n| n.to_string_lossy().to_string()) {
            if name == "mcp.json" || name.starts_with(".env") {
                return true;
            }
        }
    }
    false
}

/// Cut a line to at most [`MAX_LINE_CHARS`] chars (on a char boundary).
fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => line[..i].to_string(),
        None => line.to_string(),
    }
}

/// [`repo_search`] with an explicit deadline (the test seam: a deadline in the past → `timedOut`).
pub fn repo_search_until(
    workspace: &Path,
    initiatives_dir: &Path,
    query: &str,
    path: Option<&str>,
    max_results: Option<usize>,
    deadline: Instant,
) -> Result<SearchResult, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("query is longer than {} characters", MAX_QUERY_CHARS));
    }
    let rel = path.map(str::trim).filter(|p| !p.is_empty() && *p != ".");
    if let Some(p) = rel {
        check_relative(p)?;
    }
    let max = max_results.unwrap_or(DEFAULT_MAX_RESULTS).clamp(1, MAX_RESULTS_CAP);
    let needle = query.to_lowercase();

    let root = workspace
        .canonicalize()
        .map_err(|_| "the plan's workspace is not readable".to_string())?;
    let init_dir = initiatives_dir.canonicalize().ok();
    let start = match rel {
        None => root.clone(),
        Some(p) => root
            .join(p)
            .canonicalize()
            .map_err(|_| "path not found in the workspace".to_string())?,
    };
    if !start.starts_with(&root) {
        return Err("path escapes the workspace".to_string());
    }

    let mut out = SearchResult::default();
    let start_is_file = start.is_file();
    if is_excluded(&start, &root, init_dir.as_deref(), start_is_file) {
        out.excluded = true;
        return Ok(out);
    }

    let mut budget = Budget { max, used_bytes: 64, out: &mut out };
    if start_is_file {
        if Instant::now() >= deadline {
            budget.out.timed_out = true;
        } else {
            search_file(&start, &root, &needle, &mut budget);
        }
        return Ok(out);
    }

    let mut stack: Vec<PathBuf> = vec![start];
    'walk: while let Some(dir) = stack.pop() {
        if Instant::now() >= deadline {
            budget.out.timed_out = true;
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            if Instant::now() >= deadline {
                budget.out.timed_out = true;
                break 'walk;
            }
            let Ok(ft) = entry.file_type() else { continue };
            let p = entry.path();
            if ft.is_symlink() {
                // Never follow a symlinked dir; a symlinked file only when it resolves inside.
                let Ok(target) = p.canonicalize() else { continue };
                if target.is_dir()
                    || !target.starts_with(&root)
                    || is_excluded(&target, &root, init_dir.as_deref(), true)
                    || is_excluded(&p, &root, init_dir.as_deref(), true)
                {
                    continue;
                }
                search_file(&target, &root, &needle, &mut budget);
            } else if ft.is_dir() {
                let name = entry.file_name();
                if WORKSPACE_IGNORED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    continue;
                }
                // `p` is canonical: root is canonical and no symlink was followed to reach it.
                if is_excluded(&p, &root, init_dir.as_deref(), false) {
                    continue;
                }
                subdirs.push(p);
            } else if ft.is_file() {
                if is_excluded(&p, &root, init_dir.as_deref(), true) {
                    continue;
                }
                search_file(&p, &root, &needle, &mut budget);
            }
            if budget.out.truncated {
                break 'walk;
            }
        }
        // Depth-first in name order.
        subdirs.reverse();
        stack.extend(subdirs);
    }
    Ok(out)
}

struct Budget<'a> {
    max: usize,
    used_bytes: usize,
    out: &'a mut SearchResult,
}

/// Scan one (already vetted) file; stops and sets `truncated` once a cap is hit.
fn search_file(file: &Path, root: &Path, needle: &str, budget: &mut Budget<'_>) {
    if budget.out.truncated {
        return;
    }
    let Ok(meta) = std::fs::metadata(file) else { return };
    if meta.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(bytes) = std::fs::read(file) else { return };
    let Ok(text) = String::from_utf8(bytes) else { return };
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    for (i, line) in text.lines().enumerate() {
        if !line.to_lowercase().contains(needle) {
            continue;
        }
        if budget.out.matches.len() >= budget.max {
            budget.out.truncated = true;
            return;
        }
        let m = SearchMatch { path: rel.clone(), line: i + 1, text: clip(line.trim_end()) };
        let size = serde_json::to_string(&m).map(|s| s.len() + 1).unwrap_or(MAX_OUTPUT_BYTES);
        if budget.used_bytes + size > MAX_OUTPUT_BYTES {
            budget.out.truncated = true;
            return;
        }
        budget.used_bytes += size;
        budget.out.matches.push(m);
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tmp_dir;
    use std::os::unix::fs::symlink;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn search(ws: &Path, q: &str, path: Option<&str>) -> Result<SearchResult, String> {
        let nowhere = ws.join("no-initiatives-dir");
        repo_search(ws, &nowhere, q, path, None)
    }

    fn paths(r: &SearchResult) -> Vec<String> {
        r.matches.iter().map(|m| m.path.clone()).collect()
    }

    #[test]
    fn repo_search_finds_literal_case_insensitive() {
        let ws = tmp_dir("rs-ws");
        write(&ws, "src/a.rs", "fn main() {}\nlet BriefStore = 1;\n");
        write(&ws, "docs/b.md", "the briefstore doc\nnothing\n");
        write(&ws, "c.txt", "brief.*store is literal\n");
        let r = search(&ws, "briefstore", None).unwrap();
        assert_eq!(paths(&r), vec!["docs/b.md", "src/a.rs"]);
        assert_eq!(r.matches[1].line, 2);
        assert_eq!(r.matches[1].text, "let BriefStore = 1;");
        assert!(!r.truncated && !r.timed_out && !r.excluded);
        // Literal, not a regex.
        assert_eq!(paths(&search(&ws, "brief.*store", None).unwrap()), vec!["c.txt"]);
        // Scoped to a sub-path.
        assert_eq!(paths(&search(&ws, "BRIEFSTORE", Some("src")).unwrap()), vec!["src/a.rs"]);
        assert!(search(&ws, "   ", None).is_err());
    }

    #[test]
    fn repo_search_dotdot_refused() {
        let ws = tmp_dir("rs-ws");
        write(&ws, "a/x.txt", "secret\n");
        for p in ["../", "..", "a/../../x", "a/..", "./../etc"] {
            let e = search(&ws, "secret", Some(p)).unwrap_err();
            assert!(e.contains(".."), "{p}: {e}");
        }
    }

    #[test]
    fn repo_search_absolute_refused() {
        let ws = tmp_dir("rs-ws");
        write(&ws, "x.txt", "root\n");
        for p in ["/etc", "/", "\\etc"] {
            assert!(search(&ws, "root", Some(p)).unwrap_err().contains("relative"), "{p}");
        }
        let abs = ws.join("x.txt").to_string_lossy().to_string();
        assert!(search(&ws, "root", Some(&abs)).is_err());
    }

    #[test]
    fn repo_search_symlink_escape_refused() {
        let ws = tmp_dir("rs-ws");
        let outside = tmp_dir("rs-outside");
        write(&outside, "leak.txt", "needle outside\n");
        write(&ws, "inside.txt", "needle inside\n");
        symlink(&outside, ws.join("dirlink")).unwrap();
        symlink(outside.join("leak.txt"), ws.join("filelink.txt")).unwrap();
        // An in-workspace file link is still searched.
        symlink(ws.join("inside.txt"), ws.join("ok-link.txt")).unwrap();
        let r = search(&ws, "needle", None).unwrap();
        assert_eq!(paths(&r), vec!["inside.txt", "inside.txt"]);
        assert!(r.matches.iter().all(|m| !m.text.contains("outside")));
        // Pointing `path` at the links is refused too.
        assert!(search(&ws, "needle", Some("dirlink")).unwrap_err().contains("escapes"));
        assert!(search(&ws, "needle", Some("filelink.txt")).unwrap_err().contains("escapes"));
        assert!(search(&ws, "needle", Some("dirlink/leak.txt")).unwrap_err().contains("escapes"));
    }

    #[test]
    fn repo_search_output_capped() {
        let ws = tmp_dir("rs-ws");
        let body: String = (0..500).map(|i| format!("hit {i}\n")).collect();
        write(&ws, "many.txt", &body);
        let r = repo_search(&ws, &ws.join("none"), "hit", None, Some(30)).unwrap();
        assert_eq!(r.matches.len(), 30);
        assert!(r.truncated);
        let r = repo_search(&ws, &ws.join("none"), "hit", None, Some(10_000)).unwrap();
        assert_eq!(r.matches.len(), MAX_RESULTS_CAP, "max is clamped to 200");
        assert!(r.truncated);
        let r = search(&ws, "hit", None).unwrap();
        assert_eq!(r.matches.len(), DEFAULT_MAX_RESULTS);
        // Long lines are clipped, and the byte cap holds.
        let long: String = (0..400).map(|_| format!("{}\n", "hit ".repeat(200))).collect();
        write(&ws, "long.txt", &long);
        let r = repo_search(&ws, &ws.join("none"), "hit", Some("long.txt"), Some(200)).unwrap();
        assert!(r.matches.iter().all(|m| m.text.chars().count() <= MAX_LINE_CHARS));
        assert!(serde_json::to_string(&r).unwrap().len() <= MAX_OUTPUT_BYTES);
        assert!(r.truncated);
    }

    #[test]
    fn repo_search_time_capped() {
        let ws = tmp_dir("rs-ws");
        write(&ws, "a.txt", "needle\n");
        let past = Instant::now() - Duration::from_millis(1);
        let r = repo_search_until(&ws, &ws.join("none"), "needle", None, None, past).unwrap();
        assert!(r.timed_out);
        assert!(r.matches.is_empty());
        let r = repo_search_until(&ws, &ws.join("none"), "needle", Some("a.txt"), None, past).unwrap();
        assert!(r.timed_out);
    }

    #[test]
    fn repo_search_skips_ignored_dirs() {
        let ws = tmp_dir("rs-ws");
        for d in WORKSPACE_IGNORED_DIRS {
            write(&ws, &format!("{d}/f.txt"), "needle\n");
        }
        write(&ws, "src/keep.txt", "needle\n");
        // Oversized and non-UTF-8 files are skipped.
        write(&ws, "big.txt", &"needle\n".repeat((MAX_FILE_BYTES / 7 + 10) as usize));
        std::fs::write(ws.join("bin.dat"), [0xff, 0xfe, b'n', b'e', b'e', b'd', b'l', b'e']).unwrap();
        assert_eq!(paths(&search(&ws, "needle", None).unwrap()), vec!["src/keep.txt"]);
    }

    #[test]
    fn repo_search_never_returns_mcp_tokens() {
        let ws = tmp_dir("rs-ws");
        let token = format!("deadbeef{}", "0".repeat(56));
        write(&ws, &format!(".johnnyone/initiatives/x/plan-agent/mcp.json"), &format!("Bearer {token}\n"));
        write(&ws, "other/plan-agent/mcp.json", &format!("Bearer {token}\n"));
        write(&ws, "other/plan-agent/notes.txt", &format!("{token}\n"));
        write(&ws, "stray/mcp.json", &format!("{token}\n"));
        write(&ws, ".env.local", &format!("TOKEN={token}\n"));
        write(&ws, "app/.env", &format!("TOKEN={token}\n"));
        write(&ws, "src/ok.rs", "no secrets here\n");
        symlink(ws.join(".johnnyone/initiatives"), ws.join("init-link")).unwrap();
        symlink(ws.join("stray/mcp.json"), ws.join("token-link.txt")).unwrap();
        for q in [token.as_str(), "deadbeef", "Bearer", "TOKEN"] {
            assert!(search(&ws, q, None).unwrap().matches.is_empty(), "query {q}");
        }
        for p in [
            ".johnnyone",
            ".johnnyone/initiatives/x/plan-agent/mcp.json",
            "other/plan-agent",
            "other/plan-agent/mcp.json",
            "stray/mcp.json",
            ".env.local",
            "app/.env",
            "init-link",
            "init-link/x/plan-agent/mcp.json",
            "token-link.txt",
        ] {
            let r = search(&ws, "deadbeef", Some(p)).unwrap();
            assert!(r.matches.is_empty(), "path {p}");
            assert!(r.excluded, "path {p} must report excluded");
        }
    }

    #[test]
    fn repo_search_excludes_real_initiatives_dir_inside_workspace() {
        let ws = tmp_dir("rs-ws");
        // The plan store lives INSIDE the workspace under an innocuous name.
        let store = ws.join("plans-store");
        write(&store, "abc/plan/overview.md", "needle in another plan\n");
        write(&ws, "src/x.rs", "needle in code\n");
        let r = repo_search(&ws, &store, "needle", None, None).unwrap();
        assert_eq!(paths(&r), vec!["src/x.rs"]);
        let r = repo_search(&ws, &store, "needle", Some("plans-store/abc"), None).unwrap();
        assert!(r.excluded && r.matches.is_empty());
    }
}
