//! The plan agent's MCP tools (Plans tab, initiative 8ad3ae71 phase 02 tasks 03-06; master §5.3).
//!
//! - [`TOOLS`] is the ONE table of tools: `tools/list` serializes it, and `tools/call` validates the
//!   arguments against the SAME `input_schema` before running the tool, so the advertised and the
//!   enforced schema cannot drift (task 04 decision: a small in-house validator, not a crate).
//! - Every tool receives the caller's [`TokenScope`]; no schema has an `initiativeId`/`planId`
//!   property and every object schema is `additionalProperties:false`, so scope can only come from
//!   the token (brief §"MCP security" 3).
//! - Write tools go through `brief_store` (the single write chokepoint, D3) as
//!   `Author::Agent { session_id }`, which records `brief_version_created {author:'agent', sessionId}`.
//! - No tool executes a command or writes a file; `repo_search` is the read-only confined walker
//!   in `services::repo_search`.
//! - [`record_rejection`] is the audit of refused calls (task 06): an `agent_mcp_rejected
//!   {reason, method, tool?, count}` event when the call resolves to a plan, else a `tracing::warn!`
//!   with a global counter. Neither ever carries token material or header values.

use crate::services::brief_model::{
    BriefField, FieldOp, OutlineOp, MAX_FIELD_CHARS, MAX_ITEM_CHARS, MAX_TITLE_CHARS, MAX_WHERE_ITEMS,
};
use crate::services::brief_store::{self, Author};
use crate::services::mcp_tokens::TokenScope;
use crate::services::{agent_plans, repo_search, settings as settings_service};
use crate::state::app_state::AppState;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

// ───────────────────────────── registry ─────────────────────────────

/// What a tool run produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    /// Success: becomes `structuredContent` (always a JSON object) and the text content.
    Ok(Value),
    /// A failure the model can act on (bad field, unknown id, `no change`, frozen): `isError: true`.
    Error(String),
    /// Arguments that pass the schema but break a cross-field rule (exactly one of value/add/remove):
    /// a JSON-RPC `-32602`, audited like a schema failure.
    InvalidArgs(String),
}

/// One MCP tool. `input_schema` is a fn so the table stays `const`.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> Value,
    pub run: fn(&AppState, &TokenScope, &Map<String, Value>) -> ToolOutcome,
}

/// The ten tools of master §5.3, in advertised order.
pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "brief_get",
        description: "Read this plan's brief (six fields + draft outline). Omit `version` for the latest.",
        input_schema: schema_brief_get,
        run: run_brief_get,
    },
    ToolDef {
        name: "brief_update",
        description: "Change ONE brief field. Give exactly one of: `value` (replace; a string, or an array of strings for `where`), `add` (append a line / a `where` item), `remove` (drop an exactly matching line / item). Creates a new version authored by the agent.",
        input_schema: schema_brief_update,
        run: run_brief_update,
    },
    ToolDef {
        name: "brief_versions",
        description: "List this plan's brief versions (metadata only: version, author, note, changed fields, time).",
        input_schema: schema_empty,
        run: run_brief_versions,
    },
    ToolDef {
        name: "outline_add_phase",
        description: "Append a phase to the draft outline. Returns the new phase id (`NN`).",
        input_schema: schema_outline_add_phase,
        run: run_outline_add_phase,
    },
    ToolDef {
        name: "outline_add_task",
        description: "Append a task to a phase of the draft outline. Returns the new task id (`NN/MM`).",
        input_schema: schema_outline_add_task,
        run: run_outline_add_task,
    },
    ToolDef {
        name: "outline_update",
        description: "Rename an outline phase or task by id.",
        input_schema: schema_outline_update,
        run: run_outline_update,
    },
    ToolDef {
        name: "outline_move",
        description: "Move an outline phase or task to `toIndex` within its own parent (ids never change).",
        input_schema: schema_outline_move,
        run: run_outline_move,
    },
    ToolDef {
        name: "outline_delete",
        description: "Delete an outline phase (with its tasks) or a task by id.",
        input_schema: schema_outline_delete,
        run: run_outline_delete,
    },
    ToolDef {
        name: "plan_check",
        description: "Grade the latest brief against the brief-check rules (ok / warn / fail items).",
        input_schema: schema_empty,
        run: run_plan_check,
    },
    ToolDef {
        name: "repo_search",
        description: "Read-only, case-insensitive literal search in this plan's workspace. `path` narrows it to a relative sub-path. Capped output and time.",
        input_schema: schema_repo_search,
        run: run_repo_search,
    },
];

/// Look a tool up by name.
pub fn find_tool(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The `tools/list` result.
pub fn tools_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.input_schema)() }))
        .collect();
    json!({ "tools": tools })
}

