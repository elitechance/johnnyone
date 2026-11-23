//! Per-session MCP bearer tokens for the plan agent (Plans tab, initiative 8ad3ae71 phase 02 task 01;
//! brief §"MCP security" 1, 2 and 4; master D5).
//!
//! A token is 256 bits from the OS RNG, hex-encoded (64 lowercase chars). It is bound to ONE plan
//! (`initiative_id`) and ONE chat session (`session_id`, the plan's `briefing_session_id`) with the
//! role `plan_agent`. Only `sha256_hex(raw)` is stored in `mcp_tokens` (migration 031); the raw
//! token exists only:
//! - in memory, inside a [`RawToken`] (whose `Debug` is redacted and which has no `Display` /
//!   `Serialize`), between [`mint`] and [`write_mcp_config`], and
//! - in `<initiatives_dir>/<id>/plan-agent/mcp.json`, a `0600` file in a `0700` per-plan directory,
//!   inside `mcpServers.j1.headers.Authorization` — the only place the claude CLI reads it from.
//!
//! It is never put on a command line, in an env var, in a log line or in an event. No `tracing` call
//! in this module takes a `RawToken` or a hash.
//!
//! Lifecycle: [`mint`] revokes any live token of the plan in the same transaction (at most one live
//! token per plan — also enforced by the partial unique index `idx_mcp_tokens_active`);
//! [`revoke_for_initiative`] / [`revoke_for_session`] set `revoked_at` and delete the config file
//! (idempotent). [`verify`] maps a presented token to its [`TokenScope`] or a [`Reject`] reason.

use crate::services::settings as settings_service;
use crate::state::app_state::AppState;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

/// The only role a token can carry today (the `mcp_tokens.role` CHECK constraint).
pub const ROLE_PLAN_AGENT: &str = "plan_agent";

/// Length of a hex-encoded 256-bit token.
pub const TOKEN_HEX_LEN: usize = 64;

/// Name of the per-plan directory holding the agent's MCP config.
pub const PLAN_AGENT_DIR: &str = "plan-agent";

/// Name of the MCP config file inside [`PLAN_AGENT_DIR`].
pub const MCP_CONFIG_FILE: &str = "mcp.json";

/// Default bound address of the local host API (see `main.rs`).
const DEFAULT_HOST_PORT: u16 = 7788;

// ───────────────────────────── types ─────────────────────────────

/// A raw token. Deliberately NO `Display`, `Serialize` or `Clone`; `Debug` prints `RawToken(…)`, so
/// an accidental `{:?}` in a log line or an error cannot leak it. Read it only with [`RawToken::expose`]
/// at the single point it is written into the config file.
pub struct RawToken(String);

impl RawToken {
    /// The raw token text. Callers: [`write_mcp_config`] and tests.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl RawToken {
    /// Test-only: wrap a known string (production tokens come only from [`mint`]).
    pub(crate) fn for_test(raw: String) -> Self {
        RawToken(raw)
    }
}

impl std::fmt::Debug for RawToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RawToken(…)")
    }
}

/// What a valid token grants: one plan, one session, one role. Passed into every MCP tool; no tool
/// ever reads an initiative id from its arguments (brief §"MCP security" 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenScope {
    pub initiative_id: String,
    pub session_id: String,
    pub role: String,
}

/// Why a presented token was refused. The caller always gets one undistinguished `401`; the reason
/// goes only to the audit (task 06). `Revoked` still resolves to its plan so the rejection can be
/// recorded on that plan's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    /// Not exactly 64 hex characters.
    Malformed,
    /// Well-formed, but no row has this hash.
    Unknown,
    /// The row exists but `revoked_at` is set.
    Revoked { initiative_id: String },
}

impl Reject {
    /// Fixed audit reason string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Reject::Malformed => "malformed_token",
            Reject::Unknown => "unknown_token",
            Reject::Revoked { .. } => "revoked",
        }
    }
}

// ───────────────────────────── pure helpers ─────────────────────────────

