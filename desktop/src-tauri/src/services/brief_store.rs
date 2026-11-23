//! The versioned brief store (Plans tab, initiative 8ad3ae71 phase 00 task 05).
//!
//! A briefing Initiative owns an APPEND-ONLY history in `brief_versions` (migration 031). Every
//! change — a human edit over GraphQL/RPC (author `you`) or the plan agent over MCP (author
//! `agent`) — goes through the ONE chokepoint [`write_version`]: inside one `BEGIN IMMEDIATE`
//! transaction it checks the plan is still in `briefing`, reads the latest version, applies the
//! caller's pure transformation (from `brief_model`), refuses a no-op, inserts version n+1 with the
//! changed fields, and re-renders `agent_plans.brief`; after COMMIT it appends a
//! `brief_version_created` event (which also pushes `agent_plan_run_updated` to the web).
//!
//! Nothing here UPDATEs a `brief_versions` row: an undo/revert is a NEW version copying old content.

use crate::services::agent_plans::append_event;
use crate::services::brief_model::{
    apply_field_op, apply_outline_op, changed_fields, changed_outline_ids, copy_field,
    render_brief_md, BriefField, BriefFields, FieldOp, Outline, OutlineOp,
};
use crate::state::app_state::AppState;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::json;

// ───────────────────────────── types ─────────────────────────────

/// Who made a change. Stored as `you` / `agent`; the agent's session id goes into the event
/// payload only (never into `brief_versions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    You,
    Agent { session_id: String },
}

impl Author {
    pub fn as_str(&self) -> &'static str {
        match self {
            Author::You => "you",
            Author::Agent { .. } => "agent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub version: i64,
    pub changed_fields: Vec<String>,
}

/// One full version, plus the plan state a reader needs to know whether it may still edit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefVersion {
    pub initiative_id: String,
    pub version: i64,
    pub author: String,
    /// Serialized in the GraphQL shape (`doneWhen`, `outOfScope`); the stored `fields_json` and the
    /// field KEYS used as arguments stay snake_case (`done_when`).
    #[serde(serialize_with = "serialize_fields_wire")]
    pub fields: BriefFields,
    pub outline: Outline,
    pub changed: Vec<String>,
    pub change_note: String,
    pub created_at: String,
    pub accepted_brief_version: Option<i64>,
    pub initiative_status: String,
}

/// The wire (GraphQL `BriefFields`) shape of the six fields.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BriefFieldsWire<'a> {
    outcome: &'a str,
    why: &'a str,
    #[serde(rename = "where")]
    where_: &'a [String],
    done_when: &'a str,
    out_of_scope: &'a str,
    decided: &'a str,
}

fn serialize_fields_wire<S: serde::Serializer>(f: &BriefFields, s: S) -> Result<S::Ok, S::Error> {
    BriefFieldsWire {
        outcome: &f.outcome,
        why: &f.why,
        where_: &f.where_,
        done_when: &f.done_when,
        out_of_scope: &f.out_of_scope,
        decided: &f.decided,
    }
    .serialize(s)
}

/// Version metadata only (the version strip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefVersionMeta {
    pub version: i64,
    pub author: String,
    pub change_note: String,
    pub changed: Vec<String>,
    pub created_at: String,
}

// ───────────────────────────── helpers ─────────────────────────────

/// The frozen-brief refusal. Exact wording is part of the contract (MCP returns it to the agent).
pub fn frozen_error(version: i64) -> String {
    format!("the plan left briefing; the brief is frozen at v{}", version)
}

struct Latest {
    version: i64,
    fields: BriefFields,
    outline: Outline,
}

fn parse_fields(s: &str) -> Result<BriefFields, String> {
    serde_json::from_str(s).map_err(|e| format!("corrupt brief fields_json: {}", e))
}

fn parse_outline(s: &str) -> Result<Outline, String> {
    serde_json::from_str(s).map_err(|e| format!("corrupt brief outline_json: {}", e))
}

fn latest_in(conn: &Connection, id: &str) -> Result<Option<Latest>, String> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT version, fields_json, outline_json FROM brief_versions WHERE initiative_id = ?1 ORDER BY version DESC LIMIT 1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match row {
        None => Ok(None),
        Some((version, f, o)) => Ok(Some(Latest {
            version,
            fields: parse_fields(&f)?,
            outline: parse_outline(&o)?,
        })),
    }
}

/// `(initiative_status, accepted_brief_version)` of the plan row, or a not-found error.
fn plan_state(conn: &Connection, id: &str) -> Result<(String, Option<i64>), String> {
    conn.query_row(
        "SELECT initiative_status, accepted_brief_version FROM agent_plans WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("plan {} not found", id))
}

/// Refuse unless the plan is in `briefing`. The frozen version reported is the accepted one when
/// recorded, else the latest written.
fn assert_briefing(conn: &Connection, id: &str) -> Result<(), String> {
    let (status, accepted) = plan_state(conn, id)?;
    if status != "briefing" {
        let latest = latest_in(conn, id)?.map(|l| l.version).unwrap_or(0);
        return Err(frozen_error(accepted.unwrap_or(latest)));
    }
    Ok(())
}

fn event_payload(version: i64, author: &Author, changed: &[String], note: &str) -> serde_json::Value {
    let mut payload = json!({
        "version": version,
        "author": author.as_str(),
        "changedFields": changed,
        "note": note,
    });
    if let Author::Agent { session_id } = author {
        payload["sessionId"] = json!(session_id);
    }
    payload
}

// ───────────────────────────── the chokepoint ─────────────────────────────

/// THE single write path. `f` receives the latest fields + outline and the version number this
/// write will get (for stamping `added_in` on new outline items) and returns the new content.
/// Errors: plan not in briefing (`the plan left briefing; the brief is frozen at v<n>`), `f`'s own
/// error, or `no change` when nothing differs (no row is written).
pub fn write_version<F>(
    state: &AppState,
    initiative_id: &str,
    author: &Author,
    note: &str,
    f: F,
) -> Result<WriteResult, String>
where
    F: FnOnce(&BriefFields, &Outline, i64) -> Result<(BriefFields, Outline), String>,
{
    let result = state.db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        assert_briefing(&tx, initiative_id)?;
        let (base_version, base_fields, base_outline) = match latest_in(&tx, initiative_id)? {
            Some(l) => (l.version, l.fields, l.outline),
            None => (0, BriefFields::default(), Outline::default()),
        };
        let next = base_version + 1;
        let (fields, outline) = f(&base_fields, &base_outline, next)?;
        let mut changed = changed_fields(&base_fields, &fields);
        changed.extend(changed_outline_ids(&base_outline, &outline));
        if changed.is_empty() {
            return Err("no change".to_string());
        }
        insert_version(&tx, initiative_id, next, author.as_str(), &fields, &outline, &changed, note)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(WriteResult { version: next, changed_fields: changed })
    })?;
    // After COMMIT, never inside the txn: append_event takes the connection lock itself.
    append_event(
        state,
        initiative_id,
        None,
        "brief_version_created",
        event_payload(result.version, author, &result.changed_fields, note),
    )?;
    Ok(result)
}

/// Insert one version row and re-render `agent_plans.brief`. Called only by [`write_version`] and
/// [`seed_v1`], always inside their transaction.
#[allow(clippy::too_many_arguments)]
fn insert_version(
    tx: &Connection,
    id: &str,
    version: i64,
    author: &str,
    fields: &BriefFields,
    outline: &Outline,
    changed: &[String],
    note: &str,
) -> Result<(), String> {
    let fields_json = serde_json::to_string(fields).map_err(|e| e.to_string())?;
    let outline_json = serde_json::to_string(outline).map_err(|e| e.to_string())?;
    let changed_json = serde_json::to_string(changed).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json, changed_json, change_note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, version, author, fields_json, outline_json, changed_json, note],
    )
    .map_err(|e| format!("Failed to write brief version {}: {}", version, e))?;
    tx.execute(
        "UPDATE agent_plans SET brief = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![id, render_brief_md(fields, outline)],
    )
    .map_err(|e| format!("Failed to sync rendered brief: {}", e))?;
    Ok(())
}

