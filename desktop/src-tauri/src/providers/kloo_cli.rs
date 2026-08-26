//! kloo `--benchmark` invocation + `KLOO_RESULT_JSON` parser.
//!
//! Pure: no spawn, no network, no API keys. The loop (phase 03) feeds
//! [`build_config`] into `cli_runner::spawn_cli` and parses stdout here.

use super::{ChunkType, CliSpawnConfig, StreamChunk};
use crate::services::verify_policy::{
    check_verify, effective_verify_dir, quote_argv_for_shell,
};
use serde::{Deserialize, Serialize};

/// Relative to `$HOME`. The brief names `~/etc/kloo.json`.
pub const DEFAULT_PROFILE_REL: &str = "etc/kloo.json";
pub const DEFAULT_PROVIDER: &str = "openrouter";
pub const DEFAULT_CTX: u32 = 32_768;

/// Resolve `~/etc/kloo.json` from `$HOME` at runtime (overridable per request).
pub fn default_profile_path() -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.trim().is_empty() => {
            format!(
                "{}/{}",
                home.trim_end_matches('/'),
                DEFAULT_PROFILE_REL
            )
        }
        _ => format!("/{}", DEFAULT_PROFILE_REL),
    }
}

/// Inputs for a single kloo oneshot. The caller supplies the model slug
/// (escalation tier); this builder never picks a model itself.
#[derive(Debug, Clone)]
pub struct KlooRunRequest {
    pub workspace: String,
    pub prompt: String,
    pub files: Vec<String>,
    pub verify: String,
    /// Optional jailed subdirectory. Child cwd becomes `workspace.join(cwd)`.
    pub cwd: Option<String>,
    pub model: String,
    pub ctx: Option<u32>,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub cli_path: Option<String>,
    /// Optional; omitted from default argv. kloo writes `--status-file` only
    /// on the visible TUI path; `--benchmark` emits stdout instead.
    pub status_file: Option<String>,
}

pub fn build_config(req: &KlooRunRequest) -> Result<CliSpawnConfig, String> {
    if req.files.is_empty() {
        return Err("shape: empty files".to_string());
    }
    let argv = check_verify(&req.verify).map_err(|e| format!("shape: {e}"))?;
    let verify = quote_argv_for_shell(&argv);

    let command = req
        .cli_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or("kloo")
        .to_string();
    let profile = req
        .profile
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_profile_path);
    let provider = req
        .provider
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(DEFAULT_PROVIDER);
    let ctx = req.ctx.unwrap_or(DEFAULT_CTX);

    let mut args = vec![
        "--benchmark".to_string(),
        "--profile".to_string(),
        profile.to_string(),
        "--provider".to_string(),
        provider.to_string(),
        "--model".to_string(),
        req.model.clone(),
        "--ctx".to_string(),
        ctx.to_string(),
    ];
    for path in &req.files {
        if let Some(rebased) = rebase_allow_path(path, req.cwd.as_deref()) {
            args.push("--allow".to_string());
            args.push(rebased);
        }
    }
    args.push("--verify".to_string());
    args.push(verify);
    args.push("--stop-on".to_string());
    args.push("off-scope-edit".to_string());
    args.push("--no-mcp".to_string());
    if let Some(status) = req
        .status_file
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        args.push("--status-file".to_string());
        args.push(status.to_string());
    }
    // End-of-options so a prompt that starts with '-' / '---' is the task, not a flag.
    // kloo's cobra/pflag root does not SetInterspersed(false).
    args.push("--".to_string());
    args.push(req.prompt.clone());

    Ok(CliSpawnConfig {
        command,
        args,
        working_directory: effective_verify_dir(
            std::path::Path::new(&req.workspace),
            req.cwd.as_deref(),
        )
        .map_err(|e| format!("shape: {e}"))?
        .to_string_lossy()
        .into_owned(),
        env_vars: vec![],
    })
}

/// Strip the `cwd/` prefix from a workspace-relative `files[]` path.
/// Paths that are not under `cwd` are omitted (never forwarded unrebased).
pub(crate) fn rebase_allow_path(path: &str, cwd: Option<&str>) -> Option<String> {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty() && *c != ".") else {
        return Some(path.to_string());
    };
    let cwd = cwd.trim_end_matches('/');
    if path == cwd {
        return Some(".".to_string());
    }
    let prefix = format!("{cwd}/");
    path.strip_prefix(&prefix).map(str::to_string)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct FilesChanged {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct HookResult {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub passed: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct VerifyResult {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub passed: bool,
    pub exit_code: Option<i32>,
}

/// Parsed `KLOO_RESULT_JSON` object. Unknown extra fields are ignored so a
/// kloo upgrade cannot break this crate.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct KlooResult {
    #[serde(default)]
    pub success: bool,
    pub failure_code: Option<String>,
    #[serde(default)]
    pub files_changed: FilesChanged,
    pub off_scope_edits: Option<u64>,
    pub rail_fires: Option<serde_json::Value>,
    pub postchecks: Option<Vec<HookResult>>,
    pub verify: Option<VerifyResult>,
    pub failure_detail: Option<serde_json::Value>,
    #[serde(default)]
    pub transcript_tail: Option<String>,
}

/// Parse a `--status-file` body (raw JSON, no prefix).
pub fn parse_result_json(bytes: &[u8]) -> Result<KlooResult, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid KLOO_RESULT_JSON: {}", e))
}