/// Lowercase hex SHA-256 of `raw`.
pub fn sha256_hex(raw: &str) -> String {
    hex_lower(&Sha256::digest(raw.as_bytes()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// True when `s` is exactly [`TOKEN_HEX_LEN`] ASCII hex digits.
pub fn is_well_formed(s: &str) -> bool {
    s.len() == TOKEN_HEX_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 32 bytes from the OS RNG → 64 lowercase hex chars.
fn generate() -> Result<RawToken, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("OS random source failed: {}", e))?;
    Ok(RawToken(hex_lower(&bytes)))
}

/// The port of the local host API, from `JOHNNYONE_HOST_ADDR` (the same variable `main.rs` binds),
/// default 7788.
pub fn host_port() -> u16 {
    port_from_addr(std::env::var("JOHNNYONE_HOST_ADDR").ok().as_deref())
}

/// Pure: the port of an `ip:port` address, or the default 7788 when absent/unparseable.
pub fn port_from_addr(addr: Option<&str>) -> u16 {
    addr.and_then(|a| a.trim().rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(DEFAULT_HOST_PORT)
}

/// A plan id is used as a path segment; refuse anything that could leave `<initiatives_dir>`.
fn safe_segment(id: &str) -> Result<&str, String> {
    let ok = !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0');
    if ok {
        Ok(id)
    } else {
        Err("invalid initiative id for the plan-agent directory".to_string())
    }
}

/// `<initiatives_dir>/<id>/plan-agent` — per plan, never named after token material.
pub fn plan_agent_dir(initiatives_dir: &Path, initiative_id: &str) -> Result<PathBuf, String> {
    Ok(initiatives_dir.join(safe_segment(initiative_id)?).join(PLAN_AGENT_DIR))
}

/// `<initiatives_dir>/<id>/plan-agent/mcp.json`.
pub fn mcp_config_path(initiatives_dir: &Path, initiative_id: &str) -> Result<PathBuf, String> {
    Ok(plan_agent_dir(initiatives_dir, initiative_id)?.join(MCP_CONFIG_FILE))
}

/// The config file body: exactly one `type:"http"` server named `j1` whose only secret is the
/// `Authorization` header.
fn mcp_config_json(port: u16, raw: &RawToken) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            "j1": {
                "type": "http",
                "url": format!("http://127.0.0.1:{}/mcp", port),
                "headers": { "Authorization": format!("Bearer {}", raw.expose()) }
            }
        }
    })
}

// ───────────────────────────── store ─────────────────────────────

/// Mint a fresh token for `(initiative_id, session_id)`. In ONE transaction: revoke every live token
/// of the plan (a replaced session's token dies here), then insert the new hash. Returns the raw
/// token, which the caller must write with [`write_mcp_config`] and then drop.
pub fn mint(state: &AppState, initiative_id: &str, session_id: &str) -> Result<RawToken, String> {
    let raw = generate()?;
    let hash = sha256_hex(raw.expose());
    state.db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE mcp_tokens SET revoked_at = datetime('now') WHERE initiative_id = ?1 AND revoked_at IS NULL",
            params![initiative_id],
        )
        .map_err(|e| format!("Failed to revoke the previous MCP token: {}", e))?;
        tx.execute(
            "INSERT INTO mcp_tokens (token_hash, session_id, initiative_id, role) VALUES (?1, ?2, ?3, ?4)",
            params![hash, session_id, initiative_id, ROLE_PLAN_AGENT],
        )
        .map_err(|e| format!("Failed to store the MCP token: {}", e))?;
        tx.commit().map_err(|e| e.to_string())
    })?;
    tracing::info!(target: "mcp", %initiative_id, %session_id, "plan-agent MCP token minted");
    Ok(raw)
}

