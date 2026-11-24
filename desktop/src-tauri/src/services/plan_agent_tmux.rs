//! The plan agent runs VISIBLY in tmux (user decision 2026-11-23, D1), while its structured
//! stream-json stays the only data path.
//!
//! One tmux session per plan agent, `johnnyone_<sessionId>` (a J1-owned name, so the PR #75 retire
//! and orphan-sweep rules apply). Each chat turn runs inside that session's pane as a generated
//! script `<plan>/plan-agent/turns/<n>.sh`:
//!
//! ```text
//! { claude <plan-agent args> "$(cat <n>.prompt)" 2>><n>.stderr; echo $? ><n>.rc; } \
//!     | tee -a <n>.jsonl | johnnyone-desktop --plan-agent-view
//! mv <n>.rc <n>.exit
//! ```
//!
//! - J1 reads replies and tool calls ONLY from `turns/<n>.jsonl` (append-only), through the same
//!   parser as the headless path (`claude_code::parse_line`), never from the pane.
//! - The turn is complete when the jsonl carries the `result` event (the parser's final chunk). The
//!   `<n>.exit` sidecar (written after the whole pipeline, so the jsonl is flushed) tells a crash
//!   apart: an exit with no `result` event is a chat error carrying the last stderr line.
//! - The prompt text never enters a shell string: it is written to `<n>.prompt` (0600) and passed
//!   as ONE argument via `"$(cat …)"` — the shell does not re-parse a command substitution's
//!   output inside double quotes. Every other word is single-quoted ([`shell_quote`]).
//! - The token is never on the pane's command line or in the script: the args hold only the PATH
//!   of the 0600 `mcp.json`. `turns/` is 0700, its files 0600.
//! - The pane shows a human-readable live view ([`view_lines`], run as the engine binary's
//!   `--plan-agent-view` filter).
//!
//! tmux is reached through the [`TmuxRunner`] seam so tests never touch a tmux server.

use crate::providers::cli_runner::{self, CliProcess, ExternalTurn, StopFuture};
use crate::providers::{ChunkType, CliSpawnConfig, StreamChunk};
use crate::services::{mcp_tokens, plan_agent, settings as settings_service};
use crate::state::app_state::AppState;
use rusqlite::params;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// The engine binary's argv[1] that turns it into the pane's pretty-printer (see `main.rs`).
pub const VIEW_FLAG: &str = "--plan-agent-view";

/// argv[1] for the viewer handshake: the engine prints [`PROBE_TOKEN`] and exits before any startup.
pub const PROBE_FLAG: &str = "--plan-agent-view-probe";

/// The exact handshake answer. The script execs the viewer only after the file's identity matches
/// the running engine AND this answer comes back; otherwise it falls back to `cat`.
pub const PROBE_TOKEN: &str = "j1-plan-agent-view:1";

/// The chat error for a second send while a turn is in flight.
pub const PLAN_AGENT_BUSY: &str = "The plan agent is still answering the last message. Try again when it finishes.";

/// Shell filter that strips terminal controls from text bound for the pane: C0 (keeping `\t` and
/// `\n`), DEL, and UTF-8-encoded C1 (U+0080–U+009F). So model/tool/stderr text can never carry
/// an escape sequence (OSC 52 clipboard writes, title changes, …) into the viewer's terminal.
/// The C1 substitution LOOPS (`:a … ta`) until nothing matches: a single pass over overlapping
/// bytes (`\302\302\233\233`) would leave a freshly formed `\302\233` behind (F-3-37).
pub const SHELL_SANITIZE: &str =
    r"LC_ALL=C tr -d '\000-\010\013-\037\177' | LC_ALL=C sed -e ':a' -e 's/\xc2[\x80-\x9f]//g' -e 'ta'";

/// `<plan-agent dir>/turns`.
pub const TURNS_DIR: &str = "turns";

/// How often the tail polls the jsonl / exit sidecar.
const POLL: Duration = Duration::from_millis(200);

/// Poll rounds between pane-liveness checks (a pane killed mid-turn writes no exit sidecar).
const LIVENESS_EVERY: u32 = 10;

/// Boxed future returned by [`TmuxRunner::run`].
pub type TmuxFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;

/// The one tmux seam: run `tmux <args>` and return stdout. Production uses [`LiveTmux`]; tests use
/// a recording fake, so no test depends on a tmux server.
pub trait TmuxRunner: Send + Sync {
    fn run(&self, args: Vec<String>) -> TmuxFuture<'_>;
}

/// The real tmux (`terminal::run_tmux`).
pub struct LiveTmux;

impl TmuxRunner for LiveTmux {
    fn run(&self, args: Vec<String>) -> TmuxFuture<'_> {
        Box::pin(crate::terminal::run_tmux(args))
    }
}

// ───────────────────────────── pure parts ─────────────────────────────

/// The plan agent's tmux session name: `johnnyone_<sessionId>` (the same derivation every J1 pane
/// uses, so `owned_session_id_from_tmux_name` and the orphan sweep recognise it).
pub fn pane_session_name(session_id: &str) -> String {
    crate::terminal::tmux_session_name(session_id)
}

/// POSIX single-quoting: the result is ONE shell word whose value is exactly `s`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Every file of turn `n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPaths {
    pub n: u32,
    pub script: PathBuf,
    pub prompt: PathBuf,
    pub jsonl: PathBuf,
    pub stderr: PathBuf,
    pub rc: PathBuf,
    pub exit: PathBuf,
    /// Written by the engine once it stopped following the turn (its tail ended for any reason,
    /// or the restart reconcile recorded it). A turn with neither `exit` nor `settled` was in
    /// flight when the previous engine died.
    pub settled: PathBuf,
    /// Written at turn start by an engine that settles its turns. A turn WITHOUT it was started
    /// by an older binary (no `.settled` support): the restart reconcile settles it silently.
    pub started: PathBuf,
}

impl TurnPaths {
    pub fn new(dir: &Path, n: u32) -> Self {
        let f = |ext: &str| dir.join(format!("{n}.{ext}"));
        Self { n, script: f("sh"), prompt: f("prompt"), jsonl: f("jsonl"), stderr: f("stderr"), rc: f("rc"), exit: f("exit"), settled: f("settled"), started: f("started") }
    }
}

/// `<initiatives_dir>/<id>/plan-agent/turns`.
pub fn turns_dir(initiatives_dir: &Path, initiative_id: &str) -> Result<PathBuf, String> {
    Ok(mcp_tokens::plan_agent_dir(initiatives_dir, initiative_id)?.join(TURNS_DIR))
}

/// The next turn number: one past the highest `<n>.<ext>` in `dir` (1 for an empty/missing dir).
pub fn next_turn_number(dir: &Path) -> u32 {
    let max = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.split('.').next()?.parse::<u32>().ok()))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    max + 1
}

/// The pane's pretty-printer: its path and the identity (`dev:ino:size:mtime`) of the file that
/// must be at that path for the script to run it. `identity: None` → the script always uses `cat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub path: String,
    pub identity: Option<String>,
}

/// `dev:ino:size:mtime` of a file — the same fields as `stat -L -c '%d:%i:%s:%Y'`.
pub fn file_identity(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some(format!("{}:{}:{}:{}", m.dev(), m.ino(), m.size(), m.mtime()))
}

/// Split a `build_config_with` config into (config without the prompt, prompt). The prompt is the
/// final positional argument there.
pub fn split_prompt(mut cfg: CliSpawnConfig) -> (CliSpawnConfig, String) {
    let prompt = cfg.args.pop().unwrap_or_default();
    (cfg, prompt)
}

/// The turn script (POSIX sh). `cfg` must NOT hold the prompt (see [`split_prompt`]); the prompt is
/// read from `paths.prompt` at run time. `viewer` is the pretty-printer (see [`Viewer`]).
pub fn turn_script(cfg: &CliSpawnConfig, paths: &TurnPaths, viewer: &Viewer) -> String {
    let q = |p: &Path| shell_quote(&p.to_string_lossy());
    let n = paths.n;
    let mut s = String::new();
    s.push_str("#!/bin/sh\n");
    // Every file this turn creates (stderr, rc, exit) is private, whatever the tmux server's umask.
    s.push_str("umask 077\n");
    s.push_str(&format!("# J1 plan agent, turn {n}. Generated by the engine; the data path is {n}.jsonl, never this pane.\n"));
    for (k, v) in &cfg.env_vars {
        // Env names are engine constants; still refuse anything that is not a plain identifier.
        if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.is_empty() {
            continue;
        }
        if v.is_empty() {
            s.push_str(&format!("unset {k}\n"));
        } else {
            s.push_str(&format!("{k}={}; export {k}\n", shell_quote(v)));
        }
    }
    let exit_now = |code: u32| format!("{{ echo {code} > {}; exit {code}; }}", q(&paths.exit));
    if !cfg.working_directory.is_empty() {
        s.push_str(&format!("cd {} || {}\n", shell_quote(&cfg.working_directory), exit_now(97)));
    }
    s.push_str(&format!("printf '\\n%s\\n' {}\n", shell_quote(&format!("── J1 plan agent · turn {n} ──"))));
    // Echo the user's message for the human watching: read from the 0600 file at run time and passed
    // as a printf ARGUMENT, so the text is never part of the script.
    s.push_str(&format!(
        "printf '▶ you: %s\\n' \"$(head -n 1 {} | cut -c1-200 | {SHELL_SANITIZE})\"\n",
        q(&paths.prompt)
    ));
    let mut cmd = shell_quote(&cfg.command);
    for a in &cfg.args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    // The prompt: one argument, read from its 0600 file; never interpolated into this script.
    cmd.push_str(&format!(" \"$(cat {})\"", q(&paths.prompt)));
    // The viewer runs only when the file at its path is the engine that wrote this script
    // (identity check, no exec) AND it answers the handshake; anything else → `cat`. So a swapped
    // or rolled-back binary is never exec'd with an argument it might not understand.
    let viewer_ok = match &viewer.identity {
        Some(id) => format!(
            "[ \"$(stat -L -c '%d:%i:%s:%Y' {v} 2>/dev/null)\" = {id} ] && [ \"$({v} {probe} </dev/null 2>/dev/null)\" = {token} ]",
            v = shell_quote(&viewer.path),
            id = shell_quote(id),
            probe = PROBE_FLAG,
            token = shell_quote(PROBE_TOKEN),
        ),
        None => "false".to_string(),
    };
    s.push_str(&format!(
        "{{ {cmd} </dev/null 2>>{stderr}; echo $? > {rc}; }} | tee -a {jsonl} | {{ if {viewer_ok}; then exec {viewer_q} {flag}; else {SHELL_SANITIZE}; fi; }}\n",
        stderr = q(&paths.stderr),
        rc = q(&paths.rc),
        jsonl = q(&paths.jsonl),
        viewer_q = shell_quote(&viewer.path),
        flag = VIEW_FLAG,
    ));
    // After the WHOLE pipeline: tee has flushed the jsonl, so `<n>.exit` appearing means the data
    // is complete.
    s.push_str(&format!("rc=$(cat {} 2>/dev/null || echo 127)\n", q(&paths.rc)));
    s.push_str(&format!("mv -f {} {} 2>/dev/null || echo \"$rc\" > {}\n", q(&paths.rc), q(&paths.exit), q(&paths.exit)));
    s.push_str(&format!(
        "if [ \"$rc\" != 0 ]; then printf '✗ turn {n} failed (exit %s)\\n' \"$rc\"; tail -n 5 {} 2>/dev/null | {SHELL_SANITIZE}; fi\n",
        q(&paths.stderr)
    ));
    s
}

