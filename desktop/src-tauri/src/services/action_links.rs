//! Discord action links, host side.
//!
//! An agent posts a Discord message with link buttons ("[Yes] [No]", "[Run phase 02]"). Each
//! button is `https://johnnyone.pages.dev/act?t=<jwt>`: a signed, 120 s, single-use credential
//! for ONE allowlisted action. The worker owns the key (`ACTION_LINK_SECRET`) and the single-use
//! ledger (D1 `action_link_redemptions`); see `worker/lib/action-links/`. This module is the
//! host's half:
//!
//! * [`mint`] — behind the host mutation `mintActionLink(act, args, label)` on 127.0.0.1:7788.
//!   Validates the allowlist and that the target exists HERE, asks the worker to sign over the
//!   authenticated worker connection (the relay credential — the secret never leaves the
//!   worker), and remembers the jti in [`Registry`]. The URL is returned to the caller and never
//!   logged.
//! * [`perform`] — the relay RPC `perform_action_link`, sent by the worker only after it verified
//!   the token and claimed its jti. The host performs ONLY a link it minted, with the same action
//!   and args, inside its window, once ([`Registry`] entry is taken), through the SAME path a
//!   human uses in the console:
//!     - `answer`       → [`crate::agent::AgentService::deliver_console_reply`] (the relay
//!                        `terminal_command` text path, i.e. typing the reply in the console);
//!     - `approve_plan` → [`agent_plans::accept_brief`] (the console's Accept);
//!     - `run_phase`    → [`agent_plans::run_initiative_from_phase`] (Run from phase);
//!     - `ack`          → nothing.
//! * [`record_event`] — the relay RPC `record_action_link_event`: the worker's preview/rejection
//!   outcomes, mirrored onto the plan timeline.
//!
//! Audit: every mint, preview, redeem and rejection that can be tied to a plan becomes an
//! `action_link_*` plan event carrying the jti — never the token or URL.
//!
//! For a minting agent (the alert-discord skill), two rules for `answer` links:
//! - **always group a question's options** (one `group` for its Yes / No / ...). Ungrouped links
//!   for one question are only serialised here ([`lock_answer`]): the first tap answers, a later
//!   one is refused as answered;
//! - **use a new group for each question.** A group closes once any of its links is redeemed
//!   (done, failed or answered), and minting into a closed group is refused.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::services::agent_plans;
use crate::services::settings::RelayConfig;
use crate::state::app_state::AppState;

/// The v1 allowlist. Anything else is refused at mint and at perform.
pub const ACTIONS: [&str; 4] = ["answer", "approve_plan", "run_phase", "ack"];
pub const TTL_SECS: i64 = 120;
/// The refusal for a jti this host has no record of. The worker matches on `unknown_link`.
pub const UNKNOWN_LINK: &str = "unknown_link: this host has no record of this link (J1 restarted since it was made?)";
pub const SKEW_SECS: i64 = 5;
const REPLY_MAX: usize = 200;
const LABEL_MAX: usize = 120;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// One allowlisted action with its validated arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    Answer { session_id: String, reply: String },
    ApprovePlan { plan_id: String },
    RunPhase { plan_id: String, phase_id: String },
    Ack { alert_id: Option<String> },
}

