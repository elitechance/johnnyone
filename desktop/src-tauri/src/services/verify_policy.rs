//! Verify-string policy: tokenize (quoting only) + allowlisted runners.
//!
//! Authorization is `argv[0]` ∈ the runner allowlist. There is **no**
//! metacharacter reject set — `;`, `|`, `{a,b}` are literal argv tokens
//! (Amendment 7). kloo `--verify` is `quote_argv_for_shell` of an already
//! approved argv (Amendment 8).

use crate::services::task_spec::jail_rel_path;
use std::path::{Path, PathBuf};

const RULE_NOT_ALLOWLISTED: &str = "verify_not_allowlisted";
const RULE_NOT_SCOPED: &str = "verify_not_scoped";

const ALLOWED_RUNNERS: &[&str] = &["cargo", "npx", "go", "python", "python3", "node"];

/// Documented complement of the allowlist — not a character blocklist.
const DENIED_RUNNERS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "rm", "curl", "wget", "ssh", "sudo", "chmod", "chown",
    "dd", "kill", "nc", "npm", "pnpm", "yarn", "make",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyPolicyError {
    pub rule: String,
    pub detail: String,
}

impl VerifyPolicyError {
    fn not_allowlisted(detail: impl Into<String>) -> Self {
        Self {
            rule: RULE_NOT_ALLOWLISTED.to_string(),
            detail: detail.into(),
        }
    }

    fn not_scoped(detail: impl Into<String>) -> Self {
        Self {
            rule: RULE_NOT_SCOPED.to_string(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for VerifyPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.rule, self.detail)
    }
}

impl std::error::Error for VerifyPolicyError {}

/// Parse `verify` into argv. Quoting only — no expansion, globbing, or operators.
/// Pure: no `std::process`, no `std::fs`.
pub fn parse_verify(raw: &str) -> Result<Vec<String>, VerifyPolicyError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(VerifyPolicyError::not_allowlisted(
            "empty verify",
        ));
    }
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_token = false;

    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
            i += 1;
            continue;
        }
        if in_double {
            if c == '\\' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_double = false;
            } else {
                cur.push(c);
            }
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            cur.push(chars[i + 1]);
            in_token = true;
            i += 2;
            continue;
        }
        if c == '\'' {
            in_single = true;
            in_token = true;
            i += 1;
            continue;
        }
        if c == '"' {
            in_double = true;
            in_token = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut cur));
                in_token = false;
            }
            i += 1;
            continue;
        }
        cur.push(c);
        in_token = true;
        i += 1;
    }

    if in_single || in_double {
        return Err(VerifyPolicyError::not_allowlisted(
            "unbalanced quotes",
        ));
    }
    if in_token {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return Err(VerifyPolicyError::not_allowlisted(
            "empty verify",
        ));
    }
    for t in &tokens {
        if t.is_empty() {
            return Err(VerifyPolicyError::not_allowlisted(
                "empty token",
            ));
        }
        if t.chars().any(|ch| ch.is_whitespace()) {
            return Err(VerifyPolicyError::not_allowlisted(format!(
                "whitespace inside token: {t:?}"
            )));
        }
    }
    Ok(tokens)
}