/// The `--allowedTools` names the claude CLI uses for these tools on the `j1` server.
pub fn allowed_tool_names() -> Vec<String> {
    TOOLS.iter().map(|t| format!("mcp__j1__{}", t.name)).collect()
}

// ───────────────────────────── schemas ─────────────────────────────

/// Max chars of a change note.
const MAX_NOTE_CHARS: usize = 500;
/// Max chars of an outline id argument.
const MAX_ID_CHARS: usize = 16;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn note_prop() -> Value {
    json!({ "type": "string", "maxLength": MAX_NOTE_CHARS, "description": "Why (shown in the version strip)." })
}

fn title_prop() -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": MAX_TITLE_CHARS, "description": "One line." })
}

fn id_prop(what: &str) -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": MAX_ID_CHARS, "description": what })
}

fn schema_empty() -> Value {
    object(json!({}), &[])
}

fn schema_brief_get() -> Value {
    object(json!({ "version": { "type": "integer", "minimum": 1 } }), &[])
}

fn schema_brief_update() -> Value {
    let fields: Vec<&str> = BriefField::ALL.iter().map(|f| f.as_str()).collect();
    object(
        json!({
            "field": { "type": "string", "enum": fields },
            "value": {
                "type": ["string", "array"],
                "maxLength": MAX_FIELD_CHARS,
                "maxItems": MAX_WHERE_ITEMS,
                "items": { "type": "string", "maxLength": MAX_ITEM_CHARS },
                "description": "Replace the field: a string, or an array of strings for `where`."
            },
            "add": { "type": "string", "minLength": 1, "maxLength": MAX_FIELD_CHARS },
            "remove": { "type": "string", "minLength": 1, "maxLength": MAX_FIELD_CHARS },
            "note": note_prop(),
        }),
        &["field"],
    )
}

fn schema_outline_add_phase() -> Value {
    object(json!({ "title": title_prop(), "note": note_prop() }), &["title"])
}

fn schema_outline_add_task() -> Value {
    object(
        json!({ "phaseId": id_prop("Phase id, e.g. 00"), "title": title_prop(), "note": note_prop() }),
        &["phaseId", "title"],
    )
}

fn schema_outline_update() -> Value {
    object(
        json!({ "id": id_prop("Phase (NN) or task (NN/MM) id"), "title": title_prop(), "note": note_prop() }),
        &["id", "title"],
    )
}

fn schema_outline_move() -> Value {
    object(
        json!({
            "id": id_prop("Phase (NN) or task (NN/MM) id"),
            "toIndex": { "type": "integer", "minimum": 0, "maximum": 1000 },
            "note": note_prop(),
        }),
        &["id", "toIndex"],
    )
}

fn schema_outline_delete() -> Value {
    object(json!({ "id": id_prop("Phase (NN) or task (NN/MM) id"), "note": note_prop() }), &["id"])
}

fn schema_repo_search() -> Value {
    object(
        json!({
            "query": { "type": "string", "minLength": 1, "maxLength": repo_search::MAX_QUERY_CHARS },
            "path": { "type": "string", "maxLength": 512, "description": "Relative to the workspace; no `..`." },
            "maxResults": { "type": "integer", "minimum": 1, "maximum": repo_search::MAX_RESULTS_CAP },
        }),
        &["query"],
    )
}

// ───────────────────────────── validator ─────────────────────────────

/// The schema keywords [`validate`] understands. A schema using anything else is a bug, caught by
/// the `every_schema_uses_supported_keywords` test.
pub const SUPPORTED_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "enum",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "items",
    "maxItems",
    "description",
];