fn valid_id(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_phase(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && !v.contains("..")
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn has_control(v: &str) -> bool {
    v.chars()
        .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
}

/// Any character that renders as nothing: Cf ([`is_format_char`]) or [`is_invisible_char`].
fn has_invisible(v: &str) -> bool {
    v.chars().any(|c| is_format_char(c) || is_invisible_char(c))
}

/// Characters outside Cf that still render as nothing (H-3-48): the Hangul fillers (U+115F,
/// U+1160, U+3164, U+FFA0), the braille blank (U+2800), the combining grapheme joiner (U+034F)
/// and the Khmer inherent vowels (U+17B4, U+17B5). None is whitespace, so `"\u{3164}!ls"` would
/// pass a first-character check while a CLI's own trimming may still read it as `!ls`. Same set
/// as the worker's `INVISIBLE_RE`.
pub fn is_invisible_char(c: char) -> bool {
    matches!(
        c,
        '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{2800}' | '\u{3164}' | '\u{FFA0}'
    )
}

/// Unicode general category Cf (format characters: zero-width space/joiners, bidi controls, BOM,
/// word joiner, tag characters, ...). Invisible, so a label or reply carrying one does not say
/// what it looks like it says — and a ZWSP in front of `!` hides the command from a naive
/// first-character check (H-2-31). Rust's std has no category table; this is the full Cf set as
/// of Unicode 15.1, the same set the worker rejects with `/\p{Cf}/u`.
pub fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD
        | 0x0600..=0x0605
        | 0x061C
        | 0x06DD
        | 0x070F
        | 0x0890..=0x0891
        | 0x08E2
        | 0x180E
        | 0x200B..=0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x2064
        | 0x2066..=0x206F
        | 0xFEFF
        | 0xFFF9..=0xFFFB
        | 0x110BD
        | 0x110CD
        | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F)
}

/// First characters a reply may not start with (after leading spaces). Agent CLIs give them
/// meaning beyond text: `!` runs a shell command (Claude Code's bash mode), `/` is a slash command
/// (and `//` the console's key directive), `#` writes to memory. A tapped link must only ever
/// type an ANSWER. Same set as the worker's `validateReply`.
pub const REPLY_FORBIDDEN_FIRST: [char; 3] = ['!', '/', '#'];

/// Same rules as the worker's `validateReply`: one line of text that cannot be read as a command.
pub fn validate_reply(reply: &str) -> Result<(), String> {
    if reply.trim().is_empty() {
        return Err("reply must not be empty".into());
    }
    if reply.chars().count() > REPLY_MAX {
        return Err(format!("reply must be at most {REPLY_MAX} characters"));
    }
    if has_control(reply) {
        return Err("reply must be one line with no control characters".into());
    }
    if has_invisible(reply) {
        return Err("reply must not contain invisible characters".into());
    }
    if let Some(first) = reply.trim_start().chars().next() {
        if REPLY_FORBIDDEN_FIRST.contains(&first) {
            return Err(format!("reply must not start with {first}"));
        }
    }
    Ok(())
}

/// Same shape as the worker's group ids: a short opaque token chosen by the minter.
pub fn validate_group(group: &str) -> Result<(), String> {
    if group.is_empty() || group.len() > 64 || !valid_id(group) {
        return Err("group must be 1-64 characters of [A-Za-z0-9_-]".into());
    }
    Ok(())
}

pub fn validate_label(label: &str) -> Result<(), String> {
    if label.trim().is_empty() {
        return Err("label is required".into());
    }
    if label.chars().count() > LABEL_MAX {
        return Err(format!("label must be at most {LABEL_MAX} characters"));
    }
    if has_control(label) {
        return Err("label must be one line with no control characters".into());
    }
    if has_invisible(label) {
        return Err("label must not contain invisible characters".into());
    }
    Ok(())
}