/// The pane's command: `sh <script>` (two argv words; tmux execs them directly, no shell string).
pub fn pane_argv(paths: &TurnPaths) -> Vec<String> {
    vec!["sh".to_string(), paths.script.to_string_lossy().to_string()]
}

/// `new-session` for the first turn, chained with `remain-on-exit on` so the turn's output stays
/// visible after it ends (the next turn respawns the dead pane).
pub fn new_session_args(name: &str, cwd: &str, argv: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["new-session", "-d", "-s", name, "-x", "120", "-y", "36"].iter().map(|s| s.to_string()).collect();
    if !cwd.is_empty() {
        a.push("-c".to_string());
        a.push(cwd.to_string());
    }
    a.extend(argv.iter().cloned());
    a.extend([";", "set-option", "-w", "-t", &format!("={name}:"), "remain-on-exit", "on"].iter().map(|s| s.to_string()));
    a
}

/// `respawn-pane -k` for a later turn (kills a leftover process, keeps the pane's history).
pub fn respawn_args(pane_id: &str, argv: &[String]) -> Vec<String> {
    let mut a = vec!["respawn-pane".to_string(), "-k".to_string(), "-t".to_string(), pane_id.to_string()];
    a.extend(argv.iter().cloned());
    a
}

/// The `list-panes -a` format the pane lookups parse.
pub const PANES_FORMAT: &str = "#{pane_id}\t#{pane_dead}\t#{session_name}";

/// `(pane_id, dead)` of the first pane of EXACTLY `name` out of a [`PANES_FORMAT`] listing.
pub fn pane_of(listing: &str, name: &str) -> Option<(String, bool)> {
    listing.lines().find_map(|line| {
        let mut p = line.splitn(3, '\t');
        let id = p.next()?.trim();
        let dead = p.next()?.trim() == "1";
        let n = p.next()?.trim_end_matches(['\r', '\n']);
        (n == name && !id.is_empty()).then(|| (id.to_string(), dead))
    })
}

/// The chat error for a turn that ended without a `result` event.
pub fn crash_error(exit_code: Option<i32>, stderr: &str) -> String {
    let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
    let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string());
    match last {
        Some(line) => format!("Plan agent turn failed (exit {code}): {line}"),
        None => format!("Plan agent turn failed (exit {code}) with no output."),
    }
}

/// Splits appended bytes into complete lines; a trailing partial line waits for its newline.
#[derive(Default)]
pub struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            out.push(String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string());
        }
        out
    }

    /// The unterminated rest (a final line written without a newline), consumed.
    pub fn take_rest(&mut self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let rest = String::from_utf8_lossy(&std::mem::take(&mut self.partial)).to_string();
        Some(rest)
    }
}

// ───────────────────────────── pane pretty-printer ─────────────────────────────

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>())
    }
}

fn tool_result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The pane's human-readable lines for one stream-json line: assistant text, `→ tool(args)` for a
/// call, `✓ v5` (or `✓ <summary>` / `✗ <error>`) for a result, a footer for the `result` event. A
/// non-JSON line is shown as is. Display only — J1 never reads these back.
pub fn view_lines(line: &str) -> Vec<String> {
    view_lines_raw(line).iter().map(|l| strip_controls(l)).collect()
}

/// Drop terminal control characters: C0 except `\t` (`\n` never occurs inside a view line), DEL
/// and C1 (U+0080–U+009F). No escape sequence from model, tool or stderr text reaches the pane.
pub fn strip_controls(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\t' || !(c.is_control()))
        .collect()
}

fn view_lines_raw(line: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return if line.trim().is_empty() { vec![] } else { vec![line.to_string()] };
    };
    let mut out = Vec::new();
    match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "system" => {
            if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("?");
                out.push(format!("● session ready (model {model})"));
            }
        }
        "assistant" => {
            for b in v.pointer("/message/content").and_then(|c| c.as_array()).into_iter().flatten() {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            out.extend(t.lines().map(str::to_string));
                        }
                    }
                    Some("tool_use") => {
                        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let name = name.strip_prefix("mcp__j1__").unwrap_or(name);
                        let args = b.get("input").map(|i| i.to_string()).unwrap_or_default();
                        out.push(format!("→ {name}({})", clip(&args, 160)));
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for b in v.pointer("/message/content").and_then(|c| c.as_array()).into_iter().flatten() {
                if b.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                    continue;
                }
                let text = tool_result_text(b);
                if b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
                    out.push(format!("✗ {}", clip(&text, 160)));
                    continue;
                }
                let version = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|j| j.get("version").and_then(|n| n.as_i64()));
                match version {
                    Some(n) => out.push(format!("✓ v{n}")),
                    None => out.push(format!("✓ {}", clip(&text, 120))),
                }
            }
        }
        "result" => {
            if v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
                out.push("✗ turn ended with an error".to_string());
            } else {
                let cost = v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0);
                out.push(format!("── done · ${cost:.4} ──"));
            }
        }
        _ => {}
    }
    out
}

/// The `--plan-agent-view` filter: stdin stream-json → [`view_lines`] on stdout. It ALWAYS drains
/// stdin to EOF, even when stdout is gone, so `tee` (and the jsonl data path behind it) never sees
/// a broken pipe because of the view.
pub fn run_view_filter() {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut out_ok = true;
    for line in stdin.lock().split(b'\n') {
        let Ok(bytes) = line else { break };
        if !out_ok {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for l in view_lines(text.trim_end_matches('\r')) {
            if writeln!(stdout, "{l}").is_err() {
                out_ok = false;
                break;
            }
        }
        if out_ok && stdout.flush().is_err() {
            out_ok = false;
        }
    }
}

/// The viewer: this engine binary (its `--plan-agent-view` mode), pinned to the identity of the
/// RUNNING image (`/proc/self/exe`), so a binary replaced on disk fails the script's check.
pub fn engine_viewer() -> Viewer {
    let path = std::env::current_exe()
        .map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string())
        .unwrap_or_default();
    Viewer { path, identity: file_identity(Path::new("/proc/self/exe")) }
}

// ───────────────────────────── the turn ─────────────────────────────

/// Write turn `n`'s files (0600 in the 0700 `turns/` dir) and start it in the pane: a new
/// `johnnyone_<sid>` session on the first turn, `respawn-pane` on later ones. Returns the paths.
pub async fn start_turn(
    tmux: &dyn TmuxRunner,
    session_id: &str,
    dir: &Path,
    cfg: CliSpawnConfig,
    viewer: &Viewer,
) -> Result<TurnPaths, String> {
    ensure_private_dir(dir)?;
    let (cfg, prompt) = split_prompt(cfg);
    let (paths, mut script_file) = reserve_turn(dir, next_turn_number(dir))?;
    write_new_private(&paths.prompt, prompt.as_bytes())?;
    // This engine follows the turn and settles it (`<n>.settled`); the reconcile trusts that.
    write_new_private(&paths.started, b"")?;
    // Create the jsonl up front so the tail can open it before claude writes anything.
    write_new_private(&paths.jsonl, b"")?;
    {
        use std::io::Write;
        script_file
            .write_all(turn_script(&cfg, &paths, viewer).as_bytes())
            .map_err(|e| format!("Failed to write {}: {e}", paths.script.display()))?;
    }
    let name = pane_session_name(session_id);
    let argv = pane_argv(&paths);
    // Log the pane command (paths only — the token is never in it, pinned by
    // `turn_command_never_contains_token`).
    tracing::info!(%session_id, tmux = %name, argv = ?argv, "plan agent turn in tmux");
    let listing = tmux.run(vec!["list-panes".into(), "-a".into(), "-F".into(), PANES_FORMAT.into()]).await.unwrap_or_default();
    match pane_of(&listing, &name) {
        Some((pane_id, _)) => {
            tmux.run(respawn_args(&pane_id, &argv)).await?;
        }
        None => {
            tmux.run(new_session_args(&name, &cfg.working_directory, &argv)).await?;
        }
    }
    Ok(paths)
}

fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("Failed to create the plan-agent turns dir: {e}"))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("Failed to restrict the plan-agent turns dir: {e}"))
}

/// Create `path` 0600, failing if it already exists (`create_new`: an earlier turn's file is never
/// truncated or shared).
fn create_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = create_new_private(path).map_err(|e| format!("Failed to create {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Reserve the first free turn number `>= start` by creating its script with `create_new`: two
/// concurrent starts can never get the same `n` (the loser moves on to the next number).
pub fn reserve_turn(dir: &Path, start: u32) -> Result<(TurnPaths, std::fs::File), String> {
    let mut n = start.max(1);
    for _ in 0..10_000 {
        let paths = TurnPaths::new(dir, n);
        let taken = [&paths.prompt, &paths.jsonl, &paths.exit].iter().any(|p| p.exists());
        if !taken {
            match create_new_private(&paths.script) {
                Ok(f) => return Ok((paths, f)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("Failed to reserve plan-agent turn {n}: {e}")),
            }
        }
        n += 1;
    }
    Err("no free plan-agent turn number".to_string())
}

/// One turn at a time per plan agent: holding a slot = a turn in flight. Released on drop (when
/// the turn's tail ends: result, crash, pane gone, or Stop).
pub struct TurnSlot {
    set: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    session_id: String,
}

impl Drop for TurnSlot {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.session_id);
        }
    }
}

/// Claim the session's turn slot; `Err(PLAN_AGENT_BUSY)` while another turn holds it.
pub fn claim_turn(state: &AppState, session_id: &str) -> Result<TurnSlot, String> {
    let mut set = state.plan_agent_turns.lock().map_err(|_| "plan agent turn lock poisoned".to_string())?;
    if !set.insert(session_id.to_string()) {
        return Err(PLAN_AGENT_BUSY.to_string());
    }
    Ok(TurnSlot { set: state.plan_agent_turns.clone(), session_id: session_id.to_string() })
}

/// Is a turn in flight for this plan agent?
pub fn turn_in_flight(state: &AppState, session_id: &str) -> bool {
    state.plan_agent_turns.lock().map(|s| s.contains(session_id)).unwrap_or(false)
}

fn read_from(path: &Path, offset: &mut u64) -> Vec<u8> {
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    if f.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_ok() {
        *offset += buf.len() as u64;
    }
    buf
}

fn read_exit(path: &Path) -> Option<Option<i32>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.trim().parse::<i32>().ok())
}

fn error_chunk(message: String) -> StreamChunk {
    StreamChunk {
        chunk_type: ChunkType::Error,
        content: message,
        tool_name: None,
        tool_input: None,
        tool_output: None,
        cost_usd: None,
        input_tokens: None,
        output_tokens: None,
        is_final: true,
        session_id: None,
    }
}