/// Seed version 1 (author `you`) from the raw ask: `outcome` = its first non-empty line, `why` =
/// the remaining lines (may be empty); empty outline. Idempotent: returns `Ok(None)` when any
/// version already exists.
pub fn seed_v1(state: &AppState, id: &str, raw_ask: &str) -> Result<Option<WriteResult>, String> {
    let fields = seed_fields(raw_ask);
    let outline = Outline::default();
    let result = state.db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        assert_briefing(&tx, id)?;
        if latest_in(&tx, id)?.is_some() {
            return Ok(None);
        }
        let changed = changed_fields(&BriefFields::default(), &fields);
        insert_version(&tx, id, 1, Author::You.as_str(), &fields, &outline, &changed, "seeded from the ask")?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(WriteResult { version: 1, changed_fields: changed }))
    })?;
    if let Some(r) = &result {
        append_event(
            state,
            id,
            None,
            "brief_version_created",
            event_payload(r.version, &Author::You, &r.changed_fields, "seeded from the ask"),
        )?;
    }
    Ok(result)
}

/// Pure: split a raw ask into the v1 fields.
pub fn seed_fields(raw_ask: &str) -> BriefFields {
    let mut lines = raw_ask.lines().map(str::trim).skip_while(|l| l.is_empty());
    let outcome = lines.next().unwrap_or("").to_string();
    let why = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    BriefFields { outcome, why, ..Default::default() }
}

// ───────────────────────────── reads ─────────────────────────────

/// One version (`None` = latest), with the plan's `accepted_brief_version` + `initiative_status`.
pub fn get_version(state: &AppState, id: &str, version: Option<i64>) -> Result<BriefVersion, String> {
    state.db.with_conn(|conn| {
        let (initiative_status, accepted_brief_version) = plan_state(conn, id)?;
        let row: Option<(i64, String, String, String, String, String, String)> = conn
            .query_row(
                "SELECT version, author, fields_json, outline_json, changed_json, change_note, created_at FROM brief_versions
                 WHERE initiative_id = ?1 AND (?2 IS NULL OR version = ?2) ORDER BY version DESC LIMIT 1",
                params![id, version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (version, author, f, o, c, change_note, created_at) = row.ok_or_else(|| match version {
            Some(v) => format!("brief version {} not found", v),
            None => "this plan has no brief versions yet".to_string(),
        })?;
        Ok(BriefVersion {
            initiative_id: id.to_string(),
            version,
            author,
            fields: parse_fields(&f)?,
            outline: parse_outline(&o)?,
            changed: serde_json::from_str(&c).unwrap_or_default(),
            change_note,
            created_at,
            accepted_brief_version,
            initiative_status,
        })
    })
}

/// Metadata of every version, oldest first.
pub fn list_versions(state: &AppState, id: &str) -> Result<Vec<BriefVersionMeta>, String> {
    state.db.with_conn(|conn| {
        plan_state(conn, id)?;
        let mut stmt = conn
            .prepare(
                "SELECT version, author, change_note, changed_json, created_at FROM brief_versions WHERE initiative_id = ?1 ORDER BY version ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id], |r| {
                let changed: String = r.get(3)?;
                Ok(BriefVersionMeta {
                    version: r.get(0)?,
                    author: r.get(1)?,
                    change_note: r.get(2)?,
                    changed: serde_json::from_str(&changed).unwrap_or_default(),
                    created_at: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    })
}

/// Number of versions of a plan (0 = never versioned → legacy accept path).
pub fn version_count(state: &AppState, id: &str) -> Result<i64, String> {
    state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM brief_versions WHERE initiative_id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    })
}

// ───────────────────────────── writes ─────────────────────────────

/// One field op (`brief_update` / `updateBriefField`).
pub fn update_field(
    state: &AppState,
    id: &str,
    field: BriefField,
    op: FieldOp,
    author: &Author,
    note: &str,
) -> Result<WriteResult, String> {
    write_version(state, id, author, note, |fields, outline, _| {
        Ok((apply_field_op(fields, field, op)?, outline.clone()))
    })
}

/// Revert to `target_version`: the whole brief (fields + outline) or one field. Always a NEW
/// version copying the target's content; the history is never rewritten.
pub fn revert(
    state: &AppState,
    id: &str,
    target_version: i64,
    field: Option<BriefField>,
    author: &Author,
) -> Result<WriteResult, String> {
    let target = get_version(state, id, Some(target_version))?;
    let note = match field {
        Some(f) => format!("undo {}", f.as_str()),
        None => format!("revert to v{}", target_version),
    };
    write_version(state, id, author, &note, |fields, outline, _| match field {
        Some(f) => Ok((copy_field(fields, &target.fields, f), outline.clone())),
        None => Ok((target.fields.clone(), target.outline.clone())),
    })
}

/// One outline op. Returns the write result and the affected id (the new id for an add).
pub fn outline_op(
    state: &AppState,
    id: &str,
    op: OutlineOp,
    author: &Author,
    note: &str,
) -> Result<(WriteResult, String), String> {
    let mut affected = String::new();
    let result = write_version(state, id, author, note, |fields, outline, next| {
        let (o, aid) = apply_outline_op(outline, op, author.as_str(), next)?;
        affected = aid;
        Ok((fields.clone(), o))
    })?;
    Ok((result, affected))
}

// ───────────────────────────── planning queue (task 08) ─────────────────────────────

/// Queue Start planning until the conflicting initiatives finish (`Some(ids)`), or clear the queue
/// (`None`). The ids must be a non-empty subset of the brief's CURRENT `file_conflict`
/// initiatives (from `brief_check`), so a stale or arbitrary id cannot park a plan forever.
pub fn queue_planning(state: &AppState, id: &str, after: Option<Vec<String>>) -> Result<(), String> {
    let Some(after) = after else {
        let was_queued = state.db.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            assert_briefing(&tx, id)?;
            let n = tx
                .execute(
                    "UPDATE agent_plans SET planning_queued_after = NULL, updated_at = datetime('now') WHERE id = ?1 AND planning_queued_after IS NOT NULL",
                    params![id],
                )
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(n > 0)
        })?;
        if was_queued {
            append_event(state, id, None, "brief_planning_unqueued", json!({}))?;
        }
        return Ok(());
    };

    let mut ids: Vec<String> = Vec::new();
    for a in after.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !ids.iter().any(|x| x == a) {
            ids.push(a.to_string());
        }
    }
    if ids.is_empty() {
        return Err("queue planning needs at least one conflicting initiative id".to_string());
    }
    let latest = get_version(state, id, None)?;
    if latest.initiative_status != "briefing" {
        return Err(frozen_error(latest.accepted_brief_version.unwrap_or(latest.version)));
    }
    let ctx = crate::services::brief_check::gather_ctx(state, id, &latest.fields)?;
    let conflicts = crate::services::brief_check::conflicts(&latest.fields, &ctx);
    let mut titles = Vec::with_capacity(ids.len());
    for a in &ids {
        match conflicts.iter().find(|(r, _)| &r.id == a) {
            Some((r, _)) => titles.push(r.title.clone()),
            None => {
                return Err(format!(
                    "`{}` is not a running initiative whose files conflict with this brief",
                    a
                ))
            }
        }
    }
    let json_ids = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
    state.db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        assert_briefing(&tx, id)?;
        tx.execute(
            "UPDATE agent_plans SET planning_queued_after = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, json_ids],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    })?;
    append_event(
        state,
        id,
        None,
        "brief_planning_queued",
        json!({ "after": ids, "afterTitles": titles }),
    )?;
    Ok(())
}

/// Pure: a queued plan may start once EVERY initiative it waits on is finished —
/// `initiative_status = 'done'`, or `status = 'closed'`, or gone (deleted). `statuses` maps an
/// initiative id → `(status, initiative_status)`.
pub fn queued_brief_ready(
    after: &[String],
    statuses: &std::collections::HashMap<String, (String, String)>,
) -> bool {
    after.iter().all(|id| match statuses.get(id) {
        None => true,
        Some((status, initiative_status)) => initiative_status == "done" || status == "closed",
    })
}

/// `(status, initiative_status)` per initiative id. An initiative has a planning and a development
/// row; it is reported by its LEAST finished row so a done planning row cannot release a queue
/// while its development row is still running.
fn initiative_statuses(
    conn: &Connection,
    ids: &[String],
) -> Result<std::collections::HashMap<String, (String, String)>, String> {
    let mut map: std::collections::HashMap<String, (String, String)> = Default::default();
    let mut stmt = conn
        .prepare("SELECT status, initiative_status FROM agent_plans WHERE initiative_id = ?1")
        .map_err(|e| e.to_string())?;
    for id in ids {
        let rows = stmt
            .query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let finished = |r: &(String, String)| r.1 == "done" || r.0 == "closed";
        for row in rows {
            let row = row.map_err(|e| e.to_string())?;
            // Keep the first row, but let an unfinished row replace a finished one.
            let replace = match map.get(id) {
                None => true,
                Some(existing) => finished(existing) && !finished(&row),
            };
            if replace {
                map.insert(id.clone(), row);
            }
        }
    }
    Ok(map)
}

/// The 60 s tick body: start every queued briefing whose wait list is finished.
pub async fn release_queued_briefs(state: &AppState) -> Result<usize, String> {
    release_queued_briefs_with(state, |state, id| async move {
        crate::services::agent_plans::accept_brief(&state, &id, None)
            .await
            .map(|_| ())
    })
    .await
}

/// [`release_queued_briefs`] with the accept step injected (tests drive it without a planner
/// spawn). Not-ready plans are skipped untouched. On an accept error the queue is cleared and
/// `brief_planning_queue_failed {error}` is recorded — the user decides what to do next.
pub async fn release_queued_briefs_with<F, Fut>(state: &AppState, accept: F) -> Result<usize, String>
where
    F: Fn(AppState, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let queued: Vec<(String, String)> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, planning_queued_after FROM agent_plans
                  WHERE initiative_status = 'briefing' AND status != 'closed' AND planning_queued_after IS NOT NULL
                  ORDER BY updated_at, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    })?;
    let mut released = 0;
    for (id, raw) in queued {
        let outcome = match serde_json::from_str::<Vec<String>>(&raw) {
            Err(e) => Err(format!("unreadable planning queue: {}", e)),
            Ok(after) => {
                let statuses = state.db.with_conn(|conn| initiative_statuses(conn, &after))?;
                if !queued_brief_ready(&after, &statuses) {
                    continue;
                }
                accept(state.clone(), id.clone()).await
            }
        };
        match outcome {
            Ok(()) => released += 1,
            Err(error) => {
                tracing::warn!(%id, %error, "queued planning failed to start; queue cleared");
                state.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE agent_plans SET planning_queued_after = NULL, updated_at = datetime('now') WHERE id = ?1",
                        params![id],
                    )
                    .map_err(|e| e.to_string())
                })?;
                append_event(state, &id, None, "brief_planning_queue_failed", json!({ "error": error }))?;
            }
        }
    }
    Ok(released)
}