impl LinkAction {
    /// Parse + validate `act` with its `args` object. Unknown keys are refused, mirroring the
    /// worker's allowlist (`worker/lib/action-links/actions.ts`) so neither side trusts the other.
    pub fn parse(act: &str, args: &Value) -> Result<Self, String> {
        if !ACTIONS.contains(&act) {
            return Err(format!("action {act:?} is not allowlisted"));
        }
        let obj = args
            .as_object()
            .ok_or_else(|| "args must be an object".to_string())?;
        let allowed: &[&str] = match act {
            "answer" => &["sessionId", "reply"],
            "approve_plan" => &["planId"],
            "run_phase" => &["planId", "phaseId"],
            _ => &["alertId"],
        };
        if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(format!("unexpected argument {k:?}"));
        }
        let get = |k: &str| -> Result<String, String> {
            obj.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("missing argument {k:?}"))
        };
        let id = |k: &str| -> Result<String, String> {
            let v = get(k)?;
            if valid_id(&v) {
                Ok(v)
            } else {
                Err(format!("{k} is not a valid id"))
            }
        };
        match act {
            "answer" => {
                let reply = get("reply")?;
                validate_reply(&reply)?;
                Ok(Self::Answer {
                    session_id: id("sessionId")?,
                    reply,
                })
            }
            "approve_plan" => Ok(Self::ApprovePlan {
                plan_id: id("planId")?,
            }),
            "run_phase" => {
                let phase_id = get("phaseId")?;
                if !valid_phase(&phase_id) {
                    return Err("phaseId is not a valid phase id".into());
                }
                Ok(Self::RunPhase {
                    plan_id: id("planId")?,
                    phase_id,
                })
            }
            _ => {
                let alert_id = match obj.get("alertId") {
                    None => None,
                    Some(_) => Some(id("alertId")?),
                };
                Ok(Self::Ack { alert_id })
            }
        }
    }

    pub fn act(&self) -> &'static str {
        match self {
            Self::Answer { .. } => "answer",
            Self::ApprovePlan { .. } => "approve_plan",
            Self::RunPhase { .. } => "run_phase",
            Self::Ack { .. } => "ack",
        }
    }

    /// The canonical args object (the exact shape the worker signs).
    pub fn args_json(&self) -> Value {
        match self {
            Self::Answer { session_id, reply } => {
                json!({ "sessionId": session_id, "reply": reply })
            }
            Self::ApprovePlan { plan_id } => json!({ "planId": plan_id }),
            Self::RunPhase { plan_id, phase_id } => {
                json!({ "planId": plan_id, "phaseId": phase_id })
            }
            Self::Ack { alert_id: Some(a) } => json!({ "alertId": a }),
            Self::Ack { alert_id: None } => json!({}),
        }
    }

    /// The plan whose timeline records this link's events, if any.
    pub fn plan_id(&self, state: &AppState) -> Option<String> {
        match self {
            Self::ApprovePlan { plan_id } | Self::RunPhase { plan_id, .. } => Some(plan_id.clone()),
            Self::Answer { session_id, .. } => {
                agent_plans::session_plan_owner(state, session_id).map(|(plan_id, _role)| plan_id)
            }
            Self::Ack { .. } => None,
        }
    }
}

/// What the worker returns for a mint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MintedLink {
    pub url: String,
    pub jti: String,
    pub expires_at: i64,
}

/// A minted, not-yet-performed link.
#[derive(Clone, Debug)]
pub struct Registered {
    pub action: LinkAction,
    pub expires_at: i64,
    pub label: String,
}

pub type Registry = Arc<Mutex<HashMap<String, Registered>>>;

/// Per-session async locks for performing answer links (H-3-43). See [`lock_answer`].
pub type AnswerLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

