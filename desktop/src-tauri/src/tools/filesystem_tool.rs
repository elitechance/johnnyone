use super::tool_schema::{RiskLevel, ToolDefinition};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing;

/// Maximum file size for read operations (10 MB).
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum number of entries to return from a directory listing.
const MAX_DIR_ENTRIES: usize = 1000;

/// Maximum search depth for recursive operations.
const MAX_SEARCH_DEPTH: u32 = 10;

/// Directories that are never accessible.
const BLOCKED_PATHS: &[&str] = &[
    "/etc/shadow",
    "/etc/passwd",
    "/etc/sudoers",
    "/proc",
    "/sys",
    "/dev",
];

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum FilesystemInput {
    #[serde(rename = "read")]
    Read { path: String },
    #[serde(rename = "write")]
    Write { path: String, content: String },
    #[serde(rename = "list")]
    List {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    #[serde(rename = "search")]
    Search {
        path: String,
        pattern: String,
        #[serde(default)]
        max_depth: Option<u32>,
    },
    #[serde(rename = "stat")]
    Stat { path: String },
    #[serde(rename = "mkdir")]
    Mkdir {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    #[serde(rename = "delete")]
    Delete { path: String },
}

/// Filesystem tool with path sandboxing and safety guardrails.
pub struct FilesystemTool {
    /// Base sandbox directories. If set, only paths under these are accessible.
    sandbox_roots: Vec<PathBuf>,
}

impl FilesystemTool {
    pub fn new() -> Self {
        // Default sandbox: user's home directory
        let home = dirs_or_home();
        Self {
            sandbox_roots: vec![home],
        }
    }

    /// Execute a filesystem operation.
    pub async fn execute(&self, input: &serde_json::Value) -> Result<serde_json::Value, String> {
        let params: FilesystemInput = serde_json::from_value(input.clone())
            .map_err(|e| format!("Invalid filesystem input: {}", e))?;

        match params {
            FilesystemInput::Read { path } => self.read_file(&path).await,
            FilesystemInput::Write { path, content } => self.write_file(&path, &content).await,
            FilesystemInput::List { path, recursive } => self.list_dir(&path, recursive).await,
            FilesystemInput::Search {
                path,
                pattern,
                max_depth,
            } => {
                self.search(&path, &pattern, max_depth.unwrap_or(MAX_SEARCH_DEPTH))
                    .await
            }
            FilesystemInput::Stat { path } => self.stat(&path).await,
            FilesystemInput::Mkdir { path, recursive } => self.mkdir(&path, recursive).await,
            FilesystemInput::Delete { path } => self.delete(&path).await,
        }
    }

    /// Validate that a path is safe to access.
    fn validate_path(&self, path_str: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(path_str);

        // Resolve to absolute path
        let abs_path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Cannot resolve relative path: {}", e))?
                .join(path)
        };

        // Canonicalize to resolve symlinks and ..
        // Note: the path might not exist yet (for write operations), so we check the parent
        let check_path = if abs_path.exists() {
            abs_path
                .canonicalize()
                .map_err(|e| format!("Cannot canonicalize path: {}", e))?
        } else {
            // For new files, canonicalize the parent
            let parent = abs_path
                .parent()
                .ok_or_else(|| "Path has no parent directory".to_string())?;
            if !parent.exists() {
                return Err(format!("Parent directory does not exist: {}", parent.display()));
            }
            let canonical_parent = parent
                .canonicalize()
                .map_err(|e| format!("Cannot canonicalize parent: {}", e))?;
            canonical_parent.join(
                abs_path
                    .file_name()
                    .ok_or_else(|| "Path has no filename".to_string())?,
            )
        };

        // Check against blocked paths
        let path_str = check_path.to_string_lossy();
        for blocked in BLOCKED_PATHS {
            if path_str.starts_with(blocked) {
                return Err(format!("Access to {} is blocked", blocked));
            }
        }

        // Check sandbox
        if !self.sandbox_roots.is_empty() {
            let in_sandbox = self
                .sandbox_roots
                .iter()
                .any(|root| check_path.starts_with(root));
            if !in_sandbox {
                return Err(format!(
                    "Path {} is outside the allowed sandbox directories",
                    check_path.display()
                ));
            }
        }

        Ok(check_path)
    }

    /// Read a file and return its contents.
    async fn read_file(&self, path_str: &str) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(path = %path.display(), "Reading file");

        // Check file size
        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| format!("Cannot read file metadata: {}", e))?;

        if !metadata.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }

        if metadata.len() > MAX_READ_SIZE {
            return Err(format!(
                "File too large: {} bytes (max {} bytes)",
                metadata.len(),
                MAX_READ_SIZE
            ));
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "content": content,
            "size_bytes": metadata.len(),
        }))
    }

    /// Write content to a file.
    async fn write_file(&self, path_str: &str, content: &str) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(path = %path.display(), size = content.len(), "Writing file");

        fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write file: {}", e))?;

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "bytes_written": content.len(),
            "success": true,
        }))
    }

    /// List directory contents.
    async fn list_dir(
        &self,
        path_str: &str,
        recursive: bool,
    ) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(path = %path.display(), recursive = recursive, "Listing directory");

        if !path.is_dir() {
            return Err(format!("{} is not a directory", path.display()));
        }

        let entries = if recursive {
            self.list_recursive(&path, 0, MAX_SEARCH_DEPTH).await?
        } else {
            self.list_flat(&path).await?
        };

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "entries": entries,
            "count": entries.len(),
        }))
    }

    async fn list_flat(&self, path: &Path) -> Result<Vec<serde_json::Value>, String> {
        let mut entries = Vec::new();
        let mut dir = fs::read_dir(path)
            .await
            .map_err(|e| format!("Failed to read directory: {}", e))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| format!("Failed to read entry: {}", e))?
        {
            if entries.len() >= MAX_DIR_ENTRIES {
                break;
            }

            let metadata = entry.metadata().await.ok();
            let file_type = if metadata.as_ref().map_or(false, |m| m.is_dir()) {
                "directory"
            } else if metadata.as_ref().map_or(false, |m| m.is_symlink()) {
                "symlink"
            } else {
                "file"
            };

            entries.push(serde_json::json!({
                "name": entry.file_name().to_string_lossy(),
                "path": entry.path().to_string_lossy(),
                "type": file_type,
                "size": metadata.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }

        Ok(entries)
    }

    async fn list_recursive(
        &self,
        root_path: &Path,
        _depth: u32,
        max_depth: u32,
    ) -> Result<Vec<serde_json::Value>, String> {
        // Iterative approach using a stack to avoid recursive async fn issues
        let mut all_entries = Vec::new();
        let mut stack: Vec<(PathBuf, u32)> = vec![(root_path.to_path_buf(), 0)];

        while let Some((path, depth)) = stack.pop() {
            if depth > max_depth || all_entries.len() >= MAX_DIR_ENTRIES {
                continue;
            }

            let entries = self.list_flat(&path).await?;

            for entry in &entries {
                if entry.get("type").and_then(|t| t.as_str()) == Some("directory") {
                    if let Some(p) = entry.get("path").and_then(|p| p.as_str()) {
                        stack.push((PathBuf::from(p), depth + 1));
                    }
                }
            }

            all_entries.extend(entries);
        }

        Ok(all_entries)
    }

    /// Search for files matching a pattern.
    async fn search(
        &self,
        path_str: &str,
        pattern: &str,
        max_depth: u32,
    ) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(
            path = %path.display(),
            pattern = %pattern,
            "Searching files"
        );

        let mut matches = Vec::new();
        self.search_recursive(&path, pattern, 0, max_depth, &mut matches)
            .await?;

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "pattern": pattern,
            "matches": matches,
            "count": matches.len(),
        }))
    }

    async fn search_recursive(
        &self,
        root_path: &Path,
        pattern: &str,
        _depth: u32,
        max_depth: u32,
        matches: &mut Vec<serde_json::Value>,
    ) -> Result<(), String> {
        // Iterative approach using a stack to avoid recursive async fn issues
        let mut stack: Vec<(std::path::PathBuf, u32)> = vec![(root_path.to_path_buf(), 0)];

        while let Some((path, depth)) = stack.pop() {
            if depth > max_depth || matches.len() >= MAX_DIR_ENTRIES {
                continue;
            }

            let mut dir = match fs::read_dir(&path).await {
                Ok(d) => d,
                Err(_) => continue,
            };

            while let Some(entry) = dir
                .next_entry()
                .await
                .map_err(|e| format!("Failed to read entry: {}", e))?
            {
                if matches.len() >= MAX_DIR_ENTRIES {
                    break;
                }

                let name = entry.file_name().to_string_lossy().to_string();

                // Simple glob matching: * matches any sequence, ? matches single char
                if glob_match(pattern, &name) {
                    matches.push(serde_json::json!({
                        "name": name,
                        "path": entry.path().to_string_lossy(),
                    }));
                }

                // Queue subdirectories for processing
                if let Ok(metadata) = entry.metadata().await {
                    if metadata.is_dir() {
                        stack.push((entry.path(), depth + 1));
                    }
                }
            }
        }

        Ok(())
    }

    /// Get file/directory metadata.
    async fn stat(&self, path_str: &str) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| format!("Cannot stat: {}", e))?;

        let file_type = if metadata.is_dir() {
            "directory"
        } else if metadata.is_symlink() {
            "symlink"
        } else {
            "file"
        };

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "type": file_type,
            "size": metadata.len(),
            "readonly": metadata.permissions().readonly(),
            "modified": metadata.modified().ok().map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
            }),
        }))
    }

    /// Create a directory.
    async fn mkdir(&self, path_str: &str, recursive: bool) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(path = %path.display(), recursive = recursive, "Creating directory");

        if recursive {
            fs::create_dir_all(&path)
                .await
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        } else {
            fs::create_dir(&path)
                .await
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "success": true,
        }))
    }

    /// Delete a file (not directories, for safety).
    async fn delete(&self, path_str: &str) -> Result<serde_json::Value, String> {
        let path = self.validate_path(path_str)?;

        tracing::info!(path = %path.display(), "Deleting file");

        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| format!("Cannot access: {}", e))?;

        if metadata.is_dir() {
            return Err("Cannot delete directories with this tool. Use shell 'rm -r' for directory deletion.".to_string());
        }

        fs::remove_file(&path)
            .await
            .map_err(|e| format!("Failed to delete file: {}", e))?;

        Ok(serde_json::json!({
            "path": path.to_string_lossy(),
            "success": true,
        }))
    }

    /// Return the tool definition for capability reporting.
    pub fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "filesystem".to_string(),
            description: "Read, write, list, search, and manage files on the desktop. Paths are sandboxed to the user's home directory.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write", "list", "search", "stat", "mkdir", "delete"],
                        "description": "The filesystem action to perform"
                    },
                    "path": {
                        "type": "string",
                        "description": "The file or directory path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (for 'write' action)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (for 'search' action)"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Whether to operate recursively"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum search depth"
                    }
                }
            }),
            requires_approval: false,
            risk_level: RiskLevel::Medium,
        }
    }
}

impl Default for FilesystemTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple glob pattern matching supporting * and ? wildcards.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    glob_match_impl(&pattern_chars, &text_chars)
}

fn glob_match_impl(pattern: &[char], text: &[char]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }

    if pattern[0] == '*' {
        // Try matching * with zero or more characters
        for i in 0..=text.len() {
            if glob_match_impl(&pattern[1..], &text[i..]) {
                return true;
            }
        }
        return false;
    }

    if text.is_empty() {
        return false;
    }

    if pattern[0] == '?' || pattern[0] == text[0] {
        return glob_match_impl(&pattern[1..], &text[1..]);
    }

    false
}

/// Get the user's home directory or fall back to current directory.
fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "lib.rs"));
        assert!(!glob_match("*.rs", "main.ts"));
        assert!(glob_match("test_*", "test_hello"));
        assert!(glob_match("??.rs", "ab.rs"));
        assert!(!glob_match("??.rs", "abc.rs"));
        assert!(glob_match("*", "anything"));
    }
}