/// Find lines starting with `KLOO_RESULT_JSON`, take the **last**, parse.
pub fn parse_result_from_stdout(text: &str) -> Result<KlooResult, String> {
    let mut last: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = stripped_result_payload(trimmed) {
            last = Some(rest);
        }
    }
    let payload = last.ok_or_else(|| "no KLOO_RESULT_JSON line in stdout".to_string())?;
    parse_result_json(payload.as_bytes())
}

/// Map a stdout line to a `Text` chunk. stderr is handled separately by
/// `spawn_cli` as `ChunkType::Stderr` and must not go through this parser.
pub fn parse_line(line: &str) -> Option<StreamChunk> {
    Some(StreamChunk {
        chunk_type: ChunkType::Text,
        content: line.to_string(),
        tool_name: None,
        tool_input: None,
        tool_output: None,
        cost_usd: None,
        input_tokens: None,
        output_tokens: None,
        is_final: false,
        session_id: None,
    })
}

fn stripped_result_payload(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("KLOO_RESULT_JSON ")
        .or_else(|| line.strip_prefix("KLOO_RESULT_JSON\t"))
        .or_else(|| {
            line.strip_prefix("KLOO_RESULT_JSON")
                .map(str::trim_start)
                .filter(|s| s.starts_with('{'))
        })?;
    let rest = rest.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_req() -> KlooRunRequest {
        KlooRunRequest {
            workspace: "/tmp/ws".to_string(),
            prompt: "implement add()".to_string(),
            files: vec!["src/add.rs".to_string()],
            verify: "cargo test add -- --exact".to_string(),
            cwd: None,
            model: "qwen/qwen3-coder".to_string(),
            ctx: Some(32_768),
            profile: None,
            provider: None,
            cli_path: None,
            status_file: None,
        }
    }

    fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].as_str())
    }

    #[test]
    fn build_config_includes_benchmark_verify_allow_no_required_status_file() {
        let cfg = build_config(&sample_req()).unwrap();
        assert_eq!(cfg.command, "kloo");
        assert_eq!(cfg.working_directory, "/tmp/ws");
        assert!(cfg.args.iter().any(|a| a == "--benchmark"));
        assert_eq!(
            flag_value(&cfg.args, "--verify"),
            Some("'cargo' 'test' 'add' '--' '--exact'")
        );
        assert_eq!(flag_value(&cfg.args, "--allow"), Some("src/add.rs"));
        let expected_profile = default_profile_path();
        assert_eq!(
            flag_value(&cfg.args, "--profile"),
            Some(expected_profile.as_str())
        );
        assert!(expected_profile.ends_with("/etc/kloo.json"));
        assert_eq!(flag_value(&cfg.args, "--provider"), Some("openrouter"));
        assert_eq!(flag_value(&cfg.args, "--model"), Some("qwen/qwen3-coder"));
        assert_eq!(flag_value(&cfg.args, "--ctx"), Some("32768"));
        assert_eq!(flag_value(&cfg.args, "--stop-on"), Some("off-scope-edit"));
        assert!(cfg.args.iter().any(|a| a == "--no-mcp"));
        assert_eq!(
            cfg.args[cfg.args.len() - 2..],
            ["--".to_string(), "implement add()".to_string()]
        );
        assert!(!cfg.args.iter().any(|a| a == "--status-file"));
        assert!(cfg.env_vars.is_empty());
    }

    #[test]
    fn build_config_separates_prompt_from_flags() {
        let mut req = sample_req();
        req.prompt = "--allow /etc".to_string();
        let cfg = build_config(&req).unwrap();
        assert_eq!(
            cfg.args[cfg.args.len() - 2..],
            ["--".to_string(), "--allow /etc".to_string()]
        );
        let allow_paths: Vec<_> = cfg
            .args
            .windows(2)
            .filter(|w| w[0] == "--allow")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(allow_paths, vec!["src/add.rs"]);
        assert_eq!(cfg.args.last().map(String::as_str), Some("--allow /etc"));
    }

    #[test]
    fn build_config_repeats_allow_per_file() {
        let mut req = sample_req();
        req.files = vec!["src/add.rs".into(), "src/lib.rs".into()];
        let cfg = build_config(&req).unwrap();
        let allows: Vec<_> = cfg
            .args
            .windows(2)
            .filter(|w| w[0] == "--allow")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(allows, vec!["src/add.rs", "src/lib.rs"]);
        assert!(
            !cfg.args.iter().any(|a| a.contains("src/add.rs,src/lib.rs")),
            "must not comma-join --allow paths"
        );
    }

    #[test]
    fn build_config_default_ctx_32768() {
        let mut req = sample_req();
        req.ctx = None;
        let cfg = build_config(&req).unwrap();
        assert_eq!(flag_value(&cfg.args, "--ctx"), Some("32768"));
    }

    #[test]
    fn build_config_uses_task_ctx_when_set() {
        let mut req = sample_req();
        req.ctx = Some(16_384);
        let cfg = build_config(&req).unwrap();
        assert_eq!(flag_value(&cfg.args, "--ctx"), Some("16384"));
    }

    #[test]
    fn build_config_rejects_empty_verify() {
        let mut req = sample_req();
        req.verify = "   ".into();
        let err = build_config(&req).unwrap_err();
        assert!(err.contains("verify"), "{err}");
    }

    #[test]
    fn build_config_does_not_pass_postcheck_for_verify() {
        let cfg = build_config(&sample_req()).unwrap();
        assert!(
            !cfg.args.iter().any(|a| a == "--postcheck"),
            "verify must not be mapped to --postcheck"
        );
    }

    #[test]
    fn default_profile_resolves_home() {
        let path = default_profile_path();
        assert!(path.ends_with("/etc/kloo.json"), "{path}");
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                assert!(path.starts_with(home.trim_end_matches('/')), "{path}");
            }
        }
    }

    #[test]
    fn build_config_does_not_contain_api_key() {
        let cfg = build_config(&sample_req()).unwrap();
        let blob = format!("{:?} {:?}", cfg.args, cfg.env_vars).to_lowercase();
        assert!(!blob.contains("api_key"));
        assert!(!blob.contains("apikey"));
        assert!(!blob.contains("openrouter_api_key"));
    }

    fn success_json() -> &'static str {
        r#"{
            "success": true,
            "files_changed": {"count": 1, "paths": ["src/add.rs"]},
            "off_scope_edits": 0,
            "verify": {"command": "cargo test add -- --exact", "passed": true, "exit_code": 0},
            "extra_future_field": {"ignored": true}
        }"#
    }

    #[test]
    fn parse_success_object() {
        let r = parse_result_json(success_json().as_bytes()).unwrap();
        assert!(r.success);
        assert_eq!(r.files_changed.paths, vec!["src/add.rs"]);
        assert_eq!(r.verify.as_ref().map(|v| v.passed), Some(true));
    }

    #[test]
    fn parse_verify_failed() {
        let raw = r#"{
            "success": false,
            "failure_code": "verify_failed",
            "verify": {"command": "cargo test", "passed": false, "exit_code": 1}
        }"#;
        let r = parse_result_json(raw.as_bytes()).unwrap();
        assert!(!r.success);
        assert_eq!(r.failure_code.as_deref(), Some("verify_failed"));
        assert_eq!(r.verify.as_ref().map(|v| v.passed), Some(false));
    }

    #[test]
    fn parse_postcheck_failed() {
        let raw = r#"{
            "success": false,
            "failure_code": "postcheck_failed",
            "verify": {"command": "cargo test", "passed": true, "exit_code": 0},
            "postchecks": [{"command": "lint", "passed": false, "exit_code": 1}]
        }"#;
        let r = parse_result_json(raw.as_bytes()).unwrap();
        assert_eq!(r.failure_code.as_deref(), Some("postcheck_failed"));
        let hooks = r.postchecks.unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(!hooks[0].passed);
        assert_eq!(hooks[0].exit_code, Some(1));
    }

    #[test]
    fn parse_off_scope_edit() {
        let raw = r#"{
            "success": false,
            "failure_code": "off_scope_edit",
            "off_scope_edits": 1,
            "files_changed": {"count": 0, "paths": []}
        }"#;
        let r = parse_result_json(raw.as_bytes()).unwrap();
        assert_eq!(r.failure_code.as_deref(), Some("off_scope_edit"));
        assert_eq!(r.off_scope_edits, Some(1));
    }

    #[test]
    fn parse_stdout_last_line_wins() {
        let stdout = "\
noise
KLOO_RESULT_JSON {\"success\":false,\"failure_code\":\"verify_failed\"}
more noise
KLOO_RESULT_JSON {\"success\":true,\"files_changed\":{\"count\":1,\"paths\":[\"src/add.rs\"]},\"verify\":{\"passed\":true}}
";
        let r = parse_result_from_stdout(stdout).unwrap();
        assert!(r.success);
        assert_eq!(r.files_changed.paths, vec!["src/add.rs"]);
    }

    #[test]
    fn parse_result_keeps_transcript_tail_from_real_kloo_line() {
        // Fixture copied from kloo docs/configuration.md (KLOO_RESULT_JSON example).
        let stdout = r#"KLOO_RESULT_JSON {"model":"...","endpoint":"...","ctx":8000,"reason":"...","success":false,"steps":1,"tokens":123,"elapsed_seconds":1.23,"tokens_per_sec":100,"compactions":0,"verify":{"command":"npm run build","passed":false,"exit_code":1},"failure_code":"verify_failed","failure_detail":{"source":"verify","reason":"answered","class":"verify_failed","message":"FAIL"},"tool_counters":{"invalid_tool_calls":0,"repeated_read_file":0,"repeated_edits":0,"failed_edits":0,"no_op_edits":0,"verify_attempts":1,"tool_errors":0},"error":"...","transcript_tail":"..."}"#;
        let r = parse_result_from_stdout(stdout).unwrap();
        assert!(!r.success);
        assert_eq!(r.failure_code.as_deref(), Some("verify_failed"));
        assert_eq!(r.transcript_tail.as_deref(), Some("..."));
    }

    #[test]
    fn parse_stdout_without_prefix_errors() {
        let err = parse_result_from_stdout("just a log\nno result\n").unwrap_err();
        assert!(err.contains("no KLOO_RESULT_JSON"), "{err}");
    }

    #[test]
    fn parse_status_file_body_no_prefix() {
        let r = parse_result_json(success_json().as_bytes()).unwrap();
        assert!(r.success);
    }

    #[test]
    fn parse_unknown_extra_field_does_not_fail() {
        let raw = r#"{"success":true,"brand_new_kloo_field":99,"files_changed":{"count":0,"paths":[]}}"#;
        let r = parse_result_json(raw.as_bytes()).unwrap();
        assert!(r.success);
    }

    #[cfg(unix)]
    #[test]
    fn build_config_rejects_symlink_cwd_escape() {
        let ws = std::env::temp_dir().join(format!(
            "j1-kloo-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&ws).unwrap();
        std::os::unix::fs::symlink("/etc", ws.join("link")).unwrap();
        let mut req = sample_req();
        req.workspace = ws.to_string_lossy().into_owned();
        req.cwd = Some("link".into());
        let err = build_config(&req).unwrap_err();
        assert!(err.contains("escapes") || err.contains("verify_not_allowlisted"), "{err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_line_is_text_not_stderr() {
        let chunk = parse_line(r#"KLOO_RESULT_JSON {"success":true}"#).unwrap();
        assert_eq!(chunk.chunk_type, ChunkType::Text);
        assert!(chunk.content.contains("KLOO_RESULT_JSON"));
    }

    #[test]
    fn build_config_verify_is_posix_quoted_not_join() {
        let mut req = sample_req();
        req.verify = "cargo test a;whoami".into();
        let cfg = build_config(&req).unwrap();
        assert_eq!(
            flag_value(&cfg.args, "--verify"),
            Some("'cargo' 'test' 'a;whoami'")
        );
        assert_ne!(
            flag_value(&cfg.args, "--verify"),
            Some(req.verify.as_str())
        );
        req.verify = "cargo test '$(whoami)'".into();
        let cfg = build_config(&req).unwrap();
        assert_eq!(
            flag_value(&cfg.args, "--verify"),
            Some("'cargo' 'test' '$(whoami)'")
        );
        let quoted = flag_value(&cfg.args, "--verify").unwrap();
        assert!(!crate::services::verify_policy::shell_string_has_unquoted_meta(quoted));
    }

    #[test]
    fn build_config_cwd_narrows_working_directory_and_rebases_allow() {
        let mut req = sample_req();
        req.workspace = "/tmp/ws".into();
        req.cwd = Some("desktop/src-tauri".into());
        req.files = vec![
            "desktop/src-tauri/src/lib.rs".into(),
            "web/src/app/x.ts".into(),
        ];
        req.verify = "cargo test mul -- --exact".into();
        let cfg = build_config(&req).unwrap();
        assert!(
            cfg.working_directory.ends_with("desktop/src-tauri"),
            "{}",
            cfg.working_directory
        );
        let allows: Vec<_> = cfg
            .args
            .windows(2)
            .filter(|w| w[0] == "--allow")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(allows, vec!["src/lib.rs"]);
        assert!(
            !allows.iter().any(|a| a.contains("web/src")),
            "unrebased path must not be forwarded: {allows:?}"
        );
        assert_eq!(
            flag_value(&cfg.args, "--verify"),
            Some(&*quote_argv_for_shell(
                &check_verify("cargo test mul -- --exact").unwrap()
            ))
        );
    }
}
