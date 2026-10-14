//! The workflow arena's data API (migration 026).
//!
//! A *workflow* is the searched artifact: the four prompt parts — role, instruction, contract, hop
//! — a phase-type is executed with, versioned, with `parent_id` lineage. A *run* is one execution
//! of one workflow, scored by one number: the test gate's exit code. A *preference* records that
//! one run beat another. A *champion* is the one workflow per `(repo, phase_type, family)` that
//! J1 actually uses, and it materialises as an ordinary plan file, so nothing in the execution
//! path changes.
//!
//! # Every rule lives in exactly one named function
//!
//! This module follows the precedent set by [`crate::services::agent_plans::effective_gate`]: a
//! rule that more than one caller needs is a `pub fn` with a name, not a clause inlined at each
//! call site. The repo has already been bitten by three independent writers of one SQL clause
//! drifting apart. The rules here, and the only places they live:
//!
//! | rule | the one function |
//! |---|---|
//! | what a seat budget is | [`parse_seat_map`] |
//! | when two runs are comparable | [`budgets_are_comparable`] |
//! | whether a run may become champion | [`promotion_block`] |
//! | when the search has halted | [`workflow_search_state`] + [`REJECTION_STRIKE_LIMIT`] |
//! | the next version number in a family | [`next_version_in_family`] |
//!
//! # The score
//!
//! `gate_exit` is the score and the only score. `gate_collected` is **advisory and nullable** — a
//! gate that prints nothing a parser recognises is a legitimate gate with a well-known exit code,
//! so `None` is a normal result, never an error. [`promotion_block`] reads `gate_exit` alone, and
//! that is deliberate: the moment anything branches on `gate_collected`, a gate that collects
//! nothing stops being reportable and starts being load-bearing.

use crate::state::app_state::AppState;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// How many consecutive rejections in one `(repo, phase_type, family)` halt the search.
///
/// "Consecutive" means trailing in [`workflow_decisions`](../../../migrations/026_add_workflow_arena.sql)
/// `seq` order: a promotion in the same family resets the streak, which is also the only way out of
/// a halt. Three is the design's number; it is a constant so that the enforcement point
/// ([`start_workflow_run`]) and the reporting point ([`workflow_search_state`]) can never disagree
/// about it.
pub const REJECTION_STRIKE_LIMIT: i64 = 3;

/// The largest `fan_out` a workflow may declare.
///
/// Two reasons, and the second is the one that bites. First, `fan_out` is a SEAT COST bounded by
/// the fleet, and no fleet this project will ever run has a thousand seats, so a larger number is a
/// typo or a hostile input rather than a budget. Second, `Workflow.fanOut` is served over GraphQL
/// inside `workflows: [Workflow!]!`; an unrepresentable number there does not degrade to a bad
/// field, it nulls the whole non-null chain up to the response root and takes every other field in
/// the document with it. This project has already shipped that outage once (a 38-billion token
/// count emptied the terminal tab bar on every device via `listAiSessions`).
///
/// The GraphQL fields are `Float` now, so the wire can carry anything; this bound is about
/// `fan_out` being a seat count. The REAL cap is the fleet's — that belongs to the lease pool,
/// which does not exist yet; when it does, this constant should become a lookup of the family's
/// actual seat capacity and this comment should go.
pub const MAX_FAN_OUT: i64 = 1_024;

/// **The fan-out cap.** `None` means this fan-out is allowed for this family; `Some(reason)` is the
/// refusal.
///
/// **This is a SEAM.** Today it is backed by the single constant [`MAX_FAN_OUT`], which is a
/// sanity bound and not a real capacity. The real caps live in `services::leases::SeatCaps` on Dev
/// B's branch — per family, with a database override, and an UNCAPPED family refused rather than
/// guessed at. When the branches are joined, the body of this function becomes that lookup and
/// nothing above it changes; `family` is already a parameter for exactly that reason, even though
/// the constant ignores it.
///
/// Both writers go through here: [`create_workflow_version`], which bounds the declaration, and
/// [`measure_run_budget`], which bounds what a run actually holds.
pub fn fan_out_is_allowed(family: &str, fan_out: i64) -> Option<String> {
    let _ = family; // see the doc comment: the real cap is per family, this stand-in is not.
    if fan_out < 1 {
        return Some(format!(
            "a fan-out of {} holds no seat, so it cannot run",
            fan_out
        ));
    }
    if fan_out > MAX_FAN_OUT {
        return Some(format!(
            "a fan-out of {} exceeds the stand-in cap of {} seats; the real per-family cap is the \
             lease pool's",
            fan_out, MAX_FAN_OUT
        ));
    }
    None
}

/// The largest `seed` a run may declare. A seed is a REPLAY INDEX — "the Nth time we ran this
/// workflow the same way" — so it is genuinely bounded by how many times a human will ever ask.
/// Bounded for the same wire reason as [`MAX_FAN_OUT`]: `WorkflowRun.seed` sits under
/// `workflowRuns: [WorkflowRun!]!`.
pub const MAX_SEED: i64 = 1_000_000;

// ---------------------------------------------------------------------------------------------
// Row types. camelCase JSON so the host RPC payload matches the worker GraphQL field names.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub repo: String,
    pub phase_type: String,
    pub family: String,
    pub version: i64,
    pub parent_id: Option<String>,
    pub role: String,
    pub instruction: String,
    pub contract: String,
    /// JSON: steps, parallel groups, returns. Validated as JSON on write by [`parse_hop`]; served
    /// back as the stored string, because the host does not own the Hop's shape.
    pub hop: String,
    pub fan_out: i64,
    pub is_champion: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub plan_id: Option<String>,
    pub seed: i64,
    /// The score. `None` means the gate never ran — which [`promotion_block`] refuses, because a
    /// run that was never scored has not earned anything.
    pub gate_exit: Option<i64>,
    /// **Advisory and nullable.** See the module docs. Never branch on this.
    pub gate_collected: Option<i64>,
    /// JSON object, model → seats. The seat budget this run was given.
    pub seats_held: String,
    /// JSON object, fleet contention at start. Stored for a human to read when two runs disagree;
    /// not part of the v1 comparability rule, which is [`budgets_are_comparable`] over
    /// `seats_held`.
    pub fleet_busy: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// DERIVED, not stored: whether this run started on a quiet fleet and can therefore be scored
    /// at all. Computed from `fleet_busy` by [`scoreability`] in the single row mapper, so it can
    /// never drift from the column it is derived from, and no migration adds a column that could.
    pub scoreable: bool,
    /// Why not, when `scoreable` is false. This is the field that turns "these two runs are
    /// mysteriously never comparable" into something a human can read.
    pub not_scoreable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPref {
    pub id: String,
    pub winner_run_id: String,
    pub loser_run_id: String,
    /// False when the two runs' seat budgets differed. A non-comparable preference is still
    /// recorded — it happened — but it cannot promote. See [`promotion_block`].
    pub comparable: bool,
    pub reason: Option<String>,
    pub created_at: String,
}

/// The search's state for one `(repo, phase_type, family)`, as data rather than a log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSearchState {
    pub repo: String,
    pub phase_type: String,
    pub family: String,
    /// Rejections since the last promotion in this family, counted from the end of the decision
    /// log. Capped only by how many rejections are actually there.
    pub consecutive_rejections: i64,
    /// `consecutive_rejections >= REJECTION_STRIKE_LIMIT`. While true, [`start_workflow_run`]
    /// refuses to start another run in this family.
    pub halted: bool,
    pub strike_limit: i64,
    /// The reasons of the trailing rejections, newest first — the one thing a halted search needs
    /// to show a human.
    pub recent_rejections: Vec<String>,
    /// The current champion's workflow id, if this family has one.
    pub champion_workflow_id: Option<String>,
}

/// Input for [`create_workflow_version`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowVersion {
    /// The PLAN's workspace path. `workflows.repo` is derived from it by [`repo_identity`] — it is
    /// never accepted from a caller, because a client-supplied identity is a client-supplied
    /// champion key.
    pub workspace_path: String,
    /// Must be one of [`KNOWN_PHASE_TYPES`].
    pub phase_type: String,
    pub family: String,
    /// Lineage. When set, it must name an existing workflow in the SAME
    /// `(repo, phase_type, family)` — a parent from another family is not lineage, it is a
    /// category error, and allowing it is how a Hop tuned for one context window ends up
    /// attributed to another.
    pub parent_id: Option<String>,
    pub role: String,
    pub instruction: String,
    pub contract: String,
    pub hop: String,
    pub fan_out: i64,
}

/// Input for [`start_workflow_run`].
///
/// **`seats_held` and `fleet_busy` are deliberately NOT here.** They used to be, taken from the
/// caller — and the caller is a browser, which cannot measure either. The UI was sending the
/// DECLARED budget and `{}` for contention (honestly labelled as a declaration in its own code),
/// which reduced `comparable` to "did two clients type the same numbers". That is the exact failure
/// `schema.md` says those two columns exist to prevent: "the first thing a busy fleet does is
/// promote a champion because the other box was quiet."
///
/// The host is the only thing that can know, so the host takes the measurement — see
/// [`measure_run_budget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowRun {
    pub workflow_id: String,
    pub plan_id: Option<String>,
    pub seed: i64,
    /// Seats to hold for this run, overriding the workflow's own `fan_out`. `None` means "whatever
    /// the workflow declares", which is the normal case.
    ///
    /// The model FAMILY is deliberately not a parameter: it is the workflow's, read from the
    /// workflow row. A caller-supplied family that disagreed with it would be a category error with
    /// a silent outcome — a run attributed to a family whose context window it never used — and the
    /// row already holds the answer, so there is nothing to supply.
    pub seats: Option<i64>,
}

/// The two JSON snapshots that only the host can take: what this run holds, and what the rest of
/// the fleet was holding at the same moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBudget {
    pub seats_held: String,
    pub fleet_busy: String,
}

/// How long any human-written reason may be. Rejections and resumes are read on a phone, three at
/// a time on a halted search; nothing at any layer bounded them before, so a single paste could
/// put a megabyte in the decision log and then serve it on every read of that family's state.
pub const MAX_REASON_CHARS: usize = 2_000;

/// The largest advisory collected-test count the arena will accept: `Number.MAX_SAFE_INTEGER`.
///
/// This field was DELIBERATELY left unbounded — refusing a real gate result because an advisory
/// number is large is the wrong trade — and that made it the one field able to carry a value an
/// IEEE double cannot hold. The corruption happens BEFORE GraphQL: the host serialises an exact
/// `i64` and `JSON.parse` rounds it in the worker. No error, no null, no log — a wrong number that
/// looks healthy, which is worse than either a refusal or a visible gap. The input side rounds too,
/// so a client sending 2^53+1 has the rounded value reach the resolver.
///
/// A collected test count cannot legitimately exceed nine quadrillion, so the bound costs nothing
/// real and a loud refusal beats a quiet wrong number. `gate_exit` is bounded separately and far
/// more tightly: it is a process exit code.
pub const MAX_GATE_COLLECTED: i64 = 9_007_199_254_740_991;

/// How long any one prompt part (role / instruction / contract) or a hop may be. Generous — these
/// are real prompt bodies — but not unbounded, for the same reason as above: they are served on
/// `workflows: [Workflow!]!`, so one pathological row slows every champion listing.
pub const MAX_PROMPT_CHARS: usize = 100_000;

/// **Does this text contain anything a human can actually SEE?**
///
/// `str::trim` is not enough, and that was a real hole: a reason of `"\u{200b}"` (zero-width
/// space), `"\u{feff}"` (BOM) or `"\0"` passed every blank check, because none of those is Unicode
/// `White_Space`. So a rejection — or a resume — whose reason renders as NOTHING was accepted and
/// stored, and that reason is the one thing a halted search has to show a human.
///
/// Deliberately a DENYLIST of known-invisible code points plus `char::is_control` and
/// `char::is_whitespace`, rather than a full Unicode `Default_Ignorable_Code_Point` table: that
/// would mean a new dependency for one predicate. It is conservative in the safe direction — text
/// made only of characters in this set is refused, and anything it fails to recognise can only
/// cause a reason to be ACCEPTED, never a visible one to be rejected.
///
/// **The list has already needed growing once, which is the honest argument for and against the
/// table.** An audit of 48 inputs found nine code points passing the first version: the braille
/// pattern BLANK, the hangul fillers (`U+3164`, `U+FFA0`, `U+115F`, `U+1160`), the khmer inherent
/// vowels (`U+17B4`, `U+17B5`), and the variation selectors (`U+FE00`-`U+FE0F`). All are here now.
///
/// The durable answer is `Default_Ignorable_Code_Point` plus the braille blank, and that needs a
/// Unicode property table. I have NOT taken a dependency for it, on the grounds that the
/// consequence of a miss is bounded and unglamorous — one rejection whose reason renders blank, in
/// a field nothing branches on — and that `unicode-properties`-class crates carry a table that is
/// itself versioned against a Unicode release, so "correct" becomes a thing to keep up to date
/// rather than a thing that is done. If a third audit finds a third batch, that calculus flips and
/// the table is the right call; this comment is the record of why it was not yet.
fn has_visible_content(text: &str) -> bool {
    text.chars().any(|c| {
        !c.is_whitespace()
            && !c.is_control()
            && !matches!(c,
                '\u{00ad}'              // soft hyphen
                | '\u{061c}'            // arabic letter mark
                | '\u{115f}'..='\u{1160}' // hangul choseong / jungseong filler
                | '\u{17b4}'..='\u{17b5}' // khmer inherent vowels AQ / AA
                | '\u{180e}'            // mongolian vowel separator
                | '\u{200b}'..='\u{200f}' // zero-width space .. RTL mark
                | '\u{202a}'..='\u{202e}' // bidi embedding/override
                | '\u{2060}'..='\u{2064}' // word joiner .. invisible plus
                | '\u{2066}'..='\u{2069}' // bidi isolates
                | '\u{2800}'             // braille pattern BLANK — not Default_Ignorable, but it
                                          // renders as nothing, which is the property that matters
                | '\u{3164}'            // hangul filler
                | '\u{fe00}'..='\u{fe0f}' // variation selectors 1-16
                | '\u{feff}'            // BOM / zero-width no-break space
                | '\u{ffa0}'            // halfwidth hangul filler
                | '\u{fff9}'..='\u{fffb}' // interlinear annotation
            )
    })
}

/// **The human-text rule.** One reader for every field a person types: it must contain something
/// visible, and it must be bounded.
///
/// Returns the trimmed text. Used by every reason ([`reject_workflow_run`],
/// [`resume_workflow_search`], [`record_workflow_pref`]) and by the four prompt parts, so there is
/// one definition of "a human wrote something here".
fn require_visible_text(label: &str, value: &str, max_chars: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if !has_visible_content(trimmed) {
        return Err(format!(
            "{} must contain visible text: {} has no visible characters (whitespace, control \
             characters and zero-width or formatting characters do not count)",
            label, label
        ));
    }
    let count = trimmed.chars().count();
    if count > max_chars {
        return Err(format!(
            "{} is {} characters, which is over the {} limit",
            label, count, max_chars
        ));
    }
    Ok(trimmed.to_string())
}

/// **The phase types the arena recognises.**
///
/// A closed set, because `phase_type` is a third of the champion key: free text there means one
/// typo silently starts a second, empty search in a family that looks like the real one.
///
/// `planning-phase` and `dev-phase` are what J1 actually produces today (the client derives them
/// from the run type and can emit nothing else). `ui-phase` is named by the design — it is the
/// gate-less case the mock shows — and is accepted so that the design's own example is
/// expressible; nothing produces it yet.
///
/// **This closes the free-text hole and does NOT close the coarseness problem**, which is the worse
/// of the two: every dev phase in a repo still shares one champion, so a migration phase and a UI
/// phase compete as though they were the same kind of work. Fixing that needs a finer phase_type at
/// the point the client derives it, which is not a host change — see the report.
pub const KNOWN_PHASE_TYPES: &[&str] = &["planning-phase", "dev-phase", "ui-phase"];

/// **The phase-type rule.** One validator, so the key cannot be polluted from any writer.
pub fn validate_phase_type(phase_type: &str) -> Result<String, String> {
    let trimmed = phase_type.trim();
    if KNOWN_PHASE_TYPES.contains(&trimmed) {
        return Ok(trimmed.to_string());
    }
    Err(format!(
        "'{}' is not a phase type this arena recognises ({}). A champion is keyed on \
         (repo, phase_type, family), so free text here starts a second search that looks like the \
         real one",
        trimmed,
        KNOWN_PHASE_TYPES.join(", ")
    ))
}

/// **The repo identity rule.** What `workflows.repo` means, derived from a workspace path.
///
/// The client used to send `basename(workspace_path)`, which collides: `/a/johnnyone` and
/// `/b/johnnyone` are different checkouts of possibly different repositories and shared one
/// champion.
///
/// The identity is the git REMOTE when there is one, normalised, and the absolute path when there
/// is not. The remote is the right answer and the path is only a fallback, because this project
/// works out of git WORKTREES constantly: keying on the path alone would give a worktree of
/// johnnyone a different champion from the main checkout, which is the same collision problem
/// wearing the opposite sign. Normalisation (scheme, credentials, trailing `.git`, case) exists so
/// that `git@github.com:x/y.git` and `https://github.com/x/y` are one identity rather than two.
///
/// Derived HOST-side from a path the host can see, never accepted from a caller — a client-supplied
/// identity is a client-supplied champion key.
pub fn repo_identity(workspace_path: &str) -> Result<String, String> {
    let path = std::path::Path::new(workspace_path.trim());
    if workspace_path.trim().is_empty() {
        return Err("workspacePath is required to derive the repo identity".to_string());
    }
    if let Some(remote) = git_remote_url(path) {
        return Ok(normalise_remote(&remote));
    }
    // No remote (or no git at all): the absolute path, canonicalised when the directory exists so
    // that `/a/./b` and `/a/b` are one identity.
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(format!("path:{}", absolute.to_string_lossy()))
}

