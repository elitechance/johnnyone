//! Atomic filesystem publish helpers for J1-owned writes.
//!
//! Every file or directory this process publishes uses a sibling temp name plus
//! `rename`, so a crash cannot leave a torn `brief.md` / `tasks.json` / tree
//! under the final path. Lifted from the private `host_files` helper so there
//! is one implementation (decision 14).

use std::fs;
use std::path::{Path, PathBuf};

/// Sibling path with `suffix` appended to the file/dir name (e.g. `.j1tmp`).
/// Stays in the same directory as `target`.
fn sidecar(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    target.with_file_name(name)
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent directory: {}", e))?;
        }
    }
    Ok(())
}

/// Write `bytes` to `path` atomically (sibling `*.j1tmp` + `rename`).
/// Creates parent directories. On rename failure the temp sibling is deleted.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    ensure_parent(path)?;
    let tmp = sidecar(path, ".j1tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    fs::write(&tmp, bytes).map_err(|e| format!("Failed to write file: {}", e))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Failed to finalize file: {}", e)
    })
}

/// Recursively copy `src` onto `dest` by building a sibling temp tree and
/// renaming it into place. Readers of `dest` see either the previous complete
/// tree or the new complete tree — never a half-copied listing.
///
/// If `dest` already exists it is renamed aside, the temp tree is published,
/// then the aside is removed. A failed publish restores the aside.
pub fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!(
            "copy_tree source is not a directory: {}",
            src.display()
        ));
    }
    ensure_parent(dest)?;
    let tmp = sidecar(dest, ".j1tmp");
    if tmp.exists() {
        fs::remove_dir_all(&tmp)
            .map_err(|e| format!("Failed to clear leftover temp tree: {}", e))?;
    }
    copy_dir_walk(src, &tmp, None)?;

    if dest.exists() {
        let aside = sidecar(dest, ".j1old");
        if aside.exists() {
            remove_path(&aside)?;
        }
        fs::rename(dest, &aside).map_err(|e| {
            let _ = fs::remove_dir_all(&tmp);
            format!("Failed to move existing dest aside: {}", e)
        })?;
        if let Err(e) = fs::rename(&tmp, dest) {
            let _ = fs::rename(&aside, dest);
            let _ = fs::remove_dir_all(&tmp);
            return Err(format!("Failed to publish copied tree: {}", e));
        }
        let _ = remove_path(&aside);
    } else if let Err(e) = fs::rename(&tmp, dest) {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("Failed to publish copied tree: {}", e));
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to remove {}: {}", path.display(), e))
    } else {
        fs::remove_file(path).map_err(|e| format!("Failed to remove {}: {}", path.display(), e))
    }
}

/// Test-only fixture copy: skip cargo `target/` trees. Thin wrapper over
/// the single walker used by [`copy_tree`].
pub fn copy_dir_skipping_target(src: &Path, dst: &Path) -> Result<(), String> {
    copy_dir_walk(src, dst, Some(is_cargo_target_dir))
}

fn is_cargo_target_dir(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "target")
}

/// One recursive copy. `skip_dir` (if set) is consulted only for directories.
fn copy_dir_walk(
    src: &Path,
    dest: &Path,
    skip_dir: Option<fn(&Path) -> bool>,
) -> Result<(), String> {
    fs::create_dir_all(dest)
        .map_err(|e| format!("Failed to create {}: {}", dest.display(), e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {}", src.display(), e))? {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let is_dir = entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or_else(|_| src_path.is_dir());
        if is_dir {
            if skip_dir.map(|f| f(&src_path)).unwrap_or(false) {
                continue;
            }
            copy_dir_walk(&src_path, &dest_path, skip_dir)?;
        } else {
            fs::copy(&src_path, &dest_path).map_err(|e| {
                format!(
                    "Failed to copy {} -> {}: {}",
                    src_path.display(),
                    dest_path.display(),
                    e
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "j1-atomic-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn leftover_j1tmp(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".j1tmp") || n.ends_with(".j1old"))
            .collect()
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let dir = tmp_dir("replace");
        let dest = dir.join("brief.md");
        fs::write(&dest, b"old-bytes").unwrap();

        write_atomic(&dest, b"new-bytes").unwrap();

        assert_eq!(fs::read(&dest).unwrap(), b"new-bytes");
        assert!(
            leftover_j1tmp(&dir).is_empty(),
            "no sibling temp must remain: {:?}",
            leftover_j1tmp(&dir)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_creates_parent_dirs() {
        let dir = tmp_dir("parents");
        let dest = dir.join("nested").join("deep").join("file.txt");

        write_atomic(&dest, b"hello").unwrap();

        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        assert!(dest.parent().unwrap().is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_tree_round_trip() {
        let root = tmp_dir("round");
        let src = root.join("src");
        fs::create_dir_all(src.join("phases/00-x")).unwrap();
        fs::create_dir_all(src.join("artifacts")).unwrap();
        fs::create_dir_all(src.join(".git")).unwrap();
        fs::write(src.join("overview.md"), b"# Plan\n").unwrap();
        fs::write(src.join("brief.md"), b"the brief").unwrap();
        fs::write(src.join("phases/00-x/overview.md"), b"# Phase\n").unwrap();
        fs::write(src.join("artifacts/note.txt"), b"note").unwrap();
        fs::write(src.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();

        let dest = root.join("dest");
        copy_tree(&src, &dest).unwrap();

        assert_eq!(fs::read(dest.join("overview.md")).unwrap(), b"# Plan\n");
        assert_eq!(fs::read(dest.join("brief.md")).unwrap(), b"the brief");
        assert_eq!(
            fs::read(dest.join("phases/00-x/overview.md")).unwrap(),
            b"# Phase\n"
        );
        assert_eq!(fs::read(dest.join("artifacts/note.txt")).unwrap(), b"note");
        assert_eq!(
            fs::read(dest.join(".git/HEAD")).unwrap(),
            b"ref: refs/heads/main\n"
        );
        assert!(
            leftover_j1tmp(&root).is_empty(),
            "no sibling temp must remain: {:?}",
            leftover_j1tmp(&root)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_tree_refuses_or_replaces_safely() {
        // Replace: dest already has a complete old tree. After copy, dest is the
        // new complete tree (not a mix) and never a half-copied listing.
        let root = tmp_dir("replace-tree");
        let src = root.join("src");
        fs::create_dir_all(src.join("phases")).unwrap();
        fs::write(src.join("overview.md"), b"new-overview").unwrap();
        fs::write(src.join("phases/only-new.md"), b"new-phase").unwrap();

        let dest = root.join("dest");
        fs::create_dir_all(dest.join("old-only")).unwrap();
        fs::write(dest.join("overview.md"), b"old-overview").unwrap();
        fs::write(dest.join("old-only/stale.txt"), b"stale").unwrap();

        copy_tree(&src, &dest).unwrap();

        assert_eq!(fs::read(dest.join("overview.md")).unwrap(), b"new-overview");
        assert_eq!(
            fs::read(dest.join("phases/only-new.md")).unwrap(),
            b"new-phase"
        );
        assert!(
            !dest.join("old-only").exists(),
            "old dest contents must not remain mixed into the published tree"
        );
        assert!(
            leftover_j1tmp(&root).is_empty(),
            "no sibling temp/aside must remain: {:?}",
            leftover_j1tmp(&root)
        );
        let _ = fs::remove_dir_all(&root);
    }
}