/// Spawn the 60 s release tick (called from `main.rs` beside the orphan sweep). Uses
/// `tauri::async_runtime::spawn` because `setup` has no tokio reactor entered.
pub fn spawn_planning_queue_release(state: AppState) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            if let Err(error) = release_queued_briefs(&state).await {
                tracing::warn!(%error, "planning queue release tick failed");
            }
        }
    });
}

// ───────────────────────────── RPC-facing helpers (phase 01 task 02) ─────────────────────────
//
// The host RPC arms stay param-pullers that make ONE service call each; the parsing of wire
// strings (`field`, `op`) into model types lives here, next to the model, and is unit-tested.

/// `update_brief_field` over the wire: `field` is a brief key, `op` is `value` | `add` | `remove`.
/// For `op='value'`, `values` (an array, for `where`) wins over `value` (a string). `add`/`remove`
/// take the string `value`.
pub fn update_field_named(
    state: &AppState,
    id: &str,
    field: &str,
    op: &str,
    value: Option<&serde_json::Value>,
    values: Option<&serde_json::Value>,
    author: &Author,
    note: &str,
) -> Result<WriteResult, String> {
    let field = BriefField::parse(field)?;
    let line = || -> Result<String, String> {
        value
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("op `{}` needs a string 'value'", op))
    };
    let op = match op {
        "value" => match values.filter(|v| !v.is_null()).or(value.filter(|v| !v.is_null())) {
            Some(v) => FieldOp::Value(v.clone()),
            None => return Err("op `value` needs 'value' (string) or 'values' (string[])".to_string()),
        },
        "add" => FieldOp::Add(line()?),
        "remove" => FieldOp::Remove(line()?),
        other => return Err(format!("unknown op `{}` (expected value, add or remove)", other)),
    };
    update_field(state, id, field, op, author, note)
}

/// `reset_brief_to_version` over the wire: `field` (a brief key) = per-field undo, absent = whole.
pub fn revert_named(
    state: &AppState,
    id: &str,
    target_version: i64,
    field: Option<&str>,
    author: &Author,
) -> Result<WriteResult, String> {
    let field = field.map(BriefField::parse).transpose()?;
    revert(state, id, target_version, field, author)
}

/// Diff two stored versions of one plan.
pub fn diff_versions(
    state: &AppState,
    id: &str,
    from_version: i64,
    to_version: i64,
) -> Result<crate::services::brief_model::BriefDiff, String> {
    let a = get_version(state, id, Some(from_version))?;
    let b = get_version(state, id, Some(to_version))?;
    Ok(crate::services::brief_model::diff(&a.fields, &a.outline, &b.fields, &b.outline))
}

/// Grade the latest version (an unversioned plan is graded as an empty brief).
pub fn check_latest(
    state: &AppState,
    id: &str,
) -> Result<Vec<crate::services::brief_check::CheckItem>, String> {
    use crate::services::brief_check::{check, gather_ctx};
    let fields = match get_version(state, id, None) {
        Ok(v) => v.fields,
        Err(_) => {
            crate::services::agent_plans::get_plan(state, id)?;
            BriefFields::default()
        }
    };
    let ctx = gather_ctx(state, id, &fields)?;
    Ok(check(&fields, &ctx))
}

/// One outline op for the wire: the write result plus the affected id.
pub fn outline_op_json(
    state: &AppState,
    id: &str,
    op: OutlineOp,
    author: &Author,
) -> Result<serde_json::Value, String> {
    let (r, affected) = outline_op(state, id, op, author, "")?;
    Ok(json!({ "version": r.version, "changedFields": r.changed_fields, "id": affected }))
}

/// `update_planning_queue` over the wire: set (non-empty ids) or clear (null / absent), then the
/// refreshed run.
pub fn queue_planning_run(
    state: &AppState,
    id: &str,
    after: Option<Vec<String>>,
) -> Result<crate::services::agent_plans::AgentPlanRun, String> {
    queue_planning(state, id, after)?;
    crate::services::agent_plans::get_plan(state, id)
}

// ───────────────────────────── draft plans (phase 01 task 01) ─────────────────────────────
//
// A draft plan IS a briefing Initiative (an `agent_plans` row at `initiative_status='briefing'`);
// there is no parallel entity. Rename/delete reuse `update_plan_title` / `delete_plan`.

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPlanLatest {
    pub version: i64,
    pub author: String,
    pub created_at: String,
}