/// The repository's remote URL: `origin` when it exists, otherwise the first remote by name.
///
/// It used to read `remote.origin.url` alone, and that was wrong for a whole normal shape of
/// checkout: a fork with an `upstream` remote and no `origin` HAS a remote but fell through to the
/// path fallback, so two worktrees of one such repository got two champions — precisely the
/// collision the remote key exists to prevent.
///
/// `origin` is preferred rather than merely first because it is the conventional name for "where
/// this checkout belongs", and alphabetical order among the rest is an arbitrary rule chosen only
/// so that the answer is STABLE: an identity that depends on which remote was added first would
/// make the champion key depend on local history.
///
/// NOTE this shells out with a blocking `std::process::Command`. Every current caller is
/// synchronous, so that is fine today — but a future async caller (the coordinator reaching
/// `create_workflow_version` from a task) would block a runtime thread, and the fix then is
/// `spawn_blocking` at the call site, not here.
fn git_remote_url(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote"])
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut names: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    let chosen = if names.iter().any(|name| *name == "origin") {
        "origin"
    } else {
        names.first().copied()?
    };
    let url = std::process::Command::new("git")
        .args(["remote", "get-url", chosen])
        .current_dir(path)
        .output()
        .ok()?;
    if !url.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&url.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// `git@github.com:owner/repo.git`, `https://user:tok@github.com/owner/repo.git` and
/// `https://github.com/Owner/Repo` all normalise to `git:github.com/owner/repo`.
fn normalise_remote(url: &str) -> String {
    let mut rest = url.trim();
    for prefix in ["https://", "http://", "ssh://", "git://"] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped;
            break;
        }
    }
    // Strip any `user:password@` or `git@` userinfo.
    if let Some((_, after)) = rest.split_once('@') {
        rest = after;
    }
    // `host:owner/repo` (scp form) becomes `host/owner/repo` — but a `:` followed by digits is a
    // PORT, not the scp separator, and blindly rewriting it made `ssh://git@host:2222/o/r` a second
    // identity for the same repository. A port is not part of which repo this is, so it is dropped
    // rather than folded into the path.
    let rest = match rest.split_once(':') {
        Some((host, after)) if after.chars().take_while(|c| *c != '/').all(|c| c.is_ascii_digit())
            && after.starts_with(|c: char| c.is_ascii_digit()) =>
        {
            // `host:2222/owner/repo` -> `host/owner/repo`
            match after.split_once('/') {
                Some((_port, tail)) => format!("{}/{}", host, tail),
                // `host:2222` with no path is not a repo URL; keep it whole rather than invent one.
                None => rest.to_string(),
            }
        }
        Some((host, after)) => format!("{}/{}", host, after),
        None => rest.to_string(),
    };
    let rest = rest.strip_suffix('/').unwrap_or(&rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    format!("git:{}", rest.to_lowercase())
}

// ---------------------------------------------------------------------------------------------
// The rules. One function each.
// ---------------------------------------------------------------------------------------------

/// **The seat-budget reader.** The single parser of the `seats_held` / `fleet_busy` encoding: a
/// JSON object of model name → seat count.
///
/// Entries whose count is zero are dropped, so `{"glm":3}` and `{"glm":3,"qwen":0}` are the same
/// budget. Without that, "I held no Qwen seats" and "I did not mention Qwen" would compare
/// unequal and every preference between them would be silently downgraded to non-comparable.
///
/// **This ERASES a distinction the lease pool deliberately preserves, and the erasure is the point.**
/// `LeasePool` keeps idle families at `0` rather than omitting them, and documents that `"0 busy"`
/// and `"not in the snapshot"` must stay distinguishable *for the comparability check*. On this
/// side they must NOT: an all-zero snapshot is exactly what "the fleet was quiet" looks like coming
/// out of a pool that enumerates every family, and if zero entries survived here, a quiet fleet
/// would read as contention and nothing would ever be scoreable. So the guarantee the pool offers
/// is one this consumer destroys on read, on purpose. Nothing is broken by that today; it is
/// written down because the two comments otherwise contradict each other, and the next reader
/// deserves to know which one the behaviour follows.
///
/// A negative count, a non-integer count, a non-object payload and malformed JSON are all errors
/// rather than empty maps: an unreadable budget must not look like "no seats", which would make
/// two broken runs compare equal and promotable.
pub fn parse_seat_map(raw: &str) -> Result<BTreeMap<String, i64>, String> {
    let value: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|e| format!("seat budget is not JSON: {}", e))?;
    let object = value
        .as_object()
        .ok_or_else(|| "seat budget must be a JSON object of model → seats".to_string())?;
    let mut out = BTreeMap::new();
    for (model, count) in object {
        let count = count
            .as_i64()
            .ok_or_else(|| format!("seat count for '{}' is not an integer", model))?;
        if count < 0 {
            return Err(format!("seat count for '{}' is negative", model));
        }
        if count > 0 {
            out.insert(model.clone(), count);
        }
    }
    Ok(out)
}