/// Holds one session's answer lock; on drop, forgets the session's entry once nobody else holds
/// or waits on it, so the map does not grow with every session ever answered.
pub struct AnswerGuard {
    locks: AnswerLocks,
    session_id: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for AnswerGuard {
    fn drop(&mut self) {
        self.guard.take();
        let mut map = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if map
            .get(&self.session_id)
            .is_some_and(|l| Arc::strong_count(l) == 1)
        {
            map.remove(&self.session_id);
        }
    }
}

/// Wait for, then hold, `session_id`'s answer lock. `perform` holds it from the session/ask
/// check through the delivery to the transcript record, so two links answering the same session
/// run one after the other: the second sees the first's recorded answer and is refused. Waiters
/// clone the `Arc` under the map's mutex, so a guard's drop never removes a lock still in use.
pub async fn lock_answer(state: &AppState, session_id: &str) -> AnswerGuard {
    let lock = {
        let mut map = state
            .answer_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.entry(session_id.to_string()).or_default().clone()
    };
    let guard = lock.lock_owned().await;
    AnswerGuard {
        locks: state.answer_locks.clone(),
        session_id: session_id.to_string(),
        guard: Some(guard),
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn lock(reg: &Registry) -> std::sync::MutexGuard<'_, HashMap<String, Registered>> {
    reg.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the host asks the worker to sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintRequest {
    pub act: String,
    /// The canonical args object, as JSON.
    pub args: String,
    pub label: String,
    /// Siblings share a group: redeeming one uses up the rest (a Yes/No pair).
    pub group: Option<String>,
    /// Human description of the target (plan / agent titles), shown on /act.
    pub target_title: Option<String>,
}

/// Seam: who signs a link. Production: the worker's `createActionLink`.
pub trait LinkSigner: Send + Sync {
    fn sign<'a>(&'a self, req: &'a MintRequest) -> BoxFut<'a, MintedLink>;
}

/// The real signer: POST `createActionLink` to the worker with the relay credential (the same
/// headers the desktop registers with), resolved at call time. A short-lived JWT credential that
/// has expired is refreshed once and the call retried, as registration does. The response carries
/// the URL; it is never logged.
pub struct WorkerSigner {
    pub state: AppState,
}

impl LinkSigner for WorkerSigner {
    fn sign<'a>(&'a self, req: &'a MintRequest) -> BoxFut<'a, MintedLink> {
        Box::pin(async move {
            // No test may reach a worker (H-1-13). `RelayConfig::resolve` already returns only an
            // injected config under cfg(test); this makes the transport itself refuse as well.
            #[cfg(test)]
            {
                let _ = req;
                Err::<MintedLink, String>(NETWORK_DISABLED_IN_TESTS.to_string())
            }
            #[cfg(not(test))]
            self.sign_live(req).await
        })
    }
}

/// The error the real signer returns in tests instead of opening a connection.
#[cfg(test)]
pub const NETWORK_DISABLED_IN_TESTS: &str = "mintActionLink: network disabled in tests";

impl WorkerSigner {
    #[cfg_attr(test, allow(dead_code))]
    async fn sign_live(&self, req: &MintRequest) -> Result<MintedLink, String> {
        let config = RelayConfig::resolve(&self.state).ok_or_else(|| {
            "mintActionLink: the host is not connected to a worker (sign in first)".to_string()
        })?;
        match sign_with(&config, req).await {
            Err(e) if crate::agent::registration::is_unauthenticated_register_error(&e) => {
                if crate::services::relay::refresh_access_token(&self.state)
                    .await
                    .unwrap_or(false)
                {
                    let fresh = RelayConfig::resolve(&self.state).unwrap_or(config);
                    sign_with(&fresh, req).await
                } else {
                    Err(e)
                }
            }
            other => other,
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
async fn sign_with(config: &RelayConfig, req: &MintRequest) -> Result<MintedLink, String> {
    let url = format!("{}/graphql", config.worker_url.trim_end_matches('/'));
    let body = json!({
        "query": "mutation($i: CreateActionLinkInput!) { createActionLink(input: $i) { url jti expiresAt } }",
        "variables": { "i": {
            "act": req.act, "args": req.args, "label": req.label,
            "group": req.group, "targetTitle": req.target_title,
        } },
    });
    let response = crate::agent::registration::apply_register_headers(
        reqwest::Client::new()
            .post(&url)
            .header("Content-Type", "application/json"),
        &config.tenant_id,
        &config.user_id,
        &config.access_token,
    )
    .json(&body)
    .timeout(std::time::Duration::from_secs(15))
    .send()
    .await
    .map_err(|e| format!("mintActionLink: worker unreachable: {}", e.without_url()))?;
    let status = response.status();
    let v: Value = response
        .json()
        .await
        .map_err(|_| format!("mintActionLink: worker answered {status} with no JSON"))?;
    if let Some(errors) = v.get("errors").and_then(Value::as_array) {
        let msg = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("mintActionLink: worker refused: {msg}"));
    }
    serde_json::from_value(v["data"]["createActionLink"].clone())
        .map_err(|_| "mintActionLink: worker response had no link".to_string())
}

/// Seam: who performs an action. Production: [`LivePerformer`], the console's own paths.
pub trait ActionPerformer: Send + Sync {
    /// Type `reply` into the session. `Ok(true)` when the delivery itself already recorded the
    /// reply as the session's `prompt` row (a shell-provider or attached agent's console path
    /// does, H-3-45), so `perform` does not record it a second time.
    fn answer<'a>(&'a self, session_id: &'a str, reply: &'a str) -> BoxFut<'a, bool>;
    fn approve_plan<'a>(&'a self, plan_id: &'a str) -> BoxFut<'a, ()>;
    fn run_phase<'a>(&'a self, plan_id: &'a str, phase_id: &'a str) -> BoxFut<'a, ()>;
}

pub struct LivePerformer {
    pub state: Arc<AppState>,
}

impl ActionPerformer for LivePerformer {
    fn answer<'a>(&'a self, session_id: &'a str, reply: &'a str) -> BoxFut<'a, bool> {
        Box::pin(crate::agent::AgentService::deliver_console_reply(
            &self.state,
            session_id,
            reply,
        ))
    }
    fn approve_plan<'a>(&'a self, plan_id: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            agent_plans::accept_brief(&self.state, plan_id, None)
                .await
                .map(|_| ())
        })
    }
    fn run_phase<'a>(&'a self, plan_id: &'a str, phase_id: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            agent_plans::run_initiative_from_phase(
                (*self.state).clone(),
                plan_id.to_string(),
                Some(phase_id.to_string()),
                None,
                None,
            )
            .await
            .map(|_| ())
        })
    }
}