/// Map a presented token to its scope. Malformed input never reaches the DB. The stored hash is
/// compared with the computed one in constant time even though the lookup is by hash, so a later
/// change to lookup-by-prefix cannot open a timing side channel.
pub fn verify(state: &AppState, presented: &str) -> Result<TokenScope, Reject> {
    if !is_well_formed(presented) {
        return Err(Reject::Malformed);
    }
    let computed = sha256_hex(&presented.to_ascii_lowercase());
    let row: Option<(String, String, String, String, Option<String>)> = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT token_hash, session_id, initiative_id, role, revoked_at FROM mcp_tokens WHERE token_hash = ?1",
                params![computed],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .unwrap_or_else(|error| {
            tracing::warn!(target: "mcp", %error, "MCP token lookup failed");
            None
        });
    let Some((stored, session_id, initiative_id, role, revoked_at)) = row else {
        return Err(Reject::Unknown);
    };
    if !bool::from(stored.as_bytes().ct_eq(computed.as_bytes())) {
        return Err(Reject::Unknown);
    }
    if revoked_at.is_some() {
        return Err(Reject::Revoked { initiative_id });
    }
    Ok(TokenScope { initiative_id, session_id, role })
}

/// Does the plan have a live (non-revoked) token for exactly this session?
pub fn has_live_token(state: &AppState, initiative_id: &str, session_id: &str) -> Result<bool, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM mcp_tokens WHERE initiative_id = ?1 AND session_id = ?2 AND revoked_at IS NULL",
            params![initiative_id, session_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
    })
}

/// Remove a plan's config file (and its then-empty directory). Missing is fine.
fn delete_config(state: &AppState, initiative_id: &str) {
    let store = settings_service::resolve_initiatives_dir(state);
    let Ok(path) = mcp_config_path(&store, initiative_id) else {
        return;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(target: "mcp", %initiative_id, %error, "failed to delete the plan-agent MCP config")
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir(dir); // only succeeds when empty
    }
}

/// Revoke the plan's live token(s) and delete its config file. Idempotent; `Ok(true)` only when a
/// live token was revoked. `why` is an audit word (`accepted`, `deleted`, …), logged with the id.
pub fn revoke_for_initiative(state: &AppState, initiative_id: &str, why: &str) -> Result<bool, String> {
    let n = state.db.with_conn(|conn| {
        conn.execute(
            "UPDATE mcp_tokens SET revoked_at = datetime('now') WHERE initiative_id = ?1 AND revoked_at IS NULL",
            params![initiative_id],
        )
        .map_err(|e| format!("Failed to revoke MCP tokens: {}", e))
    })?;
    delete_config(state, initiative_id);
    if n > 0 {
        tracing::info!(target: "mcp", %initiative_id, %why, "plan-agent MCP token revoked");
    }
    Ok(n > 0)
}

/// Revoke every live token held by `session_id` and delete the config file of every plan it ever
/// held a token for. Idempotent; `Ok(true)` only when a live token was revoked.
pub fn revoke_for_session(state: &AppState, session_id: &str, why: &str) -> Result<bool, String> {
    let (initiatives, n) = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT DISTINCT initiative_id FROM mcp_tokens WHERE session_id = ?1")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map(params![session_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE mcp_tokens SET revoked_at = datetime('now') WHERE session_id = ?1 AND revoked_at IS NULL",
                params![session_id],
            )
            .map_err(|e| format!("Failed to revoke MCP tokens: {}", e))?;
        Ok((ids, n))
    })?;
    // Only delete a plan's file when its CURRENT live token (if any) is not someone else's.
    for initiative_id in &initiatives {
        let other_live = state.db.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM mcp_tokens WHERE initiative_id = ?1 AND revoked_at IS NULL",
                params![initiative_id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })?;
        if other_live == 0 {
            delete_config(state, initiative_id);
        }
    }
    if n > 0 {
        tracing::info!(target: "mcp", %session_id, %why, "plan-agent MCP token revoked");
    }
    Ok(n > 0)
}

/// Write `<initiatives_dir>/<id>/plan-agent/mcp.json`: the directory is created `0700` (and forced
/// back to `0700` if it pre-existed), the file is created `0600` (and forced back to `0600` if it
/// pre-existed with wider bits) and truncated. Returns the file path — the ONLY thing that goes on
/// the claude command line.
pub fn write_mcp_config(initiatives_dir: &Path, initiative_id: &str, raw: &RawToken) -> Result<PathBuf, String> {
    write_mcp_config_on_port(initiatives_dir, initiative_id, raw, host_port())
}