/// One row of the `/plans` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPlanRow {
    pub id: String,
    pub title: String,
    pub workspace_path: String,
    pub updated_at: String,
    /// `None` only for a legacy briefing row that was never versioned.
    pub latest: Option<DraftPlanLatest>,
    pub check_summary: crate::services::brief_check::CheckSummary,
    /// The initiative ids a queued Start planning waits on (empty = not queued).
    pub queued_after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDraftPlanInput {
    pub title: String,
    pub workspace_path: String,
    #[serde(default)]
    pub ask: Option<String>,
}

/// Every draft plan (`briefing`, not closed), newest latest-version first (a never-versioned row
/// sorts by its `updated_at`), each with its check summary and queue.
pub fn list_draft_plans(state: &AppState) -> Result<Vec<DraftPlanRow>, String> {
    use crate::services::brief_check::{check, check_summary, gather_ctx};
    let rows: Vec<(String, String, String, String, Option<String>)> = state.db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, workspace_path, updated_at, planning_queued_after FROM agent_plans
                  WHERE initiative_status = 'briefing' AND status != 'closed'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    })?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, title, workspace_path, updated_at, queued) in rows {
        let (latest, fields) = match get_version(state, &id, None) {
            Ok(v) => (
                Some(DraftPlanLatest { version: v.version, author: v.author, created_at: v.created_at }),
                v.fields,
            ),
            Err(_) => (None, BriefFields::default()),
        };
        let ctx = gather_ctx(state, &id, &fields).unwrap_or_default();
        out.push(DraftPlanRow {
            id,
            title,
            workspace_path,
            updated_at,
            latest,
            check_summary: check_summary(&check(&fields, &ctx)),
            queued_after: queued
                .and_then(|q| serde_json::from_str::<Vec<String>>(&q).ok())
                .unwrap_or_default(),
        });
    }
    let key = |r: &DraftPlanRow| {
        r.latest.as_ref().map(|l| l.created_at.clone()).unwrap_or_else(|| r.updated_at.clone())
    };
    out.sort_by(|a, b| {
        key(b)
            .cmp(&key(a))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Create a briefing Initiative WITHOUT seeding a version; its chat session is re-tagged
/// `kind='agent'` + `Plan agent - <title>` so it stays out of the user's /terminal list.
async fn create_draft_row(
    state: &AppState,
    title: &str,
    workspace_path: &str,
    ask: Option<String>,
) -> Result<crate::services::agent_plans::AgentPlanRun, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("title must not be empty".to_string());
    }
    let run = crate::services::agent_plans::create_briefing_run(
        state,
        crate::db::models::CreateBriefingInput {
            title: Some(title.to_string()),
            workspace_path: workspace_path.to_string(),
            brief: ask,
            worker_provider: "claude_code".to_string(),
            reviewer_provider: "claude_code".to_string(),
            model: None,
            dev_worker_provider: None,
            dev_reviewer_provider: None,
            executor_config: None,
        },
    )
    .await?;
    if let Some(session_id) = run.plan.briefing_session_id.as_deref() {
        state.db.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET kind = 'agent', title = ?2 WHERE id = ?1",
                params![session_id, format!("Plan agent - {}", title)],
            )
            .map_err(|e| format!("Failed to tag the plan-agent session: {}", e))
        })?;
    }
    Ok(run)
}

/// New draft plan: a briefing Initiative (providers `claude_code`), its plan-agent session, and v1
/// seeded from the ask. Spawns nothing (no CLI, no tmux) — the agent runs on the first chat turn.
pub async fn create_draft_plan(
    state: &AppState,
    input: CreateDraftPlanInput,
) -> Result<crate::services::agent_plans::AgentPlanRun, String> {
    let ask = input.ask.clone().filter(|a| !a.trim().is_empty());
    let run = create_draft_row(state, &input.title, &input.workspace_path, ask.clone()).await?;
    seed_v1(state, &run.plan.id, ask.as_deref().unwrap_or(""))?;
    crate::services::agent_plans::get_plan(state, &run.plan.id)
}