/// Follow turn `paths` until it completes, sending every parsed chunk of `<n>.jsonl` to `tx`:
/// - a parsed final chunk (the `result` event) completes the turn;
/// - `<n>.exit` without a `result` event → one Error chunk ([`crash_error`] with the last stderr line);
/// - the pane gone/dead with no exit sidecar (killed) → one Error chunk;
/// - `cancel` set (Stop / retire) → the channel just closes.
pub async fn tail_turn<F>(
    tmux: Arc<dyn TmuxRunner>,
    pane_name: String,
    paths: TurnPaths,
    parse_line: F,
    tx: mpsc::Sender<StreamChunk>,
    cancel: Arc<AtomicBool>,
    poll: Duration,
) where
    F: Fn(&str) -> Option<StreamChunk> + Send + Sync + 'static,
{
    let mut offset = 0u64;
    let mut lines = LineBuffer::default();
    let mut round = 0u32;
    // Send every complete line; `true` once the final (result) chunk went out.
    let drain = |offset: &mut u64, lines: &mut LineBuffer, flush_rest: bool| {
        let mut out: Vec<String> = lines.push(&read_from(&paths.jsonl, offset));
        if flush_rest {
            out.extend(lines.take_rest());
        }
        out.into_iter().filter_map(|l| parse_line(&l)).collect::<Vec<_>>()
    };
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        // Read the exit sidecar BEFORE the data: the script writes it after the pipeline, so data
        // read after seeing it is complete.
        let exit = read_exit(&paths.exit);
        for chunk in drain(&mut offset, &mut lines, exit.is_some()) {
            let fin = chunk.is_final;
            if tx.send(chunk).await.is_err() {
                return;
            }
            if fin {
                return;
            }
        }
        if let Some(code) = exit {
            let stderr = std::fs::read_to_string(&paths.stderr).unwrap_or_default();
            let _ = tx.send(error_chunk(crash_error(code, &stderr))).await;
            return;
        }
        round += 1;
        if round % LIVENESS_EVERY == 0 {
            let listing = tmux
                .run(vec!["list-panes".into(), "-a".into(), "-F".into(), PANES_FORMAT.into()])
                .await;
            // Only a successful listing that lacks the pane (or shows it dead) counts: a tmux
            // hiccup is not a crash. The pane belongs to THIS turn only while no newer turn's
            // script exists — a newer turn means this one was superseded.
            let superseded = paths
                .script
                .parent()
                .map(|d| next_turn_number(d) > paths.n + 1)
                .unwrap_or(false);
            let gone = superseded
                || match &listing {
                    Ok(l) => pane_of(l, &pane_name).map(|(_, dead)| dead).unwrap_or(true),
                    Err(_) => false,
                };
            if gone && read_exit(&paths.exit).is_none() {
                // One last look: the sidecar may land between the listing and here.
                tokio::time::sleep(poll).await;
                if read_exit(&paths.exit).is_none() {
                    for chunk in drain(&mut offset, &mut lines, true) {
                        let fin = chunk.is_final;
                        if tx.send(chunk).await.is_err() || fin {
                            return;
                        }
                    }
                    let stderr = std::fs::read_to_string(&paths.stderr).unwrap_or_default();
                    let msg = crash_error(None, &stderr).replace("(exit unknown)", "(pane closed)");
                    let _ = tx.send(error_chunk(msg)).await;
                    return;
                }
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// The in-flight turn as an [`ExternalTurn`]: Stop/retire cancels the tail and kills the pane's
/// process (`respawn-pane -k` with a fixed notice, so the pane's history stays readable).
pub struct PaneTurn {
    tmux: Arc<dyn TmuxRunner>,
    pane_name: String,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl ExternalTurn for PaneTurn {
    fn stop(&mut self) -> StopFuture<'_> {
        Box::pin(async move {
            self.cancel.store(true, Ordering::SeqCst);
            let listing = self
                .tmux
                .run(vec!["list-panes".into(), "-a".into(), "-F".into(), PANES_FORMAT.into()])
                .await?;
            if let Some((pane_id, false)) = pane_of(&listing, &self.pane_name) {
                let notice = vec!["printf".to_string(), "%s\\n".to_string(), "── turn stopped by J1 ──".to_string()];
                self.tmux.run(respawn_args(&pane_id, &notice)).await?;
            }
            Ok(())
        })
    }

    fn is_running(&self) -> bool {
        !self.done.load(Ordering::SeqCst) && !self.cancel.load(Ordering::SeqCst)
    }
}

/// Start a plan-agent turn in its pane and follow its jsonl: the same `(CliProcess, Receiver)`
/// contract as `cli_runner::spawn_cli`, so the chat paths consume it unchanged.
pub async fn spawn_turn_with<F>(
    tmux: Arc<dyn TmuxRunner>,
    session_id: &str,
    dir: &Path,
    cfg: CliSpawnConfig,
    viewer: &Viewer,
    parse_line: F,
    poll: Duration,
    slot: Option<TurnSlot>,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>, TurnPaths), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + Sync + 'static,
{
    let paths = start_turn(tmux.as_ref(), session_id, dir, cfg, viewer).await?;
    let (tx, rx) = mpsc::channel::<StreamChunk>(256);
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let pane_name = pane_session_name(session_id);
    {
        let (tmux, pane_name, paths, cancel, done) = (tmux.clone(), pane_name.clone(), paths.clone(), cancel.clone(), done.clone());
        tokio::spawn(async move {
            tail_turn(tmux, pane_name, paths.clone(), parse_line, tx, cancel, poll).await;
            // This engine followed the turn to its end: the restart reconcile must leave it alone.
            let _ = write_new_private(&paths.settled, b"tail\n");
            done.store(true, Ordering::SeqCst);
            // The turn is over: the plan agent may take the next one.
            drop(slot);
        });
    }
    let turn = PaneTurn { tmux, pane_name, cancel, done };
    Ok((CliProcess::external(Box::new(turn), session_id.to_string()), rx, paths))
}

/// The chat paths' one spawn call: a plan-agent turn (`plan_slot` = the turn slot the chat path
/// claimed BEFORE storing anything) runs in its tmux pane; every other turn is the unchanged
/// headless `spawn_cli`.
pub async fn spawn_chat_turn<F>(
    state: &AppState,
    config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
    plan_slot: Option<TurnSlot>,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + Sync + 'static,
{
    spawn_chat_turn_with(state, Arc::new(LiveTmux), config, session_id, parse_line, plan_slot).await
}

/// [`spawn_chat_turn`] with an explicit tmux (tests pass a fake).
pub(crate) async fn spawn_chat_turn_with<F>(
    state: &AppState,
    tmux: Arc<dyn TmuxRunner>,
    config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
    plan_slot: Option<TurnSlot>,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + Sync + 'static,
{
    match plan_slot {
        None => cli_runner::spawn_cli(config, session_id, parse_line).await,
        Some(slot) => spawn_plan_agent_turn_with(state, tmux, config, session_id, parse_line, Some(slot)).await,
    }
}

/// Start a plan-agent turn. `slot`: the turn slot the caller already holds (the chat paths claim
/// it before storing the user message, F-2-19); `None` claims it here.
pub(crate) async fn spawn_plan_agent_turn_with<F>(
    state: &AppState,
    tmux: Arc<dyn TmuxRunner>,
    config: CliSpawnConfig,
    session_id: String,
    parse_line: F,
    slot: Option<TurnSlot>,
) -> Result<(CliProcess, mpsc::Receiver<StreamChunk>), String>
where
    F: Fn(&str) -> Option<StreamChunk> + Send + Sync + 'static,
{
    let Some((initiative_id, _)) = plan_agent::plan_for_agent_session(state, &session_id)? else {
        return Err("not a plan-agent session".to_string());
    };
    // The authoritative one-turn-at-a-time claim (held until the turn's tail ends).
    let slot = match slot {
        Some(slot) if slot.session_id == session_id => slot,
        Some(_) => return Err("turn slot belongs to another session".to_string()),
        None => claim_turn(state, &session_id)?,
    };
    let store = settings_service::resolve_initiatives_dir(state);
    let dir = turns_dir(&store, &initiative_id)?;
    let (process, rx, _) =
        spawn_turn_with(tmux, &session_id, &dir, config, &engine_viewer(), parse_line, POLL, Some(slot)).await?;
    // Record the pane name on the row (the Raw view and the sweep key off `johnnyone_<sid>`).
    let name = pane_session_name(&session_id);
    let _ = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE sessions SET tmux_session_name = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![name, session_id],
        )
        .map_err(|e| e.to_string())
    });
    Ok((process, rx))
}

// ───────────────────────────── engine restart (item E, F-1-3) ─────────────────────────────

/// The assistant message recorded for a turn the previous engine never finished following.
pub const INTERRUPTED_BY_RESTART: &str =
    "The plan agent's turn was interrupted by an engine restart. Send your message again.";

/// The pane notice when the reconcile silently ends an older binary's still-running turn.
const LEGACY_NOTICE: &str = "── turn ended by J1 (engine restart) ──";

/// The pane notice when the reconcile kills a turn that was still running.
const INTERRUPTED_NOTICE: &str = "── turn interrupted by an engine restart ──";

/// One turn the reconcile recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedTurn {
    pub session_id: String,
    pub n: u32,
    /// The pane was still running the turn and was killed (`respawn-pane -k`).
    pub killed: bool,
}

/// Engine start: for each LIVE plan agent (plan in briefing, agent row active), if its LATEST turn
/// has a script and `<n>.started` but neither `<n>.exit` nor `<n>.settled` (the previous engine
/// died while following it). A turn without `<n>.started` came from an older binary and is only
/// settled, silently.
/// - pane alive → kill the turn with `respawn-pane -k` and a fixed notice (chosen over
///   re-attaching a tail: the chat request that was waiting for the reply died with the engine, so
///   nobody would read a re-attached stream, and the token must not keep a headless turn alive);
/// - pane dead or gone → nothing to kill.
///
/// Either way a failed assistant message ([`INTERRUPTED_BY_RESTART`], `finish_reason =
/// 'interrupted'`) is recorded — filling the host path's empty placeholder row when it is the
/// session's last message — and `<n>.settled` is written so a later start never records it twice.
/// The turn files stay in place; slots are in-memory, so there is nothing to release. The reconcile
/// holds the session's turn slot while it works, so it can never touch a turn this engine started.
pub(crate) async fn reconcile_interrupted_turns_with(state: &AppState, tmux: &dyn TmuxRunner) -> Vec<InterruptedTurn> {
    let agents: Vec<(String, String)> = state
        .db
        .with_conn(|conn| {
            let mut stmt = conn
                // Only a LIVE plan agent (G-1-7): a plan still in briefing whose agent row is active.
                // A retired/accepted/closed plan's agent gets no "interrupted" message.
                .prepare(
                    "SELECT p.id, p.briefing_session_id FROM agent_plans p JOIN sessions s ON s.id = p.briefing_session_id \
                     WHERE p.initiative_status = 'briefing' AND p.status != 'closed' AND s.status = 'active' ORDER BY p.id",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "plan agent restart reconcile: lookup failed");
            Vec::new()
        });
    let store = settings_service::resolve_initiatives_dir(state);
    let mut out = Vec::new();
    let mut listing: Option<String> = None;
    for (initiative_id, session_id) in agents {
        let Ok(dir) = turns_dir(&store, &initiative_id) else { continue };
        let n = next_turn_number(&dir).saturating_sub(1);
        if n == 0 {
            continue;
        }
        let paths = TurnPaths::new(&dir, n);
        if !paths.script.exists() || paths.exit.exists() || paths.settled.exists() {
            continue;
        }
        let legacy = !paths.started.exists();
        // Hold the slot: a turn this engine is running (or starts now) is never touched.
        let Ok(_slot) = claim_turn(state, &session_id) else { continue };
        if next_turn_number(&dir) != n + 1 {
            continue; // a newer turn appeared meanwhile
        }
        if listing.is_none() {
            listing = Some(
                tmux.run(vec!["list-panes".into(), "-a".into(), "-F".into(), PANES_FORMAT.into()])
                    .await
                    .unwrap_or_default(),
            );
        }
        let pane = pane_of(listing.as_deref().unwrap_or(""), &pane_session_name(&session_id));
        if legacy {
            // Started by an older binary that never wrote `.settled` (its Stop/timeout looks the
            // same as a crash): record NOTHING (G-1-7). A pane still running it is killed
            // silently (G-2-16) so a headless turn cannot outlive the restart; then it is settled.
            if let Some((pane_id, false)) = pane {
                let notice = vec!["printf".to_string(), "%s\\n".to_string(), LEGACY_NOTICE.to_string()];
                if let Err(error) = tmux.run(respawn_args(&pane_id, &notice)).await {
                    tracing::warn!(%session_id, %error, "plan agent restart reconcile: legacy kill failed; will retry next start");
                    continue;
                }
            }
            let _ = write_new_private(&paths.settled, b"legacy\n");
            continue;
        }
        let mut killed = false;
        if let Some((pane_id, false)) = pane {
            let notice = vec!["printf".to_string(), "%s\\n".to_string(), INTERRUPTED_NOTICE.to_string()];
            match tmux.run(respawn_args(&pane_id, &notice)).await {
                Ok(_) => killed = true,
                Err(error) => {
                    tracing::warn!(%session_id, %error, "plan agent restart reconcile: kill failed; will retry next start");
                    continue;
                }
            }
        }
        if let Err(error) = record_interrupted(state, &session_id) {
            tracing::warn!(%session_id, %error, "plan agent restart reconcile: message write failed");
            continue;
        }
        let _ = write_new_private(&paths.settled, b"interrupted\n");
        tracing::info!(%session_id, turn = n, killed, "plan agent turn interrupted by an engine restart");
        out.push(InterruptedTurn { session_id, n, killed });
    }
    out
}

/// Write the interrupted assistant message: fill the session's trailing empty assistant
/// placeholder (the host paths insert one before streaming) or insert a new row.
fn record_interrupted(state: &AppState, session_id: &str) -> Result<(), String> {
    state.db.with_conn(|conn| {
        let last: Option<(String, String, String)> = conn
            .query_row(
                "SELECT id, role, content FROM messages WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        match last {
            Some((id, role, content)) if role == "assistant" && content.trim().is_empty() => conn
                .execute(
                    "UPDATE messages SET content = ?1, finish_reason = 'interrupted' WHERE id = ?2",
                    params![INTERRUPTED_BY_RESTART, id],
                )
                .map(|_| ())
                .map_err(|e| e.to_string()),
            _ => conn
                .execute(
                    "INSERT INTO messages (id, session_id, role, content, finish_reason) VALUES (?1, ?2, 'assistant', ?3, 'interrupted')",
                    params![uuid::Uuid::new_v4().to_string(), session_id, INTERRUPTED_BY_RESTART],
                )
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    })
}

/// Run the reconcile once at engine start against the real tmux.
pub fn spawn_restart_reconcile(state: AppState) {
    // `tauri::async_runtime::spawn`: called from Tauri's `setup` (no tokio reactor entered).
    tauri::async_runtime::spawn(async move {
        let done = reconcile_interrupted_turns_with(&state, &LiveTmux).await;
        tracing::info!(interrupted = done.len(), "plan agent restart reconcile");
    });
}

/// The recording tmux fake shared by this module's and `plan_agent`'s tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::{TmuxFuture, TmuxRunner};
    use std::sync::Mutex;

    /// A fake tmux: records every call; `list-panes` answers from `panes` (one PANES_FORMAT
    /// listing). Spawns nothing.
    #[derive(Default)]
    pub(crate) struct FakeTmux {
        calls: Mutex<Vec<Vec<String>>>,
        panes: Mutex<String>,
    }

    impl TmuxRunner for FakeTmux {
        fn run(&self, args: Vec<String>) -> TmuxFuture<'_> {
            self.calls.lock().unwrap().push(args.clone());
            let out = if args.first().map(String::as_str) == Some("list-panes") {
                self.panes.lock().unwrap().clone()
            } else {
                String::new()
            };
            Box::pin(async move { Ok(out) })
        }
    }

    impl FakeTmux {
        pub(crate) fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
        pub(crate) fn set_panes(&self, s: &str) {
            *self.panes.lock().unwrap() = s.to_string();
        }
    }

}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::claude_code::{self, build_config_with};
    use crate::services::plan_agent::plan_agent_extras;
    use crate::services::plan_mcp::fixture::{plan, use_store};
    use crate::test_support::{test_state, tmp_dir};

    use super::fake::FakeTmux;

    const FAST: Duration = Duration::from_millis(5);

    /// A viewer that is never run (no identity → the script uses `cat`).
    fn nv(p: &str) -> Viewer {
        Viewer { path: p.to_string(), identity: None }
    }

    /// A recorded `claude --print --output-format stream-json --verbose` plan-agent turn (one tool
    /// call + result + final text), shaped like CLI 2.1.x output.
    const FIXTURE: &str = concat!(
        r#"{"type":"system","subtype":"init","session_id":"cli-1","model":"claude-opus","tools":[]}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me update the brief."}]},"session_id":"cli-1"}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"mcp__j1__brief_update","input":{"field":"why","add":"x"}}]},"session_id":"cli-1"}"#, "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"{\"version\":5,\"changed_fields\":[\"why\"]}"}]},"session_id":"cli-1"}"#, "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done. CHOICES: a | b | c"}]},"session_id":"cli-1"}"#, "\n",
        r#"{"type":"result","subtype":"success","is_error":false,"result":"Done.","session_id":"cli-1","total_cost_usd":0.0123,"usage":{"input_tokens":10,"output_tokens":20}}"#, "\n",
    );

    fn plain_cfg(prompt: &str) -> CliSpawnConfig {
        build_config_with(prompt, "/ws", "", None, Some("cli-1"), Some(&plan_agent::launch_extras(Path::new("/store/P/plan-agent/mcp.json"))))
    }

    async fn collect(mut rx: mpsc::Receiver<StreamChunk>) -> Vec<StreamChunk> {
        let mut v = Vec::new();
        while let Some(c) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("tail hung") {
            v.push(c);
        }
        v
    }

    fn as_json(chunks: &[StreamChunk]) -> Vec<serde_json::Value> {
        chunks.iter().map(|c| serde_json::to_value(c).unwrap()).collect()
    }

    // ── session name + lifecycle ──

    #[test]
    fn pane_session_name_is_johnnyone_uuid() {
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(pane_session_name(&id), format!("johnnyone_{id}"));
        assert_eq!(crate::terminal::owned_session_id_from_tmux_name(&pane_session_name(&id)), Some(id));
    }

    #[tokio::test]
    async fn first_turn_creates_session_later_turns_respawn() {
        let dir = tmp_dir("pat-turns").join("turns");
        let tmux = FakeTmux::default();
        let sid = uuid::Uuid::new_v4().to_string();
        let name = format!("johnnyone_{sid}");
        let p1 = start_turn(&tmux, &sid, &dir, plain_cfg("one"), &nv("/bin/viewer")).await.unwrap();
        assert_eq!(p1.n, 1);
        let calls = tmux.calls();
        let new = calls.iter().find(|c| c[0] == "new-session").expect("new-session");
        assert_eq!(new[3], name);
        let semi = new.iter().position(|a| a == ";").unwrap();
        assert_eq!(&new[semi - 2..semi], &["sh".to_string(), p1.script.to_string_lossy().to_string()]);
        assert_eq!(&new[semi + 1..], &["set-option", "-w", "-t", &format!("={name}:"), "remain-on-exit", "on"]);
        // The pane now exists (dead after turn 1): turn 2 respawns it, never a second session.
        tmux.set_panes(&format!("%7\t1\t{name}\n%3\t0\tj1\n"));
        let p2 = start_turn(&tmux, &sid, &dir, plain_cfg("two"), &nv("/bin/viewer")).await.unwrap();
        assert_eq!(p2.n, 2);
        let calls = tmux.calls();
        assert_eq!(calls.iter().filter(|c| c[0] == "new-session").count(), 1);
        let respawn = calls.iter().find(|c| c[0] == "respawn-pane").unwrap();
        assert_eq!(respawn, &respawn_args("%7", &pane_argv(&p2)));
        // Nothing ever targets a non-J1 session.
        assert!(calls.iter().flatten().all(|a| !a.contains("=j1") && a != "%3"));
    }

    #[test]
    fn pane_of_matches_exact_name_only() {
        let l = "%1\t0\tjohnnyone_abc-1\n%2\t1\tjohnnyone_abc\n%3\t0\tj1\n";
        assert_eq!(pane_of(l, "johnnyone_abc"), Some(("%2".into(), true)));
        assert_eq!(pane_of(l, "johnnyone_ab"), None);
        assert_eq!(pane_of(l, "j1"), Some(("%3".into(), false)));
    }

    // ── security: token + injection ──

    #[tokio::test]
    async fn turn_command_never_contains_token() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&extras.mcp_config_path).unwrap()).unwrap();
        let token = v["mcpServers"]["j1"]["headers"]["Authorization"].as_str().unwrap().trim_start_matches("Bearer ").to_string();
        let hash = mcp_tokens::sha256_hex(&token);
        let cfg = build_config_with("hello", &p.ws.to_string_lossy(), "", None, Some("cli-1"), Some(&extras));
        let tmux = Arc::new(FakeTmux::default());
        let (_proc, _rx) = spawn_plan_agent_turn_with(&state, tmux.clone(), cfg, p.session.clone(), claude_code::parse_line, None).await.unwrap();
        let calls = tmux.calls();
        let new = calls.iter().find(|c| c[0] == "new-session").expect("the turn ran in tmux");
        assert_eq!(new[3], format!("johnnyone_{}", p.session));
        let dir = turns_dir(&store, &p.id).unwrap();
        let paths = TurnPaths::new(&dir, 1);
        let script = std::fs::read_to_string(&paths.script).unwrap();
        let tmux_cmd = calls.iter().map(|c| c.join(" ")).collect::<Vec<_>>().join("\n");
        for secret in [&token, &hash] {
            assert!(!tmux_cmd.contains(secret.as_str()), "the tmux command string holds token material");
            assert!(!script.contains(secret.as_str()), "the turn script holds token material");
        }
        // The pane command carries the config PATH (inside the script), never the token.
        assert!(script.contains(&shell_quote(&extras.mcp_config_path)));
        assert!(script.contains("'--tools' ''"), "--tools \"\" survives into the pane command");
        assert!(script.contains("'--strict-mcp-config'"));
        assert!(script.contains("'--permission-mode' 'default'"));
        // turns/ is 0700, the files 0600.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        for f in [&paths.script, &paths.prompt, &paths.jsonl] {
            assert_eq!(std::fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600, "{}", f.display());
        }
        // The row records the pane name.
        let tmux_name: Option<String> = state
            .db
            .with_conn(|c| c.query_row("SELECT tmux_session_name FROM sessions WHERE id=?1", params![p.session], |r| r.get(0)).map_err(|e| e.to_string()))
            .unwrap();
        assert_eq!(tmux_name, Some(format!("johnnyone_{}", p.session)));
    }

    #[tokio::test]
    async fn prompt_with_shell_metacharacters_cannot_inject() {
        // Harmless payloads only: if quoting ever broke, the real `sh` run below would execute them.
        let evil = "x'; echo pwned; echo '\" $(echo subst-ran) `echo backtick-ran` ; | & > /dev/null\nnewline $HOME \\";
        let dir = tmp_dir("pat-inject").join("turns");
        let tmux = FakeTmux::default();
        let paths = start_turn(&tmux, "S", &dir, plain_cfg(evil), &nv("/bin/viewer")).await.unwrap();
        let script = std::fs::read_to_string(&paths.script).unwrap();
        // The text lives only in the prompt file, byte for byte; neither the script nor any tmux
        // argument contains any part of it.
        assert_eq!(std::fs::read_to_string(&paths.prompt).unwrap(), evil);
        for needle in ["pwned", "subst-ran", "backtick-ran", "newline", "$HOME"] {
            assert!(!script.contains(needle), "prompt text reached the script: {needle}");
            assert!(tmux.calls().iter().flatten().all(|a| !a.contains(needle)), "prompt text reached tmux argv");
        }
        assert!(script.contains(&format!("\"$(cat {})\"", shell_quote(&paths.prompt.to_string_lossy()))));
        // Run the generated command line through a real POSIX sh with `claude` swapped for printf:
        // the prompt must arrive as ONE argument, verbatim, with no side effect. (A plain `sh`
        // child, no tmux, no claude CLI.)
        let marker = tmp_dir("pat-inject-marker").join("pwned");
        let evil2 = format!("{evil}; touch {}", marker.display());
        let dir2 = tmp_dir("pat-inject2").join("turns");
        let mut cfg = plain_cfg(&evil2);
        cfg.command = "printf".into();
        cfg.args = vec!["[%s]\\n".into(), cfg.args.pop().unwrap()];
        cfg.working_directory = String::new();
        let p2 = start_turn(&FakeTmux::default(), "S", &dir2, cfg, &nv("/nonexistent/viewer")).await.unwrap();
        let out = std::process::Command::new("sh").arg(&p2.script).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let jsonl = std::fs::read_to_string(&p2.jsonl).unwrap();
        assert_eq!(jsonl, format!("[{evil2}]\n"), "one argument, verbatim");
        assert!(!marker.exists(), "the prompt executed a command");
        assert_eq!(std::fs::read_to_string(&p2.exit).unwrap().trim(), "0");
        // F-1-9: the pane echoes the user's first line, as data (printf argument), verbatim.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let first: String = evil2.lines().next().unwrap().chars().take(200).collect();
        assert!(stdout.contains(&format!("▶ you: {first}\n")), "{stdout}");
        // F-1-13: `umask 077` — every file the turn left behind is 0600, including the ones the
        // shell itself created (stderr, exit), whatever this process's umask is.
        use std::os::unix::fs::PermissionsExt;
        let mut names = Vec::new();
        for e in std::fs::read_dir(&dir2).unwrap() {
            let e = e.unwrap();
            names.push(e.file_name().to_string_lossy().to_string());
            assert_eq!(e.metadata().unwrap().permissions().mode() & 0o777, 0o600, "{:?}", e.path());
        }
        names.sort();
        assert_eq!(names, ["1.exit", "1.jsonl", "1.prompt", "1.sh", "1.started", "1.stderr"], "rc was renamed to exit");
    }

    /// F-1-16: the script execs the viewer only when the file at its path has the recorded identity
    /// AND answers the handshake; otherwise `cat`. Fake viewers are tiny sh scripts.
    #[tokio::test]
    async fn viewer_runs_only_after_identity_and_probe() {
        use std::os::unix::fs::PermissionsExt;
        let base = tmp_dir("pat-viewer");
        let marker = base.join("viewer-ran");
        let write_viewer = |name: &str, probe_answer: &str| {
            let path = base.join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\ncase \"$1\" in\n  {PROBE_FLAG}) echo '{probe_answer}' ;;\n  {VIEW_FLAG}) echo ran >> '{m}'; sed 's/^/V:/' ;;\n  *) echo ran >> '{m}' ;;\nesac\n",
                    m = marker.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let run = |viewer: Viewer| async move {
            let dir = tmp_dir("pat-viewer-turn").join("turns");
            let mut cfg = plain_cfg("hello");
            cfg.command = "printf".into();
            cfg.args = vec!["[%s]\\n".into(), cfg.args.pop().unwrap()];
            cfg.working_directory = String::new();
            let p = start_turn(&FakeTmux::default(), "S", &dir, cfg, &viewer).await.unwrap();
            let out = std::process::Command::new("sh").arg(&p.script).output().unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        // Good viewer: identity matches and the probe answers → the pane shows the viewer's output.
        let good = write_viewer("good", PROBE_TOKEN);
        let out = run(Viewer { path: good.to_string_lossy().into(), identity: file_identity(&good) }).await;
        assert!(out.contains("V:[hello]"), "{out}");
        std::fs::remove_file(&marker).unwrap();
        // Replaced on disk (identity differs) → `cat`, and the binary is never executed at all.
        let id_before = file_identity(&good);
        // (A "rollback" binary: running it at all — even just for the probe — would be the bug.)
        std::fs::write(&good, format!("#!/bin/sh\necho started >> '{}'\n# a different, longer binary\n", marker.display())).unwrap();
        let out = run(Viewer { path: good.to_string_lossy().into(), identity: id_before }).await;
        assert!(out.contains("[hello]") && !out.contains("V:"), "{out}");
        assert!(!marker.exists(), "a replaced viewer was executed");
        // Identity matches but the handshake fails (a binary that does not know the probe) → `cat`,
        // and it is never exec'd as the viewer.
        let old = write_viewer("old", "something else");
        let out = run(Viewer { path: old.to_string_lossy().into(), identity: file_identity(&old) }).await;
        assert!(out.contains("[hello]") && !out.contains("V:"), "{out}");
        assert!(!marker.exists(), "a viewer that failed the handshake was exec'd");
        // No identity → `cat`.
        let out = run(nv(&good.to_string_lossy())).await;
        assert!(out.contains("[hello]") && !out.contains("V:"), "{out}");
    }

    // ── F-2-21: no terminal escapes reach the pane ──

    #[test]
    fn view_lines_strip_terminal_controls() {
        let osc52 = "\u{1b}]52;c;QUJD\u{07}";
        let line = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"text","text": format!("hi {osc52} there \u{9b}31m red\tok")},
            {"type":"tool_use","name":"mcp__j1__brief_get","input":{"x": osc52}}
        ]}})
        .to_string();
        let result = serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","content": format!("bad {osc52}")}
        ]}})
        .to_string();
        let out: Vec<String> = [line.as_str(), result.as_str(), "raw \u{1b}[2J line"].iter().flat_map(|l| view_lines(l)).collect();
        assert!(!out.is_empty());
        for l in &out {
            assert!(!l.chars().any(|c| c.is_control() && c != '\t'), "control char in {l:?}");
        }
        assert!(out[0].contains("hi ]52;c;QUJD there 31m red\tok"), "{out:?}");
        assert_eq!(strip_controls("a\u{7f}b\u{85}c\u{1b}d\te"), "abcd\te");
    }

    #[tokio::test]
    async fn pane_shell_output_strips_terminal_controls() {
        // A "claude" that writes escape sequences to stdout AND stderr, then fails; and a prompt
        // that carries one too. Whatever the pane prints must contain no ESC/BEL/C1 — while the
        // jsonl data path keeps the bytes untouched. (A plain `sh` child, no tmux, no claude CLI.)
        let dir = tmp_dir("pat-esc").join("turns");
        let mut cfg = plain_cfg("hello \u{1b}]52;c;QUJD\u{07} you");
        let prompt = cfg.args.pop().unwrap();
        cfg.command = "sh".into();
        cfg.args = vec![
            "-c".into(),
            r#"printf 'boom \033]52;c;QUJD\007 \302\233x\n' >&2; printf '{"x":1} \033[2J\n'; exit 3"#.into(),
            prompt,
        ];
        cfg.working_directory = String::new();
        let p = start_turn(&FakeTmux::default(), "S", &dir, cfg, &nv("/nonexistent")).await.unwrap();
        let out = std::process::Command::new("sh").arg(&p.script).output().unwrap();
        let stdout = out.stdout;
        let text = String::from_utf8_lossy(&stdout);
        assert!(text.contains("boom ]52;c;QUJD x"), "the stderr tail is shown, sanitized: {text}");
        assert!(text.contains("▶ you: hello ]52;c;QUJD you"), "{text}");
        for bad in [0x1bu8, 0x07] {
            assert!(!stdout.contains(&bad), "byte {bad:#x} reached the pane: {text:?}");
        }
        assert!(!text.contains('\u{9b}'), "a C1 control reached the pane");
        // The data path is untouched.
        assert!(std::fs::read(&p.jsonl).unwrap().contains(&0x1b));
    }

    // ── F-1-1: one turn at a time ──

    #[tokio::test]
    async fn second_send_while_turn_in_flight_is_refused_and_first_unharmed() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let cfg = build_config_with("one", &p.ws.to_string_lossy(), "", None, None, Some(&extras));
        let tmux = Arc::new(FakeTmux::default());
        let (_proc, rx) = spawn_plan_agent_turn_with(&state, tmux.clone(), cfg.clone(), p.session.clone(), claude_code::parse_line, None).await.unwrap();
        // Turn 1 created the pane and it is running.
        tmux.set_panes(&format!("%4\t0\tjohnnyone_{}\n", p.session));
        // The chat paths' claim refuses before anything is stored…
        let e = crate::services::chat_host::claim_plan_agent_turn(&state, &p.session, "claude_code").err().expect("busy");
        assert_eq!(e, PLAN_AGENT_BUSY);
        // The Plans chat has no Stop control, so the text must not point at one.
        assert_eq!(e, "The plan agent is still answering the last message. Try again when it finishes.");
        // …and the spawn itself refuses (the authoritative claim).
        let Err(e) = spawn_plan_agent_turn_with(&state, tmux.clone(), cfg.clone(), p.session.clone(), claude_code::parse_line, None).await else {
            panic!("a second turn started while one was in flight");
        };
        assert_eq!(e, PLAN_AGENT_BUSY);
        // Turn 1 is unharmed: no respawn, no second turn's files, and it still completes.
        assert!(tmux.calls().iter().all(|c| c[0] != "respawn-pane"), "turn 1 was respawned");
        let dir = turns_dir(&store, &p.id).unwrap();
        assert_eq!(next_turn_number(&dir), 2);
        std::fs::write(TurnPaths::new(&dir, 1).jsonl, FIXTURE).unwrap();
        let chunks = collect(rx).await;
        assert_eq!(chunks.last().unwrap().chunk_type, ChunkType::Result);
        // Once turn 1 is over, the next send is accepted.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!turn_in_flight(&state, &p.session));
        assert!(crate::services::chat_host::claim_plan_agent_turn(&state, &p.session, "claude_code").unwrap().is_some());
    }

    #[test]
    fn concurrent_starts_never_share_a_turn_number() {
        // Deterministic: a start that read a stale number moves on instead of reusing it.
        let dir = tmp_dir("pat-reserve");
        let (a, _fa) = reserve_turn(&dir, 1).unwrap();
        let (b, _fb) = reserve_turn(&dir, 1).unwrap();
        assert_eq!((a.n, b.n), (1, 2));
        // Real concurrency: 8 threads start at once on one dir → 8 distinct numbers.
        let dir = tmp_dir("pat-race").join("turns");
        let ns: Vec<u32> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..8)
                .map(|i| {
                    let dir = dir.clone();
                    sc.spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                        rt.block_on(start_turn(&FakeTmux::default(), "S", &dir, plain_cfg(&format!("t{i}")), &nv("/v"))).unwrap().n
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut sorted = ns.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 8, "{ns:?}");
        // And each turn's prompt is its own.
        let prompts: std::collections::BTreeSet<String> =
            (1..=8).map(|n| std::fs::read_to_string(TurnPaths::new(&dir, n).prompt).unwrap()).collect();
        assert_eq!(prompts.len(), 8);
        // Two claims for one session: exactly one wins.
        let (state, _r) = test_state();
        let a = claim_turn(&state, "S1");
        let b = claim_turn(&state, "S1");
        assert!(a.is_ok() && b.is_err());
        drop(a);
        assert!(claim_turn(&state, "S1").is_ok(), "released on drop");
    }

    struct DummyTurn(Arc<AtomicBool>);
    impl ExternalTurn for DummyTurn {
        fn stop(&mut self) -> StopFuture<'_> {
            self.0.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
        fn is_running(&self) -> bool {
            !self.0.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn stale_turn_cleanup_never_touches_a_later_turn() {
        let (state, _r) = test_state();
        let stopped1 = Arc::new(AtomicBool::new(false));
        let stopped2 = Arc::new(AtomicBool::new(false));
        let t1 = CliProcess::external(Box::new(DummyTurn(stopped1.clone())), "S".into());
        let t2 = CliProcess::external(Box::new(DummyTurn(stopped2.clone())), "S".into());
        let (id1, id2) = (t1.turn_id(), t2.turn_id());
        assert_ne!(id1, id2);
        // Turn 1 was replaced by turn 2 in the map (e.g. after a Stop); turn 1 then times out.
        state.active_processes.lock().await.insert("S".into(), t2);
        drop(t1);
        assert!(crate::services::chat_host::take_own_process(&state.active_processes, "S", id1).await.is_none());
        assert_eq!(state.active_processes.lock().await.get("S").map(|p| p.turn_id()), Some(id2), "turn 2 still registered");
        assert!(!stopped2.load(Ordering::SeqCst), "turn 2 not stopped");
        // Turn 2's own cleanup takes it.
        assert!(crate::services::chat_host::take_own_process(&state.active_processes, "S", id2).await.is_some());
        assert!(state.active_processes.lock().await.is_empty());
    }

    #[tokio::test]
    async fn superseded_turn_tail_ends_even_with_a_live_pane() {
        let dir = tmp_dir("pat-super").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes("%1\t0\tjohnnyone_S\n"); // alive — but now running turn 2
        let (_p, rx, _paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("one"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        start_turn(tmux.as_ref(), "S", &dir, plain_cfg("two"), &nv("/v")).await.unwrap();
        let chunks = collect(rx).await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_type, ChunkType::Error);
    }

    #[test]
    fn shell_quote_round_trips() {
        for s in ["plain", "", "it's", "a'b'c", "$(x) `y` \"z\" \\ \n ; |"] {
            let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", shell_quote(s))).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), s);
        }
    }

    // ── data path ──

    #[tokio::test]
    async fn completion_detected_from_jsonl_result_event() {
        let dir = tmp_dir("pat-done").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        let (mut proc, rx, paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        assert!(proc.is_running());
        // The pane is alive the whole time and NO exit sidecar is written: only the result event
        // can complete the turn.
        tmux.set_panes("%1\t0\tjohnnyone_S\n");
        std::fs::write(&paths.jsonl, FIXTURE).unwrap();
        let chunks = collect(rx).await;
        let last = chunks.last().unwrap();
        assert_eq!(last.chunk_type, ChunkType::Result);
        assert!(last.is_final);
        assert_eq!(last.session_id.as_deref(), Some("cli-1"));
        assert!(!paths.exit.exists());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!proc.is_running(), "the turn is done once the result event is read");
    }

    #[tokio::test]
    async fn crash_without_result_is_an_error_with_last_stderr_line() {
        let dir = tmp_dir("pat-crash").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes("%1\t0\tjohnnyone_S\n");
        let (_proc, rx, paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        let partial: String = FIXTURE.lines().take(2).map(|l| format!("{l}\n")).collect();
        std::fs::write(&paths.jsonl, partial).unwrap();
        std::fs::write(&paths.stderr, "warming up\nError: MCP server j1 failed to start\n\n").unwrap();
        std::fs::write(&paths.exit, "1\n").unwrap();
        let chunks = collect(rx).await;
        assert_eq!(chunks.len(), 3, "{chunks:?}");
        assert_eq!(chunks[1].content, "Let me update the brief.");
        let err = chunks.last().unwrap();
        assert_eq!(err.chunk_type, ChunkType::Error);
        assert!(err.is_final);
        assert_eq!(err.content, "Plan agent turn failed (exit 1): Error: MCP server j1 failed to start");
        assert_eq!(crash_error(Some(2), ""), "Plan agent turn failed (exit 2) with no output.");
    }

    #[tokio::test]
    async fn pane_killed_mid_turn_is_an_error_not_a_hang() {
        let dir = tmp_dir("pat-gone").join("turns");
        let tmux = Arc::new(FakeTmux::default()); // no pane listed → gone
        let (_proc, rx, _paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        let chunks = collect(rx).await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_type, ChunkType::Error);
        assert!(chunks[0].content.contains("pane closed"), "{}", chunks[0].content);
    }

    #[tokio::test]
    async fn structured_parse_identical_to_headless_path() {
        let dir = tmp_dir("pat-parse").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes("%1\t0\tjohnnyone_S\n");
        let (_proc, rx, paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        // Written in uneven pieces (a line split mid-way) the way tee appends.
        let bytes = FIXTURE.as_bytes();
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&paths.jsonl).unwrap();
            for piece in bytes.chunks(37) {
                f.write_all(piece).unwrap();
                f.flush().unwrap();
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        std::fs::write(&paths.exit, "0\n").unwrap();
        let got = collect(rx).await;
        // The headless path: `spawn_cli` feeds each stdout line to the same parser.
        let headless: Vec<StreamChunk> = FIXTURE.lines().filter_map(claude_code::parse_line).collect();
        assert_eq!(as_json(&got), as_json(&headless));
        // And the folded tool calls match too.
        assert_eq!(crate::services::chat_host::fold_tool_calls(&got), crate::services::chat_host::fold_tool_calls(&headless));
    }

    #[tokio::test]
    async fn stop_cancels_tail_and_kills_only_the_agent_pane() {
        let dir = tmp_dir("pat-stop").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes("%9\t0\tjohnnyone_S\n%3\t0\tj1\n");
        let (mut proc, rx, _paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        proc.kill().await.unwrap();
        assert!(!proc.is_running());
        assert!(collect(rx).await.is_empty(), "a stopped turn just closes the stream");
        let respawns: Vec<_> = tmux.calls().into_iter().filter(|c| c[0] == "respawn-pane").collect();
        assert_eq!(respawns.last().unwrap()[3], "%9");
        assert!(respawns.iter().all(|c| c[3] == "%9"));
    }

    // ── follow-ups (brief plan-agent-followups) ──

    /// Run the host chat path for a plan agent against a fake tmux, in the background.
    fn host_turn(
        state: &AppState,
        tmux: Arc<FakeTmux>,
        sid: &str,
        text: &str,
        idle: Duration,
    ) -> tokio::task::JoinHandle<Result<crate::services::chat_host::ChatRunResult, String>> {
        let (st, sid, text) = (state.clone(), sid.to_string(), text.to_string());
        tokio::spawn(async move {
            crate::services::chat_host::send_chat_message_blocking_with(&st, tmux, sid, text, idle).await
        })
    }

    fn message_count(state: &AppState, sid: &str) -> i64 {
        state
            .db
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM messages WHERE session_id = ?1", params![sid], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    fn last_message(state: &AppState, sid: &str) -> (String, String, Option<String>) {
        state
            .db
            .with_conn(|c| {
                c.query_row(
                    "SELECT role, content, finish_reason FROM messages WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                    params![sid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    async fn wait_for(path: &Path) {
        for _ in 0..500 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{} never appeared", path.display());
    }

    /// G (F-3-37): the C1 strip loops — overlapping bytes cannot re-form a C1 after one pass.
    #[test]
    fn shell_sanitize_loops_until_no_c1_is_left() {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(r"printf 'x\302\302\233\233y' | {SHELL_SANITIZE}"))
            .output()
            .unwrap();
        assert!(out.status.success());
        let bytes = out.stdout;
        assert!(!bytes.windows(2).any(|w| w[0] == 0xc2 && (0x80..=0x9f).contains(&w[1])), "a C1 survived: {bytes:?}");
        assert_eq!(bytes, b"xy".to_vec());
    }

    /// C (F-2-19): a send refused because a turn holds the slot stores NOTHING — the claim comes
    /// before the user-message insert — and spawns nothing.
    #[tokio::test]
    async fn refused_send_stores_nothing() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let _held = claim_turn(&state, &p.session).unwrap(); // a turn in flight
        let tmux = Arc::new(FakeTmux::default());
        let e = host_turn(&state, tmux.clone(), &p.session, "second", Duration::from_secs(5)).await.unwrap().err().expect("busy");
        assert_eq!(e, PLAN_AGENT_BUSY);
        assert_eq!(message_count(&state, &p.session), 0, "a refused send stored a message");
        assert!(tmux.calls().is_empty(), "a refused send touched tmux");
        assert_eq!(next_turn_number(&turns_dir(&store, &p.id).unwrap()), 1, "no turn files");
    }

    /// The body of the function whose signature starts with `sig` (brace-matched). Panics when the
    /// signature is missing, so a renamed function fails the pin instead of skipping it.
    fn fn_body<'a>(src: &'a str, sig: &str) -> &'a str {
        let at = src.find(sig).unwrap_or_else(|| panic!("signature `{sig}` not found"));
        let open = at + src[at..].find('{').expect("body brace");
        let mut depth = 0i32;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &src[at..open + i + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced body for `{sig}`");
    }

    fn pos(body: &str, needle: &str, name: &str) -> usize {
        body.find(needle).unwrap_or_else(|| panic!("{name}: `{needle}` not found in the function body"))
    }

    /// C (G-1-8): every chat path claims the slot before it stores the user message (the Tauri
    /// command and the relay are not unit-runnable — no AppHandle / socket — so their ORDER is
    /// pinned, anchored to each function's own body; every anchor must exist).
    #[test]
    fn all_chat_paths_claim_before_storing() {
        for (name, src, sig, insert) in [
            (
                "chat_host",
                include_str!("chat_host.rs"),
                "pub(crate) async fn send_chat_message_blocking_with(",
                "'user', ?3)",
            ),
            ("commands/chat", include_str!("../commands/chat.rs"), "pub async fn send_chat_message(", "'user', ?3)"),
            ("relay", include_str!("../agent/mod.rs"), "async fn handle_relay_chat(", "'user', ?3)"),
        ] {
            let body = fn_body(src, sig);
            let claim = pos(body, "let plan_turn", name);
            assert!(body[claim..claim + 200].contains("claim_plan_agent_turn("), "{name}: plan_turn is the claim");
            let spawn = claim + pos(&body[claim..], "spawn_chat_turn", name);
            assert!(body[claim..spawn].contains(".map(|t| &t.extras)"), "{name}: extras come from the claim");
            assert!(body[spawn..].contains("plan_turn.map(|t| t.slot)"), "{name}: the slot goes into the spawn");
            let stored = pos(body, insert, name);
            assert!(claim < stored, "{name}: user message stored before the claim");
            assert_eq!(body.matches("plan_agent_turn_extras(").count(), 0, "{name}: bypasses the claim");
        }
    }

    /// D + F wiring (G-1-6 b/c): the relay loop and the Tauri command use the shared idle-timeout,
    /// own-turn kill and final-content helpers (the host path is covered behaviourally above).
    #[test]
    fn relay_and_command_paths_use_the_shared_turn_helpers() {
        // The relay's turn loop is the `stream_relay_turn` seam (live UI bugs 2, item 1; its
        // behaviour is unit-tested in `agent::relay_turn_tests`); `handle_relay_chat` calls it with
        // the shared idle timeout, then stores.
        // The store is the `persist` hook the seam calls once the turn is over (L-1-5: before the
        // final message is published).
        let handler = fn_body(include_str!("../agent/mod.rs"), "async fn handle_relay_chat(");
        let hook = pos(handler, "let persist = |o: &RelayTurnOutcome| {", "relay");
        let store = pos(handler, "INSERT INTO messages (id, session_id, role, content, finish_reason", "relay");
        let call = pos(handler, "Self::stream_relay_turn(", "relay");
        assert!(handler[call..].contains("chat_host::TURN_IDLE_TIMEOUT"), "relay: the shared idle timeout");
        assert!(handler[call..].contains("persist,"), "relay: the store is the seam persist hook");
        assert!(hook < store && store < call, "relay: the store lives in the persist hook");
        let relay = fn_body(include_str!("../agent/mod.rs"), "pub(crate) async fn stream_relay_turn<");
        let recv = pos(relay, "chat_host::recv_turn_chunk(", "relay");
        let timed_out = pos(relay, "TurnRecv::TimedOut => {", "relay");
        assert!(relay[timed_out..timed_out + 300].contains("should_kill_process = true;"), "relay: a timeout kills");
        let end = pos(relay, "chat_host::end_own_turn(&state.active_processes, &req.session_id, turn_id, should_kill_process)", "relay");
        let fin = pos(relay, "chat_host::final_assistant_content(", "relay");
        let persisted = pos(relay, "persist(&outcome);", "relay");
        let message = pos(relay, "AgentMessage::RelayChatMessage(", "relay");
        assert!(recv < end && end < fin && fin < persisted && persisted < message, "relay: recv → end_own_turn → final content → store → final message");
        for body in [handler, relay] {
            assert!(!body.contains("while let Some(chunk) = rx.recv().await"), "relay: an unbounded wait came back");
            assert!(!body.contains("take_own_process("), "relay: bypasses end_own_turn");
        }

        let cmd = fn_body(include_str!("../commands/chat.rs"), "pub async fn send_chat_message(");
        // G-2-15: the idle-timeout arm sets the kill flag, exactly like the relay.
        let arm = pos(cmd, "Err(_) => {", "commands/chat");
        let arm_body = &cmd[arm..arm + 400];
        assert!(arm_body.contains("finish_reason = \"timeout\".to_string();"), "commands/chat: not the timeout arm");
        assert!(arm_body.contains("should_kill_process = true;"), "commands/chat: a timeout must kill the turn");
        let t = pos(cmd, "chat_host::TURN_IDLE_TIMEOUT", "commands/chat");
        let end = pos(cmd, "chat_host::end_own_turn(&active_processes, &sid, turn_id, should_kill_process)", "commands/chat");
        let fin = pos(cmd, "chat_host::final_assistant_content(", "commands/chat");
        let store = pos(cmd, "UPDATE messages SET content = ?1", "commands/chat");
        assert!(t < end && end < fin && fin < store, "commands/chat: timeout → end_own_turn → final content → store");
    }

    /// F (F-1-10): text streamed, then the turn crashed — the stored reply keeps the text AND the
    /// crash line.
    #[tokio::test]
    async fn crash_after_partial_text_keeps_the_crash_line() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes(&format!("%4\t0\tjohnnyone_{}\n", p.session));
        let h = host_turn(&state, tmux.clone(), &p.session, "hi", Duration::from_secs(10));
        let t = TurnPaths::new(&turns_dir(&store, &p.id).unwrap(), 1);
        wait_for(&t.script).await;
        let partial: String = FIXTURE.lines().take(2).map(|l| format!("{l}\n")).collect();
        std::fs::write(&t.jsonl, partial).unwrap();
        std::fs::write(&t.stderr, "Error: MCP server j1 failed to start\n").unwrap();
        std::fs::write(&t.exit, "1\n").unwrap();
        let r = h.await.unwrap().unwrap();
        let content = r.assistant_message.content;
        assert!(content.starts_with("Let me update the brief."), "{content}");
        assert!(content.ends_with("Plan agent turn failed (exit 1): Error: MCP server j1 failed to start"), "{content}");
        assert_eq!(r.assistant_message.finish_reason.as_deref(), Some("error"));
        // The pure rule all 3 paths share.
        use crate::services::chat_host::final_assistant_content as fin;
        assert_eq!(fin("text", Some("boom"), &[], "error"), ("text\n\nboom".into(), "error".into()));
        assert_eq!(fin("text", None, &[], "stop"), ("text".into(), "stop".into()));
        assert_eq!(fin("", Some("boom"), &[], "stop"), ("boom".into(), "error".into()));
        assert_eq!(fin("a boom", Some("boom"), &[], "error").0, "a boom", "never twice");
        assert_eq!(fin("text", Some("t/o"), &[], "timeout").1, "timeout");
        assert!(fin("", None, &["e1".into()], "stop").0.contains("Recent stderr:\ne1"));
    }

    /// D + H (F-1-4, F-2-20, F-2-23): an idle-timed-out pane turn is killed through
    /// `PaneTurn::stop` on the agent's OWN pane, and that releases the real slot.
    #[tokio::test]
    async fn idle_timeout_stops_the_agents_pane_and_frees_the_slot() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes(&format!("%3\t0\tj1\n%7\t0\tjohnnyone_{}\n", p.session));
        let r = host_turn(&state, tmux.clone(), &p.session, "hi", Duration::from_millis(300)).await.unwrap().unwrap();
        assert_eq!(r.assistant_message.content, crate::services::chat_host::TURN_TIMEOUT_MESSAGE);
        assert_eq!(r.assistant_message.finish_reason.as_deref(), Some("timeout"));
        let respawns: Vec<Vec<String>> = tmux.calls().into_iter().filter(|c| c[0] == "respawn-pane").collect();
        // [0] starts the turn in the existing pane; [1] is the stop.
        let stop = respawns.last().unwrap();
        assert_eq!(&stop[..4], &["respawn-pane", "-k", "-t", "%7"]);
        assert!(stop.join(" ").contains("turn stopped by J1"), "{stop:?}");
        assert!(respawns.iter().all(|c| c[3] == "%7"), "another pane was touched: {respawns:?}");
        assert!(state.active_processes.lock().await.get(&p.session).is_none());
        // The tail saw the cancel and dropped the slot: a new claim succeeds.
        for _ in 0..100 {
            if !turn_in_flight(&state, &p.session) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(claim_turn(&state, &p.session).is_ok(), "the timed-out turn still holds the slot");
    }

    /// G-1-12: a chat to a NON-plan session whose job ended (a finished worker, or an archived row)
    /// is refused before anything is stored or spawned — the single entry for all 3 chat paths.
    #[tokio::test]
    async fn chat_to_a_finished_or_archived_session_is_refused_and_spawns_nothing() {
        let (state, _r) = test_state();
        let mk = || {
            crate::services::sessions::create_session(
                &state,
                crate::db::models::CreateSessionInput {
                    // `shell`: if the gate were missing, the host path would store the message and
                    // then fail on the shell check — never a real CLI.
                    provider: Some("shell".into()),
                    model: None,
                    working_directory: None,
                    title: Some("T1 Worker - x".into()),
                    kind: Some("agent".into()),
                    setup_commands: None,
                    tmux_session_name: None,
                },
            )
            .unwrap()
            .id
        };
        let worker = mk();
        let plan_id = uuid::Uuid::new_v4().to_string();
        state
            .db
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_session_id, \
                     worker_provider, reviewer_provider, initiative_id, initiative_status) \
                     VALUES (?1, 'development', 't', '/w', '/p', 'approved', ?2, 'claude_code', 'claude_code', ?1, 'development')",
                    params![plan_id, worker],
                )
                .map_err(|e| e.to_string())?;
                c.execute(
                    "INSERT INTO agent_plan_phases (id, plan_id, phase_id, phase_title, phase_index, status) VALUES ('ph1', ?1, '01-p', 'x', 0, 'passed')",
                    params![plan_id],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        let archived = mk();
        crate::services::sessions::mark_session_archived(&state, &archived).unwrap();
        // G-2-21: the job-ended text for the finished worker (archived or not); a plain archived
        // chat gets the generic archived text.
        let archived_worker = mk();
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET reviewer_session_id = ?1 WHERE id = ?2", params![archived_worker, plan_id]).map_err(|e| e.to_string()))
            .unwrap();
        crate::services::sessions::mark_session_archived(&state, &archived_worker).unwrap();
        for (sid, want) in [
            (&worker, crate::services::chat_host::CHAT_SESSION_ENDED),
            (&archived_worker, crate::services::chat_host::CHAT_SESSION_ENDED),
            (&archived, crate::services::chat_host::CHAT_SESSION_ARCHIVED),
        ] {
            let tmux = Arc::new(FakeTmux::default());
            let e = host_turn(&state, tmux.clone(), sid, "hello", Duration::from_secs(5)).await.unwrap().err().expect("refused");
            assert_eq!(e, want);
            assert_eq!(message_count(&state, sid), 0, "a refused chat stored a message");
            assert!(tmux.calls().is_empty());
            assert!(crate::services::chat_host::claim_plan_agent_turn(&state, sid, "claude_code").is_err());
        }
        // G-2-20: a CLOSED plan's briefing session (its retire failed) is refused, not chatted to.
        let (st2, _r2) = test_state();
        let store = use_store(&st2);
        let p = plan(&st2, &store, "Closed").await;
        st2.db
            .with_conn(|c| c.execute("UPDATE agent_plans SET status = 'closed' WHERE id = ?1", params![p.id]).map_err(|e| e.to_string()))
            .unwrap();
        let tmux = Arc::new(FakeTmux::default());
        let e = host_turn(&st2, tmux.clone(), &p.session, "hello", Duration::from_secs(5)).await.unwrap().err().expect("refused");
        assert_eq!(e, crate::services::chat_host::CHAT_PLAN_AGENT_RETIRED);
        assert_eq!(message_count(&st2, &p.session), 0);
        assert!(tmux.calls().is_empty());
        // A live standalone session and a not-yet-known relay session still chat.
        let live = mk();
        assert!(crate::services::chat_host::claim_plan_agent_turn(&state, &live, "claude_code").unwrap().is_none());
        assert!(crate::services::chat_host::claim_plan_agent_turn(&state, "no-such-row", "claude_code").unwrap().is_none());
    }

    /// H (F-2-20): Stop (`stop_generation`, also used by retire) releases the REAL slot.
    #[tokio::test]
    async fn stop_releases_the_real_slot() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let extras = plan_agent_extras(&state, &p.session).unwrap().unwrap();
        let cfg = build_config_with("one", &p.ws.to_string_lossy(), "", None, None, Some(&extras));
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes(&format!("%5\t0\tjohnnyone_{}\n", p.session));
        let (proc, _rx) = spawn_plan_agent_turn_with(&state, tmux.clone(), cfg, p.session.clone(), claude_code::parse_line, None).await.unwrap();
        state.active_processes.lock().await.insert(p.session.clone(), proc);
        assert!(claim_turn(&state, &p.session).is_err(), "held while the turn runs");
        crate::services::chat_host::stop_generation(&state, p.session.clone()).await.unwrap();
        for _ in 0..100 {
            if !turn_in_flight(&state, &p.session) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(claim_turn(&state, &p.session).is_ok(), "Stop did not release the slot");
        let dir = turns_dir(&store, &p.id).unwrap();
        assert!(TurnPaths::new(&dir, 1).settled.exists(), "a followed-to-the-end turn is settled");
    }

    /// H (F-1-2): a final `result` line written WITHOUT a trailing newline, then `.exit`, is still
    /// read (flush_rest) as the Result chunk — not a crash.
    #[tokio::test]
    async fn partial_final_result_line_plus_exit_is_a_result() {
        let dir = tmp_dir("pat-partial").join("turns");
        let tmux = Arc::new(FakeTmux::default());
        tmux.set_panes("%1\t0\tjohnnyone_S\n");
        let (_p, rx, paths) =
            spawn_turn_with(tmux.clone(), "S", &dir, plain_cfg("hi"), &nv("/v"), claude_code::parse_line, FAST, None).await.unwrap();
        std::fs::write(&paths.jsonl, FIXTURE.trim_end_matches('\n')).unwrap();
        std::fs::write(&paths.exit, "0\n").unwrap();
        let chunks = collect(rx).await;
        let last = chunks.last().unwrap();
        assert_eq!(last.chunk_type, ChunkType::Result, "{chunks:?}");
        assert!(last.is_final);
        assert!(chunks.iter().all(|c| c.chunk_type != ChunkType::Error));
    }

    /// E (F-1-3): engine restart mid-turn.
    #[tokio::test]
    async fn restart_reconcile_records_interrupted_turns_once() {
        let (state, _r) = test_state();
        let store = use_store(&state);
        // a: pane alive, turn in flight → killed + recorded (fills the host placeholder row).
        // b: pane gone → recorded, nothing to kill. c: finished turn (exit). d: settled turn.
        let (a, b, c, d) = (plan(&state, &store, "A").await, plan(&state, &store, "B").await, plan(&state, &store, "C").await, plan(&state, &store, "D").await);
        let start = |p: &crate::services::plan_mcp::fixture::Plan| {
            let dir = turns_dir(&store, &p.id).unwrap();
            let paths = TurnPaths::new(&dir, 1);
            (dir, paths)
        };
        for p in [&a, &b, &c, &d] {
            let (dir, _) = start(p);
            start_turn(&FakeTmux::default(), &p.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        }
        std::fs::write(&start(&c).1.exit, "0\n").unwrap();
        std::fs::write(&start(&d).1.settled, "tail\n").unwrap();
        state
            .db
            .with_conn(|conn| {
                conn.execute("INSERT INTO messages (id, session_id, role, content) VALUES ('u1', ?1, 'user', 'hi')", params![a.session]).map_err(|e| e.to_string())?;
                conn.execute("INSERT INTO messages (id, session_id, role, content) VALUES ('a1', ?1, 'assistant', '')", params![a.session]).map_err(|e| e.to_string())
            })
            .unwrap();
        let tmux = FakeTmux::default();
        tmux.set_panes(&format!("%2\t0\tjohnnyone_{}\n%3\t1\tjohnnyone_{}\n%9\t0\tj1\n", a.session, b.session));

        let got = reconcile_interrupted_turns_with(&state, &tmux).await;
        let mut got_ids: Vec<(String, bool)> = got.iter().map(|t| (t.session_id.clone(), t.killed)).collect();
        got_ids.sort();
        let mut want = vec![(a.session.clone(), true), (b.session.clone(), false)];
        want.sort();
        assert_eq!(got_ids, want);
        let respawns: Vec<Vec<String>> = tmux.calls().into_iter().filter(|c| c[0] == "respawn-pane").collect();
        assert_eq!(respawns.len(), 1);
        assert_eq!(&respawns[0][..4], &["respawn-pane", "-k", "-t", "%2"]);
        assert!(respawns[0].join(" ").contains("interrupted by an engine restart"));
        // a: the placeholder row now carries the failure; b: a new assistant row.
        assert_eq!(message_count(&state, &a.session), 2);
        assert_eq!(last_message(&state, &a.session), ("assistant".into(), INTERRUPTED_BY_RESTART.into(), Some("interrupted".into())));
        assert_eq!(last_message(&state, &b.session), ("assistant".into(), INTERRUPTED_BY_RESTART.into(), Some("interrupted".into())));
        assert_eq!(message_count(&state, &c.session) + message_count(&state, &d.session), 0);
        // The files stay; nothing is held.
        let (_, pa) = start(&a);
        assert!(pa.script.exists() && pa.jsonl.exists() && pa.prompt.exists() && !pa.exit.exists());
        assert!(!turn_in_flight(&state, &a.session));
        // Idempotent: a second start records nothing and kills nothing.
        let calls = tmux.calls().len();
        assert!(reconcile_interrupted_turns_with(&state, &tmux).await.is_empty());
        assert_eq!(tmux.calls().len(), calls);
        assert_eq!(message_count(&state, &b.session), 1);
        // G-1-7: a legacy turn (an older binary: no `.started`) is settled once, silently.
        let f = plan(&state, &store, "F").await;
        let (dir, pf) = start(&f);
        start_turn(&FakeTmux::default(), &f.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        std::fs::remove_file(&pf.started).unwrap();
        let calls = tmux.calls().len();
        assert!(reconcile_interrupted_turns_with(&state, &tmux).await.is_empty());
        assert!(pf.settled.exists(), "legacy turn settled");
        assert_eq!(message_count(&state, &f.session), 0, "no false 'interrupted' for a legacy turn");
        assert!(
            tmux.calls()[calls..].iter().all(|c| c[0] != "respawn-pane"),
            "a legacy turn without a live pane: nothing killed"
        );
        // G-2-16: a legacy turn whose pane is STILL RUNNING is killed silently, then settled.
        let l = plan(&state, &store, "L").await;
        let (dir, pl) = start(&l);
        start_turn(&FakeTmux::default(), &l.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        std::fs::remove_file(&pl.started).unwrap();
        let live_tmux = FakeTmux::default();
        live_tmux.set_panes(&format!("%8\t0\tjohnnyone_{}\n", l.session));
        assert!(reconcile_interrupted_turns_with(&state, &live_tmux).await.is_empty(), "nothing recorded");
        let kills: Vec<Vec<String>> = live_tmux.calls().into_iter().filter(|c| c[0] == "respawn-pane").collect();
        assert_eq!(kills.len(), 1);
        assert_eq!(&kills[0][..4], &["respawn-pane", "-k", "-t", "%8"]);
        assert!(pl.settled.exists());
        assert_eq!(message_count(&state, &l.session), 0);
        // G-1-7: a plan that left briefing (accepted) or whose agent is retired gets nothing.
        let g = plan(&state, &store, "G").await;
        let (dir, pg) = start(&g);
        start_turn(&FakeTmux::default(), &g.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET initiative_status = 'planning' WHERE id = ?1", params![g.id]).map_err(|e| e.to_string()))
            .unwrap();
        let h = plan(&state, &store, "H").await;
        let (dir, ph) = start(&h);
        start_turn(&FakeTmux::default(), &h.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        crate::services::sessions::mark_session_archived(&state, &h.session).unwrap();
        assert!(reconcile_interrupted_turns_with(&state, &tmux).await.is_empty());
        assert_eq!(message_count(&state, &g.session) + message_count(&state, &h.session), 0);
        assert!(!pg.settled.exists() && !ph.settled.exists(), "not touched at all");
        assert!(tmux.calls()[calls..].iter().all(|c| c[0] != "respawn-pane"), "nothing killed");
        // A turn this engine is running (slot held) is never touched.
        let e = plan(&state, &store, "E").await;
        let (dir, _) = start(&e);
        start_turn(&FakeTmux::default(), &e.session, &dir, plain_cfg("hi"), &nv("/v")).await.unwrap();
        let _held = claim_turn(&state, &e.session).unwrap();
        assert!(reconcile_interrupted_turns_with(&state, &tmux).await.is_empty());
        assert_eq!(message_count(&state, &e.session), 0);
    }

    #[test]
    fn line_buffer_splits_and_keeps_partial() {
        let mut b = LineBuffer::default();
        assert_eq!(b.push(b"ab"), Vec::<String>::new());
        assert_eq!(b.push(b"c\nde\r\nf"), vec!["abc".to_string(), "de".to_string()]);
        assert_eq!(b.take_rest(), Some("f".to_string()));
        assert_eq!(b.take_rest(), None);
    }

    #[test]
    fn next_turn_number_counts_up() {
        let dir = tmp_dir("pat-n");
        assert_eq!(next_turn_number(&dir.join("missing")), 1);
        std::fs::write(dir.join("1.jsonl"), "").unwrap();
        std::fs::write(dir.join("3.sh"), "").unwrap();
        assert_eq!(next_turn_number(&dir), 4);
    }

    // ── pane view ──

    #[test]
    fn view_lines_render_text_tools_and_versions() {
        let all: Vec<String> = FIXTURE.lines().flat_map(view_lines).collect();
        assert_eq!(
            all,
            vec![
                "● session ready (model claude-opus)",
                "Let me update the brief.",
                r#"→ brief_update({"add":"x","field":"why"})"#,
                "✓ v5",
                "Done. CHOICES: a | b | c",
                "── done · $0.0123 ──",
            ]
        );
        assert_eq!(view_lines("not json"), vec!["not json"]);
        assert!(view_lines("").is_empty());
        let err = r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":[{"type":"text","text":"no change"}]}]}}"#;
        assert_eq!(view_lines(err), vec!["✗ no change"]);
    }

    #[test]
    fn turn_script_pipes_through_tee_and_viewer() {
        let paths = TurnPaths::new(Path::new("/s/P/plan-agent/turns"), 4);
        let (cfg, prompt) = split_prompt(plain_cfg("hello"));
        assert_eq!(prompt, "hello");
        let s = turn_script(&cfg, &paths, &Viewer { path: "/opt/j1/johnnyone-desktop".into(), identity: Some("1:2:3:4".into()) });
        assert!(s.contains("| tee -a '/s/P/plan-agent/turns/4.jsonl' |"));
        assert!(s.contains("exec '/opt/j1/johnnyone-desktop' --plan-agent-view"));
        assert!(s.contains("'--resume' 'cli-1'"));
        assert!(s.contains("unset CLAUDECODE"));
        assert!(s.contains("2>>'/s/P/plan-agent/turns/4.stderr'"));
        assert!(s.starts_with("#!/bin/sh\numask 077\n"), "umask first");
        assert!(s.contains("</dev/null 2>>'/s/P/plan-agent/turns/4.stderr'"), "claude's stdin is /dev/null");
        assert!(s.contains("= '1:2:3:4' ] && [ \"$('/opt/j1/johnnyone-desktop' --plan-agent-view-probe </dev/null 2>/dev/null)\" = 'j1-plan-agent-view:1' ]"));
        assert!(!s.contains("'hello'"), "the prompt is never in the script");
    }
}