/// The `sessions` columns that decide whether a link may answer a session.
struct SessionFacts {
    title: String,
    status: String,
    kind: String,
    attached_tmux: bool,
}

fn session_facts(state: &AppState, session_id: &str) -> Result<Option<SessionFacts>, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT title, status, kind, attached_tmux FROM sessions WHERE id = ?1",
            [session_id],
            |r| {
                Ok(SessionFacts {
                    title: r.get(0)?,
                    status: r.get(1)?,
                    kind: r.get(2)?,
                    attached_tmux: r.get::<_, i64>(3)? != 0,
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other.to_string()),
        })
    })
}

/// Where a session's latest question stands, from what is already persisted in `session_reports`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskState {
    /// An ask with no later human answer: a link may answer it.
    Open,
    /// The latest ask already got an answer (a console reply, or another link).
    Answered,
    /// No ask was ever recorded.
    NoAsk,
}

/// Is this session's agent waiting on a human right now?
///
/// An ASK is a `blocked` report from the agent itself (its baked "I need a human" command,
/// `agent_blocked`) — NOT J1's own `role = 'system'` blocked notes such as "Could not send keys"
/// (H-2-26) — or `question_watch`'s "pane … is waiting" narration. An ANSWER is a later `prompt`
/// row from anyone but the watchdog: a reply typed in the console (`record_user_prompt`) or a
/// redeemed answer link (`perform` records one, H-2-28). Everything else after the ask — the
/// agent's own progress `update`s, a watchdog nudge — leaves the ask open (H-2-24). Rows are
/// ordered by `rowid` (insertion order; `created_at` only has second resolution).
pub fn ask_state(state: &AppState, session_id: &str) -> Result<AskState, String> {
    use rusqlite::OptionalExtension;
    state.db.with_conn(|conn| {
        let ask: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM session_reports WHERE session_id = ?1 AND ( \
                   (kind = 'blocked' AND COALESCE(role, '') <> 'system') \
                   OR (kind = 'update' AND summary LIKE 'pane % is waiting%') ) \
                 ORDER BY rowid DESC LIMIT 1",
                [session_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(ask) = ask else {
            return Ok(AskState::NoAsk);
        };
        let answered: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM session_reports WHERE session_id = ?1 \
                   AND rowid > ?2 AND kind = 'prompt' AND COALESCE(role, '') <> 'watchdog')",
                rusqlite::params![session_id, ask],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(if answered { AskState::Answered } else { AskState::Open })
    })
}