/// Copy a draft plan: a new briefing Initiative in the same workspace titled `<title> (copy)`
/// whose v1 (author `you`, note `duplicated from <id> v<n>`) is the source's latest fields +
/// outline. The source must still be in briefing.
pub async fn duplicate_draft_plan(
    state: &AppState,
    id: &str,
) -> Result<crate::services::agent_plans::AgentPlanRun, String> {
    let source = crate::services::agent_plans::get_plan(state, id)?.plan;
    if source.initiative_status != "briefing" {
        return Err("only a draft plan (in briefing) can be duplicated".to_string());
    }
    let latest = get_version(state, id, None).ok();
    let title = format!("{} (copy)", source.title);
    let run = create_draft_row(state, &title, &source.workspace_path, None).await?;
    let new_id = run.plan.id.clone();
    match latest {
        Some(v) => {
            let note = format!("duplicated from {} v{}", id, v.version);
            let (fields, outline) = (v.fields, v.outline);
            match write_version(state, &new_id, &Author::You, &note, |_, _, _| Ok((fields, outline))) {
                Ok(_) => {}
                // An entirely empty source brief: seed an empty v1 instead.
                Err(e) if e == "no change" => {
                    seed_v1(state, &new_id, "")?;
                }
                Err(e) => return Err(e),
            }
        }
        None => {
            seed_v1(state, &new_id, source.brief.as_deref().unwrap_or(""))?;
        }
    }
    crate::services::agent_plans::get_plan(state, &new_id)
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::test_state;
    use serde_json::json;

    /// A bare briefing row (no session, no store dirs) — enough for the store.
    pub(crate) fn briefing_row(state: &AppState, id: &str, workspace: &str) {
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, initiative_id, initiative_status) \
                     VALUES (?1, 'planning', ?1, ?2, '/nonexistent-plan', 'draft', 'claude_code', 'claude_code', ?1, 'briefing')",
                    params![id, workspace],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
    }

    fn set_value(state: &AppState, id: &str, field: &str, v: serde_json::Value) -> Result<WriteResult, String> {
        update_field(state, id, BriefField::parse(field).unwrap(), FieldOp::Value(v), &Author::You, "")
    }

    fn versions(state: &AppState, id: &str) -> Vec<i64> {
        list_versions(state, id).unwrap().into_iter().map(|m| m.version).collect()
    }

    fn events_of(state: &AppState, id: &str, ty: &str) -> Vec<serde_json::Value> {
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

    #[test]
    fn every_write_is_a_new_version() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        assert_eq!(seed_v1(&state, "P", "Ship plans.\nBecause.").unwrap().unwrap().version, 1);
        assert!(seed_v1(&state, "P", "again").unwrap().is_none(), "seed is idempotent");
        let v1 = get_version(&state, "P", None).unwrap();
        assert_eq!((v1.fields.outcome.as_str(), v1.fields.why.as_str(), v1.author.as_str()), ("Ship plans.", "Because.", "you"));

        assert_eq!(set_value(&state, "P", "outcome", json!("Ship the Plans tab.")).unwrap().version, 2);
        let r = update_field(&state, "P", BriefField::Where, FieldOp::Add("web".into()), &Author::Agent { session_id: "S".into() }, "").unwrap();
        assert_eq!((r.version, r.changed_fields.clone()), (3, vec!["where".to_string()]));
        let (r, pid) = outline_op(&state, "P", OutlineOp::AddPhase { title: "Store".into() }, &Author::You, "").unwrap();
        assert_eq!((r.version, pid.as_str()), (4, "00"));
        assert_eq!(r.changed_fields, vec!["outline:00"]);
        assert_eq!(versions(&state, "P"), vec![1, 2, 3, 4], "dense 1..n");
        let latest = get_version(&state, "P", None).unwrap();
        assert_eq!(latest.version, 4);
        assert_eq!(latest.outline.phases[0].added_in, 4);
        assert_eq!(get_version(&state, "P", Some(3)).unwrap().author, "agent");
        assert!(get_version(&state, "P", Some(9)).is_err());
    }

    #[test]
    fn no_op_write_is_rejected_and_creates_no_row() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "Same.").unwrap();
        let e = set_value(&state, "P", "outcome", json!("Same.")).unwrap_err();
        assert_eq!(e, "no change");
        // Adding an existing line is a model-level no-op → also `no change`.
        let e = update_field(&state, "P", BriefField::Outcome, FieldOp::Add("Same.".into()), &Author::You, "").unwrap_err();
        assert_eq!(e, "no change");
        // A model error surfaces verbatim and writes nothing.
        let e = update_field(&state, "P", BriefField::Why, FieldOp::Remove("x".into()), &Author::You, "").unwrap_err();
        assert!(e.contains("has no line"), "{e}");
        assert_eq!(versions(&state, "P"), vec![1]);
        assert_eq!(events_of(&state, "P", "brief_version_created").len(), 1, "only the seed's event");
    }

    #[test]
    fn revert_whole_creates_new_version_copying_target() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "One.").unwrap();
        set_value(&state, "P", "outcome", json!("Two.")).unwrap();
        outline_op(&state, "P", OutlineOp::AddPhase { title: "A".into() }, &Author::You, "").unwrap();
        let r = revert(&state, "P", 1, None, &Author::You).unwrap();
        assert_eq!(r.version, 4);
        assert_eq!(r.changed_fields, vec!["outcome", "outline:00"]);
        let v4 = get_version(&state, "P", None).unwrap();
        let v1 = get_version(&state, "P", Some(1)).unwrap();
        assert_eq!((v4.fields.clone(), v4.outline.clone()), (v1.fields, v1.outline));
        assert_eq!(v4.change_note, "revert to v1");
        // History intact: v2/v3 still hold what they held.
        assert_eq!(get_version(&state, "P", Some(2)).unwrap().fields.outcome, "Two.");
        assert_eq!(versions(&state, "P"), vec![1, 2, 3, 4]);
    }

    #[test]
    fn revert_field_changes_only_that_field() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "One.\nreason").unwrap();
        set_value(&state, "P", "outcome", json!("Two.")).unwrap();
        set_value(&state, "P", "why", json!("new reason")).unwrap();
        let r = revert(&state, "P", 1, Some(BriefField::Outcome), &Author::You).unwrap();
        assert_eq!((r.version, r.changed_fields.clone()), (4, vec!["outcome".to_string()]));
        let v4 = get_version(&state, "P", None).unwrap();
        assert_eq!(v4.fields.outcome, "One.");
        assert_eq!(v4.fields.why, "new reason", "other fields keep their latest value");
        assert_eq!(v4.change_note, "undo outcome");
    }

    #[test]
    fn write_updates_agent_plans_brief_with_render() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "Ship it.").unwrap();
        set_value(&state, "P", "out_of_scope", json!("Billing")).unwrap();
        let brief: String = state
            .db
            .with_conn(|c| c.query_row("SELECT brief FROM agent_plans WHERE id='P'", [], |r| r.get(0)).map_err(|e| e.to_string()))
            .unwrap();
        let v = get_version(&state, "P", None).unwrap();
        assert_eq!(brief, render_brief_md(&v.fields, &v.outline));
        assert_eq!(brief, "# Outcome\nShip it.\n\n## Out of scope\nBilling\n");
    }

    #[test]
    fn write_emits_brief_version_created_event_with_author_and_changed_fields() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "Ship it.").unwrap();
        update_field(
            &state,
            "P",
            BriefField::Where,
            FieldOp::Value(json!(["web", "desktop"])),
            &Author::Agent { session_id: "SESS-1".into() },
            "drafted where",
        )
        .unwrap();
        set_value(&state, "P", "why", json!("because")).unwrap();
        let evs = events_of(&state, "P", "brief_version_created");
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0]["version"], json!(1));
        assert_eq!(evs[1], json!({"version": 2, "author": "agent", "changedFields": ["where"], "note": "drafted where", "sessionId": "SESS-1"}));
        assert_eq!(evs[2]["author"], json!("you"));
        assert!(evs[2].get("sessionId").is_none(), "a human write carries no session id");
    }

    #[test]
    fn write_refused_when_not_briefing() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "Ship it.").unwrap();
        set_value(&state, "P", "why", json!("w")).unwrap();
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET initiative_status='planning' WHERE id='P'", []).map_err(|e| e.to_string()))
            .unwrap();
        let e = set_value(&state, "P", "outcome", json!("x")).unwrap_err();
        assert_eq!(e, "the plan left briefing; the brief is frozen at v2");
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET accepted_brief_version=1 WHERE id='P'", []).map_err(|e| e.to_string()))
            .unwrap();
        let e = outline_op(&state, "P", OutlineOp::AddPhase { title: "A".into() }, &Author::You, "").unwrap_err();
        assert_eq!(e, "the plan left briefing; the brief is frozen at v1", "the accepted version wins");
        assert_eq!(versions(&state, "P"), vec![1, 2]);
        assert!(set_value(&state, "NOPE", "outcome", json!("x")).unwrap_err().contains("not found"));
    }

    #[test]
    fn concurrent_writes_get_distinct_versions() {
        let (state, _root) = test_state();
        briefing_row(&state, "P", "/w");
        seed_v1(&state, "P", "Start.").unwrap();
        std::thread::scope(|s| {
            for t in 0..2 {
                let state = &state;
                s.spawn(move || {
                    for i in 0..20 {
                        update_field(
                            state,
                            "P",
                            BriefField::Decided,
                            FieldOp::Add(format!("thread {t} line {i}")),
                            &Author::You,
                            "",
                        )
                        .expect("concurrent write");
                    }
                });
            }
        });
        assert_eq!(versions(&state, "P"), (1..=41).collect::<Vec<i64>>(), "41 dense versions, no PK error");
        assert_eq!(get_version(&state, "P", None).unwrap().fields.decided.lines().count(), 40);
    }
}

/// Phase 00 task 07: Start planning freezes the latest version (`agent_plans::freeze_and_flip_brief`,
/// i.e. `accept_brief` minus the planner spawn — no CLI, no tmux).
#[cfg(test)]
mod accept_freeze_tests {
    use super::*;
    use crate::db::models::{AgentPlanEvent, CreateBriefingInput};
    use crate::services::agent_plans::{
        create_briefing_run, event_actor, event_category, event_summary, freeze_and_flip_brief,
    };
    use crate::services::settings as settings_service;
    use crate::test_support::{test_state, tmp_dir};
    use serde_json::json;
    use std::path::Path;