/// **The comparability rule.** Two runs are comparable when their seat budgets are identical.
///
/// This is the whole of `workflow_prefs.comparable`, and it is a function rather than an inline
/// comparison because two callers need it: [`record_workflow_pref`], which writes the flag, and
/// the tests that pin it. The refusal that makes it bite lives in [`promotion_block`].
///
/// An UNREADABLE budget on either side is not comparable. That is the conservative direction: the
/// cost of a false "not comparable" is one preference that cannot promote, and the cost of a false
/// "comparable" is promoting a champion because the other box happened to be quiet — which is the
/// exact failure `seats_held` was added to prevent.
///
/// This is the SEAT half only. Equal seat budgets do not imply equal contention — `seats_held` is
/// what a run was GRANTED, `fleet_busy` is what everything else was holding at the same moment —
/// so the flag that actually gets stored is decided by [`preference_comparability`], which combines
/// this with the per-run [`run_is_scoreable`]. Primitive, then policy; the policy has exactly one
/// writer.
pub fn budgets_are_comparable(left_seats_held: &str, right_seats_held: &str) -> bool {
    match (parse_seat_map(left_seats_held), parse_seat_map(right_seats_held)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// **THE `fleet_busy` CONTRACT.** `fleet_busy` records FOREIGN contention only: what everything
/// *else* on the fleet was holding when this run started, **excluding the run's own seats**. An
/// EMPTY map therefore means "the fleet was quiet".
///
/// Written out as a named contract rather than left in prose because the value's producer lives on
/// another branch (`services::leases::LeasePool::fleet_busy_json`) and the two possible readings
/// are indistinguishable in exactly one case: a pool reporting INCLUSIVE totals would emit
/// precisely `seats_held` for a quiet fleet, and under this contract that reads as "a foreign load
/// exactly equal to my own" — NOT quiet. That direction is the safe one (a refusal, never a false
/// crowning), but it means a producer using the other convention makes every run unscoreable
/// rather than silently wrong. [`scoreability`] is the single reader and
/// `the_fleet_busy_contract_is_foreign_contention_only` pins it, so a mismatch at integration
/// surfaces as a named failing test instead of an arena that quietly never promotes.
///
/// **Whether a single run can be scored at all.** `None` means yes; `Some(reason)` names the
/// foreign contention that disqualifies it.
///
/// Two concurrent runs contend, there is no 429 to notice, and the slower one simply looks like the
/// worse workflow — so a run that started while the fleet was busy measured the fleet, not the
/// workflow, and cannot be scored against anything.
///
/// **This replaces an earlier, WRONG rule, and the history is the point.** The first ruling was
/// that two runs are comparable iff their `fleet_busy` maps are EQUAL — equality rather than a
/// tolerance, on the correct grounds that a tolerance needs a threshold and a threshold is an
/// opinion. On contact with a real fleet that collapses to "never": contention is a live
/// measurement, two runs minutes apart are never byte-identical, and the result is a feature that
/// looks finished and promotes nothing. A feature that silently does nothing is worse than one
/// that refuses loudly. So the rule EXCLUDES rather than compares: **the arena scores on a quiet
/// fleet, and says so, per run, when the fleet was not quiet.**
fn scoreability(fleet_busy: &str) -> Option<String> {
    match parse_seat_map(fleet_busy) {
        Err(error) => Some(format!("its fleet contention is unreadable: {}", error)),
        Ok(foreign) if foreign.is_empty() => None,
        Ok(foreign) => {
            let total: i64 = foreign.values().sum();
            Some(format!(
                "the fleet was not quiet when it started: {} foreign {} held ({})",
                total,
                if total == 1 { "seat was" } else { "seats were" },
                foreign
                    .iter()
                    .map(|(model, seats)| format!("{}={}", model, seats))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

/// [`scoreability`] for a whole run — the public form, and what `WorkflowRun.scoreable` reports so
/// that a client can say WHY a run cannot compete instead of showing a mysteriously inert one.
pub fn run_is_scoreable(run: &WorkflowRun) -> Option<String> {
    scoreability(&run.fleet_busy)
}

/// Whether two runs saw the SAME foreign contention.
///
/// **Not part of any rule.** It was briefly the "like-for-like" escape in
/// [`preference_comparability`], and that escape is gone: it re-admitted exactly the pairs the
/// previous rule admitted, so it changed no outcome at all. Kept as a public predicate because the
/// audit suite drives it directly to show that two identically-UNREADABLE maps are not treated as
/// equal — a property of [`parse_seat_map`] worth pinning somewhere.
pub fn contention_was_identical(left_fleet_busy: &str, right_fleet_busy: &str) -> bool {
    match (parse_seat_map(left_fleet_busy), parse_seat_map(right_fleet_busy)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// **The comparability policy.** The single writer of `workflow_prefs.comparable`: `None` when the
/// two runs are comparable, `Some(reason)` naming why they are not.
///
/// Three conditions, all necessary:
///
/// 1. identical seat budgets ([`budgets_are_comparable`]);
/// 2. both runs SCOREABLE — neither started under foreign contention ([`run_is_scoreable`]) — or,
///    There is NO escape for two runs that saw identical contention — see the rule's own
///    history in the match below;
/// 3. the same `(repo, phase_type, family)`. `family` is the design's central identity claim — a
///    hop tuned around one context window does not transfer to another — so a cross-family
///    head-to-head is not an incomparable measurement, it is a measurement of two different
///    things. The pair is still RECORDED (a preference that happened is data) but it can never
///    promote.
///
/// Called only by [`record_workflow_pref`], so a client can never assert a comparability it does
/// not have; the refusal that makes the flag bite is in [`promotion_block`].
pub fn preference_comparability(
    winner: &WorkflowRun,
    loser: &WorkflowRun,
    same_champion_key: bool,
) -> Option<String> {
    if !same_champion_key {
        return Some(
            "the two runs are in different (repo, phase_type, family) keys: a champion is keyed on that triple, and a hop tuned for one model family does not transfer to another, so this pair measures two different things rather than comparing one"
                .to_string(),
        );
    }
    if !budgets_are_comparable(&winner.seats_held, &loser.seats_held) {
        return Some("the two runs held different seat budgets".to_string());
    }
    // **No escape.** A run that saw ANY foreign contention is not scoreable, full stop — even
    // against another run that saw identical contention.
    //
    // Three rules have been written here, and the first two were wrong for the same reason.
    // (1) "comparable iff the `fleet_busy` maps are EQUAL": right in principle, but contention is a
    // live measurement and two runs minutes apart are never byte-identical, so it collapses to
    // "never" and the feature looks finished while promoting nothing. (2) "exclude the contended,
    // with an escape for two runs that saw IDENTICAL contention": the full truth table admits
    // exactly the pairs equality admitted, row for row — a rule that admits the same set is not a
    // new rule, it is a better error message. (3) this one, which flips the
    // identical-but-contended row from comparable to REFUSED. That is a real behavioural
    // difference, it is enforceable, and it says one honest thing: the arena scores on a quiet
    // fleet.
    //
    // The per-run reason survives all three and is the part worth keeping: a client can say WHY a
    // run cannot compete instead of showing a mysteriously inert one.
    match (run_is_scoreable(winner), run_is_scoreable(loser)) {
        (None, None) => None,
        (Some(reason), Some(_)) => Some(format!(
            "both runs are unscoreable; the winning run: {}",
            reason
        )),
        (Some(reason), None) => Some(format!("the winning run is unscoreable: {}", reason)),
        (None, Some(reason)) => Some(format!("the losing run is unscoreable: {}", reason)),
    }
}

/// **The hop reader.** `hop` is JSON (steps, parallel groups, returns) and is validated as such on
/// write, then stored and served verbatim.
///
/// The host deliberately does not model the Hop's shape — that belongs to whatever executes it —
/// but it does refuse a payload that is not JSON at all. `test_commands` had to tolerate legacy
/// malformed rows and so has a three-state [`crate::services::agent_plans::DeclaredGate`]; this
/// column is new in migration 026, there are no legacy rows, and so the cheaper guarantee is
/// available: nothing unparseable is ever stored.
pub fn parse_hop(raw: &str) -> Result<serde_json::Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("hop is required: a workflow with no hop has nothing to execute".to_string());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("hop is not JSON: {}", e))
}

/// Everything [`promotion_block`] needs, resolved by the caller inside its transaction.
///
/// A struct rather than six positional arguments because the refusals now depend on facts that are
/// several joins away from the run (who the incumbent is, which run it was promoted on, whether the
/// preference's loser is in the same family, whether the search is halted). Resolving them once and
/// naming them keeps the rule in ONE function instead of spreading it across the call site.
#[derive(Debug, Clone)]
pub struct PromotionFacts<'a> {
    /// The run being promoted.
    pub run: &'a WorkflowRun,
    /// The preference offered as the head-to-head, if any.
    pub pref: Option<&'a WorkflowPref>,
    /// The loser run named by `pref`, if a `pref` was offered.
    pub pref_loser: Option<&'a WorkflowRun>,
    /// True when `pref`'s loser is in the SAME `(repo, phase_type, family)` as `run`.
    pub pref_loser_same_key: bool,
    /// True when `pref`'s loser is a run of the SAME workflow as `run` — a workflow beating itself.
    pub pref_loser_same_workflow: bool,
    /// The incumbent champion's workflow id, EXCLUDING `run`'s own workflow. `None` means the slot
    /// is empty or already held by this very workflow, i.e. there is nothing to displace.
    pub incumbent_workflow_id: Option<&'a str>,
    /// The run the incumbent champion was promoted on, taken from the decision log. `None` when the
    /// slot is held but no promotion is recorded for it — which the service treats as a refusal
    /// rather than a free pass.
    pub incumbent_run_id: Option<&'a str>,
    /// Whether the family's search is halted. A halt must be lifted by an explicit `resume`, not by
    /// a promotion.
    pub halted: bool,
    /// Why `run` cannot be scored, if it cannot — [`run_is_scoreable`].
    ///
    /// Checked even when there is NO preference, which is the one case the comparability rule
    /// cannot reach: seeding a family's first champion needs no head-to-head, so without this a
    /// first champion could be crowned off a run taken on a busy fleet with nothing ever looking
    /// at its contention.
    pub run_not_scoreable: Option<String>,
}

/// **The promotion rule.** `None` means this run may be promoted; `Some(reason)` is the refusal,
/// in words a human can act on.
///
/// Every refusal the design asks for, in one place, because promotion is the one irreversible step
/// in the arena:
///
/// * the family's search is HALTED — a halt is the search admitting defeat, and lifting it is its
///   own explicit, human-confirmed `resume` decision. Promotion used to be what cleared a halt,
///   which made the halt escapable with no new evidence at all.
/// * the run is not SCOREABLE — it started while the fleet was busy, so it measured the fleet.
///   Checked here as well as inside [`preference_comparability`] because seeding a family's
///   first champion needs no preference, and that path would otherwise never look at contention.
/// * `gate_exit` is `NULL` — the gate never ran, so nothing was proven.
/// * `gate_exit` is non-zero — the gate ran and failed. The exit code is the only score, and a
///   failing score cannot win.
/// * the preference does not name this run as its winner — a caller passing someone else's
///   preference is a bug, not a promotion.
/// * the preference is not `comparable` — see [`preference_comparability`].
/// * the preference's loser is a run of the SAME workflow — "a comparable head-to-head" is supposed
///   to mean the challenger out-performed something else. A workflow beating its own other run
///   proves nothing about anyone, and it was a working route to displacing an incumbent.
/// * the preference's loser is in a DIFFERENT `(repo, phase_type, family)` — beating a Qwen run must
///   not make a workflow the GLM champion. (`preference_comparability` already refuses this pair, so
///   this is the second, independent gate on the same mistake: belt and braces on the design's
///   central identity claim.)
/// * there is an incumbent to displace and the preference's loser is NOT the run the incumbent was
///   promoted on — the head-to-head has to be against the thing being displaced, or it is not a
///   head-to-head.
/// * there is an incumbent and no preference at all. Seeding the FIRST champion of a family is
///   allowed with no preference, because there is nothing to compare against; its gate still has to
///   be green.
///
/// `gate_collected` is not consulted. See the module docs.
pub fn promotion_block(facts: &PromotionFacts<'_>) -> Option<String> {
    let run = facts.run;
    if let Some(reason) = facts.run_not_scoreable.as_deref() {
        return Some(format!(
            "run {} cannot be scored: {}. The arena scores on a quiet fleet",
            run.id, reason
        ));
    }
    if facts.halted {
        return Some(format!(
            "the search for this (repo, phase_type, family) is halted: a halt is lifted by an \
             explicit, human-confirmed resume with a reason, not by promoting run {}. Promotion \
             used to clear a halt, which made the halt escapable with no new evidence",
            run.id
        ));
    }
    match run.gate_exit {
        None => {
            return Some(format!(
                "run {} has no gate result: the gate never ran, so nothing is proven",
                run.id
            ));
        }
        Some(0) => {}
        Some(exit) => {
            return Some(format!(
                "run {} failed its gate (exit {}): the exit code is the only score",
                run.id, exit
            ));
        }
    }
    match facts.pref {
        Some(pref) => {
            if pref.winner_run_id != run.id {
                return Some(format!(
                    "preference {} names run {} as the winner, not run {}",
                    pref.id, pref.winner_run_id, run.id
                ));
            }
            if !pref.comparable {
                return Some(format!(
                    "preference {} is not comparable, so it cannot promote — see \
                     `preference_comparability`: identical seat budgets, identical fleet \
                     contention, and the same (repo, phase_type, family)",
                    pref.id
                ));
            }
            if !facts.pref_loser_same_key {
                return Some(format!(
                    "preference {} was won against a run in a different (repo, phase_type, \
                     family): beating a run from another model family says nothing about this one",
                    pref.id
                ));
            }
            if facts.pref_loser_same_workflow {
                return Some(format!(
                    "preference {} was won against another run of the SAME workflow: a workflow \
                     beating itself is not a head-to-head and proves nothing about any other \
                     candidate",
                    pref.id
                ));
            }
            match facts.incumbent_workflow_id {
                // Seeding, or re-promoting the workflow that already holds the slot: there is
                // nothing being displaced, so there is nothing the loser has to be.
                None => None,
                Some(incumbent) => match facts.incumbent_run_id {
                    None => Some(format!(
                        "workflow {} holds the champion slot but no promotion is recorded for it, \
                         so there is no incumbent run to beat. Refusing rather than guessing",
                        incumbent
                    )),
                    Some(incumbent_run_id) => {
                        if facts.pref_loser.map(|loser| loser.id.as_str())
                            != Some(incumbent_run_id)
                        {
                            return Some(format!(
                                "preference {} was not won against the incumbent: displacing \
                                 workflow {} needs a comparable preference whose loser is run {}, \
                                 the run it was promoted on",
                                pref.id, incumbent, incumbent_run_id
                            ));
                        }
                        None
                    }
                },
            }
        }
        None => match facts.incumbent_workflow_id {
            None => None,
            Some(incumbent) => Some(format!(
                "workflow {} is already the champion of this (repo, phase_type, family); \
                 displacing it needs a comparable preference naming run {} as the winner",
                incumbent, run.id
            )),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Row mappers. One per table, used by every read, so no two queries shape a row differently.
// ---------------------------------------------------------------------------------------------

const WORKFLOW_COLUMNS: &str = "id, repo, phase_type, family, version, parent_id, role, \
                                instruction, contract, hop, fan_out, is_champion, created_at";

fn map_workflow(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workflow> {
    Ok(Workflow {
        id: row.get(0)?,
        repo: row.get(1)?,
        phase_type: row.get(2)?,
        family: row.get(3)?,
        version: row.get(4)?,
        parent_id: row.get(5)?,
        role: row.get(6)?,
        instruction: row.get(7)?,
        contract: row.get(8)?,
        hop: row.get(9)?,
        fan_out: row.get(10)?,
        is_champion: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
    })
}

const RUN_COLUMNS: &str = "id, workflow_id, plan_id, seed, gate_exit, gate_collected, \
                           seats_held, fleet_busy, started_at, ended_at";

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRun> {
    Ok(WorkflowRun {
        id: row.get(0)?,
        workflow_id: row.get(1)?,
        plan_id: row.get(2)?,
        seed: row.get(3)?,
        gate_exit: row.get(4)?,
        gate_collected: row.get(5)?,
        seats_held: row.get(6)?,
        fleet_busy: row.get(7)?,
        started_at: row.get(8)?,
        ended_at: row.get(9)?,
        scoreable: false,
        not_scoreable_reason: None,
    })
    .map(with_scoreability)
}

/// Fill the two derived scoreability fields. Applied by [`map_run`] alone, so every read of a run —
/// every one, including the ones inside transactions — carries the same answer.
fn with_scoreability(mut run: WorkflowRun) -> WorkflowRun {
    let reason = scoreability(&run.fleet_busy);
    run.scoreable = reason.is_none();
    run.not_scoreable_reason = reason;
    run
}

const PREF_COLUMNS: &str = "id, winner_run_id, loser_run_id, comparable, reason, created_at";

fn map_pref(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowPref> {
    Ok(WorkflowPref {
        id: row.get(0)?,
        winner_run_id: row.get(1)?,
        loser_run_id: row.get(2)?,
        comparable: row.get::<_, i64>(3)? != 0,
        reason: row.get(4)?,
        created_at: row.get(5)?,
    })
}

// ---------------------------------------------------------------------------------------------
// Connection-level helpers. Everything that touches SQL goes through one of these, so the
// transactional operations and the plain reads share the same statements.
// ---------------------------------------------------------------------------------------------

fn read_workflow(conn: &Connection, id: &str) -> Result<Workflow, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflows WHERE id = ?1", WORKFLOW_COLUMNS),
        params![id],
        map_workflow,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow {} not found", id))
}

fn read_run(conn: &Connection, id: &str) -> Result<WorkflowRun, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflow_runs WHERE id = ?1", RUN_COLUMNS),
        params![id],
        map_run,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow run {} not found", id))
}

fn read_pref(conn: &Connection, id: &str) -> Result<WorkflowPref, String> {
    conn.query_row(
        &format!("SELECT {} FROM workflow_prefs WHERE id = ?1", PREF_COLUMNS),
        params![id],
        map_pref,
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("workflow preference {} not found", id))
}

/// The champion key of a workflow: `(repo, phase_type, family)`.
///
/// One function so that "same family" is never spelled out as three field comparisons at a call
/// site. Three separate `==`s is how one of the three gets forgotten, and `family` is the design's
/// central identity claim.
pub fn champion_key(workflow: &Workflow) -> (&str, &str, &str) {
    (
        workflow.repo.as_str(),
        workflow.phase_type.as_str(),
        workflow.family.as_str(),
    )
}

/// The run a SPECIFIC workflow was promoted on, from the decision log.
///
/// The most recent `promote` decision whose run belongs to `champion_workflow_id`. That scoping is
/// the whole point and it was missing: the query used to take the latest `promote` in the FAMILY,
/// and `promotion_block` then never checked that the run it named belonged to the workflow actually
/// holding the slot. When the two separately-stored facts disagreed — a writer that bypasses this
/// service moving `is_champion` without logging — a challenger could displace the sitting champion
/// by beating a run of a completely different workflow, and a preference against the sitting
/// champion's OWN run was refused. The rule was pointing at the wrong thing in both directions.
///
/// `None` when that workflow has no recorded promotion, which now covers both the absent case and
/// the stale one. [`promotion_block`] treats it as a refusal rather than a free pass, so a champion
/// slot with no promotion behind it cannot be displaced on anybody's evidence until a real
/// promotion exists.
fn champion_promotion_run_id(
    conn: &Connection,
    champion_workflow_id: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT d.run_id FROM workflow_decisions d \
         JOIN workflow_runs r ON r.id = d.run_id \
         WHERE r.workflow_id = ?1 AND d.kind = 'promote' \
         ORDER BY d.seq DESC LIMIT 1",
        params![champion_workflow_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn champion_workflow_id(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM workflows \
         WHERE repo = ?1 AND phase_type = ?2 AND family = ?3 AND is_champion = 1",
        params![repo, phase_type, family],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// **The version rule.** The next `version` for a `(repo, phase_type, family)`: one past the
/// highest already there, starting at 1.
///
/// Monotonic WITHIN a family and independent across families, which is what makes `version` mean
/// "the Nth attempt at this phase-type on this model" rather than a global counter. Callers run it
/// inside the same transaction as the INSERT — read-then-write outside one would hand two
/// concurrent creates the same number, and the schema has no unique index on
/// `(repo, phase_type, family, version)` to catch it. (It deliberately does not: the design's only
/// uniqueness claim is one champion per family.)
pub fn next_version_in_family(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM workflows \
         WHERE repo = ?1 AND phase_type = ?2 AND family = ?3",
        params![repo, phase_type, family],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|e| e.to_string())
}

fn require_non_blank(label: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{} is required", label));
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------------------------

/// Create a new workflow version from the four prompt parts.
///
/// The new row is never a champion: becoming one is [`promote_workflow`]'s job and needs a green
/// gate and a human. `version` is assigned by [`next_version_in_family`] inside the transaction.
pub fn create_workflow_version(
    state: &AppState,
    input: NewWorkflowVersion,
) -> Result<Workflow, String> {
    let repo = repo_identity(&input.workspace_path)?;
    let phase_type = validate_phase_type(&input.phase_type)?;
    let family = require_non_blank("family", &input.family)?;
    let role = require_visible_text("role", &input.role, MAX_PROMPT_CHARS)?;
    let instruction = require_visible_text("instruction", &input.instruction, MAX_PROMPT_CHARS)?;
    let contract = require_visible_text("contract", &input.contract, MAX_PROMPT_CHARS)?;
    // Validated, then stored as the caller's own text: the host does not own the Hop's shape.
    parse_hop(&input.hop)?;
    let hop = require_visible_text("hop", &input.hop, MAX_PROMPT_CHARS)?;
    if let Some(reason) = fan_out_is_allowed(&family, input.fan_out) {
        return Err(format!("fanOut is not allowed: {}", reason));
    }

    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        if let Some(parent_id) = input.parent_id.as_deref() {
            let parent = read_workflow(&tx, parent_id)?;
            if parent.repo != repo || parent.phase_type != phase_type || parent.family != family {
                return Err(format!(
                    "parent {} is ({}, {}, {}) but this version is ({}, {}, {}): lineage does not \
                     cross families, because a hop tuned for one model does not transfer to another",
                    parent_id,
                    parent.repo,
                    parent.phase_type,
                    parent.family,
                    repo,
                    phase_type,
                    family
                ));
            }
        }
        let version = next_version_in_family(&tx, &repo, &phase_type, &family)?;
        tx.execute(
            "INSERT INTO workflows (id, repo, phase_type, family, version, parent_id, role, \
             instruction, contract, hop, fan_out, is_champion, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                id,
                repo,
                phase_type,
                family,
                version,
                input.parent_id,
                role,
                instruction,
                contract,
                hop,
                input.fan_out,
                created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_workflow(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Start a run of a workflow: the seat budget it holds, the fleet contention it started under, and
/// its replay seed.
///
/// **Refuses while the family's search is halted.** That is what "three consecutive rejections halt
/// the search" means operationally — not a log line. The only way out is
/// [`resume_workflow_search`]: an explicit, human-confirmed decision carrying a reason.
pub fn start_workflow_run(state: &AppState, input: NewWorkflowRun) -> Result<WorkflowRun, String> {
    // The HOST takes the measurement. The family comes from the workflow row and the seat count
    // from the workflow's `fan_out` unless the caller overrode it, so nothing a browser can type
    // reaches either column.
    let workflow = get_workflow(state, &input.workflow_id)?;
    let seats = input.seats.unwrap_or(workflow.fan_out);
    let budget = measure_run_budget(state, &workflow.family, seats)?;
    start_workflow_run_with_budget(state, input, budget)
}

/// **A DESIGN LIMIT, written down because nothing else writes it down: the arena cannot search over
/// `fan_out`.**
///
/// `seats_held` is derived here from the workflow's OWN `fan_out`. Comparability requires identical
/// `seats_held` ([`budgets_are_comparable`]). Therefore two workflows that declare different widths
/// can never produce a comparable preference, and a cheaper or wider challenger can never displace
/// the champion. What CAN be searched over is the four prompt parts — role, instruction, contract,
/// hop — and the model family, each within one fixed width.
///
/// The only workaround is to run the challenger on the incumbent's budget via the `seats` override,
/// and that is not a search over width: the width being tested is then not the width that ran.
///
/// This follows from the design rather than contradicting it — `schema.md` is explicit that
/// mismatched budgets cannot promote, and the reason is sound (a champion crowned on three seats
/// and a challenger run on one are not the same experiment). But the mock's own step 5 shows width
/// 1 against width 2 and step 7 promotes it, so the spec and the schema disagree about whether
/// width is searchable. Resolving that is a product decision, not a host one; this paragraph exists
/// so that whoever meets the limit meets the explanation with it.
///
/// **The budget measurement.** The one function that produces a run's `seats_held` and
/// `fleet_busy` snapshots.
///
/// **INTENDED SOURCE: `services::leases::LeasePool`** (Dev B, `feat/arena-b`), whose
/// `seats_held_json()` and `fleet_busy_json()` are shaped for exactly these two columns and which
/// is wired as `AppState.lease_pool`. That branch is not visible from this one, so this is the
/// SEAM, not the implementation: when the two branches are joined, the body below becomes
/// `state.lease_pool.seats_held_json(family, seats)` / `state.lease_pool.fleet_busy_json()` and
/// nothing above it changes.
///
/// **What it does until then, stated plainly rather than implied:** `seats_held` is the honest
/// declared budget `{family: seats}`, and `fleet_busy` is `{}` for EVERY run. The consequence is
/// that [`contention_is_comparable`] compares `{}` with `{}` and always answers true, so until the
/// lease pool is wired, comparability is effectively seats-only again — the fix to
/// `preference_comparability` is in place and its contention half has nothing real to read. That
/// state is pinned by a test (`contention_is_unmeasured_until_the_lease_pool_lands`) which goes RED
/// the moment a real measurement arrives, so wiring the pool cannot land silently.
pub fn measure_run_budget(
    _state: &AppState,
    family: &str,
    seats: i64,
) -> Result<RunBudget, String> {
    let family = require_non_blank("family", family)?;
    if let Some(reason) = fan_out_is_allowed(&family, seats) {
        return Err(format!("the run's seat budget is not allowed: {}", reason));
    }
    let mut requested = BTreeMap::new();
    requested.insert(family.clone(), seats);
    let budget = RunBudget {
        seats_held: serde_json::to_string(&requested).map_err(|e| e.to_string())?,
        fleet_busy: "{}".to_string(),
    };
    // **A run must never silently proceed at a narrower width.** The design is explicit that a run
    // QUEUES rather than shrinking, because a champion crowned on three seats and a challenger run
    // on one are not the same experiment — and a silent reduction is indistinguishable, afterwards,
    // from a workflow that was simply slower. Queuing is the caller's job (documented on Dev B's
    // lease pool), so the guard here is a refusal, not a reduction.
    //
    // Trivially satisfied by the stand-in above, which hands back exactly what was asked for. It is
    // written anyway because it becomes load-bearing the moment this body is replaced by a pool
    // that can return less than it was asked for, and a guard added at the same time as the code it
    // guards is a guard nobody wrote.
    if let Some(reason) = budget_was_honoured(&requested, &budget.seats_held) {
        return Err(reason);
    }
    Ok(budget)
}

/// **The no-shrink rule.** `None` when the fleet granted exactly the budget that was asked for;
/// `Some(reason)` when it did not.
///
/// The design is explicit that a run QUEUES rather than shrinking, because a champion crowned on
/// three seats and a challenger run on one are not the same experiment — and afterwards a silently
/// reduced run is indistinguishable from a workflow that was simply slower. Queuing is the caller's
/// job (documented on Dev B's lease pool), so the rule here is a refusal, not a reduction.
///
/// A separate named function rather than an inline comparison because the stand-in
/// [`measure_run_budget`] always hands back exactly what it was asked for, so the branch is
/// UNREACHABLE through the production path today and an inline check could not be tested at all.
/// This one can be, and is — which is the difference between a guard and a comment.
pub fn budget_was_honoured(
    requested: &BTreeMap<String, i64>,
    granted_seats_held: &str,
) -> Option<String> {
    let granted = match parse_seat_map(granted_seats_held) {
        Ok(granted) => granted,
        Err(error) => return Some(format!("the granted seat budget is unreadable: {}", error)),
    };
    if &granted == requested {
        return None;
    }
    Some(format!(
        "the fleet granted {:?} for a requested budget of {:?}: a run must queue until its full \
         budget is available, never run narrower — a reduced run is indistinguishable afterwards \
         from a slower workflow",
        granted, requested
    ))
}

/// [`start_workflow_run`] with the budget supplied instead of measured.
///
/// **Host-internal, and deliberately NOT reachable over RPC or GraphQL.** The relay path goes
/// through [`start_workflow_run`], which measures, so a browser can never declare its own
/// contention — that is the whole point of the change. This exists for the host-side caller that
/// already holds a measurement, and for tests that need to construct specific contention in order
/// to exercise the scoreability rule at all.
///
/// **THE ORDER, and it is load-bearing: SNAPSHOT FIRST, ACQUIRE SECOND.**
///
/// An earlier version of this comment prescribed the opposite ("take a lease and then start the run
/// with the snapshot it just took"), and that order is the losing one. `LeasePool::busy()` returns
/// the TOTAL seats held per family, with no exclusion of the caller's own lease and no
/// `busy_excluding` variant. Under the `fleet_busy` contract — foreign contention only, empty means
/// quiet — a snapshot taken AFTER `acquire` reads the run's own seats as foreign load, so every run
/// is unscoreable forever: the inert feature that looks finished and promotes nothing. Taken
/// BEFORE, an idle fleet reads as quiet and the rule works.
///
/// It is also right on its own terms, independent of whose API bends: a concurrent run's seats
/// belong in this run's snapshot, and its own do not. Snapshot, then acquire, then start.
pub fn start_workflow_run_with_budget(
    state: &AppState,
    input: NewWorkflowRun,
    budget: RunBudget,
) -> Result<WorkflowRun, String> {
    // Both budgets are validated BEFORE the insert: an unreadable `seats_held` would make every
    // preference involving this run non-comparable, which is a silent dead end rather than an error.
    parse_seat_map(&budget.seats_held)
        .map_err(|e| format!("seatsHeld: {}", e))?;
    parse_seat_map(&budget.fleet_busy).map_err(|e| format!("fleetBusy: {}", e))?;
    if input.seed < 0 {
        return Err(format!("seed must not be negative (got {})", input.seed));
    }
    // And an upper bound, which was missing: a replay index is bounded by how many times a human
    // will ever re-run the same workflow, and an unbounded one reaches a wire field that cannot
    // represent it — see [`MAX_SEED`].
    if input.seed > MAX_SEED {
        return Err(format!(
            "seed must be at most {} (got {}): a replay index larger than that is not a replay \
             index",
            MAX_SEED, input.seed
        ));
    }

    let id = Uuid::new_v4().to_string();
    let started_at = Utc::now().to_rfc3339();
    let seats_held = budget.seats_held.trim().to_string();
    let fleet_busy = budget.fleet_busy.trim().to_string();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let workflow = read_workflow(&tx, &input.workflow_id)?;
        let search = search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        if search.halted {
            return Err(format!(
                "the search for ({}, {}, {}) is halted after {} consecutive rejections \
                 (limit {}): lift it with an explicit, human-confirmed resume carrying a reason. \
                 Recent reasons: {}",
                workflow.repo,
                workflow.phase_type,
                workflow.family,
                search.consecutive_rejections,
                search.strike_limit,
                search.recent_rejections.join(" | ")
            ));
        }
        tx.execute(
            "INSERT INTO workflow_runs (id, workflow_id, plan_id, seed, gate_exit, \
             gate_collected, seats_held, fleet_busy, started_at, ended_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7, NULL)",
            params![
                id,
                input.workflow_id,
                input.plan_id,
                input.seed,
                seats_held,
                fleet_busy,
                started_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_run(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Record a run's gate result and end the run.
///
/// `gate_exit` is authoritative. `gate_collected` is **advisory and may be `None`** — a gate that
/// printed nothing a parser recognises still has a perfectly good exit code, and passing `None`
/// here is a normal outcome, not a failure. Nothing in this module branches on it.
/// **A re-score must be asked for.** This was an unconditional UPDATE, so a second call silently
/// replaced a green score with a red one and erased the advisory count, leaving only `ended_at` to
/// say it had happened twice — and a promoted champion's own run could be re-scored red after the
/// fact. `rescore: false` (the default everywhere above this function) now refuses a run that
/// already has a `gate_exit`.
///
/// A run the CURRENT champion was promoted on is refused even with `rescore: true`. That is the one
/// case where overwriting is not correction but retroactive falsification: the promotion already
/// happened and its evidence cannot be edited underneath it. Demote first, or score a new run.
/// Record a run's FIRST gate result. Refuses a run that already has one — use
/// [`rescore_workflow_run_gate`] to say so deliberately.
pub fn record_workflow_run_gate(
    state: &AppState,
    run_id: &str,
    gate_exit: i64,
    gate_collected: Option<i64>,
) -> Result<WorkflowRun, String> {
    set_gate_result(state, run_id, gate_exit, gate_collected, false)
}

/// Replace a run's gate result, deliberately.
///
/// The separate name IS the guard: there is no flag to forget and no default to get wrong, and a
/// reader of a call site can see which one was meant. Both entry points run the same core, so the
/// rule has one home.
///
/// Still refused for the run the CURRENT champion was promoted on — see [`set_gate_result`].
pub fn rescore_workflow_run_gate(
    state: &AppState,
    run_id: &str,
    gate_exit: i64,
    gate_collected: Option<i64>,
) -> Result<WorkflowRun, String> {
    set_gate_result(state, run_id, gate_exit, gate_collected, true)
}

/// **The advisory-count rule.** An unusable `gate_collected` becomes `None`; it never costs the
/// score.
///
/// Three positions have been taken on this field and the history matters. (1) Unbounded, because
/// "refusing a real gate result because an advisory number is large is the wrong trade" — which
/// made it the one field able to carry a value an IEEE-754 double cannot hold, silently ROUNDED by
/// `JSON.parse` in the worker before GraphQL ever saw it. (2) Bounded with a refusal, which closed
/// the rounding hole but contradicted (1)'s own reasoning: through the executor it meant a gate that
/// ran for minutes and PASSED could have its real exit code discarded over the advisory count.
/// (3) This: bound it, and on a violation drop the count rather than the result.
///
/// `None` is the right landing place and not merely a convenient one, because `None` already means
/// exactly this — "no count the host could trust". Clamping to [`MAX_GATE_COLLECTED`] instead would
/// assert a precise number that was never measured, which is the quiet-wrong-value failure the
/// bound exists to prevent, just relocated. The `warn!` is what keeps it from being silent.
///
/// Negative counts go the same way for the same reason: a negative test count is not a number to
/// correct, it is a number not to trust.
fn sanitise_collected(run_id: &str, gate_collected: Option<i64>) -> Option<i64> {
    match gate_collected {
        Some(collected) if collected < 0 => {
            tracing::warn!(
                run_id,
                collected,
                "arena: discarding a negative collected-test count; the gate's exit code is kept"
            );
            None
        }
        Some(collected) if collected > MAX_GATE_COLLECTED => {
            tracing::warn!(
                run_id,
                collected,
                max = MAX_GATE_COLLECTED,
                "arena: discarding a collected-test count above Number.MAX_SAFE_INTEGER, which \
                 would be silently rounded on the wire; the gate's exit code is kept"
            );
            None
        }
        other => other,
    }
}

fn set_gate_result(
    state: &AppState,
    run_id: &str,
    gate_exit: i64,
    gate_collected: Option<i64>,
    rescore: bool,
) -> Result<WorkflowRun, String> {
    // A gate exit is a process exit code (plus the three reserved negatives the plan-level gate
    // defines). Anything outside 32 bits is not one, and it would be served on a wire field that
    // cannot represent it. This one IS a refusal: the exit code is the score, and a score we cannot
    // represent is not a score.
    if gate_exit < i32::MIN as i64 || gate_exit > i32::MAX as i64 {
        return Err(format!(
            "gateExit {} is outside the range of a process exit code",
            gate_exit
        ));
    }
    // **The advisory count never costs the score.** See [`sanitise_collected`] — out of range or
    // negative becomes `None`, with a warning, rather than refusing the whole result.
    let gate_collected = sanitise_collected(run_id, gate_collected);
    let ended_at = Utc::now().to_rfc3339();
    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        // Read first so a missing run is "not found" rather than a silent zero-row UPDATE.
        let existing = read_run(&tx, run_id)?;
        let workflow = read_workflow(&tx, &existing.workflow_id)?;

        // **THE ORDERING RULE: re-read the halt INSIDE the write, never before the await.**
        //
        // `execute_gate` checks the halt, then awaits the gate — a whole test suite, seconds to
        // minutes, with the connection mutex released the entire time — and then writes the score
        // in a different `with_conn`. Three rejections landing in that window halted the family
        // and the score still landed: a green, promotable result on a search that had given up,
        // which could then be resumed and promoted on. That is the same leak as the one closed in
        // `promote_workflow`, one layer down and quieter, and a check before a long await is not a
        // check at all.
        //
        // This is the one place both the executor and the recorder pass through, and it already
        // holds a transaction and already reads the workflow for the champion lock — so the halt is
        // one query away from the decision rather than a round trip before it. An automated caller
        // will hit that window far more often than a human, which is why it is closed before one
        // exists.
        let search = search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        if search.halted {
            return Err(format!(
                "the search for ({}, {}, {}) is halted after {} consecutive rejections (limit \
                 {}): a halted search neither starts nor scores runs. If the gate was already \
                 running when the halt landed, its result is discarded — lift the halt with an \
                 explicit, human-confirmed resume and score again",
                workflow.repo,
                workflow.phase_type,
                workflow.family,
                search.consecutive_rejections,
                search.strike_limit
            ));
        }

        if let Some(previous) = existing.gate_exit {
            if !rescore {
                return Err(format!(
                    "run {} already has a gate result (exit {}): re-scoring a run replaces the \
                     only score the arena has, so it must be asked for explicitly via \
                     rescore_workflow_run_gate",
                    run_id, previous
                ));
            }
            // Only the SITTING champion's evidence is locked. A promote row belonging to a
            // workflow that has since been displaced is history, not evidence in use.
            let sitting = champion_workflow_id(
                &tx,
                &workflow.repo,
                &workflow.phase_type,
                &workflow.family,
            )?;
            let champion_run = match sitting.as_deref() {
                Some(champion) => champion_promotion_run_id(&tx, champion)?,
                None => None,
            };
            if champion_run.as_deref() == Some(run_id) {
                return Err(format!(
                    "run {} is the run the current champion of ({}, {}, {}) was promoted on: its \
                     score is the evidence that promotion rests on and cannot be rewritten \
                     underneath it",
                    run_id, workflow.repo, workflow.phase_type, workflow.family
                ));
            }
        }
        tx.execute(
            "UPDATE workflow_runs SET gate_exit = ?2, gate_collected = ?3, ended_at = ?4 \
             WHERE id = ?1",
            params![run_id, gate_exit, gate_collected, ended_at],
        )
        .map_err(|e| e.to_string())?;
        let updated = read_run(&tx, run_id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(updated)
    })
}

/// **Execute an arena run's gate and record the result.** This is the step that closes the arena's
/// loop: without it a `workflow_runs` row keeps `gate_exit = NULL` forever, so promotion is refused
/// for having no score and comparison is impossible — which makes the design's step 7, the one
/// human decision the whole arena exists for, unreachable.
///
/// An arena run IS an ordinary phase gate with a workflow attached. The run carries `plan_id`; the
/// plan carries `workspace_path` and the phase whose `test_commands` are the score. So this
/// delegates to [`crate::services::agent_plans::run_plan_test_gate`] — the same resolution rule
/// (`effective_gate`) and the same shell the coordinator's own ready-check uses, so what the arena
/// scores is what the coordinator would see — and feeds the outcome through [`set_gate_result`],
/// so every guard that function carries still applies.
///
/// Refusals, each for its own reason:
///
/// * **no `plan_id`** — an arena run with no plan has no `workspace_path` and no phase, so there is
///   nothing to execute. Refused rather than scored 0.
/// * **the family's search is HALTED** — a halted search must neither start nor score a run.
///   Scoring one would hand a halted family fresh evidence to promote on the moment it resumes,
///   which is the halt leaking in a quieter way than the hole already closed in
///   [`promote_workflow`].
/// * **no gate DECLARED at either level** — and this one is the trap. `TestGateRun` reports
///   `exit_code: 0` both for a gate that ran everything successfully AND for a phase that declares
///   no gate at all; `gate_declared` is the field that separates them. Storing that 0 would make an
///   ungated run look like a passing one and therefore promotable — a false pass of exactly the
///   kind this feature's "the exit code is the only score" rule exists to prevent. A phase with no
///   gate cannot compete, which is the design's own `ui-phase` case.
///
/// `exit_code` is the only thing persisted as the score. `collected` is passed through as the
/// advisory, nullable count and nothing branches on it. A MALFORMED gate is a legitimate non-zero
/// score (`TEST_GATE_EXIT_MALFORMED`), not a refusal: something was declared and could not be run,
/// and that is a failure rather than an absence.
pub async fn score_workflow_run(
    state: &AppState,
    run_id: &str,
    phase_id: Option<String>,
) -> Result<WorkflowRun, String> {
    execute_gate(state, run_id, phase_id, false).await
}

/// [`score_workflow_run`] for a run that already HAS a score: execute the gate again and replace
/// it, deliberately.
///
/// A separate entry point rather than a `rescore: bool`, for the reason the recorder's own doc
/// comment gives and the wire API had not yet honoured: the separate NAME is the guard. There is no
/// flag to forget, no default to get wrong, and a reader of a call site can see which was meant.
/// The sitting champion's promoting run is still refused here, whichever door is used.
pub async fn rescore_workflow_run(
    state: &AppState,
    run_id: &str,
    phase_id: Option<String>,
) -> Result<WorkflowRun, String> {
    execute_gate(state, run_id, phase_id, true).await
}

async fn execute_gate(
    state: &AppState,
    run_id: &str,
    phase_id: Option<String>,
    rescore: bool,
) -> Result<WorkflowRun, String> {
    // Everything that can be decided without running anything is decided first, so a refusal never
    // costs a test-suite execution.
    let (plan_id, repo, phase_type, family) = state.db.with_conn(|conn| {
        let run = read_run(conn, run_id)?;
        let workflow = read_workflow(conn, &run.workflow_id)?;
        // The HALT is checked first, before the run's own wiring. A halt is a property of the
        // family, not of this run: refusing a halted family with "that run has no plan" would
        // report the less important of the two problems, and a caller fixing the plan would then
        // discover the halt only on the next attempt.
        let search = search_state_on(conn, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        if search.halted {
            return Err(format!(
                "the search for ({}, {}, {}) is halted after {} consecutive rejections (limit \
                 {}): a halted search neither starts nor scores runs. Lift it with an explicit, \
                 human-confirmed resume",
                workflow.repo,
                workflow.phase_type,
                workflow.family,
                search.consecutive_rejections,
                search.strike_limit
            ));
        }
        let plan_id = run.plan_id.clone().ok_or_else(|| {
            format!(
                "run {} has no plan, so there is no workspace and no phase to run a gate in: an \
                 arena run must be started against the plan that executes it",
                run_id
            )
        })?;
        Ok((
            plan_id,
            workflow.repo,
            workflow.phase_type,
            workflow.family,
        ))
    })?;
    let _ = (&repo, &phase_type, &family);

    let gate = crate::services::agent_plans::run_plan_test_gate(state, plan_id.clone(), phase_id)
        .await?;

    if !gate.gate_declared {
        return Err(format!(
            "plan {} declares no test gate at either level, so run {} has nothing to be scored \
             on. `exit_code` is 0 both for a gate that passed and for a phase with no gate, so \
             recording it would make an ungated run look promotable — a phase with no gate cannot \
             compete",
            plan_id, run_id
        ));
    }

    set_gate_result(state, run_id, gate.exit_code as i64, gate.collected, rescore)
}

/// Record that one run beat another.
///
/// `comparable` is NOT a caller-supplied field: it is computed here by
/// [`preference_comparability`] from the two runs themselves — equal seat budgets, equal fleet
/// contention, and the same `(repo, phase_type, family)` — so a client cannot assert comparability
/// it does not have. A non-comparable preference is still written, with the reason it was not
/// comparable recorded in `reason` when the caller supplied none: it happened, and the design wants
/// it as data; it simply cannot promote.
///
/// **IDEMPOTENT for the same ORDERED pair.** A second call with the same `(winner, loser)` returns
/// the EXISTING row rather than inserting another. Without this, a client that reloaded the page
/// had no way to recover the `prefId` it needed in order to promote (there was no read for it
/// either — see [`list_workflow_prefs`], added alongside), so the only route forward was to record
/// the preference again: duplicate rows for a single head-to-head, in the table the design calls
/// the project-level measurement.
///
/// **If the same pair is recorded with a DIFFERENT `reason`, the first reason wins and the stored
/// row is returned unchanged.** A recorded outcome is not editable: silently rewriting the text
/// behind an existing `id` would make the measurement table's history untrustworthy, and erroring
/// would turn a benign page reload into a failure. A genuinely different judgement is a different
/// pair, or a new run. `comparable` is not recomputed either, and does not need to be — a run's
/// `seats_held` and `fleet_busy` are immutable through the public API, so the answer cannot have
/// changed.
///
/// The REVERSE pair `(loser, winner)` is a different ordered pair and is deliberately still
/// recordable: that is a contradiction rather than a duplicate, and `workflow_prefs` is a log of
/// what humans decided, not a referee of whether they changed their minds.
pub fn record_workflow_pref(
    state: &AppState,
    winner_run_id: &str,
    loser_run_id: &str,
    reason: Option<String>,
) -> Result<WorkflowPref, String> {
    if winner_run_id == loser_run_id {
        return Err(
            "a run cannot be preferred over itself: `workflow_prefs` is the project-level \
             measurement, and a self-preference corrupts any ranking computed from it"
                .to_string(),
        );
    }
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    // Optional here — a preference may be recorded without commentary — but if text IS supplied it
    // must be visible and bounded, for the same reason a rejection's is.
    let reason = match reason {
        Some(text) if text.trim().is_empty() => None,
        Some(text) => Some(require_visible_text("reason", &text, MAX_REASON_CHARS)?),
        None => None,
    };

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let winner = read_run(&tx, winner_run_id)?;
        let loser = read_run(&tx, loser_run_id)?;
        // **A failing run cannot be the winner.** Only `winner != loser` was checked, so a
        // challenger that exited 101 could be recorded as beating a champion that exited 0.
        // Promotion stayed blocked — `promotion_block` reads the gate — but `workflow_prefs` is the
        // table the design calls the project-level measurement, and a row in it asserting that the
        // failing run won is a false measurement whatever the promote path does with it.
        match winner.gate_exit {
            None => {
                return Err(format!(
                    "run {} cannot be the winner: its gate never ran, so there is nothing to \
                     prefer it on",
                    winner.id
                ));
            }
            Some(0) => {}
            Some(exit) => {
                return Err(format!(
                    "run {} failed its gate (exit {}) and cannot be recorded as the winner: the \
                     exit code is the only score, so a failing run has not beaten anything",
                    winner.id, exit
                ));
            }
        }
        // The loser must have been SCORED too. "A beat B" where B never ran is not a comparison,
        // and a preference is the project's measurement rather than a note.
        if loser.gate_exit.is_none() {
            return Err(format!(
                "run {} has no gate result, so it cannot be the loser: a preference over a run \
                 that never ran is not a comparison",
                loser.id
            ));
        }
        // Already recorded? Hand back the same row. Checked INSIDE the transaction so the
        // read-then-insert cannot interleave with another writer.
        if let Some(existing) = tx
            .query_row(
                &format!(
                    "SELECT {} FROM workflow_prefs WHERE winner_run_id = ?1 AND loser_run_id = ?2 \
                     ORDER BY created_at ASC, rowid ASC LIMIT 1",
                    PREF_COLUMNS
                ),
                params![winner_run_id, loser_run_id],
                map_pref,
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(existing);
        }
        let winner_workflow = read_workflow(&tx, &winner.workflow_id)?;
        let loser_workflow = read_workflow(&tx, &loser.workflow_id)?;
        let same_key = champion_key(&winner_workflow) == champion_key(&loser_workflow);
        let incomparable_because = preference_comparability(&winner, &loser, same_key);
        let comparable = incomparable_because.is_none();
        // When a pair is not comparable and the caller said nothing, record WHY. A row that exists
        // only to say "these two cannot be compared" is useless without the reason, and this is the
        // table the design calls the project-level measurement.
        let reason = reason.or(incomparable_because);
        tx.execute(
            "INSERT INTO workflow_prefs (id, winner_run_id, loser_run_id, comparable, reason, \
             created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                winner_run_id,
                loser_run_id,
                if comparable { 1_i64 } else { 0_i64 },
                reason,
                created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        let created = read_pref(&tx, &id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(created)
    })
}

/// Promote a run's workflow to champion of its `(repo, phase_type, family)`.
///
/// Human-confirmed: `confirmed` must be `true`. This is the one irreversible step in the arena, so
/// it is not something a client reaches by forgetting an argument.
///
/// The demote and the promote are ONE transaction, in that order. Two statements outside a
/// transaction would leave a window with two champions — which the partial unique index
/// `workflows_one_champion` rejects outright, so the failure mode is a hard error rather than
/// corruption; doing it in the wrong order inside the transaction fails the same way. Both are
/// pinned by tests.
///
/// **A promotion that promotes nothing writes nothing.** Re-promoting the workflow that already
/// holds the slot changes no row, so it no longer appends a decision either. Before this, the log
/// recorded five promotions for one actual promotion, `workflow_decisions` and
/// `workflows.is_champion` disagreed about how many there had been, and — while a promotion was
/// still what cleared a halt — each phantom row was a free escape from one.
///
/// **A promotion no longer clears a halt**, and is refused while one is in force. Lifting a halt is
/// [`resume_workflow_search`]'s job: an explicit, human-confirmed decision with a reason.
///
/// Every refusal is [`promotion_block`]'s, not this function's: the rule has one home.
pub fn promote_workflow(
    state: &AppState,
    run_id: &str,
    pref_id: Option<&str>,
    confirmed: bool,
) -> Result<Workflow, String> {
    if !confirmed {
        return Err(
            "promotion is the arena's one human-confirmed step and must be explicitly confirmed"
                .to_string(),
        );
    }
    let decision_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = read_run(&tx, run_id)?;
        let workflow = read_workflow(&tx, &run.workflow_id)?;
        let pref = match pref_id {
            Some(pref_id) => Some(read_pref(&tx, pref_id)?),
            None => None,
        };
        // The preference's LOSER, and how it relates to this run. `promotion_block` used to check
        // only that a preference existed, named this run as winner and was comparable — never what
        // was beaten — so a workflow could displace an incumbent by beating a run from another
        // model family, or another run of itself.
        let pref_loser = match pref.as_ref() {
            Some(pref) => Some(read_run(&tx, &pref.loser_run_id)?),
            None => None,
        };
        let loser_workflow = match pref_loser.as_ref() {
            Some(loser) => Some(read_workflow(&tx, &loser.workflow_id)?),
            None => None,
        };
        let incumbent =
            champion_workflow_id(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        // An already-champion workflow has nothing to displace, so it is excluded here; it still
        // has to pass every other check, because the block is consulted either way.
        let displaced = incumbent.as_deref().filter(|id| *id != workflow.id);
        let incumbent_run_id = match displaced {
            // Scoped to the INCUMBENT's own workflow, not to the family: the family's latest
            // promotion may belong to a workflow that no longer holds the slot.
            Some(incumbent) => champion_promotion_run_id(&tx, incumbent)?,
            None => None,
        };
        let search = search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        let facts = PromotionFacts {
            run: &run,
            pref: pref.as_ref(),
            pref_loser: pref_loser.as_ref(),
            pref_loser_same_key: loser_workflow
                .as_ref()
                .map(|loser| champion_key(loser) == champion_key(&workflow))
                .unwrap_or(true),
            pref_loser_same_workflow: loser_workflow
                .as_ref()
                .map(|loser| loser.id == workflow.id)
                .unwrap_or(false),
            incumbent_workflow_id: displaced,
            incumbent_run_id: incumbent_run_id.as_deref(),
            halted: search.halted,
            run_not_scoreable: run_is_scoreable(&run),
        };
        if let Some(reason) = promotion_block(&facts) {
            return Err(format!("refusing to promote: {}", reason));
        }
        // Nothing to do: this workflow already holds the slot. Returning early is what keeps the
        // decision log honest — an append here is a promotion that promoted nothing.
        if incumbent.as_deref() == Some(workflow.id.as_str()) {
            let unchanged = read_workflow(&tx, &workflow.id)?;
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(unchanged);
        }
        // Demote THEN promote, inside this transaction. The other order trips
        // `workflows_one_champion`.
        tx.execute(
            "UPDATE workflows SET is_champion = 0 \
             WHERE repo = ?1 AND phase_type = ?2 AND family = ?3 AND is_champion = 1",
            params![workflow.repo, workflow.phase_type, workflow.family],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
            params![workflow.id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
             VALUES (?1, ?2, 'promote', NULL, ?3)",
            params![decision_id, run_id, created_at],
        )
        .map_err(|e| e.to_string())?;
        let promoted = read_workflow(&tx, &workflow.id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(promoted)
    })
}

/// Reject a run, with a reason, and report the family's resulting search state.
///
/// The reason is mandatory — a refusal with nothing recorded teaches the search nothing, and the
/// one thing a halted search owes a human is the three reasons it halted on. The return value is
/// the halt state as DATA: `halted` is true once [`REJECTION_STRIKE_LIMIT`] rejections have run
/// consecutively in this family, and from then on [`start_workflow_run`] refuses.
pub fn reject_workflow_run(
    state: &AppState,
    run_id: &str,
    reason: &str,
) -> Result<WorkflowSearchState, String> {
    let reason = require_visible_text("reason", reason, MAX_REASON_CHARS).map_err(|detail| {
        format!(
            "a rejection must record a reason: three of them halt the search, and the reasons are \
             what a halted search has to show. {}",
            detail
        )
    })?;
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = read_run(&tx, run_id)?;
        let workflow = read_workflow(&tx, &run.workflow_id)?;
        tx.execute(
            "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
             VALUES (?1, ?2, 'reject', ?3, ?4)",
            params![id, run_id, reason, created_at],
        )
        .map_err(|e| e.to_string())?;
        let state_after =
            search_state_on(&tx, &workflow.repo, &workflow.phase_type, &workflow.family)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(state_after)
    })
}

/// **Lift a halt.** A human-confirmed decision, with a reason, that the search for one
/// `(repo, phase_type, family)` should continue.
///
/// This exists because a PROMOTION used to be what cleared a halt, and that made the halt
/// escapable with no new evidence at all. Two reachable forms: re-promoting the sitting champion —
/// a row-level no-op that still appended a `promote` and reset the streak — and promoting the very
/// run that had been rejected three times, which is the design's own step 8, since a rejection
/// carries a reason precisely because a human says no to a GREEN run. A three-strike halt a caller
/// could step over in one call with a run it already had is not a halt.
///
/// So clearing it is now its own explicit event, which means it is auditable: the decision log
/// shows who decided the search should continue and why, right where the three refusals are.
///
/// Refuses when the family is NOT halted. A resume row with nothing to resume would be a streak
/// reset obtained for free — exactly the hole this closes — so a no-op resume writes nothing, the
/// same discipline as a no-op promotion.
pub fn resume_workflow_search(
    state: &AppState,
    repo: &str,
    phase_type: &str,
    family: &str,
    reason: &str,
    confirmed: bool,
) -> Result<WorkflowSearchState, String> {
    if !confirmed {
        return Err(
            "lifting a halt is a human-confirmed decision and must be explicitly confirmed"
                .to_string(),
        );
    }
    let reason = require_visible_text("reason", reason, MAX_REASON_CHARS).map_err(|detail| {
        format!(
            "lifting a halt must record a reason: the halt is the search admitting defeat, and \
             continuing anyway is the decision a reader will most want explained. {}",
            detail
        )
    })?;
    let id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    state.db.with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let before = search_state_on(&tx, repo, phase_type, family)?;
        if !before.halted {
            return Err(format!(
                "the search for ({}, {}, {}) is not halted ({} of {} strikes): there is nothing \
                 to resume, and a resume with nothing to resume is a free streak reset",
                repo,
                phase_type,
                family,
                before.consecutive_rejections,
                before.strike_limit
            ));
        }
        // The decision log's FK is a run, so a resume is anchored to the last rejected run of the
        // family — the candidate the search gave up on. That keeps one shape for every decision
        // row rather than making `run_id` nullable for one kind.
        let anchor_run_id: String = tx
            .query_row(
                "SELECT d.run_id FROM workflow_decisions d \
                 JOIN workflow_runs r ON r.id = d.run_id \
                 JOIN workflows w ON w.id = r.workflow_id \
                 WHERE w.repo = ?1 AND w.phase_type = ?2 AND w.family = ?3 \
                   AND d.kind = 'reject' \
                 ORDER BY d.seq DESC LIMIT 1",
                params![repo, phase_type, family],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                // Unreachable through this service: `halted` is true only when rejections exist.
                "the search reports halted but no rejection is recorded for it".to_string()
            })?;
        tx.execute(
            "INSERT INTO workflow_decisions (id, run_id, kind, reason, created_at) \
             VALUES (?1, ?2, 'resume', ?3, ?4)",
            params![id, anchor_run_id, reason, created_at],
        )
        .map_err(|e| e.to_string())?;
        let after = search_state_on(&tx, repo, phase_type, family)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(after)
    })
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

/// List workflows, newest version first, optionally narrowed by any part of the champion key.
pub fn list_workflows(
    state: &AppState,
    repo: Option<&str>,
    phase_type: Option<&str>,
    family: Option<&str>,
) -> Result<Vec<Workflow>, String> {
    state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM workflows \
                 WHERE (?1 IS NULL OR repo = ?1) \
                   AND (?2 IS NULL OR phase_type = ?2) \
                   AND (?3 IS NULL OR family = ?3) \
                 ORDER BY repo, phase_type, family, version DESC",
                WORKFLOW_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![repo, phase_type, family], map_workflow)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn get_workflow(state: &AppState, id: &str) -> Result<Workflow, String> {
    state.db.with_conn(|conn| read_workflow(conn, id))
}

/// One run by id.
///
/// Service-level only — deliberately NOT exposed over RPC or GraphQL. `workflowRuns(workflowId:)`
/// already serves every run of a workflow and no client needs a second way in; this exists so that
/// code (and tests) re-reading a single run after a write does not have to scan a list and trust
/// that the list's ordering did not change.
pub fn get_workflow_run(state: &AppState, id: &str) -> Result<WorkflowRun, String> {
    state.db.with_conn(|conn| read_run(conn, id))
}

/// A workflow's runs, newest first.
pub fn list_workflow_runs(state: &AppState, workflow_id: &str) -> Result<Vec<WorkflowRun>, String> {
    state.db.with_conn(|conn| {
        // A missing workflow is an error, not an empty list: "no runs" and "no such workflow" are
        // different answers and a caller showing an empty arena cannot tell them apart.
        read_workflow(conn, workflow_id)?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM workflow_runs WHERE workflow_id = ?1 \
                 ORDER BY started_at DESC, rowid DESC",
                RUN_COLUMNS
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![workflow_id], map_run)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

/// Every preference involving a workflow's runs — as winner or as loser — newest first.
///
/// Added because there was NO way to read a preference back. A client that recorded one and then
/// reloaded could not recover the `prefId` that `promote_workflow` requires, so the only route
/// forward was to record it again; [`record_workflow_pref`] is idempotent now, and this is the
/// other half — a read, so that discovering an existing preference does not require issuing a
/// write.
///
/// Keyed on the WORKFLOW rather than on a run because the arena screen is champion-versus-
/// challenger: it needs every head-to-head a candidate has been in, not one pair it already knows.
pub fn list_workflow_prefs(
    state: &AppState,
    workflow_id: &str,
) -> Result<Vec<WorkflowPref>, String> {
    state.db.with_conn(|conn| {
        // A missing workflow is an error, not an empty list — the same reason as
        // `list_workflow_runs`: "no preferences yet" and "no such workflow" are different answers.
        read_workflow(conn, workflow_id)?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM workflow_prefs p \
                 WHERE EXISTS ( \
                   SELECT 1 FROM workflow_runs r \
                   WHERE r.workflow_id = ?1 AND r.id IN (p.winner_run_id, p.loser_run_id) \
                 ) \
                 ORDER BY p.created_at DESC, p.rowid DESC",
                PREF_COLUMNS
                    .split(", ")
                    .map(|column| format!("p.{}", column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![workflow_id], map_pref)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

/// **The halt rule.** The search state for one `(repo, phase_type, family)`.
///
/// "Three CONSECUTIVE rejections" is an ordering question, so it is answered by walking the
/// decision log backwards in `seq` order. Two things about that walk were wrong and are now fixed:
///
/// * it counted ROWS, so rejecting ONE run three times — a retried call, or a double-submitting
///   button — halted the search on a single human verdict. It counts DISTINCT `run_id`s now: three
///   strikes means three candidates refused, not three taps.
/// * it stopped at a `promote`, which made a promotion clear a halt. A promotion is now SKIPPED by
///   the walk, and only an explicit [`resume_workflow_search`] decision resets the streak. "The
///   search is halted until a human says why it should continue" is one event, not a side effect of
///   an unrelated operation.
///
/// `seq` rather than `created_at` because timestamps tie: two decisions in the same second are
/// indistinguishable by time, and the whole question is which came last.
pub fn workflow_search_state(
    state: &AppState,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<WorkflowSearchState, String> {
    state
        .db
        .with_conn(|conn| search_state_on(conn, repo, phase_type, family))
}

/// [`workflow_search_state`] against an arbitrary connection, so the enforcement points can
/// consult it inside their own transaction instead of re-deriving the rule.
fn search_state_on(
    conn: &Connection,
    repo: &str,
    phase_type: &str,
    family: &str,
) -> Result<WorkflowSearchState, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.kind, d.reason, d.run_id FROM workflow_decisions d \
             JOIN workflow_runs r ON r.id = d.run_id \
             JOIN workflows w ON w.id = r.workflow_id \
             WHERE w.repo = ?1 AND w.phase_type = ?2 AND w.family = ?3 \
             ORDER BY d.seq DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![repo, phase_type, family], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    // Walk backwards, newest first. `resume` ends the streak; `promote` is skipped; a `reject`
    // counts only the FIRST time its run is seen, so repeated rejections of one candidate are one
    // strike.
    let mut rejected_runs: BTreeSet<String> = BTreeSet::new();
    let mut recent_rejections = Vec::new();
    for row in rows {
        let (kind, reason, run_id) = row.map_err(|e| e.to_string())?;
        match kind.as_str() {
            "resume" => break,
            "promote" => continue,
            _ => {}
        }
        if !rejected_runs.insert(run_id) {
            continue;
        }
        if let Some(reason) = reason {
            recent_rejections.push(reason);
        }
    }
    let consecutive_rejections = rejected_runs.len() as i64;

    Ok(WorkflowSearchState {
        repo: repo.to_string(),
        phase_type: phase_type.to_string(),
        family: family.to_string(),
        consecutive_rejections,
        halted: consecutive_rejections >= REJECTION_STRIKE_LIMIT,
        strike_limit: REJECTION_STRIKE_LIMIT,
        recent_rejections,
        champion_workflow_id: champion_workflow_id(conn, repo, phase_type, family)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_state, tmp_dir};

    /// Round 3: `workflows.repo` is no longer a caller-supplied string — the host DERIVES it from a
    /// workspace path (`repo_identity`). These tests still want to talk about "repo" and "other-repo"
    /// as logical names, so this maps a logical name to a stable synthetic path and lets the host
    /// derive the identity from it exactly as it would in production.
    ///
    /// The path deliberately does not exist: `repo_identity` then finds no git remote and falls back to
    /// the absolute path, which is deterministic and distinct per name. No filesystem, no git, no
    /// dependence on the machine this runs on.
    fn repo_path(name: &str) -> String {
        format!("/nonexistent-arena-tests/{}", name)
    }

    fn repo_id(name: &str) -> String {
        repo_identity(&repo_path(name)).expect("derive the repo identity")
    }

    // --- Test shims over the three service functions that take the derived identity. ---
    //
    // They SHADOW the glob-imported originals deliberately, so that every call site in this file can go
    // on naming the logical repo. An explicit local item wins over a glob import, so `workflow_search_state("repo", …)`
    // below is this shim, which translates and then calls the real one. Nothing about the rule under
    // test changes — only how the key is spelled at the call site.

    fn workflow_search_state(
        state: &AppState,
        repo: &str,
        phase_type: &str,
        family: &str,
    ) -> Result<WorkflowSearchState, String> {
        crate::services::workflows::workflow_search_state(state, &repo_id(repo), phase_type, family)
    }

    fn resume_workflow_search(
        state: &AppState,
        repo: &str,
        phase_type: &str,
        family: &str,
        reason: &str,
        confirmed: bool,
    ) -> Result<WorkflowSearchState, String> {
        crate::services::workflows::resume_workflow_search(
            state,
            &repo_id(repo),
            phase_type,
            family,
            reason,
            confirmed,
        )
    }

    fn list_workflows(
        state: &AppState,
        repo: Option<&str>,
        phase_type: Option<&str>,
        family: Option<&str>,
    ) -> Result<Vec<Workflow>, String> {
        let repo = repo.map(repo_id);
        crate::services::workflows::list_workflows(state, repo.as_deref(), phase_type, family)
    }

    fn new_version(repo: &str, phase_type: &str, family: &str) -> NewWorkflowVersion {
        NewWorkflowVersion {
            workspace_path: repo_path(repo),
            phase_type: phase_type.to_string(),
            family: family.to_string(),
            parent_id: None,
            role: "you are a developer".to_string(),
            instruction: "implement the phase".to_string(),
            contract: "the gate must pass".to_string(),
            hop: r#"{"steps":["plan","edit","verify"]}"#.to_string(),
            fan_out: 3,
        }
    }

    fn seed_workflow(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Workflow {
        create_workflow_version(state, new_version(repo, phase_type, family)).expect("create")
    }

    /// A run with a SPECIFIC seat budget and no contention.
    ///
    /// Goes through `start_workflow_run_with_budget` because `seats_held` / `fleet_busy` are no
    /// longer caller parameters — the host measures them — and a test still has to be able to
    /// construct two runs with different budgets in order to exercise the comparability rule at
    /// all. The seam is host-internal and not reachable over RPC or GraphQL.
    fn start(state: &AppState, workflow_id: &str, seats: &str) -> WorkflowRun {
        start_with(state, workflow_id, seats, "{}")
    }

    fn start_with(
        state: &AppState,
        workflow_id: &str,
        seats: &str,
        fleet: &str,
    ) -> WorkflowRun {
        start_workflow_run_with_budget(
            state,
            NewWorkflowRun {
                workflow_id: workflow_id.to_string(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
            RunBudget {
                seats_held: seats.to_string(),
                fleet_busy: fleet.to_string(),
            },
        )
        .expect("start run")
    }

    /// A run that has been scored. `exit` 0 is a pass.
    fn scored_run(state: &AppState, workflow_id: &str, seats: &str, exit: i64) -> WorkflowRun {
        let run = start(state, workflow_id, seats);
        record_workflow_run_gate(state, &run.id, exit, Some(42)).expect("record gate")
    }

    fn pref_count(state: &AppState) -> i64 {
        state
            .db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM workflow_prefs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| e.to_string())
            })
            .expect("pref count")
    }

    fn promote_decision_count(state: &AppState) -> i64 {
        state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM workflow_decisions WHERE kind = 'promote'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("promote count")
    }

    fn champion_ids(state: &AppState, repo: &str, phase_type: &str, family: &str) -> Vec<String> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id FROM workflows WHERE repo = ?1 AND phase_type = ?2 \
                         AND family = ?3 AND is_champion = 1 ORDER BY id",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![repo_id(repo), phase_type, family], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| e.to_string())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|e| e.to_string())?);
                }
                Ok(out)
            })
            .expect("champion ids")
    }

    // -------------------------------------------------------------------------------------
    // The schema's own guarantee
    // -------------------------------------------------------------------------------------

    /// Two champions in one `(repo, phase_type, family)` must be IMPOSSIBLE, not merely avoided by
    /// the service. This goes straight at the table with raw SQL, bypassing `promote_workflow`
    /// entirely, because the partial index is the last line of defence against a future second
    /// writer of the promote clause.
    #[test]
    fn champion_index_makes_two_champions_impossible() {
        let (state, _root) = test_state();
        let a = seed_workflow(&state, "repo", "dev-phase", "glm");
        let b = seed_workflow(&state, "repo", "dev-phase", "glm");

        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![a.id],
                )
                .map_err(|e| e.to_string())?;
                // The second one must be rejected by `workflows_one_champion`.
                let second = conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![b.id],
                );
                let err = second.expect_err("a second champion must violate the unique index");
                let text = err.to_string();
                assert!(
                    text.contains("UNIQUE constraint failed")
                        || text.contains("constraint failed"),
                    "expected a uniqueness violation, got: {}",
                    text
                );
                Ok(())
            })
            .expect("raw sql");

        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![a.id]);
    }

    /// A different family is a different champion slot — the partial index must NOT be global.
    #[test]
    fn champions_are_per_family_not_global() {
        let (state, _root) = test_state();
        let glm = seed_workflow(&state, "repo", "dev-phase", "glm");
        let qwen = seed_workflow(&state, "repo", "dev-phase", "qwen");
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id IN (?1, ?2)",
                    params![glm.id, qwen.id],
                )
                .map_err(|e| e.to_string())
            })
            .expect("two families, two champions");
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm").len(), 1);
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "qwen").len(), 1);
    }

    /// The ORDER inside `promote_workflow`'s transaction is load-bearing. Promote-then-demote
    /// trips the index, which is why the shipped function demotes first. Pinned so nobody
    /// "simplifies" the order back.
    #[test]
    fn promoting_before_demoting_trips_the_index() {
        let (state, _root) = test_state();
        let incumbent = seed_workflow(&state, "repo", "dev-phase", "glm");
        let challenger = seed_workflow(&state, "repo", "dev-phase", "glm");
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![incumbent.id],
                )
                .map_err(|e| e.to_string())
            })
            .expect("seed incumbent");

        state
            .db
            .with_conn(|conn| {
                let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
                let wrong_order = tx.execute(
                    "UPDATE workflows SET is_champion = 1 WHERE id = ?1",
                    params![challenger.id],
                );
                assert!(
                    wrong_order.is_err(),
                    "promoting before demoting must be rejected"
                );
                Ok(())
            })
            .expect("tx");
    }

    // -------------------------------------------------------------------------------------
    // Versions and lineage
    // -------------------------------------------------------------------------------------

    #[test]
    fn version_is_monotonic_within_a_family_and_independent_across_families() {
        let (state, _root) = test_state();
        let one = seed_workflow(&state, "repo", "dev-phase", "glm");
        let two = seed_workflow(&state, "repo", "dev-phase", "glm");
        let other_family = seed_workflow(&state, "repo", "dev-phase", "qwen");
        let other_phase = seed_workflow(&state, "repo", "ui-phase", "glm");
        let other_repo = seed_workflow(&state, "other", "dev-phase", "glm");

        assert_eq!(one.version, 1);
        assert_eq!(two.version, 2);
        assert_eq!(other_family.version, 1);
        assert_eq!(other_phase.version, 1);
        assert_eq!(other_repo.version, 1);
        assert!(!one.is_champion, "a new version is never born champion");
    }

    #[test]
    fn lineage_records_the_parent_and_cannot_cross_families() {
        let (state, _root) = test_state();
        let parent = seed_workflow(&state, "repo", "dev-phase", "glm");

        let mut child = new_version("repo", "dev-phase", "glm");
        child.parent_id = Some(parent.id.clone());
        let child = create_workflow_version(&state, child).expect("child");
        assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(child.version, 2);

        let mut crossed = new_version("repo", "dev-phase", "qwen");
        crossed.parent_id = Some(parent.id.clone());
        let err = create_workflow_version(&state, crossed).expect_err("cross-family lineage");
        assert!(err.contains("lineage does not"), "got: {}", err);
    }

    #[test]
    fn a_workflow_needs_a_parseable_hop_and_at_least_one_seat() {
        let (state, _root) = test_state();
        let mut bad_hop = new_version("repo", "dev-phase", "glm");
        bad_hop.hop = "not json".to_string();
        assert!(create_workflow_version(&state, bad_hop)
            .expect_err("malformed hop")
            .contains("hop is not JSON"));

        let mut no_seats = new_version("repo", "dev-phase", "glm");
        no_seats.fan_out = 0;
        assert!(create_workflow_version(&state, no_seats)
            .expect_err("zero fan-out")
            .contains("holds no seat"));
    }

    // -------------------------------------------------------------------------------------
    // The gate result
    // -------------------------------------------------------------------------------------

    /// `gate_collected` is ADVISORY and NULLABLE. A gate that prints nothing a parser recognises
    /// is a legitimate gate, so `None` must round-trip as `None` — not 0, not an error — and must
    /// not change anything that depends on the gate.
    #[test]
    fn gate_collected_is_nullable_and_none_is_not_an_error() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":3}"#);
        assert_eq!(run.gate_exit, None, "a fresh run has no score");
        assert_eq!(run.gate_collected, None);
        assert_eq!(run.ended_at, None);

        let scored = record_workflow_run_gate(&state, &run.id, 0, None).expect("unparseable count");
        assert_eq!(scored.gate_exit, Some(0));
        assert_eq!(
            scored.gate_collected, None,
            "None must stay None: coercing it to 0 would invent a 'collected nothing' report"
        );
        assert!(scored.ended_at.is_some());

        // And it must not block promotion — `gate_exit` alone is the score.
        assert_eq!(
            promotion_block(&PromotionFacts {
                run: &scored,
                pref: None,
                pref_loser: None,
                pref_loser_same_key: true,
                pref_loser_same_workflow: false,
                incumbent_workflow_id: None,
                incumbent_run_id: None,
                halted: false,
                run_not_scoreable: None,
            }),
            None
        );

        // A real count round-trips too, including the meaningful 0 — on a FRESH run, because a
        // second score for the same run now has to be asked for by name.
        let other = start(&state, &workflow.id, r#"{"glm":3}"#);
        let zero = record_workflow_run_gate(&state, &other.id, 0, Some(0)).expect("zero collected");
        assert_eq!(zero.gate_collected, Some(0));

        // And the deliberate re-score keeps the same nullability: None stays None.
        let rescored = rescore_workflow_run_gate(&state, &other.id, 0, None).expect("rescore");
        assert_eq!(rescored.gate_collected, None);
    }

    /// A second score for a run must be asked for by NAME. `record_workflow_run_gate` refuses, and
    /// `rescore_workflow_run_gate` is the only way through — there is no flag to forget.
    #[test]
    fn a_second_gate_result_is_refused_unless_it_is_asked_for_by_name() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":3}"#);
        record_workflow_run_gate(&state, &run.id, 0, Some(400)).expect("first score");

        let refused = record_workflow_run_gate(&state, &run.id, 1, None)
            .expect_err("a silent overwrite must be refused");
        assert!(refused.contains("already has a gate result (exit 0)"), "got: {}", refused);
        // The original score survives the refusal intact, advisory count included.
        let kept = get_workflow_run(&state, &run.id).expect("read back");
        assert_eq!((kept.gate_exit, kept.gate_collected), (Some(0), Some(400)));

        let replaced = rescore_workflow_run_gate(&state, &run.id, 1, None).expect("deliberate");
        assert_eq!((replaced.gate_exit, replaced.gate_collected), (Some(1), None));
    }

    /// The run a sitting champion was promoted on cannot be re-scored at all — not even
    /// deliberately. Editing the evidence under a promotion that already happened is retroactive
    /// falsification, not correction.
    #[test]
    fn the_run_a_champion_was_promoted_on_cannot_be_rescored() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &run.id, None, true).expect("seed champion");

        let refused = rescore_workflow_run_gate(&state, &run.id, 1, None)
            .expect_err("rescoring a champion's evidence must be refused");
        assert!(refused.contains("cannot be rewritten underneath it"), "got: {}", refused);
        assert_eq!(
            get_workflow_run(&state, &run.id).unwrap().gate_exit,
            Some(0),
            "the champion's evidence is unchanged"
        );
    }

    #[test]
    fn recording_a_gate_result_for_an_unknown_run_is_not_found() {
        let (state, _root) = test_state();
        let err = record_workflow_run_gate(&state, "nope", 0, None).expect_err("unknown run");
        assert!(err.contains("not found"), "got: {}", err);
    }

    // -------------------------------------------------------------------------------------
    // Comparability
    // -------------------------------------------------------------------------------------

    #[test]
    fn parse_seat_map_drops_zero_seats_and_refuses_garbage() {
        assert_eq!(
            parse_seat_map(r#"{"glm":3,"qwen":0}"#).unwrap(),
            parse_seat_map(r#"{"glm":3}"#).unwrap(),
            "an explicit zero and an absent model are the same budget"
        );
        assert!(parse_seat_map("[]").is_err(), "an array is not a budget");
        assert!(parse_seat_map("nope").is_err());
        assert!(parse_seat_map(r#"{"glm":-1}"#).is_err());
        assert!(parse_seat_map(r#"{"glm":"three"}"#).is_err());
    }

    #[test]
    fn budgets_are_comparable_is_key_order_insensitive_and_fails_closed() {
        assert!(budgets_are_comparable(
            r#"{"glm":3,"deepseek":1}"#,
            r#"{"deepseek":1,"glm":3}"#
        ));
        assert!(!budgets_are_comparable(r#"{"glm":3}"#, r#"{"glm":4}"#));
        assert!(
            !budgets_are_comparable(r#"{"glm":3}"#, "garbage"),
            "an unreadable budget must fail closed, never compare equal"
        );
    }

    /// `comparable` is computed from the runs, never supplied by the caller.
    #[test]
    fn record_pref_marks_mismatched_seat_budgets_not_comparable() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let a = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let b = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
        let c = scored_run(&state, &workflow.id, r#"{"glm":8}"#, 1);

        let matched = record_workflow_pref(&state, &a.id, &b.id, Some("a was cleaner".into()))
            .expect("matched budgets");
        assert!(matched.comparable);
        assert_eq!(matched.reason.as_deref(), Some("a was cleaner"));

        let mismatched =
            record_workflow_pref(&state, &a.id, &c.id, None).expect("mismatched budgets");
        assert!(
            !mismatched.comparable,
            "different seat budgets must record comparable = 0"
        );
        // It is still recorded: a non-comparable preference is data, it just cannot promote.
        assert_eq!(
            state
                .db
                .with_conn(|conn| conn
                    .query_row("SELECT COUNT(*) FROM workflow_prefs", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|e| e.to_string()))
                .unwrap(),
            2
        );
    }

    /// **Recording the same ordered pair twice returns the SAME row.**
    ///
    /// Without this, a client that reloaded the page could not recover the `prefId` it needed in
    /// order to promote, so the only route forward was to record the preference again — duplicate
    /// rows for one head-to-head, in the table the design calls the project-level measurement.
    #[test]
    fn recording_the_same_pair_twice_returns_the_same_preference() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let a = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let b = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);

        let first = record_workflow_pref(&state, &a.id, &b.id, Some("a was cleaner".into()))
            .expect("first");
        for attempt in 0..4 {
            let again =
                record_workflow_pref(&state, &a.id, &b.id, Some(format!("different text {attempt}")))
                    .expect("again");
            assert_eq!(again.id, first.id, "attempt {attempt}: same row");
            assert_eq!(
                again.reason.as_deref(),
                Some("a was cleaner"),
                "the FIRST reason wins: a recorded outcome is not editable"
            );
            assert_eq!(again.created_at, first.created_at);
            assert_eq!(again.comparable, first.comparable);
        }
        assert_eq!(pref_count(&state), 1, "five calls, one row");

        // The REVERSE pair is a different ordered pair — a contradiction, not a duplicate — and is
        // deliberately still recordable. It needs a GREEN winner, because a failing run can no
        // longer be recorded as one at all.
        let green_b = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let forward = record_workflow_pref(&state, &a.id, &green_b.id, None).expect("forward");
        let reversed = record_workflow_pref(&state, &green_b.id, &a.id, None).expect("reverse");
        assert_ne!(reversed.id, forward.id, "opposite directions are different rows");
        assert_eq!(pref_count(&state), 3);
    }

    /// The read half: a preference can be discovered without issuing a write.
    #[test]
    fn list_workflow_prefs_returns_every_head_to_head_a_workflow_was_in() {
        let (state, _root) = test_state();
        let mine = seed_workflow(&state, "repo", "dev-phase", "glm");
        let other = seed_workflow(&state, "repo", "dev-phase", "glm");
        let unrelated = seed_workflow(&state, "repo", "ui-phase", "glm");

        assert!(
            list_workflow_prefs(&state, &mine.id).unwrap().is_empty(),
            "no preferences yet"
        );

        let a = scored_run(&state, &mine.id, r#"{"glm":3}"#, 0);
        let b = scored_run(&state, &other.id, r#"{"glm":3}"#, 1);
        let won = record_workflow_pref(&state, &a.id, &b.id, None).expect("won");

        // A preference between two OTHER workflows must not show up here.
        let c = scored_run(&state, &unrelated.id, r#"{"glm":3}"#, 0);
        let d = scored_run(&state, &unrelated.id, r#"{"glm":3}"#, 1);
        record_workflow_pref(&state, &c.id, &d.id, None).expect("unrelated");

        // Both sides count: `mine` is the winner above, and the LOSER below. The other workflow's
        // run has to be GREEN to be a winner at all.
        let b = rescore_workflow_run_gate(&state, &b.id, 0, None).expect("make b green");
        let lost = record_workflow_pref(&state, &b.id, &a.id, None).expect("lost");

        let listed: Vec<String> = list_workflow_prefs(&state, &mine.id)
            .unwrap()
            .into_iter()
            .map(|pref| pref.id)
            .collect();
        assert_eq!(listed.len(), 2, "winner and loser both count: {listed:?}");
        assert!(listed.contains(&won.id) && listed.contains(&lost.id));

        // And "no such workflow" is an error, not an empty list.
        assert!(list_workflow_prefs(&state, "nope")
            .expect_err("unknown workflow")
            .contains("not found"));
    }

    #[test]
    fn a_run_cannot_be_preferred_over_itself() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let err = record_workflow_pref(&state, &run.id, &run.id, None).expect_err("self pref");
        assert!(err.contains("cannot be preferred over itself"), "got: {}", err);
    }

    // -------------------------------------------------------------------------------------
    // Promotion
    // -------------------------------------------------------------------------------------

    #[test]
    fn promote_requires_explicit_human_confirmation() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let err = promote_workflow(&state, &run.id, None, false).expect_err("unconfirmed");
        assert!(err.contains("human-confirmed"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_seeds_the_first_champion_with_no_preference() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 0);
        let promoted = promote_workflow(&state, &run.id, None, true).expect("seed champion");
        assert!(promoted.is_champion);
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![workflow.id]
        );
    }

    #[test]
    fn promote_refuses_a_run_whose_gate_never_ran() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":1}"#);
        assert_eq!(run.gate_exit, None);
        let err = promote_workflow(&state, &run.id, None, true).expect_err("NULL gate_exit");
        assert!(err.contains("no gate result"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_run_whose_gate_failed() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":1}"#, 1);
        let err = promote_workflow(&state, &run.id, None, true).expect_err("non-zero gate_exit");
        assert!(err.contains("failed its gate (exit 1)"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_non_comparable_preference() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let challenger = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let incumbent_run = scored_run(&state, &workflow.id, r#"{"glm":8}"#, 0);
        let pref = record_workflow_pref(&state, &challenger.id, &incumbent_run.id, None)
            .expect("pref across budgets");
        assert!(!pref.comparable);

        let err = promote_workflow(&state, &challenger.id, Some(&pref.id), true)
            .expect_err("non-comparable pref");
        assert!(err.contains("not comparable"), "got: {}", err);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());
    }

    #[test]
    fn promote_refuses_a_preference_that_names_a_different_winner() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let winner = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let loser = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let pref = record_workflow_pref(&state, &winner.id, &loser.id, None).expect("pref");
        assert!(pref.comparable);

        let err =
            promote_workflow(&state, &loser.id, Some(&pref.id), true).expect_err("wrong winner");
        assert!(err.contains("as the winner, not run"), "got: {}", err);
    }

    #[test]
    fn promote_refuses_to_displace_an_incumbent_without_a_comparable_preference() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");
        let new_run = scored_run(&state, &new.id, r#"{"glm":3}"#, 0);
        let err = promote_workflow(&state, &new_run.id, None, true).expect_err("no pref");
        assert!(err.contains("already the champion"), "got: {}", err);
        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![old.id]);
    }

    /// The demote and the promote are one transaction: afterwards there is EXACTLY one champion,
    /// and it is the challenger.
    #[test]
    fn promote_demotes_the_incumbent_in_one_transaction() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");
        let new_run = scored_run(&state, &new.id, r#"{"glm":3}"#, 0);
        let pref = record_workflow_pref(&state, &new_run.id, &old_run.id, Some("faster".into()))
            .expect("pref");
        assert!(pref.comparable);

        let promoted =
            promote_workflow(&state, &new_run.id, Some(&pref.id), true).expect("promote");
        assert!(promoted.is_champion);
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![new.id],
            "exactly one champion, and it is the challenger"
        );
        assert!(!get_workflow(&state, &old.id).unwrap().is_champion);
    }

    /// A refused promotion must leave the incumbent exactly as it was — the refusal happens before
    /// any UPDATE, and the whole thing is one transaction either way.
    #[test]
    fn a_refused_promotion_leaves_the_incumbent_in_place() {
        let (state, _root) = test_state();
        let old = seed_workflow(&state, "repo", "dev-phase", "glm");
        let old_run = scored_run(&state, &old.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &old_run.id, None, true).expect("seed champion");

        let new = seed_workflow(&state, "repo", "dev-phase", "glm");

        // A FAILING run can no longer even be recorded as a winner, so the red-gate case is now
        // refused one step earlier than it used to be.
        let failing = scored_run(&state, &new.id, r#"{"glm":3}"#, 2);
        let refused = record_workflow_pref(&state, &failing.id, &old_run.id, None)
            .expect_err("a failing run cannot be the winner");
        assert!(refused.contains("failed its gate (exit 2)"), "got: {}", refused);
        // And it cannot be promoted either, with no preference to offer.
        promote_workflow(&state, &failing.id, None, true).expect_err("red gate");

        // The subject of this test: a promotion refused on a NON-COMPARABLE preference leaves the
        // incumbent exactly as it was. Green challenger, mismatched seat budget.
        let narrow = scored_run(&state, &new.id, r#"{"glm":1}"#, 0);
        let pref = record_workflow_pref(&state, &narrow.id, &old_run.id, None).expect("pref");
        assert!(!pref.comparable, "different seat budgets");
        promote_workflow(&state, &narrow.id, Some(&pref.id), true)
            .expect_err("a non-comparable preference cannot promote");

        assert_eq!(champion_ids(&state, "repo", "dev-phase", "glm"), vec![old.id]);
        assert_eq!(
            promote_decision_count(&state),
            1,
            "three refusals appended nothing to the decision log"
        );
    }

    /// Re-promoting the current champion is a no-op, not a self-displacement error — and it must
    /// LOG nothing.
    ///
    /// An append here is a promotion that promoted nothing: the decision log and
    /// `workflows.is_champion` then disagree about how many promotions there were (QA measured five
    /// rows for one promotion), and while a promotion was still what cleared a halt, each phantom
    /// row was a free escape from one.
    #[test]
    fn re_promoting_the_current_champion_is_idempotent_and_logs_nothing() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &run.id, None, true).expect("first");
        for _ in 0..4 {
            promote_workflow(&state, &run.id, None, true).expect("again");
        }
        assert_eq!(
            champion_ids(&state, "repo", "dev-phase", "glm"),
            vec![workflow.id.clone()]
        );
        assert_eq!(
            promote_decision_count(&state),
            1,
            "five calls, one real promotion, one row"
        );
    }

    // -------------------------------------------------------------------------------------
    // Rejection and the halt
    // -------------------------------------------------------------------------------------

    #[test]
    fn a_rejection_must_record_a_reason() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let err = reject_workflow_run(&state, &run.id, "   ").expect_err("blank reason");
        assert!(err.contains("must record a reason"), "got: {}", err);
        assert_eq!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .consecutive_rejections,
            0
        );
    }

    /// Three consecutive rejections halt the search, the halt is DATA, and the halt is ENFORCED:
    /// no further run can start in that family.
    #[test]
    fn three_consecutive_rejections_halt_the_search_and_block_new_runs() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");

        for (index, expected) in [(1_i64, false), (2, false), (3, true)] {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            let after = reject_workflow_run(&state, &run.id, &format!("reason {}", index))
                .expect("reject");
            assert_eq!(after.consecutive_rejections, index);
            assert_eq!(
                after.halted, expected,
                "halt at {} (limit {})",
                index, REJECTION_STRIKE_LIMIT
            );
            assert_eq!(after.strike_limit, REJECTION_STRIKE_LIMIT);
        }

        let halted = workflow_search_state(&state, "repo", "dev-phase", "glm").unwrap();
        assert!(halted.halted);
        assert_eq!(
            halted.recent_rejections,
            vec!["reason 3", "reason 2", "reason 1"],
            "newest first, so a halted search can show why"
        );

        let err = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .expect_err("halted search must refuse a new run");
        assert!(err.contains("is halted after 3 consecutive rejections"), "got: {}", err);
    }

    /// The halt is per `(repo, phase_type, family)`. Rejections in one family must not stop
    /// another — otherwise one bad model family freezes the whole arena.
    #[test]
    fn the_halt_is_scoped_to_one_repo_phase_type_family() {
        let (state, _root) = test_state();
        let glm = seed_workflow(&state, "repo", "dev-phase", "glm");
        let qwen = seed_workflow(&state, "repo", "dev-phase", "qwen");
        for index in 1..=3 {
            let run = scored_run(&state, &glm.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("glm {}", index)).expect("reject");
        }
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);
        let qwen_state = workflow_search_state(&state, "repo", "dev-phase", "qwen").unwrap();
        assert_eq!(qwen_state.consecutive_rejections, 0);
        assert!(!qwen_state.halted);
        // And a run in the untouched family still starts.
        start(&state, &qwen.id, r#"{"qwen":3}"#);
    }

    /// "CONSECUTIVE" is the whole point — but a PROMOTION is not what ends a streak.
    ///
    /// It used to be, and that made the halt escapable with no new evidence: a promotion is an
    /// unrelated operation, and re-promoting the sitting champion was a row-level no-op that still
    /// appended a decision and reset the count. Only an explicit, human-confirmed `resume` resets
    /// it now.
    #[test]
    fn a_promotion_does_not_reset_the_streak_but_a_resume_does() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        for index in 1..=2 {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("no {}", index)).expect("reject");
        }
        assert_eq!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .consecutive_rejections,
            2
        );

        // A promotion is allowed while the streak is short of the limit, and it changes nothing
        // about the streak.
        let good = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        promote_workflow(&state, &good.id, None, true).expect("promote");

        let after = workflow_search_state(&state, "repo", "dev-phase", "glm").unwrap();
        assert_eq!(
            after.consecutive_rejections, 2,
            "a promotion must NOT reset the streak"
        );
        assert!(!after.halted, "two strikes is still short of the limit");
        assert_eq!(after.champion_workflow_id.as_deref(), Some(workflow.id.as_str()));

        // A resume is refused while there is nothing to resume — otherwise it is a free reset.
        let premature = resume_workflow_search(
            &state,
            "repo",
            "dev-phase",
            "glm",
            "let us keep going",
            true,
        )
        .expect_err("nothing to resume");
        assert!(premature.contains("is not halted"), "got: {}", premature);

        // The third strike halts, and only then does a resume clear it.
        let third = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
        reject_workflow_run(&state, &third.id, "no 3").expect("reject");
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);

        let resumed = resume_workflow_search(
            &state,
            "repo",
            "dev-phase",
            "glm",
            "the gate was wrong, not the workflow",
            true,
        )
        .expect("resume");
        assert_eq!(resumed.consecutive_rejections, 0, "a resume resets the streak");
        assert!(!resumed.halted);
    }

    /// A resume is human-confirmed and must carry a reason — the same discipline as a rejection,
    /// because it is the decision a reader will most want explained.
    #[test]
    fn a_resume_needs_confirmation_and_a_reason() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        for index in 1..=3 {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("no {}", index)).expect("reject");
        }

        let unconfirmed =
            resume_workflow_search(&state, "repo", "dev-phase", "glm", "because", false)
                .expect_err("unconfirmed");
        assert!(unconfirmed.contains("human-confirmed"), "got: {}", unconfirmed);

        let reasonless = resume_workflow_search(&state, "repo", "dev-phase", "glm", "   ", true)
            .expect_err("blank reason");
        assert!(reasonless.contains("must record a reason"), "got: {}", reasonless);

        assert!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .halted,
            "neither refusal may lift the halt"
        );
    }

    /// Only a confirmed `resume` un-halts a halted search, and a PROMOTION is refused while the
    /// halt is in force.
    ///
    /// This is the sharp form of the hole: a rejection carries a reason precisely because a human
    /// says no to a GREEN run, so the thrice-rejected run is itself promotable on its gate alone.
    /// Promoting it used to clear the halt and make that workflow what J1 executes — the search's
    /// own refusal overturned by the operation it was refusing.
    #[test]
    fn only_a_confirmed_resume_un_halts_a_halted_search() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let mut last_green = None;
        for index in 1..=3 {
            let green = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
            last_green = Some(green.clone());
            reject_workflow_run(&state, &green.id, &format!("no {}", index)).expect("reject");
        }
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);

        let thrice_rejected = last_green.unwrap();
        let refused = promote_workflow(&state, &thrice_rejected.id, None, true)
            .expect_err("a halted search must refuse a promotion");
        assert!(refused.contains("is halted"), "got: {}", refused);
        assert!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .halted,
            "the refused promotion must not have lifted anything"
        );
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());

        resume_workflow_search(&state, "repo", "dev-phase", "glm", "retrying the gate", true)
            .expect("resume");
        assert!(!workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);
        start(&state, &workflow.id, r#"{"glm":3}"#);
        // And now the promotion the halt was blocking is allowed.
        promote_workflow(&state, &thrice_rejected.id, None, true).expect("promote after resume");
    }

    // -------------------------------------------------------------------------------------
    // Reads
    // -------------------------------------------------------------------------------------

    #[test]
    fn list_workflows_filters_on_any_part_of_the_champion_key() {
        let (state, _root) = test_state();
        seed_workflow(&state, "repo", "dev-phase", "glm");
        seed_workflow(&state, "repo", "dev-phase", "glm");
        seed_workflow(&state, "repo", "ui-phase", "glm");
        seed_workflow(&state, "other", "dev-phase", "qwen");

        assert_eq!(list_workflows(&state, None, None, None).unwrap().len(), 4);
        assert_eq!(
            list_workflows(&state, Some("repo"), None, None).unwrap().len(),
            3
        );
        assert_eq!(
            list_workflows(&state, Some("repo"), Some("dev-phase"), Some("glm"))
                .unwrap()
                .len(),
            2
        );
        let newest_first: Vec<i64> = list_workflows(&state, Some("repo"), Some("dev-phase"), Some("glm"))
            .unwrap()
            .into_iter()
            .map(|workflow| workflow.version)
            .collect();
        assert_eq!(newest_first, vec![2, 1]);
    }

    #[test]
    fn list_workflow_runs_distinguishes_no_runs_from_no_such_workflow() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        assert!(list_workflow_runs(&state, &workflow.id).unwrap().is_empty());
        start(&state, &workflow.id, r#"{"glm":3}"#);
        assert_eq!(list_workflow_runs(&state, &workflow.id).unwrap().len(), 1);
        assert!(list_workflow_runs(&state, "nope")
            .expect_err("unknown workflow")
            .contains("not found"));
    }

    /// The insert still validates both snapshots, even though they now come from the host rather
    /// than from a caller: an unreadable `seats_held` would make every preference involving this
    /// run non-comparable, which is a silent dead end rather than an error.
    // =========================================================================================
    // Round 3 — the executor, the identity rules, and the human-text guards
    // =========================================================================================

    /// Insert a bare plan row whose `workspace_path` is a real directory and whose plan-level
    /// `test_commands` are whatever the test wants the gate to be.
    fn seed_plan(state: &AppState, workspace: &std::path::Path, test_commands: Option<&str>) -> String {
        let id = Uuid::new_v4().to_string();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    // The column list mirrors the one the existing plan tests use
                    // (`agent_plans.rs` `block_flips_status_and_unblock_restores_it`), plus
                    // `test_commands`. `initiative_id` and `initiative_status` have to be set:
                    // the plan row MAPPER reads them as non-null (migration 017), so omitting them
                    // fails the read rather than the insert — measured, not guessed, as
                    // "Invalid column type Null at index: 20, name: initiative_status".
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, \
                     worker_provider, reviewer_provider, initiative_id, initiative_status, \
                     test_commands) \
                     VALUES (?1, 'development', 'arena', ?2, ?3, 'claude_code', 'claude_code', \
                     ?1, 'development', ?4)",
                    params![
                        id,
                        workspace.to_string_lossy(),
                        workspace.join("plan").to_string_lossy(),
                        test_commands
                    ],
                )
                .map_err(|e| e.to_string())
            })
            .expect("seed plan");
        id
    }

    fn start_against_plan(state: &AppState, workflow_id: &str, plan_id: &str) -> WorkflowRun {
        start_workflow_run(
            state,
            NewWorkflowRun {
                workflow_id: workflow_id.to_string(),
                plan_id: Some(plan_id.to_string()),
                seed: 0,
                seats: None,
            },
        )
        .expect("start run")
    }

    /// **The loop closes.** This is the test the whole round turns on: an arena run goes from
    /// `gate_exit = NULL` to a real score by EXECUTING the plan's gate, which is what makes
    /// comparison — and therefore the one human decision the arena exists for — reachable at all.
    ///
    /// Before this existed, `record_workflow_run_gate` had no caller anywhere in the tree, so every
    /// run the UI created kept a null score forever and promotion was refused for having none.
    #[tokio::test]
    async fn scoring_a_run_executes_the_plans_gate_and_records_the_exit_code() {
        let (state, _root) = test_state();
        let workspace = tmp_dir("arena-gate");
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");

        // A gate that passes.
        let green_plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));
        let run = start_against_plan(&state, &workflow.id, &green_plan);
        assert_eq!(run.gate_exit, None, "nothing has run yet");

        let scored = score_workflow_run(&state, &run.id, None)
            .await
            .expect("score");
        assert_eq!(scored.gate_exit, Some(0), "the gate passed");
        assert!(scored.ended_at.is_some(), "and the run ended");
        // Persisted, not just returned.
        assert_eq!(
            get_workflow_run(&state, &run.id).unwrap().gate_exit,
            Some(0)
        );

        // And that score is now enough to seed a champion — the loop really is closed.
        promote_workflow(&state, &run.id, None, true).expect("promote on a measured score");

        // A gate that FAILS records the failing exit code, not an error.
        let red_plan = seed_plan(&state, &workspace, Some(r#"["exit 7"]"#));
        let red_run = start_against_plan(&state, &workflow.id, &red_plan);
        let red = score_workflow_run(&state, &red_run.id, None)
            .await
            .expect("a failing gate is a score, not an error");
        assert_eq!(red.gate_exit, Some(7), "the first failing command's code");
        assert!(
            promotion_block(&PromotionFacts {
                run: &red,
                pref: None,
                pref_loser: None,
                pref_loser_same_key: true,
                pref_loser_same_workflow: false,
                incumbent_workflow_id: Some("someone-else"),
                incumbent_run_id: Some("their-run"),
                halted: false,
                run_not_scoreable: None,
            })
            .is_some(),
            "and a failing score cannot promote"
        );
    }

    /// **The `gate_declared` trap.** `TestGateRun` reports `exit_code: 0` both for a gate that ran
    /// everything successfully AND for a plan that declares no gate at all. Recording that 0 would
    /// make an ungated run look like a passing one, and therefore promotable — a false pass of
    /// exactly the kind "the exit code is the only score" exists to prevent.
    ///
    /// A phase with no gate cannot compete. That is the design's own `ui-phase` case.
    #[tokio::test]
    async fn a_plan_with_no_gate_cannot_be_scored_at_all() {
        let (state, _root) = test_state();
        let workspace = tmp_dir("arena-nogate");
        let workflow = seed_workflow(&state, "repo", "ui-phase", "glm");

        for declared in [None, Some("[]"), Some("   ")] {
            let plan = seed_plan(&state, &workspace, declared);
            let run = start_against_plan(&state, &workflow.id, &plan);
            let error = score_workflow_run(&state, &run.id, None)
                .await
                .expect_err("an ungated plan must not be scored");
            assert!(
                error.contains("declares no test gate"),
                "for {:?}: got {}",
                declared,
                error
            );
            assert_eq!(
                get_workflow_run(&state, &run.id).unwrap().gate_exit,
                None,
                "and NO score was recorded — a 0 here would read as a pass"
            );
        }
    }

    /// The executor respects the guards the recorder already carries: a second score must be asked
    /// for by name, and a sitting champion's evidence is immutable even then.
    #[tokio::test]
    async fn the_executor_respects_the_rescore_guards() {
        let (state, _root) = test_state();
        let workspace = tmp_dir("arena-rescore");
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let plan = seed_plan(&state, &workspace, Some(r#"["true"]"#));
        let run = start_against_plan(&state, &workflow.id, &plan);

        score_workflow_run(&state, &run.id, None)
            .await
            .expect("first score");

        let refused = score_workflow_run(&state, &run.id, None)
            .await
            .expect_err("a silent re-score must be refused");
        assert!(
            refused.contains("already has a gate result"),
            "got: {}",
            refused
        );

        // Deliberate re-scoring goes through …
        rescore_workflow_run(&state, &run.id, None)
            .await
            .expect("deliberate rescore");

        // … until the run is the evidence a sitting champion rests on.
        promote_workflow(&state, &run.id, None, true).expect("promote");
        let locked = rescore_workflow_run(&state, &run.id, None)
            .await
            .expect_err("a champion's evidence must not be rewritable");
        assert!(
            locked.contains("cannot be rewritten underneath it"),
            "got: {}",
            locked
        );
    }

    /// **An unscoreable run cannot be promoted, including when it is SEEDING a family.**
    ///
    /// This is the path the comparability rule cannot reach: seeding the first champion needs no
    /// head-to-head, so without the check in `promotion_block` a first champion could be crowned
    /// off a run taken on a busy fleet with nothing ever looking at its contention. Measured as a
    /// gap: reverting the check left every other test green.
    #[test]
    fn an_unscoreable_run_cannot_be_promoted_even_when_seeding() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let contended = start_with(&state, &workflow.id, r#"{"glm":3}"#, r#"{"qwen":4}"#);
        let contended =
            record_workflow_run_gate(&state, &contended.id, 0, None).expect("green gate");
        assert!(!contended.scoreable, "it started on a busy fleet");
        assert!(contended.not_scoreable_reason.is_some());

        // Green gate, no incumbent, no preference — the seeding path, and it must still refuse.
        let error = promote_workflow(&state, &contended.id, None, true)
            .expect_err("a run taken under load cannot be crowned");
        assert!(error.contains("cannot be scored"), "got: {}", error);
        assert!(error.contains("quiet fleet"), "and it must say why: {}", error);
        assert!(champion_ids(&state, "repo", "dev-phase", "glm").is_empty());

        // The same run on a quiet fleet seeds the champion, so the refusal is about contention and
        // not about anything else in the path.
        let quiet = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        assert!(quiet.scoreable);
        promote_workflow(&state, &quiet.id, None, true).expect("a quiet run may be crowned");
    }

    /// **A run must never silently proceed at a narrower width.** Tested as a unit, because the
    /// stand-in `measure_run_budget` always grants exactly what it was asked for, so the branch is
    /// unreachable through the production path until the lease pool lands — and an untestable guard
    /// is a comment.
    #[test]
    fn a_narrower_budget_than_requested_is_refused_not_reduced() {
        let three = BTreeMap::from([("glm".to_string(), 3_i64)]);
        assert_eq!(budget_was_honoured(&three, r#"{"glm":3}"#), None);

        for narrower in [r#"{"glm":1}"#, "{}", r#"{"qwen":3}"#, r#"{"glm":3,"qwen":1}"#] {
            let reason = budget_was_honoured(&three, narrower)
                .unwrap_or_else(|| panic!("{} must not be accepted as 3 glm seats", narrower));
            assert!(reason.contains("must queue"), "for {}: {}", narrower, reason);
        }
        assert!(budget_was_honoured(&three, "not json")
            .expect("unreadable")
            .contains("unreadable"));

        // And the production path still satisfies it, which is what makes the guard inert rather
        // than wrong today.
        let (state, _root) = test_state();
        let budget = measure_run_budget(&state, "glm", 3).expect("measure");
        assert_eq!(budget_was_honoured(&three, &budget.seats_held), None);
    }

    /// A run with no plan has no workspace and no phase, so there is nothing to execute. Refused
    /// rather than scored 0 — a 0 here would read as a pass.
    #[tokio::test]
    async fn scoring_a_run_with_no_plan_is_refused() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = start(&state, &workflow.id, r#"{"glm":3}"#);
        assert_eq!(run.plan_id, None);

        let error = score_workflow_run(&state, &run.id, None)
            .await
            .expect_err("no plan, nothing to run");
        assert!(error.contains("has no plan"), "got: {}", error);
        assert_eq!(
            get_workflow_run(&state, &run.id).unwrap().gate_exit,
            None,
            "and no score was invented"
        );
    }

    /// A halted family neither starts nor SCORES a run. Scoring one would hand a halted search
    /// fresh evidence to promote on the moment it resumes.
    #[tokio::test]
    async fn a_halted_family_cannot_score_a_run() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let target = start(&state, &workflow.id, r#"{"glm":3}"#);
        for index in 1..=3 {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &run.id, &format!("no {}", index)).expect("reject");
        }
        assert!(workflow_search_state(&state, "repo", "dev-phase", "glm")
            .unwrap()
            .halted);

        let error = score_workflow_run(&state, &target.id, None)
            .await
            .expect_err("a halted search must not score");
        assert!(error.contains("is halted"), "got: {}", error);
        // The halt is checked BEFORE the plan, so a halted family refuses without touching a shell.
        assert!(
            !error.contains("has no plan"),
            "the halt must be the first refusal: {}",
            error
        );
    }

    #[tokio::test]
    async fn scoring_an_unknown_run_is_not_found() {
        let (state, _root) = test_state();
        assert!(score_workflow_run(&state, "nope", None)
            .await
            .expect_err("unknown run")
            .contains("not found"));
    }

    /// **The identity rule.** `repo` is derived, and two unrelated checkouts with the same basename
    /// must not collide.
    #[test]
    fn repo_identity_does_not_collide_on_the_basename() {
        let left = repo_identity("/a/johnnyone").expect("left");
        let right = repo_identity("/b/johnnyone").expect("right");
        assert_ne!(
            left, right,
            "two unrelated checkouts called johnnyone must not share a champion"
        );
        assert!(left.starts_with("path:"), "got: {}", left);
        assert!(repo_identity("   ").is_err(), "a blank path has no identity");
    }

    /// A git remote is the identity when there is one, and the spellings of one remote collapse to
    /// a single identity — otherwise normalisation would just move the collision to a split.
    #[test]
    fn a_git_remote_is_one_identity_however_it_is_spelled() {
        let repo = tmp_dir("repo-identity");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("git");
        };
        git(&["init", "-q"]);

        let mut seen = std::collections::BTreeSet::new();
        for url in [
            "git@github.com:Owner/Repo.git",
            "https://github.com/owner/repo",
            "https://user:token@github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
        ] {
            git(&["config", "remote.origin.url", url]);
            let identity = repo_identity(&repo.to_string_lossy()).expect("identity");
            assert_eq!(identity, "git:github.com/owner/repo", "for {}", url);
            seen.insert(identity);
        }
        assert_eq!(seen.len(), 1, "one repo, one identity");

        // A repo with NO remote falls back to the path rather than to a shared empty identity.
        git(&["config", "--unset", "remote.origin.url"]);
        assert!(repo_identity(&repo.to_string_lossy())
            .expect("fallback")
            .starts_with("path:"));
    }

    /// **The phase-type rule.** A closed set, because the key cannot be polluted from any writer.
    #[test]
    fn phase_type_is_validated_against_a_closed_set() {
        for known in KNOWN_PHASE_TYPES {
            assert_eq!(validate_phase_type(known).unwrap(), *known);
        }
        assert_eq!(validate_phase_type("  dev-phase  ").unwrap(), "dev-phase");
        for unknown in ["dev-phse", "DEV-PHASE", "", "dev phase", "migration"] {
            let error = validate_phase_type(unknown).expect_err(unknown);
            assert!(error.contains("not a phase type"), "got: {}", error);
        }

        // And it bites at the write, not only in isolation.
        let (state, _root) = test_state();
        let mut typo = new_version("repo", "dev-phase", "glm");
        typo.phase_type = "dev-phse".to_string();
        assert!(create_workflow_version(&state, typo)
            .expect_err("typo")
            .contains("not a phase type"));
    }

    /// **A failing run cannot be the winner**, and a run that never ran cannot be the loser.
    #[test]
    fn a_preference_needs_a_passing_winner_and_a_scored_loser() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let green = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 0);
        let red = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 101);
        let unscored = start(&state, &workflow.id, r#"{"glm":3}"#);

        // The measured case: a challenger that exited 101 recorded as beating a champion that
        // exited 0. Promotion was already blocked, but the row itself was a false measurement.
        let error = record_workflow_pref(&state, &red.id, &green.id, None)
            .expect_err("a failing winner must be refused");
        assert!(error.contains("failed its gate (exit 101)"), "got: {}", error);

        let never_ran = record_workflow_pref(&state, &unscored.id, &green.id, None)
            .expect_err("an unscored winner must be refused");
        assert!(never_ran.contains("its gate never ran"), "got: {}", never_ran);

        let no_loser = record_workflow_pref(&state, &green.id, &unscored.id, None)
            .expect_err("an unscored loser is not a comparison");
        assert!(no_loser.contains("has no gate result"), "got: {}", no_loser);

        assert_eq!(pref_count(&state), 0, "none of the three was written");

        // The honest direction still works.
        record_workflow_pref(&state, &green.id, &red.id, None).expect("green beats red");
        assert_eq!(pref_count(&state), 1);
    }

    /// **Invisible characters must not satisfy a mandatory reason.** A zero-width space, a BOM and
    /// a NUL all pass `str::trim`, because none of them is Unicode `White_Space` — so a rejection
    /// whose reason renders as NOTHING was accepted and stored, and that reason is the one thing a
    /// halted search has to show a human.
    #[test]
    fn an_invisible_reason_is_not_a_reason() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");

        let invisible = [
            ("zero-width space", "\u{200b}"),
            ("BOM", "\u{feff}"),
            ("NUL", "\0"),
            ("soft hyphen", "\u{00ad}"),
            ("word joiner", "\u{2060}"),
            ("bidi override", "\u{202e}"),
            ("mixed invisible", " \u{200b}\t\u{feff}\n"),
        ];
        for (label, reason) in invisible {
            let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            let error = reject_workflow_run(&state, &run.id, reason)
                .expect_err(label);
            assert!(
                error.contains("must contain visible text"),
                "{}: got {}",
                label,
                error
            );
        }
        assert_eq!(
            workflow_search_state(&state, "repo", "dev-phase", "glm")
                .unwrap()
                .consecutive_rejections,
            0,
            "not one of them counted as a strike"
        );

        // A reason with visible content AND an invisible character is fine — the rule is "contains
        // something visible", not "contains nothing invisible".
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
        reject_workflow_run(&state, &run.id, "too slow\u{200b}").expect("visible enough");

        // And a resume is held to the same standard.
        for index in 2..=3 {
            let other = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);
            reject_workflow_run(&state, &other.id, &format!("no {}", index)).expect("reject");
        }
        assert!(
            resume_workflow_search(&state, "repo", "dev-phase", "glm", "\u{200b}", true)
                .expect_err("invisible resume reason")
                .contains("must contain visible text")
        );
    }

    /// Human text is BOUNDED. Nothing at any layer bounded a reason, so one paste could put a
    /// megabyte in the decision log and then serve it on every read of that family's state.
    #[test]
    fn human_text_is_length_bounded() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let run = scored_run(&state, &workflow.id, r#"{"glm":3}"#, 1);

        let too_long = "x".repeat(MAX_REASON_CHARS + 1);
        let error = reject_workflow_run(&state, &run.id, &too_long).expect_err("over the bound");
        assert!(error.contains("over the"), "got: {}", error);
        // The bound itself is allowed.
        reject_workflow_run(&state, &run.id, &"x".repeat(MAX_REASON_CHARS)).expect("at the bound");

        // The prompt parts are bounded too, on their own much larger limit.
        let mut huge = new_version("repo", "dev-phase", "glm");
        huge.instruction = "y".repeat(MAX_PROMPT_CHARS + 1);
        assert!(create_workflow_version(&state, huge)
            .expect_err("over the prompt bound")
            .contains("over the"));

        // And an invisible-only prompt part is refused by the same single reader.
        let mut invisible = new_version("repo", "dev-phase", "glm");
        invisible.role = "\u{200b}".to_string();
        assert!(create_workflow_version(&state, invisible)
            .expect_err("invisible role")
            .contains("must contain visible text"));
    }

    #[test]
    fn starting_a_run_refuses_an_unreadable_budget_snapshot() {
        let (state, _root) = test_state();
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let request = || NewWorkflowRun {
            workflow_id: workflow.id.clone(),
            plan_id: None,
            seed: 0,
            seats: None,
        };
        let bad = start_workflow_run_with_budget(
            &state,
            request(),
            RunBudget {
                seats_held: "nope".to_string(),
                fleet_busy: "{}".to_string(),
            },
        )
        .expect_err("unreadable seatsHeld");
        assert!(bad.contains("seatsHeld"), "got: {}", bad);

        let bad_fleet = start_workflow_run_with_budget(
            &state,
            request(),
            RunBudget {
                seats_held: "{}".to_string(),
                fleet_busy: "[]".to_string(),
            },
        )
        .expect_err("unreadable fleetBusy");
        assert!(bad_fleet.contains("fleetBusy"), "got: {}", bad_fleet);
    }

    /// **The host measures the budget; the caller cannot supply it.**
    ///
    /// `seats_held` is `{family: seats}` with the family read from the WORKFLOW row, and the seat
    /// count from the workflow's `fan_out` unless the caller overrode it. Nothing a browser can
    /// type reaches either column.
    #[test]
    fn the_host_measures_the_budget_from_the_workflow_not_the_caller() {
        let (state, _root) = test_state();
        let mut input = new_version("repo", "dev-phase", "glm-5.3-flash");
        input.fan_out = 7;
        let workflow = create_workflow_version(&state, input).expect("create");

        // No seats given: the workflow's own declaration, keyed on the workflow's own family.
        let declared = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .expect("start");
        assert_eq!(
            parse_seat_map(&declared.seats_held).unwrap(),
            BTreeMap::from([("glm-5.3-flash".to_string(), 7_i64)]),
            "the family comes from the workflow row and the count from its fan_out"
        );

        // An explicit override is honoured, and still keyed on the workflow's family.
        let overridden = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats: Some(2),
            },
        )
        .expect("start");
        assert_eq!(
            parse_seat_map(&overridden.seats_held).unwrap(),
            BTreeMap::from([("glm-5.3-flash".to_string(), 2_i64)])
        );

        // And the override is bounded at both ends.
        for seats in [0_i64, MAX_FAN_OUT + 1] {
            assert!(start_workflow_run(
                &state,
                NewWorkflowRun {
                    workflow_id: workflow.id.clone(),
                    plan_id: None,
                    seed: 0,
                    seats: Some(seats),
                },
            )
            .is_err());
        }
    }

    /// **A DEFECT PIN of my own, in QA's style.** Until the lease pool is wired in,
    /// `measure_run_budget` reports `{}` for contention on EVERY run, so
    /// `contention_is_comparable` compares `{}` with `{}` and always answers true — comparability
    /// is effectively seats-only again, and the contention half of the rule has nothing real to
    /// read.
    ///
    /// The rule itself is correct and tested (`budgets_are_comparable_is_key_order_insensitive...`,
    /// `qa_a_comparable_requires_matching_fleet_contention`); what is missing is the measurement.
    /// This test asserts the CURRENT, incomplete state so that wiring
    /// `services::leases::LeasePool` into `measure_run_budget` turns it RED and cannot land
    /// silently. When that happens, invert and rename it.
    #[test]
    fn contention_is_unmeasured_until_the_lease_pool_lands() {
        let (state, _root) = test_state();
        let budget = measure_run_budget(&state, "glm", 3).expect("measure");
        assert_eq!(
            budget.fleet_busy, "{}",
            "DEFECT PIN: the host reports no contention for any run, because nothing measures it \
             yet. Wiring services::leases::LeasePool here must break this line."
        );

        // The consequence, spelled out: two runs taken under genuinely different load would be
        // reported identically, so every pair is contention-comparable.
        let workflow = seed_workflow(&state, "repo", "dev-phase", "glm");
        let a = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 0,
                seats: None,
            },
        )
        .expect("start");
        let b = start_workflow_run(
            &state,
            NewWorkflowRun {
                workflow_id: workflow.id.clone(),
                plan_id: None,
                seed: 1,
                seats: None,
            },
        )
        .expect("start");
        assert!(
            a.scoreable && b.scoreable,
            "DEFECT PIN: every run reads as scoreable while contention is unmeasured"
        );
    }
}