/// The refusal for an answer link whose question already got an answer. The worker matches on
/// `answered:` and shows "The agent already got an answer, so nothing was sent."
pub const ANSWERED: &str = "answered: the agent already got an answer, so nothing was sent";

/// The mint-time refusal for a question that already got its answer. Worded for the minting
/// agent (H-3-40): what happened and what to do next.
pub const MINT_ANSWERED: &str =
    "its last question was already answered; report blocked again for a new question, then mint";

/// The session half of an answer link's target check, run at mint AND again at perform (H-3-44),
/// since a link lives 120 s: the session is an active `kind = 'agent'` session J1 spawned for a
/// plan (not an attached external tmux, not archived, not the plan agent of the Plans tab), whose
/// plan job has not ended (H-3-37: a finished plan's worker can keep a leftover pane). Returns the
/// session's facts with its owning `(plan_id, role)`.
fn check_answer_session(
    state: &AppState,
    session_id: &str,
) -> Result<(SessionFacts, String, String), String> {
    let facts = session_facts(state, session_id)?
        .ok_or_else(|| format!("session {session_id} does not exist"))?;
    if facts.status != "active" {
        return Err(format!("session {session_id} is {}", facts.status));
    }
    if facts.attached_tmux || facts.kind != "agent" {
        return Err(format!("session {session_id} is not a J1-managed agent"));
    }
    let (plan_id, role) = agent_plans::session_plan_owner(state, session_id)
        .ok_or_else(|| format!("session {session_id} does not belong to a plan"))?;
    if role == "plan_agent" {
        return Err(format!("session {session_id} is a plan agent, not a phase agent"));
    }
    // Fail closed: a lookup error refuses the link rather than typing into a finished job.
    if crate::services::agent_reaper::session_job_ended(state, session_id)? {
        return Err(format!("session {session_id}'s plan job has ended"));
    }
    Ok((facts, plan_id, role))
}

/// `answer` may target only a J1-managed agent that is asking ([`check_answer_session`]), whose
/// latest report is an open ask. Returns the agent's display name.
fn check_answer_target(state: &AppState, session_id: &str) -> Result<String, String> {
    let (facts, plan_id, role) = check_answer_session(state, session_id)?;
    match ask_state(state, session_id)? {
        AskState::Open => {}
        AskState::Answered => return Err(format!("session {session_id}: {MINT_ANSWERED}")),
        AskState::NoAsk => {
            return Err(format!(
                "session {session_id} has no open question: report blocked (agent_blocked) first, then mint"
            ))
        }
    }
    let plan_title = agent_plans::get_plan(state, &plan_id)
        .map(|r| r.plan.title)
        .unwrap_or_default();
    Ok(join_title(&[&format!("{} ({role})", facts.title), &plan_title]))
}

fn join_title(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let one_line: String = joined
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.chars().take(TARGET_TITLE_MAX).collect()
}

const TARGET_TITLE_MAX: usize = 120;

/// Refuse a link whose target is not real (or not answerable) here, BEFORE a jti is ever issued,
/// so an agent learns about it at mint time instead of the human after a tap. Returns the human
/// description of the target for the /act page. The perform path re-checks.
fn check_target(state: &AppState, action: &LinkAction) -> Result<Option<String>, String> {
    match action {
        LinkAction::Answer { session_id, .. } => check_answer_target(state, session_id).map(Some),
        LinkAction::ApprovePlan { plan_id } => {
            let run = agent_plans::get_plan(state, plan_id)?;
            if run.plan.initiative_status != "briefing" {
                return Err(format!(
                    "plan {plan_id} is not awaiting approval (status {})",
                    run.plan.initiative_status
                ));
            }
            Ok(Some(join_title(&[&run.plan.title])))
        }
        LinkAction::RunPhase { plan_id, phase_id } => {
            let run = agent_plans::get_plan(state, plan_id)?;
            let phase = run
                .phases
                .iter()
                .find(|p| &p.phase_id == phase_id)
                .ok_or_else(|| format!("Phase not found: {phase_id}"))?;
            Ok(Some(join_title(&[&run.plan.title, &phase.phase_title])))
        }
        LinkAction::Ack { .. } => Ok(None),
    }
}