    /// A real briefing Initiative (store dirs + chat session) in a temp workspace with a `src` dir.
    async fn briefing(state: &AppState, ask: &str) -> (String, String, std::path::PathBuf) {
        let workspace = tmp_dir("af-ws");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        let store = tmp_dir("af-store").canonicalize().unwrap();
        settings_service::set_setting(
            state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();
        let plan = create_briefing_run(
            state,
            CreateBriefingInput {
                title: Some("Freeze".to_string()),
                workspace_path: workspace.to_string_lossy().to_string(),
                brief: Some(ask.to_string()),
                worker_provider: "claude_code".to_string(),
                reviewer_provider: "claude_code".to_string(),
                model: None,
                dev_worker_provider: None,
                dev_reviewer_provider: None,
                executor_config: None,
            },
        )
        .await
        .expect("create_briefing_run")
        .plan;
        (plan.id, plan.plan_path, workspace)
    }

    fn passing_brief(state: &AppState, id: &str, outcome: &str) {
        seed_v1(state, id, outcome).unwrap();
        let you = Author::You;
        update_field(state, id, BriefField::Where, FieldOp::Add("src".into()), &you, "").unwrap();
        update_field(state, id, BriefField::DoneWhen, FieldOp::Add("cargo test -j 8 --lib".into()), &you, "").unwrap();
        update_field(state, id, BriefField::OutOfScope, FieldOp::Add("Billing".into()), &you, "").unwrap();
    }

    fn row(state: &AppState, id: &str) -> (String, Option<String>, Option<i64>, Option<String>) {
        state
            .db
            .with_conn(|c| {
                c.query_row(
                    "SELECT initiative_status, brief, accepted_brief_version, planning_queued_after FROM agent_plans WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    fn payloads(state: &AppState, id: &str, ty: &str) -> Vec<serde_json::Value> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare("SELECT payload_json FROM agent_plan_events WHERE plan_id = ?1 AND type = ?2 ORDER BY rowid")
                    .unwrap();
                let v = stmt
                    .query_map(params![id, ty], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
                    .collect();
                Ok(v)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn accept_versioned_brief_stores_render_and_freezes() {
        let (state, _root) = test_state();
        let (id, plan_path, _ws) = briefing(&state, "Ship the Plans tab.").await;
        passing_brief(&state, &id, "Ship the Plans tab.");
        outline_op(&state, &id, OutlineOp::AddPhase { title: "Store".into() }, &Author::You, "").unwrap();
        let latest = get_version(&state, &id, None).unwrap();
        assert_eq!(latest.version, 5);
        // A queue set earlier is cleared by the accept.
        state
            .db
            .with_conn(|c| c.execute("UPDATE agent_plans SET planning_queued_after='[\"X\"]' WHERE id=?1", params![id]).map_err(|e| e.to_string()))
            .unwrap();

        // finalBrief is ignored when versions exist.
        freeze_and_flip_brief(&state, &id, Some("IGNORED edited text".into())).expect("freeze");

        let rendered = render_brief_md(&latest.fields, &latest.outline);
        let (status, brief, accepted, queued) = row(&state, &id);
        assert_eq!(status, "planning");
        assert_eq!(brief.as_deref(), Some(rendered.trim_end()), "agent_plans.brief = render_brief_md(latest)");
        assert_eq!(accepted, Some(5));
        assert_eq!(queued, None, "planning_queued_after IS NULL after accept");
        let on_disk = std::fs::read_to_string(Path::new(&plan_path).join("brief.md")).unwrap();
        assert_eq!(on_disk, rendered.trim_end(), "plan/brief.md holds the same render");
        assert!(!on_disk.contains("IGNORED"));
        assert_eq!(payloads(&state, &id, "brief_frozen"), vec![json!({"version": 5, "checkFailures": []})]);
        assert_eq!(payloads(&state, &id, "brief_accepted").len(), 1, "the existing flow still runs");
    }

    #[tokio::test]
    async fn accept_with_check_fail_proceeds_and_records_failures() {
        let (state, _root) = test_state();
        let (id, _plan_path, _ws) = briefing(&state, "x").await;
        passing_brief(&state, &id, "Ship the tab. Then test it.");
        freeze_and_flip_brief(&state, &id, None).expect("a failing check never refuses (D11)");
        assert_eq!(row(&state, &id).0, "planning");
        assert_eq!(
            payloads(&state, &id, "brief_frozen"),
            vec![json!({"version": 4, "checkFailures": ["outcome_one_sentence"]})]
        );
    }

    #[tokio::test]
    async fn accept_without_versions_is_unchanged() {
        let (state, _root) = test_state();
        let (id, plan_path, _ws) = briefing(&state, "the raw ask").await;
        assert_eq!(version_count(&state, &id).unwrap(), 0);
        freeze_and_flip_brief(&state, &id, Some("the edited brief".into())).expect("legacy accept");
        let (status, brief, accepted, queued) = row(&state, &id);
        assert_eq!(status, "planning");
        assert_eq!(brief.as_deref(), Some("the edited brief"), "finalBrief still wins on the legacy path");
        assert_eq!((accepted, queued), (None, None));
        assert_eq!(std::fs::read_to_string(Path::new(&plan_path).join("brief.md")).unwrap(), "the edited brief");
        assert!(payloads(&state, &id, "brief_frozen").is_empty(), "no freeze without versions");
        assert_eq!(payloads(&state, &id, "brief_accepted").len(), 1);
    }

    #[tokio::test]
    async fn update_after_accept_is_frozen() {
        let (state, _root) = test_state();
        let (id, _plan_path, _ws) = briefing(&state, "x").await;
        passing_brief(&state, &id, "Ship it.");
        freeze_and_flip_brief(&state, &id, None).unwrap();
        let e = update_field(&state, &id, BriefField::Why, FieldOp::Value(json!("late")), &Author::You, "").unwrap_err();
        assert_eq!(e, "the plan left briefing; the brief is frozen at v4");
        assert_eq!(version_count(&state, &id).unwrap(), 4, "no version written after the freeze");
        // A second accept is refused by the existing guard.
        assert!(freeze_and_flip_brief(&state, &id, None).unwrap_err().contains("not in briefing"));
        // And the flip itself is a compare-and-set: a racing second flip changes nothing.
        let raced = state.db.with_conn(|c| {
            crate::services::agent_plans::apply_brief_acceptance_at(c, &id, "RACED", Some(1))
        });
        assert!(raced.unwrap_err().contains("not in briefing"));
        assert_eq!(row(&state, &id).2, Some(4), "the frozen version is not overwritten by a racing flip");
    }

    fn ev(ty: &str, payload: serde_json::Value) -> (AgentPlanEvent, serde_json::Value) {
        (
            AgentPlanEvent {
                id: String::new(),
                plan_id: "P".into(),
                phase_id: None,
                phase_index: None,
                phase_title: None,
                event_type: ty.into(),
                actor: String::new(),
                category: String::new(),
                summary: String::new(),
                status_before: None,
                status_after: None,
                reason: None,
                verdict: None,
                task_id: None,
                clarification_attempt: None,
                payload_json: payload.to_string(),
                created_at: String::new(),
            },
            payload,
        )
    }

    #[test]
    fn event_summary_for_brief_events() {
        let cases = [
            ("brief_version_created", json!({"version": 6, "author": "agent", "changedFields": ["outcome", "where"], "note": ""}), "Brief v6 · agent · outcome, where"),
            ("brief_version_created", json!({"version": 7, "author": "you", "changedFields": ["outcome"], "note": "undo outcome"}), "Brief v7 · you · undo outcome"),
            ("brief_version_created", json!({"version": 2, "author": "you", "changedFields": []}), "Brief v2 · you"),
            ("brief_frozen", json!({"version": 7, "checkFailures": []}), "Brief frozen at v7"),
            ("brief_frozen", json!({"version": 7, "checkFailures": ["a", "b"]}), "Brief frozen at v7 · started over 2 failed checks"),
            ("brief_frozen", json!({"version": 3, "checkFailures": ["a"]}), "Brief frozen at v3 · started over 1 failed check"),
            ("brief_planning_queued", json!({"after": ["I2"], "afterTitles": ["Findings ledger"]}), "Planning queued after Findings ledger"),
            ("brief_planning_queued", json!({"after": ["I2", "I3"]}), "Planning queued after I2, I3"),
            ("brief_planning_unqueued", json!({}), "Planning queue cleared"),
            ("brief_planning_queue_failed", json!({"error": "boom"}), "Queued planning failed to start: boom"),
            ("agent_mcp_rejected", json!({"reason": "revoked token", "count": 3}), "MCP call rejected: revoked token (×3)"),
            ("agent_mcp_rejected", json!({"reason": "bad host", "count": 1}), "MCP call rejected: bad host"),
        ];
        for (ty, payload, want) in cases {
            let (e, p) = ev(ty, payload);
            assert_eq!(event_summary(&e, &p), want, "{ty}");
            assert_eq!(event_category(ty), "run", "{ty}");
        }
        assert_eq!(event_actor("agent_mcp_rejected"), "plan_agent");
        for ty in ["brief_version_created", "brief_frozen", "brief_planning_queued", "brief_planning_unqueued", "brief_planning_queue_failed"] {
            assert_eq!(event_actor(ty), "coordinator", "{ty}");
        }
    }
}

/// Phase 00 task 08: queue planning until conflicting initiatives finish.
#[cfg(test)]
mod planning_queue_tests {
    use super::tests::briefing_row;
    use super::*;
    use crate::test_support::{test_state, tmp_dir};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn running(state: &AppState, id: &str, initiative: &str, ws: &str, status: &str, istatus: &str, where_: &[&str]) {
        state
            .db
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO agent_plans (id, run_type, title, workspace_path, plan_path, status, worker_provider, reviewer_provider, initiative_id, initiative_status, accepted_brief_version) \
                     VALUES (?1, 'planning', ?1 || ' title', ?2, '/none', ?3, 'claude_code', 'claude_code', ?4, ?5, 1)",
                    params![id, ws, status, initiative, istatus],
                )
                .map_err(|e| e.to_string())?;
                if id == initiative {
                    let f = BriefFields { where_: where_.iter().map(|s| s.to_string()).collect(), ..Default::default() };
                    c.execute(
                        "INSERT INTO brief_versions (initiative_id, version, author, fields_json, outline_json) VALUES (?1, 1, 'you', ?2, '{}')",
                        params![id, serde_json::to_string(&f).unwrap()],
                    )
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn set_status(state: &AppState, id: &str, status: &str, istatus: &str) {
        state
            .db
            .with_conn(|c| {
                c.execute(
                    "UPDATE agent_plans SET status = ?2, initiative_status = ?3 WHERE id = ?1",
                    params![id, status, istatus],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
    }

    fn queued(state: &AppState, id: &str) -> Option<String> {
        state
            .db
            .with_conn(|c| {
                c.query_row("SELECT planning_queued_after FROM agent_plans WHERE id = ?1", params![id], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap()
    }

    fn event_types(state: &AppState, id: &str) -> Vec<String> {
        state
            .db
            .with_conn(|c| {
                let mut stmt = c.prepare("SELECT type FROM agent_plan_events WHERE plan_id = ?1 ORDER BY rowid").unwrap();
                let v = stmt.query_map(params![id], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
                Ok(v)
            })
            .unwrap()
    }

    /// A briefing plan ME whose `where` = [src] in workspace `ws`, plus a running RUN touching src.
    fn fixture() -> (AppState, std::path::PathBuf, String) {
        let (state, root) = test_state();
        let ws = tmp_dir("pq-ws").to_string_lossy().to_string();
        briefing_row(&state, "ME", &ws);
        seed_v1(&state, "ME", "Ship.").unwrap();
        update_field(&state, "ME", BriefField::Where, FieldOp::Add("src".into()), &Author::You, "").unwrap();
        running(&state, "RUN", "RUN", &ws, "phase_worker_running", "development", &["src/lib.rs"]);
        running(&state, "QUIET", "QUIET", &ws, "phase_worker_running", "development", &["docs"]);
        (state, root, ws)
    }

    #[test]
    fn queued_ready_table() {
        let m: HashMap<String, (String, String)> = [
            ("DONE", ("approved", "done")),
            ("CLOSED", ("closed", "development")),
            ("RUNNING", ("phase_worker_running", "development")),
            ("REVIEW", ("phase_review_running", "review")),
            ("PLANNING", ("planning_planner_running", "planning")),
        ]
        .into_iter()
        .map(|(k, (a, b))| (k.to_string(), (a.to_string(), b.to_string())))
        .collect();
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let rows: Vec<(Vec<String>, bool)> = vec![
            (ids(&["DONE"]), true),
            (ids(&["CLOSED"]), true),
            (ids(&["GONE"]), true),
            (ids(&["DONE", "CLOSED", "GONE"]), true),
            (ids(&["RUNNING"]), false),
            (ids(&["REVIEW"]), false),
            (ids(&["PLANNING"]), false),
            (ids(&["DONE", "RUNNING"]), false),
            (ids(&[]), true),
        ];
        for (after, want) in rows {
            assert_eq!(queued_brief_ready(&after, &m), want, "{after:?}");
        }
    }

    #[test]
    fn queue_requires_conflict_ids() {
        let (state, _root, _ws) = fixture();
        assert!(queue_planning(&state, "ME", Some(vec![])).unwrap_err().contains("at least one"));
        assert!(queue_planning(&state, "ME", Some(vec!["  ".into()])).unwrap_err().contains("at least one"));
        let e = queue_planning(&state, "ME", Some(vec!["QUIET".into()])).unwrap_err();
        assert!(e.contains("`QUIET` is not a running initiative"), "{e}");
        assert!(queue_planning(&state, "ME", Some(vec!["NOPE".into()])).is_err());
        assert_eq!(queued(&state, "ME"), None, "a refused queue stores nothing");

        queue_planning(&state, "ME", Some(vec!["RUN".into(), "RUN".into()])).unwrap();
        assert_eq!(queued(&state, "ME").as_deref(), Some("[\"RUN\"]"));
        let evs = event_types(&state, "ME");
        assert_eq!(evs.last().map(String::as_str), Some("brief_planning_queued"));

        queue_planning(&state, "ME", None).unwrap();
        assert_eq!(queued(&state, "ME"), None);
        assert_eq!(event_types(&state, "ME").last().map(String::as_str), Some("brief_planning_unqueued"));
        // Clearing an empty queue is a silent no-op.
        let before = event_types(&state, "ME").len();
        queue_planning(&state, "ME", None).unwrap();
        assert_eq!(event_types(&state, "ME").len(), before);
    }

    #[tokio::test]
    async fn release_skips_not_ready_and_clears_on_error() {
        let (state, _root, ws) = fixture();
        queue_planning(&state, "ME", Some(vec!["RUN".into()])).unwrap();
        // A second queued plan whose accept will fail.
        briefing_row(&state, "BAD", &ws);
        seed_v1(&state, "BAD", "Other.").unwrap();
        update_field(&state, "BAD", BriefField::Where, FieldOp::Add("src/lib.rs".into()), &Author::You, "").unwrap();
        queue_planning(&state, "BAD", Some(vec!["RUN".into()])).unwrap();

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let accept = |calls: Arc<Mutex<Vec<String>>>| {
            move |_s: AppState, id: String| {
                let calls = calls.clone();
                async move {
                    calls.lock().unwrap().push(id.clone());
                    if id == "BAD" { Err("planner failed to start".to_string()) } else { Ok(()) }
                }
            }
        };

        // RUN is still running (its development row): nothing is released.
        running(&state, "RUN-DEV", "RUN", &ws, "phase_worker_running", "development", &[]);
        set_status(&state, "RUN", "approved", "done");
        assert_eq!(release_queued_briefs_with(&state, accept(calls.clone())).await.unwrap(), 0);
        assert!(calls.lock().unwrap().is_empty(), "not ready → accept never called");
        assert!(queued(&state, "ME").is_some() && queued(&state, "BAD").is_some(), "queues untouched");

        // RUN finishes: both are ready; ME starts, BAD's accept fails → its queue is cleared + recorded.
        set_status(&state, "RUN-DEV", "approved", "done");
        assert_eq!(release_queued_briefs_with(&state, accept(calls.clone())).await.unwrap(), 1);
        let mut seen = calls.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["BAD", "ME"]);
        assert_eq!(queued(&state, "BAD"), None, "error clears the queue");
        assert_eq!(event_types(&state, "BAD").last().map(String::as_str), Some("brief_planning_queue_failed"));
        // ME's accept was a stub here, so its own queue is cleared by the real accept (pinned by
        // accept_versioned_brief_stores_render_and_freezes), not by the release loop.
        assert!(queued(&state, "ME").is_some());
    }
}

/// Phase 01 task 01: draft-plan services (list / create / duplicate).
#[cfg(test)]
mod draft_plan_tests {
    use super::*;
    use crate::services::settings as settings_service;
    use crate::test_support::{test_state, tmp_dir};

    pub(crate) fn use_store(state: &AppState) -> std::path::PathBuf {
        let store = tmp_dir("dp-store").canonicalize().unwrap();
        settings_service::set_setting(
            state,
            settings_service::KEY_INITIATIVES_DIR.to_string(),
            store.to_string_lossy().to_string(),
        )
        .unwrap();
        tmp_dir("dp-ws")
    }

    async fn create(state: &AppState, ws: &std::path::Path, title: &str, ask: Option<&str>) -> String {
        create_draft_plan(
            state,
            CreateDraftPlanInput {
                title: title.into(),
                workspace_path: ws.to_string_lossy().to_string(),
                ask: ask.map(str::to_string),
            },
        )
        .await
        .expect("create_draft_plan")
        .plan
        .id
    }

    fn exec(state: &AppState, sql: &str, id: &str) {
        state.db.with_conn(|c| c.execute(sql, params![id]).map_err(|e| e.to_string())).unwrap();
    }

    #[tokio::test]
    async fn list_draft_plans_only_briefing_not_closed_sorted() {
        let (state, _root) = test_state();
        let ws = use_store(&state);
        let a = create(&state, &ws, "A", Some("First.")).await;
        let b = create(&state, &ws, "B", Some("Second.")).await;
        let closed = create(&state, &ws, "Closed", None).await;
        let planning = create(&state, &ws, "Planning", None).await;
        exec(&state, "UPDATE agent_plans SET status='closed' WHERE id=?1", &closed);
        exec(&state, "UPDATE agent_plans SET initiative_status='planning' WHERE id=?1", &planning);
        // Make A's latest version clearly the newest.
        exec(&state, "UPDATE brief_versions SET created_at='2001-01-01 00:00:00' WHERE initiative_id=?1", &b);
        update_field(&state, &a, BriefField::Why, FieldOp::Value(serde_json::json!("because")), &Author::You, "").unwrap();
        exec(&state, "UPDATE brief_versions SET created_at='2030-01-01 00:00:00' WHERE initiative_id=?1 AND version=2", &a);

        let rows = list_draft_plans(&state).unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![a.as_str(), b.as_str()]);
        let ra = &rows[0];
        assert_eq!(ra.title, "A");
        let latest = ra.latest.as_ref().unwrap();
        assert_eq!((latest.version, latest.author.as_str()), (2, "you"));
        // Outcome ok; done_when / out_of_scope / where fail; no conflict.
        assert_eq!((ra.check_summary.ok, ra.check_summary.warn, ra.check_summary.fail), (2, 0, 3));
        assert!(ra.queued_after.is_empty());
        let v = serde_json::to_value(ra).unwrap();
        for k in ["workspacePath", "updatedAt", "checkSummary", "queuedAfter"] {
            assert!(v.get(k).is_some(), "camelCase key {k}");
        }
        assert_eq!(v["latest"]["createdAt"], "2030-01-01 00:00:00");
    }

    #[tokio::test]
    async fn create_draft_plan_seeds_v1_and_agent_kind_session() {
        let (state, _root) = test_state();
        let ws = use_store(&state);
        let id = create(&state, &ws, "  My plan ", Some("Ship the tab.\nIt matters.")).await;
        let run = crate::services::agent_plans::get_plan(&state, &id).unwrap();
        assert_eq!(run.plan.initiative_status, "briefing");
        assert_eq!((run.plan.worker_provider.as_str(), run.plan.reviewer_provider.as_str()), ("claude_code", "claude_code"));
        assert_eq!(run.plan.title, "My plan");
        let v1 = get_version(&state, &id, None).unwrap();
        assert_eq!((v1.version, v1.author.as_str()), (1, "you"));
        assert_eq!((v1.fields.outcome.as_str(), v1.fields.why.as_str()), ("Ship the tab.", "It matters."));
        let sid = run.plan.briefing_session_id.expect("plan-agent session linked");
        let (kind, title): (String, String) = state
            .db
            .with_conn(|c| c.query_row("SELECT kind, title FROM sessions WHERE id=?1", params![sid], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string()))
            .unwrap();
        assert_eq!((kind.as_str(), title.as_str()), ("agent", "Plan agent - My plan"));
        // Empty ask → an empty v1; empty title → error.
        let empty = create(&state, &ws, "Empty", None).await;
        assert_eq!(get_version(&state, &empty, None).unwrap().version, 1);
        let e = create_draft_plan(&state, CreateDraftPlanInput { title: " ".into(), workspace_path: ws.to_string_lossy().to_string(), ask: None }).await.unwrap_err();
        assert!(e.contains("title must not be empty"), "{e}");
    }

    #[tokio::test]
    async fn duplicate_copies_latest_into_new_v1() {
        let (state, _root) = test_state();
        let ws = use_store(&state);
        let src = create(&state, &ws, "Source", Some("Ship.")).await;
        update_field(&state, &src, BriefField::Where, FieldOp::Add("src".into()), &Author::Agent { session_id: "S".into() }, "").unwrap();
        outline_op(&state, &src, OutlineOp::AddPhase { title: "Store".into() }, &Author::You, "").unwrap();
        let src_latest = get_version(&state, &src, None).unwrap();
        assert_eq!(src_latest.version, 3);

        let copy = duplicate_draft_plan(&state, &src).await.expect("duplicate").plan;
        assert_ne!(copy.id, src);
        assert_eq!(copy.title, "Source (copy)");
        assert_eq!(copy.workspace_path, crate::services::agent_plans::get_plan(&state, &src).unwrap().plan.workspace_path);
        assert_eq!(copy.initiative_status, "briefing");
        let v1 = get_version(&state, &copy.id, None).unwrap();
        assert_eq!((v1.version, v1.author.as_str()), (1, "you"));
        assert_eq!((v1.fields, v1.outline), (src_latest.fields, src_latest.outline));
        assert_eq!(v1.change_note, format!("duplicated from {} v3", src));
        assert_eq!(get_version(&state, &src, None).unwrap().version, 3, "the source is untouched");
    }

    #[tokio::test]
    async fn duplicate_refuses_non_briefing_source() {
        let (state, _root) = test_state();
        let ws = use_store(&state);
        let src = create(&state, &ws, "Source", Some("Ship.")).await;
        exec(&state, "UPDATE agent_plans SET initiative_status='planning' WHERE id=?1", &src);
        let before = list_draft_plans(&state).unwrap().len();
        let e = duplicate_draft_plan(&state, &src).await.unwrap_err();
        assert!(e.contains("in briefing"), "{e}");
        assert_eq!(list_draft_plans(&state).unwrap().len(), before, "no copy created");
        assert!(duplicate_draft_plan(&state, "missing").await.is_err());
    }

    #[tokio::test]
    async fn create_draft_plan_spawns_nothing() {
        let (state, _root) = test_state();
        let ws = use_store(&state);
        let id = create(&state, &ws, "Quiet", Some("x")).await;
        let plan = crate::services::agent_plans::get_plan(&state, &id).unwrap().plan;
        let sid = plan.briefing_session_id.unwrap();
        let tmux: Option<String> = state
            .db
            .with_conn(|c| c.query_row("SELECT tmux_session_name FROM sessions WHERE id=?1", params![sid], |r| r.get(0)).map_err(|e| e.to_string()))
            .unwrap();
        assert_eq!(tmux, None, "no tmux pane is named for the plan agent");
        assert!(state.active_processes.lock().await.is_empty(), "no CLI process was started");
        assert!(plan.worker_session_id.is_none(), "no planner session either");
    }
}