/// POSIX-quote each token (`'…'`, embedded `'` → `'\''`) and join on space.
/// Canonical form of an already-approved argv for kloo `--verify`.
pub fn quote_argv_for_shell<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|t| quote_posix(t.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_posix(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Effective child working directory: `workspace.join(cwd)` or `workspace`.
pub fn effective_verify_dir(workspace: &Path, cwd: Option<&str>) -> PathBuf {
    match cwd.map(str::trim).filter(|c| !c.is_empty() && *c != ".") {
        Some(rel) => workspace.join(rel),
        None => workspace.to_path_buf(),
    }
}

/// Parse then validate. The only argv mutation is prepending `npx --no-install`.
pub fn check_verify(raw: &str) -> Result<Vec<String>, VerifyPolicyError> {
    let mut argv = parse_verify(raw)?;
    if argv.first().map(String::as_str) == Some("npx")
        && !argv.iter().any(|a| a == "--no-install")
    {
        argv.insert(1, "--no-install".to_string());
    }
    validate_argv(&argv)?;
    Ok(argv)
}

pub fn validate_argv(argv: &[String]) -> Result<(), VerifyPolicyError> {
    let first = argv
        .first()
        .map(String::as_str)
        .ok_or_else(|| VerifyPolicyError::not_allowlisted("empty argv"))?;
    if DENIED_RUNNERS.contains(&first) {
        return Err(VerifyPolicyError::not_allowlisted(format!(
            "denied runner {first:?}"
        )));
    }
    if !ALLOWED_RUNNERS.contains(&first) {
        return Err(VerifyPolicyError::not_allowlisted(format!(
            "runner {first:?} is not allowlisted"
        )));
    }
    match first {
        "cargo" => validate_cargo(argv),
        "npx" => validate_npx(argv),
        "go" => validate_go(argv),
        "python" | "python3" => validate_python(argv),
        "node" => validate_node(argv),
        _ => unreachable!("allowlist and match must stay in sync"),
    }
}

fn validate_cargo(argv: &[String]) -> Result<(), VerifyPolicyError> {
    if argv.get(1).map(String::as_str) != Some("test") {
        return Err(VerifyPolicyError::not_allowlisted(
            "cargo: only `test` is allowed",
        ));
    }
    let rest = &argv[2..];
    let mut i = 0;
    let mut has_filter = false;
    while i < rest.len() {
        let t = rest[i].as_str();
        if t == "--" {
            break;
        }
        if is_cargo_chdir_flag(t) {
            return Err(VerifyPolicyError::not_allowlisted(
                "cargo test -C is not allowed; use --manifest-path",
            ));
        }
        if t == "--manifest-path" {
            let val = rest.get(i + 1).ok_or_else(|| {
                VerifyPolicyError::not_allowlisted("cargo --manifest-path missing value")
            })?;
            jail_token(val)?;
            i += 2;
            continue;
        }
        if let Some(val) = t.strip_prefix("--manifest-path=") {
            jail_token(val)?;
            i += 1;
            continue;
        }
        if t.starts_with('-') {
            if let Some(next) = rest.get(i + 1) {
                if !next.starts_with('-') && cargo_flag_takes_value(t) {
                    if is_path_like(next) {
                        jail_token(next)?;
                    }
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if is_path_like(t) {
            jail_token(t)?;
        }
        has_filter = true;
        i += 1;
    }
    if !has_filter {
        return Err(VerifyPolicyError::not_scoped(
            "bare cargo test is a whole-suite verify",
        ));
    }
    Ok(())
}

fn is_cargo_chdir_flag(t: &str) -> bool {
    t == "-C" || (t.starts_with("-C") && !t.starts_with("--"))
}

fn cargo_flag_takes_value(t: &str) -> bool {
    matches!(
        t,
        "--features"
            | "--package"
            | "-p"
            | "--target"
            | "--target-dir"
            | "--color"
            | "--jobs"
            | "-j"
            | "--exclude"
            | "--profile"
            | "--config"
            | "--message-format"
            | "--test"
            | "--bin"
            | "--example"
            | "--bench"
    )
}

fn validate_npx(argv: &[String]) -> Result<(), VerifyPolicyError> {
    let mut runner = None;
    let mut has_run = false;
    let mut file = None;
    let mut i = 1;
    while i < argv.len() {
        let t = argv[i].as_str();
        if t == "--no-install" {
            i += 1;
            continue;
        }
        if t == "--prefix" || t == "--config" {
            let val = argv.get(i + 1).ok_or_else(|| {
                VerifyPolicyError::not_allowlisted(format!("npx {t} missing value"))
            })?;
            jail_token(val)?;
            i += 2;
            continue;
        }
        if let Some(val) = t.strip_prefix("--prefix=") {
            jail_token(val)?;
            i += 1;
            continue;
        }
        if let Some(val) = t.strip_prefix("--config=") {
            jail_token(val)?;
            i += 1;
            continue;
        }
        if t == "vitest" || t == "jest" {
            runner = Some(t);
            i += 1;
            continue;
        }
        if t == "run" {
            has_run = true;
            i += 1;
            continue;
        }
        if t.starts_with('-') {
            if let Some(next) = argv.get(i + 1) {
                if !next.starts_with('-') && (t == "--config" || t == "--prefix") {
                    jail_token(next)?;
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if file.is_none() {
            file = Some(t);
        }
        if is_path_like(t) {
            jail_token(t)?;
        }
        i += 1;
    }
    match runner {
        Some("vitest") | Some("jest") => {}
        Some(other) => {
            return Err(VerifyPolicyError::not_allowlisted(format!(
                "npx package {other:?} is not vitest or jest"
            )));
        }
        None => {
            return Err(VerifyPolicyError::not_allowlisted(
                "npx must invoke vitest or jest",
            ));
        }
    }
    if !has_run || file.is_none() {
        return Err(VerifyPolicyError::not_scoped(
            "npx vitest/jest must include `run` and a file-like argument",
        ));
    }
    Ok(())
}

fn validate_go(argv: &[String]) -> Result<(), VerifyPolicyError> {
    if argv.get(1).map(String::as_str) != Some("test") {
        return Err(VerifyPolicyError::not_allowlisted(
            "go: only `test` is allowed",
        ));
    }
    for (idx, t) in argv.iter().enumerate() {
        if t == "-C" || (t.starts_with("-C") && !t.starts_with("--") && t != "-C") {
            if idx != 2 {
                return Err(VerifyPolicyError::not_allowlisted(
                    "go -C is only allowed immediately after `test`",
                ));
            }
        }
        if t == "-C" && idx != 2 {
            return Err(VerifyPolicyError::not_allowlisted(
                "go -C is only allowed immediately after `test`",
            ));
        }
    }
    let mut i = 2;
    if argv.get(i).map(String::as_str) == Some("-C") {
        let dir = argv.get(i + 1).ok_or_else(|| {
            VerifyPolicyError::not_allowlisted("go test -C missing directory")
        })?;
        jail_token(dir)?;
        i += 2;
    } else if argv
        .get(i)
        .map(|t| t.starts_with("-C") && t != "-C" && !t.starts_with("--"))
        .unwrap_or(false)
    {
        return Err(VerifyPolicyError::not_allowlisted(
            "go -C value must be a separate argument",
        ));
    }
    let mut pkg = None;
    while i < argv.len() {
        let t = argv[i].as_str();
        if t == "-C" || (t.starts_with("-C") && !t.starts_with("--")) {
            return Err(VerifyPolicyError::not_allowlisted(
                "go -C is only allowed immediately after `test`",
            ));
        }
        if t.starts_with('-') {
            if let Some(next) = argv.get(i + 1) {
                if !next.starts_with('-') {
                    if is_path_like(next) {
                        jail_token(next)?;
                    }
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if pkg.is_none() {
            pkg = Some(t);
        }
        if is_go_all_packages(t) {
            return Err(VerifyPolicyError::not_scoped(
                "go test ./... is a whole-suite verify",
            ));
        }
        if is_path_like(t) {
            jail_token(t)?;
        }
        i += 1;
    }
    let Some(pkg) = pkg else {
        return Err(VerifyPolicyError::not_scoped(
            "go test requires a package path",
        ));
    };
    if is_go_all_packages(pkg) {
        return Err(VerifyPolicyError::not_scoped(
            "go test ./... is a whole-suite verify",
        ));
    }
    Ok(())
}

fn is_go_all_packages(pkg: &str) -> bool {
    pkg == "..." || pkg == "./..." || pkg.ends_with("/...")
}

fn validate_python(argv: &[String]) -> Result<(), VerifyPolicyError> {
    if argv.get(1).map(String::as_str) != Some("-m")
        || argv.get(2).map(String::as_str) != Some("pytest")
    {
        return Err(VerifyPolicyError::not_allowlisted(
            "python: only `-m pytest` is allowed",
        ));
    }
    let mut file = None;
    let mut i = 3;
    while i < argv.len() {
        let t = argv[i].as_str();
        if t.starts_with('-') {
            if let Some(next) = argv.get(i + 1) {
                if !next.starts_with('-') {
                    if is_path_like(next) {
                        jail_token(next)?;
                    }
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if file.is_none() {
            file = Some(t);
        }
        if is_path_like(t) {
            jail_token(t)?;
        } else {
            // pytest target without a slash still has to stay in-jail if it looks like a path later
        }
        i += 1;
    }
    let Some(file) = file else {
        return Err(VerifyPolicyError::not_scoped(
            "python -m pytest requires a file path",
        ));
    };
    if is_path_like(file) {
        jail_token(file)?;
    } else {
        // bare module name — still require a file-like token
        return Err(VerifyPolicyError::not_scoped(
            "python -m pytest requires a file-like path",
        ));
    }
    Ok(())
}

fn validate_node(argv: &[String]) -> Result<(), VerifyPolicyError> {
    for t in argv.iter().skip(1) {
        if is_node_code_flag(t) {
            return Err(VerifyPolicyError::not_allowlisted(
                "node -e/-p/-c is not allowed",
            ));
        }
    }
    let mut file = None;
    for t in argv.iter().skip(1) {
        if t.starts_with('-') {
            continue;
        }
        file = Some(t.as_str());
        break;
    }
    let Some(file) = file else {
        return Err(VerifyPolicyError::not_scoped(
            "node requires a relative test-file path",
        ));
    };
    jail_token(file)?;
    Ok(())
}

fn is_node_code_flag(t: &str) -> bool {
    t == "-e"
        || t == "-p"
        || t == "-c"
        || (t.starts_with("-e") && !t.starts_with("--"))
        || (t.starts_with("-p") && !t.starts_with("--"))
        || (t.starts_with("-c") && !t.starts_with("--"))
}

fn is_path_like(token: &str) -> bool {
    if token.is_empty() || token == "--" || token.starts_with('-') {
        return false;
    }
    token.starts_with('.')
        || token.starts_with('/')
        || token.contains('/')
        || token.contains('\\')
        || Path::new(token).extension().is_some()
}

fn jail_token(token: &str) -> Result<(), VerifyPolicyError> {
    jail_rel_path(token).map_err(|e| VerifyPolicyError::not_allowlisted(e))
}

/// Planned verify spawn: authorised argv + effective cwd. No process.
pub fn plan_task_verify(
    workspace: &Path,
    verify: &str,
    cwd: Option<&str>,
) -> Result<PlannedVerify, VerifyPolicyError> {
    let argv = check_verify(verify)?;
    Ok(PlannedVerify {
        argv,
        current_dir: effective_verify_dir(workspace, cwd),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedVerify {
    pub argv: Vec<String>,
    pub current_dir: PathBuf,
}

/// True when `s` contains `;`, `$`, or backtick outside POSIX single quotes.
pub fn shell_string_has_unquoted_meta(s: &str) -> bool {
    let mut in_squote = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if !in_squote {
            if c == '\'' {
                in_squote = true;
                continue;
            }
            if matches!(c, ';' | '$' | '`' | '|' | '&' | '(' | ')' | '{' | '}') {
                return true;
            }
        } else if c == '\'' {
            in_squote = false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_string()).collect()
    }

    #[test]
    fn parse_initiative1_fixture() {
        assert_eq!(
            parse_verify("cargo test mul -- --exact").unwrap(),
            s(&["cargo", "test", "mul", "--", "--exact"])
        );
    }

    #[test]
    fn parse_quoted_filename_without_space() {
        assert_eq!(
            parse_verify(r#"npx vitest run "queue.spec.ts""#).unwrap(),
            s(&["npx", "vitest", "run", "queue.spec.ts"])
        );
    }

    #[test]
    fn reject_whitespace_inside_token() {
        let err = parse_verify(r#"npx vitest run "foo bar.spec.ts""#).unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
        assert!(err.detail.contains("whitespace"), "{err}");
    }

    #[test]
    fn reject_unbalanced_quotes() {
        let err = parse_verify(r#"cargo test "foo"#).unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
        assert!(err.detail.contains("unbalanced"), "{err}");
    }

    #[test]
    fn reject_empty_and_whitespace() {
        for raw in ["", "   ", "\t\n"] {
            let err = parse_verify(raw).unwrap_err();
            assert_eq!(err.rule, RULE_NOT_ALLOWLISTED, "{raw:?}");
        }
    }

    #[test]
    fn brace_group_is_literal_token() {
        assert_eq!(
            parse_verify("cargo test {a,b}").unwrap(),
            s(&["cargo", "test", "{a,b}"])
        );
    }

    #[test]
    fn semicolon_is_literal_in_token() {
        assert_eq!(
            parse_verify("cargo test foo;bar").unwrap(),
            s(&["cargo", "test", "foo;bar"])
        );
        assert_eq!(
            parse_verify(r#"cargo test "a;b""#).unwrap(),
            s(&["cargo", "test", "a;b"])
        );
    }

    #[test]
    fn quote_argv_posix_single_quotes() {
        assert_eq!(
            quote_argv_for_shell(&["cargo", "test", "a;whoami"]),
            "'cargo' 'test' 'a;whoami'"
        );
        assert_eq!(
            quote_argv_for_shell(&["cargo", "test", "$(whoami)"]),
            "'cargo' 'test' '$(whoami)'"
        );
        assert_eq!(quote_argv_for_shell(&["it's"]), "'it'\\''s'");
    }

    #[test]
    fn parse_verify_is_pure() {
        let src = include_str!("verify_policy.rs");
        let impl_src = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !impl_src.contains("std::process::") && !impl_src.contains("Command::"),
            "parse/check must not spawn"
        );
        assert!(
            !impl_src.contains("std::fs::"),
            "parse/check must not touch the filesystem"
        );
        assert!(
            !impl_src.contains("FORBIDDEN") && !impl_src.contains("METACHAR"),
            "no character-reject table"
        );
    }

    #[test]
    fn allow_cargo_test_with_filter() {
        let argv = check_verify("cargo test mul -- --exact").unwrap();
        assert_eq!(argv, s(&["cargo", "test", "mul", "--", "--exact"]));
    }

    #[test]
    fn allow_npx_vitest_run_prepends_no_install() {
        let argv = check_verify("npx vitest run queue.service.spec.ts").unwrap();
        assert_eq!(
            argv,
            s(&[
                "npx",
                "--no-install",
                "vitest",
                "run",
                "queue.service.spec.ts"
            ])
        );
    }

    #[test]
    fn allow_cargo_manifest_path() {
        let argv = check_verify(
            "cargo test mul --manifest-path desktop/src-tauri/Cargo.toml -- --exact",
        )
        .unwrap();
        assert!(argv.iter().any(|a| a == "--manifest-path"));
        assert!(argv.iter().any(|a| a == "desktop/src-tauri/Cargo.toml"));
    }

    #[test]
    fn reject_cargo_dash_c() {
        let err = check_verify("cargo test mul -C desktop/src-tauri -- --exact").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
        assert!(err.detail.contains("-C"), "{err}");
    }

    #[test]
    fn reject_absolute_manifest_and_go_c_escape() {
        let err = check_verify("cargo test mul --manifest-path /etc/Cargo.toml").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
        let err = check_verify("go test -C ../secret ./pkg").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
    }

    #[test]
    fn reject_positional_node_escape() {
        let err = check_verify("node ../../../tmp/x.js").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
        assert!(err.detail.contains("..") || err.detail.contains("relative"), "{err}");
    }

    #[test]
    fn reject_absolute_npx_positional() {
        let err = check_verify("npx --no-install vitest run /etc/x.spec.ts").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
    }

    #[test]
    fn reject_python_dotdot() {
        let err = check_verify("python -m pytest ../../secret/test_x.py").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
    }

    #[test]
    fn allow_go_c_immediately_after_test() {
        let argv = check_verify("go test -C desktop/src-tauri ./foo").unwrap();
        assert_eq!(argv, s(&["go", "test", "-C", "desktop/src-tauri", "./foo"]));
    }

    #[test]
    fn reject_go_c_wrong_position() {
        let err = check_verify("go test ./foo -C desktop/src-tauri").unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
    }

    #[test]
    fn reject_whole_suite_and_denied_runners() {
        assert_eq!(
            check_verify("cargo test").unwrap_err().rule,
            RULE_NOT_SCOPED
        );
        assert_eq!(
            check_verify("npm test").unwrap_err().rule,
            RULE_NOT_ALLOWLISTED
        );
        assert_eq!(
            check_verify("npx vitest").unwrap_err().rule,
            RULE_NOT_SCOPED
        );
        assert_eq!(
            check_verify("go test ./...").unwrap_err().rule,
            RULE_NOT_SCOPED
        );
        let err = check_verify(r#"node -e "require('fs').writeFileSync('x','x')""#).unwrap_err();
        assert_eq!(err.rule, RULE_NOT_ALLOWLISTED);
    }

    /// Brief acceptance 3: a model-authored `verify` cannot execute arbitrary shell.
    /// These strings would be catastrophic if they ran. We refuse to spawn them.
    #[test]
    fn model_authored_verify_cannot_exec_arbitrary_shell() {
        let root = std::env::temp_dir().join(format!(
            "j1-verify-sentinel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let sentinel_dir = root.join("keep-me");
        std::fs::create_dir_all(&sentinel_dir).unwrap();
        let marker = sentinel_dir.join("marker");
        std::fs::write(&marker, "safe").unwrap();
        let pwned = root.join("pwned");

        let cases = [
            format!("rm -rf {}", sentinel_dir.display()),
            format!("sh -c 'echo pwned > {}'", pwned.display()),
            "bash -c 'curl http://127.0.0.1:1'".to_string(),
            "npm test".to_string(),
            "cargo test".to_string(),
            format!(
                r#"node -e "require('fs').writeFileSync('{}','x')""#,
                pwned.display()
            ),
        ];
        for raw in &cases {
            let err = check_verify(raw).expect_err(raw);
            assert!(
                err.rule == RULE_NOT_ALLOWLISTED || err.rule == RULE_NOT_SCOPED,
                "{raw}: {err}"
            );
        }
        assert!(marker.is_file(), "rm must not have spawned");
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "safe");
        assert!(!pwned.exists(), "sh/node payload must not have run");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quoted_verify_does_not_split_under_shell() {
        for token in ["a;whoami", "$(whoami)", "`id`"] {
            let argv = check_verify(&format!("cargo test {token}")).unwrap();
            let quoted = quote_argv_for_shell(&argv);
            assert!(
                !shell_string_has_unquoted_meta(&quoted),
                "unquoted meta in {quoted}"
            );
            assert!(quoted.starts_with("'cargo' 'test' '"));
        }
    }
}
