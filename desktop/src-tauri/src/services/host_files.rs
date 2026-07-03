//! Host-side file manager engine (overhaul P2, design §5).
//!
//! Full CRUD over a single, user-configurable browse root (`files_root`, task 01-01), reachable
//! from the web client over relay-RPC (task 01-03). Every path argument is funnelled through
//! [`resolve_within_root`] before any filesystem call, reads are capped at 5 MiB, and uploads are
//! bounded per-chunk and in total. Deliberately separate from the plan-scoped readers in
//! `agent_plans.rs` (decision D2): this surface is rooted at `files_root`, not a plan's workspace.
//!
//! Security invariants (non-negotiable):
//! - No path reaches `std::fs` without passing [`resolve_within_root`].
//! - Nothing here logs file `content`, decoded upload bytes, or base64 payloads.

use crate::services::agent_plans::{content_type_for_path, is_text_content_type};
use crate::services::settings::{normalize_path, resolve_files_root, resolve_within_root};
use crate::state::app_state::AppState;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// 5 MiB read cap (design §5) — mirrors `agent_plans::read_host_file`.
const MAX_READ_BYTES: u64 = 5 * 1024 * 1024;
/// Max decoded bytes accepted in a single upload chunk.
const MAX_UPLOAD_CHUNK_BYTES: usize = 1024 * 1024; // 1 MiB
/// Bounded total size of a chunked upload.
const MAX_UPLOAD_TOTAL_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
/// Guard against unbounded directory listings.
const MAX_DIR_ENTRIES: usize = 5000;

// ── Wire types (camelCase — must match Phase 02 schema + Phase 03 ui interfaces) ─────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Path relative to `files_root`, forward-slash separated.
    pub path: String,
    /// `"file"` | `"dir"`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Unix millis, optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    /// The listed dir, relative to `files_root`.
    pub path: String,
    /// Absolute `files_root` (for breadcrumb display).
    pub root: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub path: String,
    pub name: String,
    pub content_type: String,
    /// `"utf8"` | `"base64"`.
    pub encoding: String,
    pub content: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpResult {
    pub path: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadChunkResult {
    pub path: String,
    pub received: u64,
    pub complete: bool,
}

// ── Internal helpers ─────────────────────────────────────────────────────────────────────────────

/// Resolve `files_root` plus its canonical form once. The canonical root is what guarded paths are
/// made relative to for the wire (`FileEntry.path`, `DirListing.path`, ...).
fn root_context(state: &AppState) -> Result<(PathBuf, PathBuf), String> {
    let root = resolve_files_root(state);
    let canonical = normalize_path(&root)?;
    Ok((root, canonical))
}

/// Forward-slash path of `abs` relative to the canonical root (empty string for the root itself).
fn to_rel(canonical_root: &Path, abs: &Path) -> String {
    abs.strip_prefix(canonical_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().to_string())
}

/// Sibling path with `suffix` appended to the file name (for atomic temp / `.part` sidecars).
/// Stays in the same (already-guarded) directory as `target`.
fn sidecar(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    target.with_file_name(name)
}

/// Write `bytes` to `target` atomically (temp sibling + rename).
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = sidecar(target, ".j1tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("Failed to write file: {}", e))?;
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Failed to finalize file: {}", e)
    })
}

// ── Operations (all rooted at files_root, guarded per-path) ───────────────────────────────────────