/// Validate `value` against `schema` (the subset in [`SUPPORTED_KEYWORDS`], with JSON Schema
/// semantics: type-specific keywords only apply to values of that type).
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    validate_at(schema, value, "arguments")
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "array" => v.is_array(),
        "boolean" => v.is_boolean(),
        _ => false,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn validate_at(schema: &Value, v: &Value, at: &str) -> Result<(), String> {
    let s = schema.as_object().ok_or_else(|| format!("{}: invalid schema", at))?;
    if let Some(t) = s.get("type") {
        let allowed: Vec<&str> = match t {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !allowed.iter().any(|t| type_matches(t, v)) {
            return Err(format!("{}: expected {}, got {}", at, allowed.join(" or "), type_name(v)));
        }
    }
    if let Some(Value::Array(options)) = s.get("enum") {
        if !options.contains(v) {
            let shown: Vec<String> = options.iter().map(|o| o.to_string()).collect();
            return Err(format!("{}: must be one of {}", at, shown.join(", ")));
        }
    }
    match v {
        Value::String(text) => {
            let n = text.chars().count() as u64;
            if let Some(min) = s.get("minLength").and_then(Value::as_u64) {
                if n < min {
                    return Err(format!("{}: must be at least {} characters", at, min));
                }
            }
            if let Some(max) = s.get("maxLength").and_then(Value::as_u64) {
                if n > max {
                    return Err(format!("{}: must be at most {} characters", at, max));
                }
            }
        }
        Value::Number(num) => {
            if let Some(i) = num.as_i64().map(|i| i as f64).or_else(|| num.as_f64()) {
                if let Some(min) = s.get("minimum").and_then(Value::as_f64) {
                    if i < min {
                        return Err(format!("{}: must be ≥ {}", at, min));
                    }
                }
                if let Some(max) = s.get("maximum").and_then(Value::as_f64) {
                    if i > max {
                        return Err(format!("{}: must be ≤ {}", at, max));
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(max) = s.get("maxItems").and_then(Value::as_u64) {
                if items.len() as u64 > max {
                    return Err(format!("{}: at most {} items", at, max));
                }
            }
            if let Some(item_schema) = s.get("items") {
                for (i, item) in items.iter().enumerate() {
                    validate_at(item_schema, item, &format!("{}[{}]", at, i))?;
                }
            }
        }
        Value::Object(map) => {
            let props = s.get("properties").and_then(Value::as_object);
            if let Some(Value::Array(required)) = s.get("required") {
                for r in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(r) {
                        return Err(format!("{}: missing required property `{}`", at, r));
                    }
                }
            }
            for (k, val) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(prop_schema) => validate_at(prop_schema, val, &format!("{}.{}", at, k))?,
                    None => {
                        if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                            return Err(format!("{}: unknown property `{}`", at, k));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ───────────────────────────── tool bodies ─────────────────────────────

fn agent(scope: &TokenScope) -> Author {
    Author::Agent { session_id: scope.session_id.clone() }
}

fn str_arg<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

fn note_arg(args: &Map<String, Value>) -> String {
    str_arg(args, "note").unwrap_or("").trim().to_string()
}

fn write_result(r: Result<brief_store::WriteResult, String>) -> ToolOutcome {
    match r {
        Ok(r) => ToolOutcome::Ok(json!({ "version": r.version, "changed_fields": r.changed_fields })),
        Err(e) => ToolOutcome::Error(e),
    }
}

fn outline_result(r: Result<(brief_store::WriteResult, String), String>, with_id: bool) -> ToolOutcome {
    match r {
        Ok((r, id)) => {
            let mut v = json!({ "version": r.version, "changed_fields": r.changed_fields });
            if with_id {
                v["id"] = json!(id);
            }
            ToolOutcome::Ok(v)
        }
        Err(e) => ToolOutcome::Error(e),
    }
}

fn run_brief_get(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let version = args.get("version").and_then(Value::as_i64);
    match brief_store::get_version(state, &scope.initiative_id, version) {
        Ok(v) => ToolOutcome::Ok(json!({
            "version": v.version,
            "author": v.author,
            // Snake_case keys: the same names `brief_update.field` takes.
            "fields": serde_json::to_value(&v.fields).unwrap_or(Value::Null),
            "outline": serde_json::to_value(&v.outline).unwrap_or(Value::Null),
            "changed": v.changed,
            "changeNote": v.change_note,
            "createdAt": v.created_at,
        })),
        Err(e) => ToolOutcome::Error(e),
    }
}

fn run_brief_update(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let given: Vec<&str> = ["value", "add", "remove"]
        .into_iter()
        .filter(|k| args.get(*k).is_some_and(|v| !v.is_null()))
        .collect();
    if given.len() != 1 {
        return ToolOutcome::InvalidArgs(
            "arguments: give exactly one of `value`, `add` or `remove`".to_string(),
        );
    }
    let field = match str_arg(args, "field").map(BriefField::parse) {
        Some(Ok(f)) => f,
        Some(Err(e)) => return ToolOutcome::InvalidArgs(e),
        None => return ToolOutcome::InvalidArgs("arguments: missing `field`".to_string()),
    };
    let op = match given[0] {
        "value" => FieldOp::Value(args["value"].clone()),
        "add" => FieldOp::Add(str_arg(args, "add").unwrap_or_default().to_string()),
        _ => FieldOp::Remove(str_arg(args, "remove").unwrap_or_default().to_string()),
    };
    write_result(brief_store::update_field(
        state,
        &scope.initiative_id,
        field,
        op,
        &agent(scope),
        &note_arg(args),
    ))
}

fn run_brief_versions(state: &AppState, scope: &TokenScope, _args: &Map<String, Value>) -> ToolOutcome {
    match brief_store::list_versions(state, &scope.initiative_id) {
        Ok(v) => ToolOutcome::Ok(json!({ "versions": v })),
        Err(e) => ToolOutcome::Error(e),
    }
}

fn outline(state: &AppState, scope: &TokenScope, args: &Map<String, Value>, op: OutlineOp) -> Result<(brief_store::WriteResult, String), String> {
    brief_store::outline_op(state, &scope.initiative_id, op, &agent(scope), &note_arg(args))
}

fn run_outline_add_phase(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let title = str_arg(args, "title").unwrap_or_default().to_string();
    outline_result(outline(state, scope, args, OutlineOp::AddPhase { title }), true)
}

fn run_outline_add_task(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let phase_id = str_arg(args, "phaseId").unwrap_or_default().to_string();
    let title = str_arg(args, "title").unwrap_or_default().to_string();
    outline_result(outline(state, scope, args, OutlineOp::AddTask { phase_id, title }), true)
}

fn run_outline_update(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let id = str_arg(args, "id").unwrap_or_default().to_string();
    let title = str_arg(args, "title").unwrap_or_default().to_string();
    outline_result(outline(state, scope, args, OutlineOp::Update { id, title }), false)
}

fn run_outline_move(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let id = str_arg(args, "id").unwrap_or_default().to_string();
    let to_index = args.get("toIndex").and_then(Value::as_u64).unwrap_or(0) as usize;
    outline_result(outline(state, scope, args, OutlineOp::Move { id, to_index }), false)
}

fn run_outline_delete(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let id = str_arg(args, "id").unwrap_or_default().to_string();
    outline_result(outline(state, scope, args, OutlineOp::Delete { id }), false)
}

fn run_plan_check(state: &AppState, scope: &TokenScope, _args: &Map<String, Value>) -> ToolOutcome {
    match brief_store::check_latest(state, &scope.initiative_id) {
        Ok(items) => ToolOutcome::Ok(json!({ "items": items })),
        Err(e) => ToolOutcome::Error(e),
    }
}

fn run_repo_search(state: &AppState, scope: &TokenScope, args: &Map<String, Value>) -> ToolOutcome {
    let workspace: Option<String> = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT workspace_path FROM agent_plans WHERE id = ?1",
                params![scope.initiative_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .unwrap_or(None);
    let Some(workspace) = workspace else {
        return ToolOutcome::Error("this plan has no workspace".to_string());
    };
    let store = settings_service::resolve_initiatives_dir(state);
    let query = str_arg(args, "query").unwrap_or_default();
    let path = str_arg(args, "path");
    let max = args.get("maxResults").and_then(Value::as_u64).map(|n| n as usize);
    match repo_search::repo_search(std::path::Path::new(&workspace), &store, query, path, max) {
        Ok(r) => ToolOutcome::Ok(serde_json::to_value(r).unwrap_or(Value::Null)),
        Err(e) => ToolOutcome::Error(e),
    }
}

// ───────────────────────────── audit (task 06) ─────────────────────────────

/// Rejections that resolve to no plan (missing / malformed / unknown token, transport guard).
static PLANLESS_REJECTIONS: AtomicU64 = AtomicU64::new(0);

/// The JSON-RPC methods this server knows. Anything else is recorded as `other`.
const KNOWN_METHODS: &[&str] = &["initialize", "ping", "tools/list", "tools/call"];

/// The audit label of a client-supplied method: a known method verbatim, `notifications/*`, `-` for
/// none, else `other`. Client text is NEVER echoed into an event — an agent could put anything in a
/// method name, including its own token.
fn method_label(method: &str) -> &'static str {
    if method.is_empty() || method == "-" {
        "-"
    } else if method.starts_with("notifications/") {
        "notifications/*"
    } else {
        KNOWN_METHODS.iter().find(|m| **m == method).copied().unwrap_or("other")
    }
}

/// The audit label of a client-supplied tool name: a registered tool verbatim, else `unknown`.
fn tool_label(tool: &str) -> &'static str {
    find_tool(tool).map(|t| t.name).unwrap_or("unknown")
}

/// Bump and return the plan's rejection counter, seeding it from the event table on first use (so
/// it survives an engine restart).
fn bump_plan_counter(state: &AppState, initiative_id: &str) -> u64 {
    let mut map = state.mcp_rejections.lock().unwrap_or_else(|p| p.into_inner());
    if !map.contains_key(initiative_id) {
        let seeded: i64 = state
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM agent_plan_events WHERE plan_id = ?1 AND type = 'agent_mcp_rejected'",
                    params![initiative_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap_or(0);
        map.insert(initiative_id.to_string(), seeded.max(0) as u64);
    }
    let n = map.get_mut(initiative_id).expect("seeded above");
    *n += 1;
    *n
}

/// Audit one refused MCP call. `reason` is a fixed enum string (`revoked`, `session_inactive`,
/// `session_replaced`, `not_briefing`, `invalid_arguments`, `unknown_tool`, `unknown_method`, the
/// token reasons and the transport reasons). Returns the count it recorded.
pub fn record_rejection(
    state: &AppState,
    initiative_id: Option<&str>,
    reason: &'static str,
    method: &str,
    tool: Option<&str>,
) -> u64 {
    match initiative_id {
        Some(id) => {
            let count = bump_plan_counter(state, id);
            let mut payload = json!({ "reason": reason, "method": method_label(method), "count": count });
            if let Some(t) = tool {
                payload["tool"] = json!(tool_label(t));
            }
            if let Err(error) = agent_plans::append_event(state, id, None, "agent_mcp_rejected", payload) {
                tracing::warn!(target: "mcp", initiative_id = %id, %error, "failed to record an MCP rejection");
            }
            count
        }
        None => {
            let count = PLANLESS_REJECTIONS.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(target: "mcp", reason, count, "MCP call rejected");
            count
        }
    }
}

/// Test-only: does `payload` (an event payload, a log line) contain `secret`?
#[cfg(test)]
pub(crate) fn redact_payload_has_no_secret(payload: &str, secret: &str) -> bool {
    secret.is_empty() || !payload.contains(secret)
}

// ───────────────────────────── test fixtures ─────────────────────────────

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::services::mcp_tokens;
    use crate::test_support::tmp_dir;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// A real draft plan (briefing row + plan-agent chat session + v1) with a minted token.
    pub(crate) struct Plan {
        pub id: String,
        pub session: String,
        pub token: String,
        pub ws: PathBuf,
        pub store: PathBuf,
    }

    impl Plan {
        pub(crate) fn bearer(&self) -> String {
            format!("Bearer {}", self.token)
        }
    }

    /// Point the initiatives dir at a fresh canonical temp dir; returns it.
    pub(crate) fn use_store(state: &AppState) -> PathBuf {
        let store = tmp_dir("mcp-store").canonicalize().unwrap();
        settings_service::set_setting(
            state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();
        store
    }

    /// Create a draft plan in a fresh workspace and mint its token (no config file, no CLI).
    pub(crate) async fn plan(state: &AppState, store: &std::path::Path, title: &str) -> Plan {
        let ws = tmp_dir("mcp-ws").canonicalize().unwrap();
        let run = brief_store::create_draft_plan(
            state,
            brief_store::CreateDraftPlanInput {
                title: title.into(),
                workspace_path: ws.to_string_lossy().to_string(),
                ask: Some(format!("{title} outcome.\nBecause.")),
            },
        )
        .await
        .expect("create_draft_plan");
        let id = run.plan.id.clone();
        let session = run.plan.briefing_session_id.clone().expect("plan-agent session");
        let raw = mcp_tokens::mint(state, &id, &session).unwrap();
        Plan { id, session, token: raw.expose().to_string(), ws, store: store.to_path_buf() }
    }

    pub(crate) fn version_count(state: &AppState, id: &str) -> i64 {
        brief_store::version_count(state, id).unwrap()
    }

    pub(crate) fn events(state: &AppState, id: &str, ty: &str) -> Vec<Value> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare("SELECT payload_json FROM agent_plan_events WHERE plan_id = ?1 AND type = ?2 ORDER BY rowid")
                    .unwrap();
                let rows = stmt
                    .query_map(params![id, ty], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
                    .collect();
                Ok(rows)
            })
            .unwrap()
    }

    /// Every event payload of every plan, raw.
    pub(crate) fn all_event_payloads(state: &AppState) -> Vec<String> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT payload_json FROM agent_plan_events").unwrap();
                let rows = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(|s| s.unwrap())
                    .collect();
                Ok(rows)
            })
            .unwrap()
    }

    /// A `tracing` subscriber that writes every event (all levels) into a shared buffer.
    #[derive(Clone, Default)]
    pub(crate) struct LogBuf(pub Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
        type Writer = LogBuf;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl LogBuf {
        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).to_string()
        }
        pub(crate) fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
            tracing_subscriber::fmt()
                .with_writer(self.clone())
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .finish()
        }
    }

    /// Serialises the log-capture tests (one capturing dispatcher at a time).
    static LOG_CAPTURE: Mutex<()> = Mutex::new(());

    /// A no-op dispatcher kept alive for the whole test process. tracing-core (0.1.36,
    /// `Dispatchers::register_dispatch`) switches to a "just one dispatcher" fast path while at most
    /// ONE scoped dispatcher is registered, and in that mode a callsite first hit on ANOTHER test
    /// thread computes its interest from that thread's (no-op) default and caches "never" — the
    /// capture then silently misses it. With a second registered dispatcher every interest is
    /// computed over all of them, so the capturing one is always consulted.
    static KEEP_TWO_DISPATCHERS: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();

    /// Run `f` on a fresh current-thread runtime with a buffer subscriber as this thread's default
    /// (every level). Work `handle_mcp` moves to `spawn_blocking` carries the dispatcher with it.
    pub(crate) fn capture_logs<R>(f: impl FnOnce(&tokio::runtime::Runtime) -> R) -> (R, String) {
        let _serial = LOG_CAPTURE.lock().unwrap_or_else(|p| p.into_inner());
        KEEP_TWO_DISPATCHERS.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
        let logs = LogBuf::default();
        let out = tracing::subscriber::with_default(logs.subscriber(), || {
            tracing::callsite::rebuild_interest_cache();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            f(&rt)
        });
        (out, logs.text())
    }

    /// File tree snapshot: `(relative path, mtime)` of every file and dir under `root`.
    pub(crate) fn tree(root: &std::path::Path) -> Vec<(String, Option<std::time::SystemTime>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let meta = std::fs::symlink_metadata(&p).ok();
                out.push((
                    p.strip_prefix(root).unwrap().to_string_lossy().to_string(),
                    meta.as_ref().and_then(|m| m.modified().ok()),
                ));
                if meta.map(|m| m.is_dir()).unwrap_or(false) {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;
    use crate::host::mcp::handle_mcp;
    use crate::test_support::test_state;

    fn call_body(tool: &str, args: Value) -> Vec<u8> {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } })
            .to_string()
            .into_bytes()
    }

    async fn call(state: &AppState, p: &Plan, tool: &str, args: Value) -> Value {
        let r = handle_mcp(state, Some(&p.bearer()), &call_body(tool, args)).await;
        assert_eq!(r.status, 200, "{:?}", r.body);
        r.body.expect("body")
    }

    /// A successful call's structured content.
    fn ok(v: &Value) -> &Value {
        assert_eq!(v["result"]["isError"], json!(false), "{v}");
        &v["result"]["structuredContent"]
    }

    /// Valid arguments for every tool (for tables).
    fn good_args(tool: &str) -> Value {
        match tool {
            "brief_get" => json!({}),
            "brief_update" => json!({ "field": "why", "add": "a new reason" }),
            "brief_versions" => json!({}),
            "outline_add_phase" => json!({ "title": "Phase" }),
            "outline_add_task" => json!({ "phaseId": "00", "title": "Task" }),
            "outline_update" => json!({ "id": "00", "title": "Renamed" }),
            "outline_move" => json!({ "id": "00", "toIndex": 0 }),
            "outline_delete" => json!({ "id": "00" }),
            "plan_check" => json!({}),
            "repo_search" => json!({ "query": "needle" }),
            other => panic!("no args for {other}"),
        }
    }

    #[test]
    fn every_schema_uses_supported_keywords() {
        fn walk(tool: &str, schema: &Value) {
            let obj = schema.as_object().unwrap_or_else(|| panic!("{tool}: schema node is not an object"));
            for (k, v) in obj {
                assert!(SUPPORTED_KEYWORDS.contains(&k.as_str()), "{tool}: unsupported keyword `{k}`");
                match k.as_str() {
                    "properties" => v.as_object().unwrap().values().for_each(|p| walk(tool, p)),
                    "items" => walk(tool, v),
                    _ => {}
                }
            }
        }
        for t in TOOLS {
            let s = (t.input_schema)();
            assert_eq!(s["type"], "object", "{}", t.name);
            assert_eq!(s["additionalProperties"], json!(false), "{}", t.name);
            let props = s["properties"].as_object().unwrap();
            for banned in ["initiativeId", "initiative_id", "planId", "plan_id"] {
                assert!(!props.contains_key(banned), "{} exposes {banned}", t.name);
            }
            walk(t.name, &s);
        }
        assert_eq!(TOOLS.len(), 10);
    }

    #[tokio::test]
    async fn tools_call_brief_update_increments_version_author_agent() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let before = version_count(&state, &p.id);
        let v = call(&state, &p, "brief_update", json!({ "field": "outcome", "value": "Ship the MCP server.", "note": "sharper" })).await;
        let sc = ok(&v);
        assert_eq!(sc["version"], json!(before + 1));
        assert_eq!(sc["changed_fields"], json!(["outcome"]));
        assert_eq!(version_count(&state, &p.id), before + 1);
        let author: String = state
            .db
            .with_conn(|c| {
                c.query_row(
                    "SELECT author FROM brief_versions WHERE initiative_id = ?1 ORDER BY version DESC LIMIT 1",
                    params![p.id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(author, "agent");
        let ev = events(&state, &p.id, "brief_version_created");
        let last = ev.last().unwrap();
        assert_eq!(last["author"], "agent");
        assert_eq!(last["sessionId"], json!(p.session));
        assert_eq!(last["version"], json!(before + 1));
        // Text content mirrors the structured content.
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), *sc);
    }

    #[tokio::test]
    async fn token_a_cannot_touch_plan_b() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let a = plan(&state, &store, "A").await;
        let b = plan(&state, &store, "B").await;
        let b_before = version_count(&state, &b.id);
        let b_brief = brief_store::get_version(&state, &b.id, None).unwrap();
        let a_before = version_count(&state, &a.id);
        // Every write tool through A's token.
        let writes = [
            ("brief_update", json!({ "field": "why", "value": "A only" })),
            ("outline_add_phase", json!({ "title": "A phase" })),
            ("outline_add_task", json!({ "phaseId": "00", "title": "A task" })),
            ("outline_update", json!({ "id": "00", "title": "A renamed" })),
            ("outline_add_phase", json!({ "title": "A phase 2" })),
            ("outline_move", json!({ "id": "01", "toIndex": 0 })),
            ("outline_delete", json!({ "id": "01" })),
        ];
        for (tool, args) in writes {
            ok(&call(&state, &a, tool, args).await);
        }
        assert_eq!(version_count(&state, &a.id), a_before + 7);
        assert_eq!(version_count(&state, &b.id), b_before, "B unchanged");
        assert_eq!(brief_store::get_version(&state, &b.id, None).unwrap(), b_brief);
        // Reads through A's token see A, never B.
        let got = call(&state, &a, "brief_get", json!({})).await;
        assert_eq!(ok(&got)["fields"]["why"], "A only");
        // Smuggling B's id is a validation error, not a cross-plan write.
        let r = call(&state, &a, "brief_update", json!({ "field": "why", "value": "x", "initiativeId": b.id })).await;
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(version_count(&state, &b.id), b_before);
    }

    #[tokio::test]
    async fn initiative_id_argument_is_a_validation_error() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let before = version_count(&state, &p.id);
        for key in ["initiativeId", "planId", "initiative_id"] {
            for t in TOOLS {
                let mut args = good_args(t.name);
                args[key] = json!(p.id);
                let r = call(&state, &p, t.name, args).await;
                assert_eq!(r["error"]["code"], -32602, "{} + {key}", t.name);
                let msg = r["error"]["message"].as_str().unwrap();
                assert!(msg.contains(&format!("unknown property `{key}`")), "{msg}");
            }
        }
        assert_eq!(version_count(&state, &p.id), before);
    }

    #[tokio::test]
    async fn every_tool_rejects_unknown_and_mistyped_args() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let before = version_count(&state, &p.id);
        let mistyped: Vec<(&str, Value)> = vec![
            ("brief_get", json!({ "version": "1" })),
            ("brief_get", json!({ "version": 0 })),
            ("brief_update", json!({ "field": "nope", "value": "x" })),
            ("brief_update", json!({ "field": "why", "value": 3 })),
            ("brief_update", json!({ "field": "why" })),
            ("brief_update", json!({ "field": "why", "add": "a", "remove": "b" })),
            ("brief_update", json!({ "field": "why", "value": "x".repeat(MAX_FIELD_CHARS + 1) })),
            ("brief_update", json!({ "field": "where", "value": [1, 2] })),
            ("brief_update", json!({ "value": "x" })),
            ("brief_versions", json!({ "x": 1 })),
            ("outline_add_phase", json!({ "title": "" })),
            ("outline_add_phase", json!({ "title": 5 })),
            ("outline_add_phase", json!({ "title": "t".repeat(MAX_TITLE_CHARS + 1) })),
            ("outline_add_task", json!({ "title": "t" })),
            ("outline_update", json!({ "id": "00" })),
            ("outline_move", json!({ "id": "00", "toIndex": -1 })),
            ("outline_move", json!({ "id": "00", "toIndex": 1.5 })),
            ("outline_delete", json!({ "id": ["00"] })),
            ("plan_check", json!({ "verbose": true })),
            ("repo_search", json!({ "query": "" })),
            ("repo_search", json!({ "query": "x", "maxResults": 201 })),
            ("repo_search", json!({ "query": "x", "path": 3 })),
        ];
        for (tool, args) in mistyped {
            let r = call(&state, &p, tool, args.clone()).await;
            assert_eq!(r["error"]["code"], -32602, "{tool} {args}");
        }
        for t in TOOLS {
            let mut args = good_args(t.name);
            args["bogus"] = json!(1);
            let r = call(&state, &p, t.name, args).await;
            assert_eq!(r["error"]["code"], -32602, "{} + unknown key", t.name);
            // `arguments` itself must be an object.
            let r = call(&state, &p, t.name, json!(["not", "an", "object"])).await;
            assert_eq!(r["error"]["code"], -32602, "{} array args", t.name);
        }
        assert_eq!(version_count(&state, &p.id), before, "no rejected call wrote");
        let rejected = events(&state, &p.id, "agent_mcp_rejected");
        assert!(rejected.iter().all(|e| e["reason"] == "invalid_arguments"));
        assert!(rejected.iter().all(|e| e.get("tool").is_some()));
    }

    #[tokio::test]
    async fn outline_tools_roundtrip() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        let r = call(&state, &p, "outline_add_phase", json!({ "title": "Store" })).await;
        assert_eq!(ok(&r)["id"], "00");
        let r = call(&state, &p, "outline_add_phase", json!({ "title": "API" })).await;
        assert_eq!(ok(&r)["id"], "01");
        let r = call(&state, &p, "outline_add_task", json!({ "phaseId": "00", "title": "Migration" })).await;
        assert_eq!(ok(&r)["id"], "00/01");
        let r = call(&state, &p, "outline_update", json!({ "id": "00/01", "title": "Migration 031" })).await;
        assert_eq!(ok(&r)["changed_fields"], json!(["outline:00/01"]));
        assert!(ok(&r).get("id").is_none());
        ok(&call(&state, &p, "outline_move", json!({ "id": "01", "toIndex": 0 })).await);
        let got = call(&state, &p, "brief_get", json!({})).await;
        let phases = &ok(&got)["outline"]["phases"];
        assert_eq!(phases[0]["id"], "01");
        assert_eq!(phases[1]["tasks"][0]["title"], "Migration 031");
        assert_eq!(phases[1]["tasks"][0]["author"], "agent");
        ok(&call(&state, &p, "outline_delete", json!({ "id": "01" })).await);
        // Tool-level failures are isError results the model can act on, not protocol errors.
        let r = call(&state, &p, "outline_delete", json!({ "id": "77" })).await;
        assert_eq!(r["result"]["isError"], json!(true));
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown outline id"));
        let r = call(&state, &p, "outline_move", json!({ "id": "00", "toIndex": 9 })).await;
        assert_eq!(r["result"]["isError"], json!(true));
        let r = call(&state, &p, "brief_update", json!({ "field": "why", "value": "Because." })).await;
        assert_eq!(r["result"]["isError"], json!(true), "no change → isError");
        let versions = call(&state, &p, "brief_versions", json!({})).await;
        let list = ok(&versions)["versions"].as_array().unwrap().clone();
        assert_eq!(list.len() as i64, version_count(&state, &p.id));
        assert_eq!(list.last().unwrap()["author"], "agent");
        // brief_get of an old version.
        let v1 = call(&state, &p, "brief_get", json!({ "version": 1 })).await;
        assert_eq!(ok(&v1)["version"], 1);
        assert_eq!(ok(&v1)["author"], "you");
    }

    #[tokio::test]
    async fn plan_check_tool_matches_brief_check() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        ok(&call(&state, &p, "brief_update", json!({ "field": "done_when", "value": "cargo test -j 8 --lib" })).await);
        let r = call(&state, &p, "plan_check", json!({})).await;
        let direct = brief_store::check_latest(&state, &p.id).unwrap();
        assert!(!direct.is_empty());
        assert_eq!(ok(&r)["items"], serde_json::to_value(&direct).unwrap());
    }

    #[tokio::test]
    async fn no_tool_writes_outside_db() {
        let (state, _root) = test_state();
        let store = use_store(&state);
        let p = plan(&state, &store, "A").await;
        crate::test_support::write_fixture_file(&p.ws.join("code.rs"), "// needle\n");
        let ws_before = tree(&p.ws);
        let store_before = tree(&store);
        std::thread::sleep(std::time::Duration::from_millis(20));
        for t in TOOLS {
            let r = call(&state, &p, t.name, good_args(t.name)).await;
            assert!(r.get("result").is_some(), "{}: {r}", t.name);
        }
        assert_eq!(tree(&p.ws), ws_before, "workspace untouched");
        assert_eq!(tree(&store), store_before, "plan store untouched");
    }

    #[test]
    fn validator_basics() {
        let s = schema_brief_update();
        assert!(validate(&s, &json!({ "field": "where", "value": ["a", "b"] })).is_ok());
        assert!(validate(&s, &json!({ "field": "why", "value": "x" })).is_ok());
        assert!(validate(&s, &json!({ "field": "where", "value": vec!["w"; MAX_WHERE_ITEMS + 1] })).is_err());
        assert!(validate(&s, &json!({ "field": "where", "value": ["p".repeat(MAX_ITEM_CHARS + 1)] })).is_err());
        assert!(validate(&s, &json!(null)).is_err());
        assert_eq!(allowed_tool_names().len(), 10);
        assert!(allowed_tool_names().iter().all(|n| n.starts_with("mcp__j1__")));
        assert!(redact_payload_has_no_secret("{\"reason\":\"revoked\"}", "deadbeef"));
        assert!(!redact_payload_has_no_secret("x deadbeef y", "deadbeef"));
    }
}