/// [`write_mcp_config`] with an explicit port (tests, and callers that already know the bound port).
pub fn write_mcp_config_on_port(
    initiatives_dir: &Path,
    initiative_id: &str,
    raw: &RawToken,
    port: u16,
) -> Result<PathBuf, String> {
    let dir = plan_agent_dir(initiatives_dir, initiative_id)?;
    let path = dir.join(MCP_CONFIG_FILE);
    let body = serde_json::to_vec_pretty(&mcp_config_json(port, raw)).map_err(|e| e.to_string())?;
    write_private(&dir, &path, &body)?;
    Ok(path)
}

#[cfg(unix)]
fn write_private(dir: &Path, path: &Path, body: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create the plan store dir: {}", e))?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Failed to create the plan-agent dir: {}", e)),
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("Failed to restrict the plan-agent dir: {}", e))?;
    // Narrow a pre-existing file BEFORE writing the secret into it.
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to restrict the MCP config: {}", e))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("Failed to open the MCP config: {}", e))?;
    file.write_all(body).map_err(|e| format!("Failed to write the MCP config: {}", e))?;
    file.sync_all().map_err(|e| format!("Failed to flush the MCP config: {}", e))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(_dir: &Path, _path: &Path, _body: &[u8]) -> Result<(), String> {
    Err("the plan agent's private MCP config needs unix file permissions".to_string())
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::brief_store::tests::briefing_row;
    use crate::test_support::{test_state, tmp_dir};
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn token_rows(state: &AppState) -> Vec<Vec<Option<String>>> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT * FROM mcp_tokens").unwrap();
                let n = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        (0..n)
                            .map(|i| r.get::<_, Option<String>>(i))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
                Ok(rows)
            })
            .unwrap()
    }

    #[test]
    fn token_is_256_bits_hex() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        let a = mint(&state, "P", "S").unwrap();
        let b = mint(&state, "P", "S").unwrap();
        for t in [&a, &b] {
            assert_eq!(t.expose().len(), 64);
            assert!(t.expose().bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        }
        assert_ne!(a.expose(), b.expose(), "two mints differ");
    }

    #[test]
    fn only_hash_is_stored() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        let raw = mint(&state, "P", "S").unwrap();
        let rows = token_rows(&state);
        assert_eq!(rows.len(), 1);
        for col in rows[0].iter().flatten() {
            assert_ne!(col, raw.expose(), "a column holds the raw token");
            assert!(!col.contains(raw.expose()));
        }
        let hash: String = state
            .db
            .with_conn(|c| c.query_row("SELECT token_hash FROM mcp_tokens", [], |r| r.get(0)).map_err(|e| e.to_string()))
            .unwrap();
        assert_eq!(hash, sha256_hex(raw.expose()));
    }

    #[test]
    fn verify_rejects_malformed_unknown_revoked() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        let raw = mint(&state, "P", "S").unwrap();
        let ok = verify(&state, raw.expose()).unwrap();
        assert_eq!(ok, TokenScope { initiative_id: "P".into(), session_id: "S".into(), role: "plan_agent".into() });

        let unknown = "a".repeat(64);
        let cases: Vec<(&str, Reject)> = vec![
            ("", Reject::Malformed),
            ("abc", Reject::Malformed),
            (&raw.expose()[..63], Reject::Malformed),
            ("z".repeat(64).leak(), Reject::Malformed),
            (format!("{} ", raw.expose()).leak(), Reject::Malformed),
            (&unknown, Reject::Unknown),
        ];
        for (input, want) in cases {
            assert_eq!(verify(&state, input).unwrap_err(), want, "input len {}", input.len());
        }
        assert!(revoke_for_initiative(&state, "P", "test").unwrap());
        assert_eq!(
            verify(&state, raw.expose()).unwrap_err(),
            Reject::Revoked { initiative_id: "P".into() }
        );
        assert!(!revoke_for_initiative(&state, "P", "test").unwrap(), "idempotent");
    }

    #[test]
    fn mint_revokes_previous_live_token() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        let old = mint(&state, "P", "S1").unwrap();
        let new = mint(&state, "P", "S2").unwrap();
        assert!(matches!(verify(&state, old.expose()), Err(Reject::Revoked { .. })));
        assert_eq!(verify(&state, new.expose()).unwrap().session_id, "S2");
        let live: i64 = state
            .db
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM mcp_tokens WHERE revoked_at IS NULL", [], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(live, 1);
        assert!(!has_live_token(&state, "P", "S1").unwrap());
        assert!(has_live_token(&state, "P", "S2").unwrap());
    }

    #[test]
    fn mcp_config_file_is_0600_in_0700_dir() {
        let store = tmp_dir("mcp-store");
        let raw = RawToken("b".repeat(64));
        let path = write_mcp_config_on_port(&store, "P", &raw, 7788).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);

        // Pre-existing with wider bits: narrowed on rewrite.
        let store2 = tmp_dir("mcp-store");
        let dir = store2.join("Q").join(PLAN_AGENT_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.join(MCP_CONFIG_FILE);
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = write_mcp_config_on_port(&store2, "Q", &raw, 7788).unwrap();
        assert_eq!(path, file);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("old"), "truncated");

        assert!(write_mcp_config_on_port(&store2, "../x", &raw, 7788).is_err());
    }

    #[test]
    fn raw_token_debug_is_redacted() {
        let raw = RawToken("c".repeat(64));
        let shown = format!("{:?}", raw);
        assert!(!shown.contains(raw.expose()));
        assert_eq!(shown, "RawToken(…)");
        let nested = format!("{:?}", Some(&raw));
        assert!(!nested.contains(raw.expose()));
    }

    #[test]
    fn mcp_config_token_only_in_authorization_header() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        let store = tmp_dir("mcp-store");
        let raw = mint(&state, "P", "S").unwrap();
        let hash = sha256_hex(raw.expose());
        let path = write_mcp_config_on_port(&store, "P", &raw, 7788).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches(raw.expose()).count(), 1, "token occurs exactly once");
        assert!(!text.contains(&hash));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let servers = v["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let j1 = &servers["j1"];
        assert_eq!(j1["type"], "http");
        assert_eq!(j1["url"], "http://127.0.0.1:7788/mcp");
        assert_eq!(j1["headers"]["Authorization"], format!("Bearer {}", raw.expose()));
        assert_eq!(j1["headers"].as_object().unwrap().len(), 1);
        // Everything EXCEPT the header is token-free.
        let mut scrubbed = v.clone();
        scrubbed["mcpServers"]["j1"]["headers"]["Authorization"] = serde_json::json!("x");
        assert!(!scrubbed.to_string().contains(raw.expose()));
        // The path and its directory name hold neither the token nor its hash; the dir is per plan.
        let p = path.to_string_lossy();
        assert!(!p.contains(raw.expose()) && !p.contains(&hash));
        assert_eq!(path.parent().unwrap(), store.join("P").join("plan-agent"));
        assert_eq!(path.file_name().unwrap(), "mcp.json");
    }

    #[test]
    fn port_from_addr_defaults_to_7788() {
        assert_eq!(port_from_addr(None), 7788);
        assert_eq!(port_from_addr(Some("127.0.0.1:9911")), 9911);
        assert_eq!(port_from_addr(Some("garbage")), 7788);
    }

    #[test]
    fn revoke_deletes_the_config_file() {
        let (state, _root) = test_state();
        let store = tmp_dir("mcp-store").canonicalize().unwrap();
        settings_service::set_setting(
            &state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();
        briefing_row(&state, "P", "/w");
        let raw = mint(&state, "P", "S").unwrap();
        let path = write_mcp_config_on_port(&store, "P", &raw, 7788).unwrap();
        assert!(path.exists());
        assert!(revoke_for_session(&state, "S", "retired").unwrap());
        assert!(!path.exists());
        assert!(!revoke_for_session(&state, "S", "retired").unwrap(), "idempotent");
    }
}