/// List a directory relative to `files_root`. Dirs first, then case-insensitive by name.
pub fn list_dir(state: &AppState, path: &str) -> Result<DirListing, String> {
    let (root, canonical_root) = root_context(state)?;
    let dir = resolve_within_root(&root, path)?;
    let meta = fs::metadata(&dir).map_err(|e| format!("Failed to inspect path: {}", e))?;
    if !meta.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let mut entries: Vec<FileEntry> = Vec::new();
    for item in fs::read_dir(&dir).map_err(|e| format!("Failed to read directory: {}", e))? {
        let item = item.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let file_type = item
            .file_type()
            .map_err(|e| format!("Failed to inspect entry: {}", e))?;
        let entry_meta = item.metadata().ok();
        let (kind, size) = if file_type.is_dir() {
            ("dir", None)
        } else {
            ("file", entry_meta.as_ref().map(|m| m.len()))
        };
        let modified = entry_meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        entries.push(FileEntry {
            name: item.file_name().to_string_lossy().to_string(),
            path: to_rel(&canonical_root, &item.path()),
            kind: kind.to_string(),
            size,
            modified,
        });
        if entries.len() > MAX_DIR_ENTRIES {
            return Err(format!(
                "Directory has more than {} entries; refine the path",
                MAX_DIR_ENTRIES
            ));
        }
    }
    entries.sort_by(|a, b| {
        let a_dir = a.kind == "dir";
        let b_dir = b.kind == "dir";
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(DirListing {
        path: to_rel(&canonical_root, &dir),
        root: canonical_root.to_string_lossy().to_string(),
        entries,
    })
}

/// Read a file (≤ 5 MiB). utf8 text is returned inline; binary as base64.
pub fn read_file(state: &AppState, path: &str) -> Result<FileContent, String> {
    let (root, canonical_root) = root_context(state)?;
    let file = resolve_within_root(&root, path)?;
    let meta = fs::metadata(&file).map_err(|e| format!("Failed to inspect file: {}", e))?;
    if !meta.is_file() {
        return Err("Path is not a file".to_string());
    }
    if meta.len() > MAX_READ_BYTES {
        return Err("File exceeds 5 MiB read cap".to_string());
    }

    let bytes = fs::read(&file).map_err(|e| format!("Failed to read file: {}", e))?;
    let content_type = content_type_for_path(&file);
    let (encoding, content) = if is_text_content_type(&content_type) {
        match String::from_utf8(bytes.clone()) {
            Ok(text) => ("utf8".to_string(), text),
            Err(_) => ("base64".to_string(), general_purpose::STANDARD.encode(&bytes)),
        }
    } else {
        ("base64".to_string(), general_purpose::STANDARD.encode(&bytes))
    };

    Ok(FileContent {
        path: to_rel(&canonical_root, &file),
        name: file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        content_type,
        encoding,
        content,
        size: meta.len(),
    })
}

/// Write a file, creating parent dirs under the root. `encoding` is `"utf8"` (default) or
/// `"base64"`; the decoded payload is capped and the final file lands atomically.
pub fn write_file(
    state: &AppState,
    path: &str,
    content: &str,
    encoding: &str,
) -> Result<FileOpResult, String> {
    let (root, canonical_root) = root_context(state)?;
    let target = resolve_within_root(&root, path)?;
    let bytes = if encoding == "base64" {
        general_purpose::STANDARD
            .decode(content)
            .map_err(|_| "Invalid base64 content".to_string())?
    } else {
        content.as_bytes().to_vec()
    };
    if bytes.len() as u64 > MAX_READ_BYTES {
        return Err("Content exceeds 5 MiB write cap".to_string());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;
    }
    write_atomic(&target, &bytes)?;
    Ok(FileOpResult {
        path: to_rel(&canonical_root, &target),
        ok: true,
    })
}

/// Create a directory (and any missing parents) under the root.
pub fn mkdir(state: &AppState, path: &str) -> Result<FileOpResult, String> {
    let (root, canonical_root) = root_context(state)?;
    let dir = resolve_within_root(&root, path)?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create directory: {}", e))?;
    Ok(FileOpResult {
        path: to_rel(&canonical_root, &dir),
        ok: true,
    })
}

/// Rename/move within the root. Both endpoints are guarded; an existing target is never overwritten.
pub fn rename(state: &AppState, from: &str, to: &str) -> Result<FileOpResult, String> {
    let (root, canonical_root) = root_context(state)?;
    let src = resolve_within_root(&root, from)?;
    let dst = resolve_within_root(&root, to)?;
    if dst.exists() {
        return Err("Rename target already exists".to_string());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;
    }
    fs::rename(&src, &dst).map_err(|e| format!("Failed to rename: {}", e))?;
    Ok(FileOpResult {
        path: to_rel(&canonical_root, &dst),
        ok: true,
    })
}

/// Delete a file or directory (recursively). Refuses to delete `files_root` itself.
pub fn delete(state: &AppState, path: &str) -> Result<FileOpResult, String> {
    let (root, canonical_root) = root_context(state)?;
    let target = resolve_within_root(&root, path)?;
    if target == canonical_root {
        return Err("Refusing to delete the files_root itself".to_string());
    }
    let meta = fs::metadata(&target).map_err(|e| format!("Failed to inspect path: {}", e))?;
    if meta.is_dir() {
        fs::remove_dir_all(&target).map_err(|e| format!("Failed to delete directory: {}", e))?;
    } else {
        fs::remove_file(&target).map_err(|e| format!("Failed to delete file: {}", e))?;
    }
    Ok(FileOpResult {
        path: to_rel(&canonical_root, &target),
        ok: true,
    })
}

/// Append one chunk of a bounded, chunked upload to a deterministic `.part` sidecar. Chunk 0 starts
/// a fresh sidecar; later chunks append. On `done` (or the last index) the sidecar is atomically
/// renamed into place.
pub fn upload_chunk(
    state: &AppState,
    path: &str,
    chunk_index: u64,
    total_chunks: u64,
    data_base64: &str,
    done: bool,
) -> Result<UploadChunkResult, String> {
    let (root, canonical_root) = root_context(state)?;
    let target = resolve_within_root(&root, path)?;
    let bytes = general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|_| "Invalid base64 chunk".to_string())?;
    if bytes.len() > MAX_UPLOAD_CHUNK_BYTES {
        return Err("Upload chunk exceeds 1 MiB cap".to_string());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory: {}", e))?;
    }
    let part = sidecar(&target, ".part");

    let existing = if chunk_index == 0 {
        0
    } else {
        fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
    };
    if existing + bytes.len() as u64 > MAX_UPLOAD_TOTAL_BYTES {
        return Err("Upload exceeds 50 MiB total cap".to_string());
    }

    let mut file = if chunk_index == 0 {
        fs::File::create(&part).map_err(|e| format!("Failed to open upload: {}", e))?
    } else {
        fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .map_err(|e| format!("Failed to open upload: {}", e))?
    };
    file.write_all(&bytes)
        .map_err(|e| format!("Failed to write upload chunk: {}", e))?;
    let received = existing + bytes.len() as u64;

    let complete = done || (total_chunks > 0 && chunk_index + 1 == total_chunks);
    if complete {
        fs::rename(&part, &target).map_err(|e| {
            let _ = fs::remove_file(&part);
            format!("Failed to finalize upload: {}", e)
        })?;
    }

    Ok(UploadChunkResult {
        path: to_rel(&canonical_root, &target),
        received,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::settings::{
        resolve_files_root, resolve_within_root, set_setting, KEY_FILES_ROOT, DEFAULT_FILES_ROOT,
    };
    use crate::test_support::{test_state, tmp_dir};

    /// Point `files_root` at a fresh temp dir and return `(state, canonical_root)`. All FS mutation
    /// in these tests is confined to that temp dir — never the real filesystem.
    fn state_with_root() -> (crate::state::app_state::AppState, std::path::PathBuf) {
        let (state, _root) = test_state();
        let files_root = tmp_dir("files");
        set_setting(
            &state,
            KEY_FILES_ROOT.to_string(),
            files_root.to_string_lossy().to_string(),
        )
        .unwrap();
        let canonical = files_root.canonicalize().unwrap();
        (state, canonical)
    }

    // ── Setting default/override (task 01-01, harness-backed) ─────────────────────────────────
    #[test]
    fn resolve_files_root_defaults_then_overrides() {
        let (state, _root) = test_state();
        assert_eq!(
            resolve_files_root(&state),
            std::path::PathBuf::from(DEFAULT_FILES_ROOT)
        );
        set_setting(&state, KEY_FILES_ROOT.to_string(), "/tmp/j1-files".to_string()).unwrap();
        assert_eq!(
            resolve_files_root(&state),
            std::path::PathBuf::from("/tmp/j1-files")
        );
    }

    // ── Guard (pure) ──────────────────────────────────────────────────────────────────────────
    #[test]
    fn guard_rejects_dotdot() {
        let root = tmp_dir("guard");
        assert!(resolve_within_root(&root, "../x").is_err());
        assert!(resolve_within_root(&root, "a/../../b").is_err());
    }

    #[test]
    fn guard_rejects_above_root() {
        let root = tmp_dir("guard");
        assert!(resolve_within_root(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn guard_rejects_symlink_escape() {
        // best-effort: skip on platforms without unix symlink support.
        #[cfg(unix)]
        {
            let root = tmp_dir("guard");
            let outside = tmp_dir("outside");
            let secret = outside.join("secret.txt");
            std::fs::write(&secret, b"top secret").unwrap();
            let link = root.join("escape");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            // Resolving through the symlink canonicalizes out of root → rejected.
            assert!(resolve_within_root(&root, "escape/secret.txt").is_err());
        }
    }

    #[test]
    fn guard_allows_in_root() {
        let root = tmp_dir("guard");
        let canonical = root.canonicalize().unwrap();
        let resolved = resolve_within_root(&root, "sub/f.txt").unwrap();
        assert!(resolved.starts_with(&canonical));
    }

    // ── Size caps ─────────────────────────────────────────────────────────────────────────────
    #[test]
    fn read_rejects_over_5mib() {
        let (state, root) = state_with_root();
        let big = root.join("big.bin");
        std::fs::write(&big, vec![0u8; (5 * 1024 * 1024) as usize + 1]).unwrap();
        let err = read_file(&state, "big.bin").unwrap_err();
        assert!(err.contains("5 MiB"));
    }

    #[test]
    fn read_ok_under_cap_utf8_and_binary() {
        let (state, _root) = state_with_root();
        write_file(&state, "hello.txt", "hello world", "utf8").unwrap();
        let text = read_file(&state, "hello.txt").unwrap();
        assert_eq!(text.encoding, "utf8");
        assert_eq!(text.content, "hello world");
        assert_eq!(text.size, 11);
        assert!(text.content_type.starts_with("text/"));

        // A file with invalid-utf8 bytes and an unknown extension → base64.
        let raw = vec![0xff, 0xfe, 0x00, 0x99];
        write_file(
            &state,
            "blob.bin",
            &general_purpose::STANDARD.encode(&raw),
            "base64",
        )
        .unwrap();
        let bin = read_file(&state, "blob.bin").unwrap();
        assert_eq!(bin.encoding, "base64");
        assert_eq!(general_purpose::STANDARD.decode(&bin.content).unwrap(), raw);
    }

    #[test]
    fn upload_rejects_oversize_chunk() {
        let (state, _root) = state_with_root();
        let oversize = general_purpose::STANDARD.encode(vec![0u8; MAX_UPLOAD_CHUNK_BYTES + 1]);
        let err = upload_chunk(&state, "up.bin", 0, 1, &oversize, true).unwrap_err();
        assert!(err.contains("1 MiB"));
    }

    #[test]
    fn upload_rejects_over_total_cap() {
        let (state, _root) = state_with_root();
        let chunk = general_purpose::STANDARD.encode(vec![7u8; MAX_UPLOAD_CHUNK_BYTES]);
        // 50 chunks of 1 MiB fill the 50 MiB cap exactly; the 51st must be rejected.
        let mut index = 0u64;
        let mut hit_cap = false;
        while index < 60 {
            match upload_chunk(&state, "big-upload.bin", index, 0, &chunk, false) {
                Ok(_) => index += 1,
                Err(e) => {
                    assert!(e.contains("50 MiB"));
                    hit_cap = true;
                    break;
                }
            }
        }
        assert!(hit_cap, "expected the total-cap guard to fire");
    }

    // ── Temp-dir round-trip (harness-backed) ──────────────────────────────────────────────────
    #[test]
    fn write_then_read_roundtrip() {
        let (state, _root) = state_with_root();
        write_file(&state, "d/a.txt", "hello", "utf8").unwrap();
        let content = read_file(&state, "d/a.txt").unwrap();
        assert_eq!(content.content, "hello");
        assert_eq!(content.encoding, "utf8");
    }

    #[test]
    fn mkdir_then_list() {
        let (state, _root) = state_with_root();
        mkdir(&state, "sub").unwrap();
        let listing = list_dir(&state, ".").unwrap();
        assert!(listing
            .entries
            .iter()
            .any(|e| e.name == "sub" && e.kind == "dir"));
    }

    #[test]
    fn rename_moves_entry() {
        let (state, _root) = state_with_root();
        write_file(&state, "a.txt", "x", "utf8").unwrap();
        rename(&state, "a.txt", "b.txt").unwrap();
        let listing = list_dir(&state, ".").unwrap();
        assert!(listing.entries.iter().any(|e| e.name == "b.txt"));
        assert!(!listing.entries.iter().any(|e| e.name == "a.txt"));

        // Renaming onto an existing target is refused.
        write_file(&state, "c.txt", "y", "utf8").unwrap();
        assert!(rename(&state, "c.txt", "b.txt").is_err());
    }

    #[test]
    fn delete_removes_entry() {
        let (state, _root) = state_with_root();
        write_file(&state, "gone.txt", "bye", "utf8").unwrap();
        delete(&state, "gone.txt").unwrap();
        let listing = list_dir(&state, ".").unwrap();
        assert!(!listing.entries.iter().any(|e| e.name == "gone.txt"));

        // Deleting the root itself is refused.
        assert!(delete(&state, ".").is_err());
    }

    #[test]
    fn chunked_upload_reconstructs_bytes() {
        let (state, _root) = state_with_root();
        let mut original = Vec::new();
        for i in 0..(300_000u32) {
            original.push((i % 256) as u8);
        }
        let mid = original.len() / 2;
        let c0 = general_purpose::STANDARD.encode(&original[..mid]);
        let c1 = general_purpose::STANDARD.encode(&original[mid..]);

        let r0 = upload_chunk(&state, "up/data.bin", 0, 2, &c0, false).unwrap();
        assert!(!r0.complete);
        let r1 = upload_chunk(&state, "up/data.bin", 1, 2, &c1, true).unwrap();
        assert!(r1.complete);

        let read = read_file(&state, "up/data.bin").unwrap();
        let bytes = general_purpose::STANDARD.decode(&read.content).unwrap();
        assert_eq!(bytes, original);
    }

    // ── serde camelCase contract ──────────────────────────────────────────────────────────────
    #[test]
    fn file_types_serialize_camelcase() {
        let content = FileContent {
            path: "d/a.txt".to_string(),
            name: "a.txt".to_string(),
            content_type: "text/plain".to_string(),
            encoding: "utf8".to_string(),
            content: "hi".to_string(),
            size: 2,
        };
        let value = serde_json::to_value(&content).unwrap();
        assert!(value.get("contentType").is_some());
        assert!(value.get("content_type").is_none());

        let entry = FileEntry {
            name: "a".to_string(),
            path: "a".to_string(),
            kind: "file".to_string(),
            size: Some(1),
            modified: None,
        };
        let entry_value = serde_json::to_value(&entry).unwrap();
        for key in ["name", "path", "kind", "size"] {
            assert!(entry_value.get(key).is_some(), "missing {}", key);
        }
        // `modified` is `None` → skipped.
        assert!(entry_value.get("modified").is_none());

        let upload = UploadChunkResult {
            path: "a".to_string(),
            received: 1,
            complete: true,
        };
        assert!(serde_json::to_value(&upload)
            .unwrap()
            .get("complete")
            .is_some());
    }
}