/// Best-effort plan-timeline event. Never carries the token or URL.
fn timeline(state: &AppState, action: &LinkAction, event: &str, payload: Value) {
    if let Some(plan_id) = action.plan_id(state) {
        if let Err(error) = agent_plans::append_event(state, &plan_id, None, event, payload) {
            tracing::warn!(%error, event, "action link: failed to record timeline event");
        }
    }
}

/// Mint a link: validate, check the target, have the worker sign, remember the jti.
pub async fn mint(
    state: &AppState,
    signer: &dyn LinkSigner,
    act: &str,
    args: &str,
    label: &str,
    group: Option<&str>,
) -> Result<MintedLink, String> {
    let parsed: Value = serde_json::from_str(args)
        .map_err(|_| "mintActionLink: args must be a JSON object".to_string())?;
    let action = LinkAction::parse(act, &parsed).map_err(|e| format!("mintActionLink: {e}"))?;
    let label = label.trim();
    validate_label(label).map_err(|e| format!("mintActionLink: {e}"))?;
    let group = group.map(str::trim).filter(|g| !g.is_empty());
    if let Some(g) = group {
        validate_group(g).map_err(|e| format!("mintActionLink: {e}"))?;
    }
    let target_title = check_target(state, &action).map_err(|e| format!("mintActionLink: {e}"))?;

    let request = MintRequest {
        act: action.act().to_string(),
        args: action.args_json().to_string(),
        label: label.to_string(),
        group: group.map(str::to_string),
        target_title,
    };
    let link = signer.sign(&request).await?;
    // Never trust the far side to have honoured the TTL: the registry window is ours.
    let expires_at = link.expires_at.min(now_secs() + TTL_SECS);
    {
        let mut reg = lock(&state.action_links);
        let now = now_secs();
        reg.retain(|_, r| r.expires_at + SKEW_SECS >= now);
        reg.insert(
            link.jti.clone(),
            Registered {
                action: action.clone(),
                expires_at,
                label: label.to_string(),
            },
        );
    }
    tracing::info!(jti = %link.jti, act = action.act(), "action link minted");
    timeline(
        state,
        &action,
        "action_link_minted",
        json!({ "jti": link.jti, "act": action.act(), "label": label, "expiresAt": expires_at }),
    );
    Ok(link)
}

fn perform_summary(action: &LinkAction) -> String {
    match action {
        LinkAction::Answer { reply, .. } => format!("Sent \"{reply}\" to the agent."),
        LinkAction::ApprovePlan { .. } => "Plan accepted. Planning has started.".into(),
        LinkAction::RunPhase { phase_id, .. } => format!("Phase {phase_id} is starting."),
        LinkAction::Ack { .. } => "Marked as seen.".into(),
    }
}

/// `perform_action_link` — the worker verified the token and claimed its jti; act on it ONLY if
/// this host minted it with these exact args and it is still in its window. Single use here too:
/// the registry entry is taken before acting.
pub async fn perform(
    state: &AppState,
    performer: &dyn ActionPerformer,
    params: &Value,
    now: i64,
) -> Result<Value, String> {
    let jti = params
        .get("jti")
        .and_then(Value::as_str)
        .ok_or_else(|| "perform_action_link: missing jti".to_string())?;
    let act = params
        .get("act")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let action = LinkAction::parse(act, params.get("args").unwrap_or(&Value::Null))
        .map_err(|e| format!("perform_action_link: {e}"))?;

    let entry = lock(&state.action_links).remove(jti);
    let refuse = |reason: &str| -> Result<Value, String> {
        tracing::warn!(jti, reason, "action link refused by host");
        timeline(
            state,
            &action,
            "action_link_rejected",
            json!({ "jti": jti, "act": action.act(), "reason": reason, "stage": "host" }),
        );
        Err(format!("perform_action_link: {reason}"))
    };
    let Some(entry) = entry else {
        // The worker recognises UNKNOWN_LINK and tells the human to ask for a new link: in
        // practice this is an engine restart since the mint (the registry is in memory).
        return refuse(UNKNOWN_LINK);
    };
    if entry.action != action {
        return refuse("the link's action does not match what this host minted");
    }
    if now > entry.expires_at + SKEW_SECS {
        return refuse("the link expired");
    }
    // An answer holds its session's lock from the checks below until its reply is recorded
    // (H-3-43): a second link for the same question waits here, then finds it answered.
    let _answer_guard = match &action {
        LinkAction::Answer { session_id, .. } => Some(lock_answer(state, session_id).await),
        _ => None,
    };
    if let LinkAction::Answer { session_id, .. } = &action {
        // The session may have been archived, attached, or its plan job ended since the mint
        // (H-3-44): the same session checks as at mint, not only the ask.
        if let Err(e) = check_answer_session(state, session_id) {
            return refuse(&e);
        }
        // The question may have been answered (console, or another link) since the mint.
        match ask_state(state, session_id) {
            Ok(AskState::Open) => {}
            Ok(_) => return refuse(ANSWERED),
            Err(e) => return refuse(&e),
        }
    }

    let result = match &action {
        LinkAction::Answer { session_id, reply } => performer.answer(session_id, reply).await,
        LinkAction::ApprovePlan { plan_id } => performer.approve_plan(plan_id).await.map(|()| false),
        LinkAction::RunPhase { plan_id, phase_id } => {
            performer.run_phase(plan_id, phase_id).await.map(|()| false)
        }
        LinkAction::Ack { .. } => Ok(false),
    };
    match result {
        Ok(recorded) => {
            if let LinkAction::Answer { session_id, reply } = &action {
                // The tapped reply is the human's answer: record it as the console records a typed
                // one, so the transcript shows it and the ask reads as answered (H-2-28) — a second
                // link for the same question is then refused instead of typing again. Unless the
                // delivery already recorded it (H-3-45), which would show it twice.
                if !recorded {
                    agent_plans::record_session_note(state, session_id, "prompt", "user", reply, None);
                }
            }
            tracing::info!(jti, act = action.act(), "action link performed");
            timeline(
                state,
                &action,
                "action_link_redeemed",
                json!({ "jti": jti, "act": action.act(), "label": entry.label }),
            );
            Ok(json!({ "summary": perform_summary(&action) }))
        }
        Err(error) => {
            tracing::warn!(jti, act = action.act(), %error, "action link: action failed");
            timeline(
                state,
                &action,
                "action_link_rejected",
                json!({ "jti": jti, "act": action.act(), "reason": error, "stage": "perform" }),
            );
            Err(format!("perform_action_link: {error}"))
        }
    }
}

/// `record_action_link_event` — mirror a worker-side preview / rejection onto the plan timeline.
pub fn record_event(state: &AppState, params: &Value) -> Result<Value, String> {
    let jti = params
        .get("jti")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let event = match kind {
        "previewed" => "action_link_previewed",
        "rejected" => "action_link_rejected",
        other => return Err(format!("record_action_link_event: unknown kind {other:?}")),
    };
    let act = params
        .get("act")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let action = LinkAction::parse(act, params.get("args").unwrap_or(&Value::Null))
        .map_err(|e| format!("record_action_link_event: {e}"))?;
    let reason: Option<String> = params
        .get("reason")
        .and_then(Value::as_str)
        .map(|r| r.chars().take(120).collect());
    timeline(
        state,
        &action,
        event,
        json!({ "jti": jti, "act": action.act(), "reason": reason, "stage": "worker" }),
    );
    Ok(json!(true))
}

#[cfg(test)]
#[path = "action_links_tests.rs"]
mod tests;
