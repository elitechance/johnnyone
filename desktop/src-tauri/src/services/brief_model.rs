//! The structured brief of a draft plan (Plans tab, initiative 8ad3ae71 phase 00): its shape and
//! every transformation on it, as PURE functions. No state, database, filesystem or clock here —
//! the versioned store (`brief_store`), the MCP tools and the host RPC all call into this module
//! so none of them re-implements a field op, an outline op, the diff or the markdown render.
//!
//! Layout:
//! - fields: [`BriefFields`], [`BriefField`], [`FieldOp`], [`apply_field_op`], [`changed_fields`]
//! - outline: [`Outline`], [`OutlinePhase`], [`OutlineTask`], [`OutlineOp`], [`apply_outline_op`],
//!   [`changed_outline_ids`] (ids are identities, never reused, D8)
//! - diff: [`BriefDiff`], [`diff`] (line-level multiset diff, task 04)
//! - render: [`render_brief_md`] (deterministic markdown handed to the planner on accept)

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

// ───────────────────────────── fields ─────────────────────────────

/// The six brief fields. Stored in `brief_versions.fields_json` with these exact snake_case keys
/// (`where` is the only list field; the rest are multi-line strings whose add/remove work per line).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BriefFields {
    pub outcome: String,
    pub why: String,
    #[serde(rename = "where")]
    pub where_: Vec<String>,
    pub done_when: String,
    pub out_of_scope: String,
    pub decided: String,
}

/// One brief field. [`BriefField::ALL`] is the canonical order used by every list this module
/// produces (changed fields, diff rows, render sections).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BriefField {
    Outcome,
    Why,
    Where,
    DoneWhen,
    OutOfScope,
    Decided,
}

impl BriefField {
    pub const ALL: [BriefField; 6] = [
        BriefField::Outcome,
        BriefField::Why,
        BriefField::Where,
        BriefField::DoneWhen,
        BriefField::OutOfScope,
        BriefField::Decided,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BriefField::Outcome => "outcome",
            BriefField::Why => "why",
            BriefField::Where => "where",
            BriefField::DoneWhen => "done_when",
            BriefField::OutOfScope => "out_of_scope",
            BriefField::Decided => "decided",
        }
    }

    /// Exact keys only (`outcome|why|where|done_when|out_of_scope|decided`).
    pub fn parse(s: &str) -> Result<BriefField, String> {
        BriefField::ALL
            .iter()
            .copied()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown brief field `{}` (expected one of: outcome, why, where, done_when, out_of_scope, decided)",
                    s
                )
            })
    }

    pub fn is_list(self) -> bool {
        matches!(self, BriefField::Where)
    }
}

/// One edit to one field. `Value` replaces (a string, or an array of strings for `where`); `Add`
/// appends a line / pushes an item if absent; `Remove` drops the exactly-matching trimmed line/item.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldOp {
    Value(serde_json::Value),
    Add(String),
    Remove(String),
}

fn string_field(fields: &BriefFields, field: BriefField) -> &str {
    match field {
        BriefField::Outcome => &fields.outcome,
        BriefField::Why => &fields.why,
        BriefField::DoneWhen => &fields.done_when,
        BriefField::OutOfScope => &fields.out_of_scope,
        BriefField::Decided => &fields.decided,
        BriefField::Where => unreachable!("`where` is a list field"),
    }
}

fn string_field_mut(fields: &mut BriefFields, field: BriefField) -> &mut String {
    match field {
        BriefField::Outcome => &mut fields.outcome,
        BriefField::Why => &mut fields.why,
        BriefField::DoneWhen => &mut fields.done_when,
        BriefField::OutOfScope => &mut fields.out_of_scope,
        BriefField::Decided => &mut fields.decided,
        BriefField::Where => unreachable!("`where` is a list field"),
    }
}

/// Trim, drop empties, dedup preserving first occurrence.
fn normalize_items<I: IntoIterator<Item = String>>(items: I) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let t = item.trim().to_string();
        if !t.is_empty() && seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

// ───────────────────────────── input limits ─────────────────────────────
//
// Phase 00 review follow-ups F-00-brief-store-2/3, done in phase 02 (the agent's MCP tools are the
// second writer): every write — human (GraphQL/RPC) or agent (MCP) — goes through
// `apply_field_op` / `apply_outline_op`, so ONE set of caps guards both, and the MCP tool schemas
// mirror the same numbers (`plan_mcp`). Single-line items (`where` entries, outline titles) refuse
// newlines and control characters so they cannot add structure to `render_brief_md`'s output.

/// Max characters of one multi-line text field (`outcome`, `why`, `done_when`, …).
pub const MAX_FIELD_CHARS: usize = 4_000;
/// Max number of `where` items.
pub const MAX_WHERE_ITEMS: usize = 50;
/// Max characters of one `where` item.
pub const MAX_ITEM_CHARS: usize = 300;
/// Max characters of an outline phase/task title.
pub const MAX_TITLE_CHARS: usize = 200;
/// Max outline phases.
pub const MAX_PHASES: usize = 20;
/// Max tasks per outline phase.
pub const MAX_TASKS_PER_PHASE: usize = 30;

/// A single-line value: no newline and no other control character.
fn check_single_line(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.chars().any(|c| c.is_control()) {
        return Err(format!("{} must be a single line without control characters", what));
    }
    let n = s.chars().count();
    if n > max {
        return Err(format!("{} is {} characters; the limit is {}", what, n, max));
    }
    Ok(())
}

/// A multi-line text value: newlines and tabs allowed, other control characters refused.
fn check_text(what: &str, s: &str) -> Result<(), String> {
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r') {
        return Err(format!("{} must not contain control characters", what));
    }
    let n = s.chars().count();
    if n > MAX_FIELD_CHARS {
        return Err(format!("{} is {} characters; the limit is {}", what, n, MAX_FIELD_CHARS));
    }
    Ok(())
}

/// Validate the ONE field an op produced.
fn check_field(fields: &BriefFields, field: BriefField) -> Result<(), String> {
    let name = field.as_str();
    if field.is_list() {
        if fields.where_.len() > MAX_WHERE_ITEMS {
            return Err(format!(
                "`{}` has {} items; the limit is {}",
                name,
                fields.where_.len(),
                MAX_WHERE_ITEMS
            ));
        }
        for item in &fields.where_ {
            check_single_line(&format!("a `{}` item", name), item, MAX_ITEM_CHARS)?;
        }
        Ok(())
    } else {
        check_text(&format!("`{}`", name), string_field(fields, field))
    }
}

/// Apply one op to one field and return the new fields. Errors are messages the caller (a human
/// form or the plan agent) can act on. An `Add` of a line/item already present is a no-op (returns
/// equal fields — the store then rejects it as `no change`); a `Remove` of an absent one is an error.
pub fn apply_field_op(
    fields: &BriefFields,
    field: BriefField,
    op: FieldOp,
) -> Result<BriefFields, String> {
    let mut out = fields.clone();
    let name = field.as_str();
    if field.is_list() {
        match op {
            FieldOp::Value(v) => {
                let arr = v.as_array().ok_or_else(|| {
                    format!("field `{}` expects an array of strings", name)
                })?;
                let mut items = Vec::with_capacity(arr.len());
                for item in arr {
                    let s = item.as_str().ok_or_else(|| {
                        format!("field `{}` expects an array of strings", name)
                    })?;
                    items.push(s.to_string());
                }
                out.where_ = normalize_items(items);
            }
            FieldOp::Add(item) => {
                let item = item.trim();
                if item.is_empty() {
                    return Err(format!("cannot add an empty item to `{}`", name));
                }
                if !out.where_.iter().any(|w| w == item) {
                    out.where_.push(item.to_string());
                }
            }
            FieldOp::Remove(item) => {
                let item = item.trim();
                let before = out.where_.len();
                out.where_.retain(|w| w != item);
                if out.where_.len() == before {
                    return Err(format!("`{}` has no item `{}`", name, item));
                }
            }
        }
        check_field(&out, field)?;
        return Ok(out);
    }

    match op {
        FieldOp::Value(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| format!("field `{}` expects a string", name))?;
            *string_field_mut(&mut out, field) = s.trim().to_string();
        }
        FieldOp::Add(line) => {
            let line = line.trim();
            if line.is_empty() {
                return Err(format!("cannot add an empty line to `{}`", name));
            }
            let current = string_field(&out, field);
            if current.lines().any(|l| l.trim() == line) {
                return Ok(out);
            }
            let next = if current.trim().is_empty() {
                line.to_string()
            } else {
                format!("{}\n{}", current.trim_end(), line)
            };
            *string_field_mut(&mut out, field) = next;
        }
        FieldOp::Remove(line) => {
            let line = line.trim();
            let current = string_field(&out, field);
            if !current.lines().any(|l| l.trim() == line) {
                return Err(format!("`{}` has no line `{}`", name, line));
            }
            let kept: Vec<&str> = current.lines().filter(|l| l.trim() != line).collect();
            *string_field_mut(&mut out, field) = kept.join("\n").trim().to_string();
        }
    }
    check_field(&out, field)?;
    Ok(out)
}

/// Copy one field's value from `src` into `dst` (used by per-field undo/revert).
pub fn copy_field(dst: &BriefFields, src: &BriefFields, field: BriefField) -> BriefFields {
    let mut out = dst.clone();
    if field.is_list() {
        out.where_ = src.where_.clone();
    } else {
        *string_field_mut(&mut out, field) = string_field(src, field).to_string();
    }
    out
}

fn field_eq(a: &BriefFields, b: &BriefFields, field: BriefField) -> bool {
    if field.is_list() {
        a.where_ == b.where_
    } else {
        string_field(a, field) == string_field(b, field)
    }
}

/// The field keys whose values differ, in canonical order.
pub fn changed_fields(a: &BriefFields, b: &BriefFields) -> Vec<String> {
    BriefField::ALL
        .iter()
        .filter(|f| !field_eq(a, b, **f))
        .map(|f| f.as_str().to_string())
        .collect()
}

// ───────────────────────────── outline ─────────────────────────────

/// The draft outline. Ids are identities (D8): phases `NN`, tasks `NN/MM`; an id is never reused,
/// even after a delete — `next_phase` / `next_task` remember the next number to hand out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outline {
    pub phases: Vec<OutlinePhase>,
    pub next_phase: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutlinePhase {
    pub id: String,
    pub title: String,
    pub author: String,
    pub added_in: i64,
    pub tasks: Vec<OutlineTask>,
    pub next_task: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutlineTask {
    pub id: String,
    pub title: String,
    pub author: String,
    pub added_in: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutlineOp {
    AddPhase { title: String },
    AddTask { phase_id: String, title: String },
    Update { id: String, title: String },
    Move { id: String, to_index: usize },
    Delete { id: String },
}

fn clean_title(title: &str) -> Result<String, String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("title must not be empty".to_string());
    }
    check_single_line("title", t, MAX_TITLE_CHARS)?;
    Ok(t.to_string())
}

fn unknown_id(id: &str) -> String {
    format!("unknown outline id `{}`", id)
}

/// Numeric suffix of an id segment (`"03"` → 3); malformed segments count as 0.
fn id_num(seg: &str) -> u32 {
    seg.parse().unwrap_or(0)
}

impl Outline {
    fn find_phase(&self, id: &str) -> Option<usize> {
        self.phases.iter().position(|p| p.id == id)
    }

    /// `(phase index, task index)` for a task id.
    fn find_task(&self, id: &str) -> Option<(usize, usize)> {
        self.phases.iter().enumerate().find_map(|(pi, p)| {
            p.tasks.iter().position(|t| t.id == id).map(|ti| (pi, ti))
        })
    }
}

fn move_within<T>(items: &mut Vec<T>, from: usize, to: usize) -> Result<(), String> {
    if to >= items.len() {
        return Err(format!(
            "toIndex {} is out of range (0..={})",
            to,
            items.len().saturating_sub(1)
        ));
    }
    let item = items.remove(from);
    items.insert(to, item);
    Ok(())
}

/// Apply one outline op. Returns the new outline and the affected id (the new id for an add).
/// `author` (`you`|`agent`) and `next_version` stamp newly added items.
pub fn apply_outline_op(
    outline: &Outline,
    op: OutlineOp,
    author: &str,
    next_version: i64,
) -> Result<(Outline, String), String> {
    let mut out = outline.clone();
    match op {
        OutlineOp::AddPhase { title } => {
            let title = clean_title(&title)?;
            if out.phases.len() >= MAX_PHASES {
                return Err(format!("the outline already has {} phases (the limit)", MAX_PHASES));
            }
            let max_used = out.phases.iter().map(|p| id_num(&p.id) + 1).max().unwrap_or(0);
            let n = out.next_phase.max(max_used);
            let id = format!("{:02}", n);
            out.next_phase = n + 1;
            out.phases.push(OutlinePhase {
                id: id.clone(),
                title,
                author: author.to_string(),
                added_in: next_version,
                tasks: Vec::new(),
                next_task: 1,
            });
            Ok((out, id))
        }
        OutlineOp::AddTask { phase_id, title } => {
            let title = clean_title(&title)?;
            let pi = out.find_phase(&phase_id).ok_or_else(|| unknown_id(&phase_id))?;
            let phase = &mut out.phases[pi];
            if phase.tasks.len() >= MAX_TASKS_PER_PHASE {
                return Err(format!(
                    "phase `{}` already has {} tasks (the limit)",
                    phase.id, MAX_TASKS_PER_PHASE
                ));
            }
            let max_used = phase
                .tasks
                .iter()
                .map(|t| id_num(t.id.rsplit('/').next().unwrap_or("")) + 1)
                .max()
                .unwrap_or(1);
            let n = phase.next_task.max(max_used).max(1);
            let id = format!("{}/{:02}", phase.id, n);
            phase.next_task = n + 1;
            phase.tasks.push(OutlineTask {
                id: id.clone(),
                title,
                author: author.to_string(),
                added_in: next_version,
            });
            Ok((out, id))
        }
        OutlineOp::Update { id, title } => {
            let title = clean_title(&title)?;
            if let Some(pi) = out.find_phase(&id) {
                out.phases[pi].title = title;
            } else if let Some((pi, ti)) = out.find_task(&id) {
                out.phases[pi].tasks[ti].title = title;
            } else {
                return Err(unknown_id(&id));
            }
            Ok((out, id))
        }
        OutlineOp::Move { id, to_index } => {
            if let Some(pi) = out.find_phase(&id) {
                move_within(&mut out.phases, pi, to_index)?;
            } else if let Some((pi, ti)) = out.find_task(&id) {
                move_within(&mut out.phases[pi].tasks, ti, to_index)?;
            } else {
                return Err(unknown_id(&id));
            }
            Ok((out, id))
        }
        OutlineOp::Delete { id } => {
            if let Some(pi) = out.find_phase(&id) {
                // Keep the allocator past the deleted id so it is never handed out again.
                out.next_phase = out.next_phase.max(id_num(&id) + 1);
                out.phases.remove(pi);
            } else if let Some((pi, ti)) = out.find_task(&id) {
                let phase = &mut out.phases[pi];
                let n = id_num(id.rsplit('/').next().unwrap_or(""));
                phase.next_task = phase.next_task.max(n + 1);
                phase.tasks.remove(ti);
            } else {
                return Err(unknown_id(&id));
            }
            Ok((out, id))
        }
    }
}

/// Flattened `(id, title, parent)` view in display order: each phase, then its tasks.
fn outline_items(o: &Outline) -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    for p in &o.phases {
        v.push((p.id.clone(), p.title.clone(), String::new()));
        for t in &p.tasks {
            v.push((t.id.clone(), t.title.clone(), p.id.clone()));
        }
    }
    v
}

/// Ids (among those present in both) whose relative order within their parent changed. Uses a
/// longest-increasing-subsequence so moving ONE item reports only that item, not every sibling
/// whose index shifted.
fn moved_ids(a: &Outline, b: &Outline) -> Vec<String> {
    let parents = |o: &Outline| -> HashMap<String, Vec<String>> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for (id, _, parent) in outline_items(o) {
            m.entry(parent).or_default().push(id);
        }
        m
    };
    let pa = parents(a);
    let pb = parents(b);
    let mut moved = Vec::new();
    for (parent, ids_b) in &pb {
        let Some(ids_a) = pa.get(parent) else { continue };
        let pos_a: HashMap<&str, usize> =
            ids_a.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
        let common: Vec<(&str, usize)> = ids_b
            .iter()
            .filter_map(|id| pos_a.get(id.as_str()).map(|&p| (id.as_str(), p)))
            .collect();
        let n = common.len();
        if n < 2 {
            continue;
        }
        // O(n²) LIS over the a-positions in b order; outlines are short.
        let mut len = vec![1usize; n];
        let mut prev = vec![usize::MAX; n];
        for i in 0..n {
            for j in 0..i {
                if common[j].1 < common[i].1 && len[j] + 1 > len[i] {
                    len[i] = len[j] + 1;
                    prev[i] = j;
                }
            }
        }
        let mut best = 0;
        for i in 1..n {
            if len[i] > len[best] {
                best = i;
            }
        }
        let mut keep = HashSet::new();
        let mut cur = best;
        loop {
            keep.insert(cur);
            if prev[cur] == usize::MAX {
                break;
            }
            cur = prev[cur];
        }
        for (i, (id, _)) in common.iter().enumerate() {
            if !keep.contains(&i) {
                moved.push(id.to_string());
            }
        }
    }
    moved.sort();
    moved
}

/// Outline ids that were added, removed, renamed or moved between `a` and `b`, each prefixed
/// `outline:`, sorted and deduplicated. Feeds `brief_versions.changed_json`.
pub fn changed_outline_ids(a: &Outline, b: &Outline) -> Vec<String> {
    let d = outline_diff(a, b);
    let mut set = BTreeSet::new();
    for item in d.added.iter().chain(d.removed.iter()) {
        set.insert(item.id.clone());
    }
    for r in &d.renamed {
        set.insert(r.id.clone());
    }
    for id in &d.moved {
        set.insert(id.clone());
    }
    set.into_iter().map(|id| format!("outline:{}", id)).collect()
}

// ───────────────────────────── diff ─────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefDiff {
    pub fields: Vec<FieldDiff>,
    pub outline: OutlineDiff,
}

impl BriefDiff {
    pub fn is_empty(&self) -> bool {
        self.fields.iter().all(|f| !f.changed) && self.outline.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDiff {
    pub field: String,
    pub changed: bool,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItemRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineRename {
    pub id: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineDiff {
    pub added: Vec<OutlineItemRef>,
    pub removed: Vec<OutlineItemRef>,
    pub renamed: Vec<OutlineRename>,
    pub moved: Vec<String>,
}

impl OutlineDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.renamed.is_empty() && self.moved.is_empty()
    }
}

/// Lines (string fields) or items (`where`) of one field, trimmed, non-empty.
fn field_lines(fields: &BriefFields, field: BriefField) -> Vec<String> {
    if field.is_list() {
        fields
            .where_
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        string_field(fields, field)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }
}

/// Multiset difference `a − b`, preserving `a`'s order.
fn multiset_minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in b {
        *counts.entry(s.as_str()).or_default() += 1;
    }
    let mut out = Vec::new();
    for s in a {
        match counts.get_mut(s.as_str()) {
            Some(c) if *c > 0 => *c -= 1,
            _ => out.push(s.clone()),
        }
    }
    out
}

fn outline_diff(a: &Outline, b: &Outline) -> OutlineDiff {
    let ia = outline_items(a);
    let ib = outline_items(b);
    let ta: HashMap<&str, &str> = ia.iter().map(|(id, t, _)| (id.as_str(), t.as_str())).collect();
    let tb: HashMap<&str, &str> = ib.iter().map(|(id, t, _)| (id.as_str(), t.as_str())).collect();
    let mut d = OutlineDiff::default();
    for (id, title, _) in &ib {
        match ta.get(id.as_str()) {
            None => d.added.push(OutlineItemRef { id: id.clone(), title: title.clone() }),
            Some(before) if *before != title.as_str() => d.renamed.push(OutlineRename {
                id: id.clone(),
                before: before.to_string(),
                after: title.clone(),
            }),
            _ => {}
        }
    }
    for (id, title, _) in &ia {
        if !tb.contains_key(id.as_str()) {
            d.removed.push(OutlineItemRef { id: id.clone(), title: title.clone() });
        }
    }
    d.moved = moved_ids(a, b);
    d
}

/// Version-to-version diff, computed server-side so the web only renders it. Every field gets a
/// row in canonical order; a field is `changed` only when a line/item was added or removed (a pure
/// reorder of lines is not a change).
pub fn diff(
    a_fields: &BriefFields,
    a_outline: &Outline,
    b_fields: &BriefFields,
    b_outline: &Outline,
) -> BriefDiff {
    let fields = BriefField::ALL
        .iter()
        .map(|&f| {
            let la = field_lines(a_fields, f);
            let lb = field_lines(b_fields, f);
            let removed = multiset_minus(&la, &lb);
            let added = multiset_minus(&lb, &la);
            FieldDiff {
                field: f.as_str().to_string(),
                changed: !removed.is_empty() || !added.is_empty(),
                removed,
                added,
            }
        })
        .collect();
    BriefDiff { fields, outline: outline_diff(a_outline, b_outline) }
}

// ───────────────────────────── render ─────────────────────────────

/// Leading words that make a `done_when` line a runnable command. The ONE list: the render wraps
/// such lines in backticks and `brief_check`'s `done_when_runnable` rule counts them.
pub const COMMAND_PREFIXES: &[&str] = &[
    "cargo", "npm", "npx", "nx", "node", "pnpm", "yarn", "lokal", "curl", "git", "cd", "make",
    "bash", "sh", "python", "pytest", "go", "tmux", "grep",
];

/// Strip a leading markdown bullet (`- ` / `* `) from a line.
pub fn strip_bullet(line: &str) -> &str {
    let t = line.trim();
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .map(str::trim)
        .unwrap_or(t)
}

/// True when the line is wrapped in backticks.
pub fn is_backticked(line: &str) -> bool {
    let t = strip_bullet(line);
    t.len() >= 2 && t.starts_with('`') && t.ends_with('`')
}

/// True when the line's first word is one of [`COMMAND_PREFIXES`] (exact word, not a prefix of a
/// longer word: `golang` is not `go`).
pub fn starts_with_command(line: &str) -> bool {
    let t = strip_bullet(line);
    let first = t.split_whitespace().next().unwrap_or("");
    COMMAND_PREFIXES.contains(&first)
}

/// A `done_when` line that is a command: a known command word or a backticked line.
pub fn is_command_line(line: &str) -> bool {
    is_backticked(line) || starts_with_command(line)
}

fn lines_of(s: &str) -> Vec<&str> {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// Neutralise markdown STRUCTURE in one line of user/agent text so a field value cannot add a
/// section, a setext heading, a thematic break or an unclosed code fence to the rendered brief (the
/// planner's input). The text itself is kept; only a leading backslash escape is added.
pub fn neutralize_line(line: &str) -> String {
    let t = line.trim_start();
    let structural = t.starts_with('#')
        || t.starts_with("```")
        || t.starts_with("~~~")
        || (!t.is_empty() && t.chars().all(|c| c == '=' || c == '-' || c == ' '));
    if structural {
        format!("\\{}", t)
    } else {
        line.to_string()
    }
}

fn text_lines(s: &str) -> Vec<String> {
    lines_of(s).into_iter().map(neutralize_line).collect()
}

/// Deterministic markdown of a brief: fixed section order, empty sections omitted, one trailing
/// newline, no timestamps. This text becomes `agent_plans.brief` on every write and the planner's
/// input on accept.
pub fn render_brief_md(fields: &BriefFields, outline: &Outline) -> String {
    let mut sections: Vec<String> = Vec::new();
    let mut text_section = |heading: &str, body: &str| {
        let lines = text_lines(body);
        if !lines.is_empty() {
            sections.push(format!("{}\n{}", heading, lines.join("\n")));
        }
    };
    text_section("# Outcome", &fields.outcome);
    text_section("## Why", &fields.why);

    let where_items: Vec<String> = fields
        .where_
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .map(|w| format!("- `{}`", w.trim_matches('`').replace('`', "'")))
        .collect();
    if !where_items.is_empty() {
        sections.push(format!("## Where\n{}", where_items.join("\n")));
    }

    let done: Vec<String> = lines_of(&fields.done_when)
        .into_iter()
        .map(|l| {
            let item = strip_bullet(l);
            if is_backticked(item) {
                format!("- {}", item)
            } else if starts_with_command(item) {
                format!("- `{}`", item)
            } else {
                format!("- {}", neutralize_line(item))
            }
        })
        .collect();
    if !done.is_empty() {
        sections.push(format!("## Done when\n{}", done.join("\n")));
    }

    let mut text_section = |heading: &str, body: &str| {
        let lines = text_lines(body);
        if !lines.is_empty() {
            sections.push(format!("{}\n{}", heading, lines.join("\n")));
        }
    };
    text_section("## Out of scope", &fields.out_of_scope);
    text_section("## Already decided", &fields.decided);

    if !outline.phases.is_empty() {
        let mut s = String::from("## Draft outline");
        for p in &outline.phases {
            s.push_str(&format!("\n### {} {}", p.id, p.title.trim()));
            for t in &p.tasks {
                s.push_str(&format!("\n- {} {}", t.id, t.title.trim()));
            }
        }
        sections.push(s);
    }

    if sections.is_empty() {
        return String::new();
    }
    let mut out = sections.join("\n\n");
    out.push('\n');
    out
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_fields() -> BriefFields {
        BriefFields {
            outcome: "A Plans tab where a chat agent edits a versioned brief.".into(),
            why: "Briefs drift.\nThe planner needs structure.".into(),
            where_: vec!["web/src/app/pages/plans".into(), "desktop/src-tauri/src/services".into()],
            done_when: "cargo test -j 8 --lib\n`npx nx build web`\nThe tab is visible".into(),
            out_of_scope: "The Plan-doc comments bar".into(),
            decided: "Headless plan agent (D1)".into(),
        }
    }

    fn sample_outline() -> Outline {
        let o = Outline::default();
        let (o, _) = apply_outline_op(&o, OutlineOp::AddPhase { title: "Store".into() }, "you", 2).unwrap();
        let (o, _) = apply_outline_op(
            &o,
            OutlineOp::AddTask { phase_id: "00".into(), title: "Migration".into() },
            "agent",
            3,
        )
        .unwrap();
        let (o, _) = apply_outline_op(&o, OutlineOp::AddPhase { title: "API".into() }, "agent", 4).unwrap();
        o
    }

    // ── task 02: render ──

    #[test]
    fn render_is_deterministic() {
        let f = sample_fields();
        let o = sample_outline();
        assert_eq!(render_brief_md(&f, &o), render_brief_md(&f, &o));
        // Construction order of the struct does not matter; the section order is fixed.
        let g = BriefFields {
            decided: f.decided.clone(),
            out_of_scope: f.out_of_scope.clone(),
            done_when: f.done_when.clone(),
            where_: f.where_.clone(),
            why: f.why.clone(),
            outcome: f.outcome.clone(),
        };
        assert_eq!(render_brief_md(&f, &o), render_brief_md(&g, &o));
        // Round-tripping through JSON (as the store does) renders the same bytes.
        let f2: BriefFields = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(render_brief_md(&f, &o), render_brief_md(&f2, &o));
        let md = render_brief_md(&f, &o);
        let order = ["# Outcome", "## Why", "## Where", "## Done when", "## Out of scope", "## Already decided", "## Draft outline"];
        let mut last = 0;
        for h in order {
            let at = md.find(h).unwrap_or_else(|| panic!("missing {h}"));
            assert!(at >= last, "{h} out of order");
            last = at;
        }
    }

    #[test]
    fn render_omits_empty_sections() {
        let f = BriefFields { outcome: "Only this.".into(), ..Default::default() };
        let md = render_brief_md(&f, &Outline::default());
        assert_eq!(md, "# Outcome\nOnly this.\n");
        for absent in ["## Why", "## Where", "## Done when", "## Out of scope", "## Already decided", "## Draft outline"] {
            assert!(!md.contains(absent), "{absent} must be omitted");
        }
        // Whitespace-only counts as empty.
        let f = BriefFields { why: "  \n \n".into(), where_: vec!["  ".into()], ..Default::default() };
        assert_eq!(render_brief_md(&f, &Outline::default()), "");
    }

    #[test]
    fn render_full_golden() {
        let golden = "# Outcome
A Plans tab where a chat agent edits a versioned brief.

## Why
Briefs drift.
The planner needs structure.

## Where
- `web/src/app/pages/plans`
- `desktop/src-tauri/src/services`

## Done when
- `cargo test -j 8 --lib`
- `npx nx build web`
- The tab is visible

## Out of scope
The Plan-doc comments bar

## Already decided
Headless plan agent (D1)

## Draft outline
### 00 Store
- 00/01 Migration
### 01 API
";
        assert_eq!(render_brief_md(&sample_fields(), &sample_outline()), golden);
        assert!(!golden.contains("20"), "no timestamps");
    }

    // ── task 02: field ops ──

    #[test]
    fn field_ops_table() {
        let base = BriefFields {
            outcome: "One.".into(),
            why: "a\nb".into(),
            where_: vec!["src/a".into()],
            done_when: "cargo test".into(),
            out_of_scope: "x".into(),
            decided: "".into(),
        };
        // (field, op, expected value of that field after | Err(substring))
        type Row = (&'static str, FieldOp, Result<serde_json::Value, &'static str>);
        let rows: Vec<Row> = vec![
            // Value on every string field
            ("outcome", FieldOp::Value(json!("  New outcome.  ")), Ok(json!("New outcome."))),
            ("why", FieldOp::Value(json!("w")), Ok(json!("w"))),
            ("done_when", FieldOp::Value(json!("npm test")), Ok(json!("npm test"))),
            ("out_of_scope", FieldOp::Value(json!("")), Ok(json!(""))),
            ("decided", FieldOp::Value(json!("d1\nd2")), Ok(json!("d1\nd2"))),
            // Value type mismatch
            ("outcome", FieldOp::Value(json!(["a"])), Err("expects a string")),
            ("decided", FieldOp::Value(json!(3)), Err("expects a string")),
            ("where", FieldOp::Value(json!("src/a")), Err("expects an array of strings")),
            ("where", FieldOp::Value(json!(["a", 1])), Err("expects an array of strings")),
            // where Value: trimmed, deduped, empties dropped
            ("where", FieldOp::Value(json!([" b ", "b", "", "c"])), Ok(json!(["b", "c"]))),
            // Add on string fields: append a line; existing line = no-op; empty = error
            ("why", FieldOp::Add(" c ".into()), Ok(json!("a\nb\nc"))),
            ("why", FieldOp::Add("b".into()), Ok(json!("a\nb"))),
            ("why", FieldOp::Add("   ".into()), Err("empty line")),
            ("decided", FieldOp::Add("first".into()), Ok(json!("first"))),
            ("outcome", FieldOp::Add("Two.".into()), Ok(json!("One.\nTwo."))),
            ("done_when", FieldOp::Add("npx nx build web".into()), Ok(json!("cargo test\nnpx nx build web"))),
            ("out_of_scope", FieldOp::Add("y".into()), Ok(json!("x\ny"))),
            // Remove on string fields
            ("why", FieldOp::Remove(" a ".into()), Ok(json!("b"))),
            ("why", FieldOp::Remove("zzz".into()), Err("has no line")),
            ("outcome", FieldOp::Remove("One.".into()), Ok(json!(""))),
            ("done_when", FieldOp::Remove("cargo test".into()), Ok(json!(""))),
            ("out_of_scope", FieldOp::Remove("nope".into()), Err("has no line")),
            ("decided", FieldOp::Remove("nope".into()), Err("has no line")),
            // where Add / Remove
            ("where", FieldOp::Add(" src/b ".into()), Ok(json!(["src/a", "src/b"]))),
            ("where", FieldOp::Add("src/a".into()), Ok(json!(["src/a"]))),
            ("where", FieldOp::Add("".into()), Err("empty item")),
            ("where", FieldOp::Remove("src/a".into()), Ok(json!([]))),
            ("where", FieldOp::Remove("src/zz".into()), Err("has no item")),
        ];
        for (i, (field, op, expect)) in rows.into_iter().enumerate() {
            let f = BriefField::parse(field).unwrap();
            let got = apply_field_op(&base, f, op.clone());
            match (got, expect) {
                (Ok(after), Ok(want)) => {
                    let v = serde_json::to_value(&after).unwrap();
                    assert_eq!(v[field], want, "row {i}: {field} {op:?}");
                    // No other field changed.
                    let changed = changed_fields(&base, &after);
                    assert!(changed.iter().all(|c| c == field), "row {i}: touched {changed:?}");
                }
                (Err(e), Err(sub)) => assert!(e.contains(sub), "row {i}: error `{e}` lacks `{sub}`"),
                (got, want) => panic!("row {i}: {field} {op:?}: got {got:?}, want {want:?}"),
            }
        }
        assert!(BriefField::parse("Outcome").is_err(), "keys are exact");
        assert!(BriefField::parse("where_").is_err());
        for f in BriefField::ALL {
            assert_eq!(BriefField::parse(f.as_str()).unwrap(), f);
        }
    }

    #[test]
    fn changed_fields_reports_only_differences() {
        let a = sample_fields();
        assert!(changed_fields(&a, &a).is_empty());
        let mut b = a.clone();
        b.decided = "other".into();
        b.outcome = "Changed.".into();
        b.where_.reverse();
        // Canonical order, regardless of mutation order.
        assert_eq!(changed_fields(&a, &b), vec!["outcome", "where", "decided"]);
        assert_eq!(
            changed_fields(&a, &copy_field(&a, &b, BriefField::Decided)),
            vec!["decided"]
        );
    }

    // ── task 03: outline ops ──

    fn add_phase(o: &Outline, t: &str) -> (Outline, String) {
        apply_outline_op(o, OutlineOp::AddPhase { title: t.into() }, "you", 1).unwrap()
    }
    fn add_task(o: &Outline, p: &str, t: &str) -> (Outline, String) {
        apply_outline_op(o, OutlineOp::AddTask { phase_id: p.into(), title: t.into() }, "agent", 1).unwrap()
    }

    #[test]
    fn outline_ids_are_stable_and_never_reused() {
        let (o, a) = add_phase(&Outline::default(), "A");
        let (o, b) = add_phase(&o, "B");
        assert_eq!((a.as_str(), b.as_str()), ("00", "01"));
        let (o, _) = apply_outline_op(&o, OutlineOp::Delete { id: "01".into() }, "you", 2).unwrap();
        let (o, c) = add_phase(&o, "C");
        assert_eq!(c, "02", "a deleted id is never handed out again");
        // Same for tasks.
        let (o, t1) = add_task(&o, "00", "t1");
        let (o, t2) = add_task(&o, "00", "t2");
        assert_eq!((t1.as_str(), t2.as_str()), ("00/01", "00/02"));
        let (o, _) = apply_outline_op(&o, OutlineOp::Delete { id: "00/02".into() }, "you", 3).unwrap();
        let (o, t3) = add_task(&o, "00", "t3");
        assert_eq!(t3, "00/03");
        // Moving does not renumber.
        let (o, _) = apply_outline_op(&o, OutlineOp::Move { id: "02".into(), to_index: 0 }, "you", 4).unwrap();
        assert_eq!(o.phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["02", "00"]);
        // Even deleting the LAST phase keeps the allocator past it.
        let (o, _) = apply_outline_op(&o, OutlineOp::Delete { id: "02".into() }, "you", 5).unwrap();
        let (_, d) = add_phase(&o, "D");
        assert_eq!(d, "03");
    }

    #[test]
    fn task_ids_are_scoped_to_phase() {
        let (o, _) = add_phase(&Outline::default(), "A");
        let (o, _) = add_phase(&o, "B");
        let (o, a1) = add_task(&o, "00", "a1");
        let (o, b1) = add_task(&o, "01", "b1");
        assert_eq!((a1.as_str(), b1.as_str()), ("00/01", "01/01"));
        let t = &o.phases[0].tasks[0];
        assert_eq!((t.author.as_str(), t.added_in), ("agent", 1));
    }

    #[test]
    fn move_reorders_within_parent_only() {
        let (o, _) = add_phase(&Outline::default(), "A");
        let (o, _) = add_phase(&o, "B");
        let (o, _) = add_task(&o, "00", "x");
        let (o, _) = add_task(&o, "00", "y");
        let (o, _) = add_task(&o, "01", "z");
        let (o, id) = apply_outline_op(&o, OutlineOp::Move { id: "00/02".into(), to_index: 0 }, "you", 2).unwrap();
        assert_eq!(id, "00/02");
        assert_eq!(o.phases[0].tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["00/02", "00/01"]);
        assert_eq!(o.phases[1].tasks.len(), 1, "the other phase is untouched");
        // to_index addresses the task's own parent: index 1 in phase 00, not across phases.
        assert!(apply_outline_op(&o, OutlineOp::Move { id: "01/01".into(), to_index: 1 }, "you", 3).is_err());
    }

    #[test]
    fn move_out_of_range_errors() {
        let (o, _) = add_phase(&Outline::default(), "A");
        let (o, _) = add_phase(&o, "B");
        let e = apply_outline_op(&o, OutlineOp::Move { id: "00".into(), to_index: 2 }, "you", 2).unwrap_err();
        assert!(e.contains("out of range"), "{e}");
        assert!(apply_outline_op(&o, OutlineOp::Move { id: "00".into(), to_index: 1 }, "you", 2).is_ok());
    }

    #[test]
    fn unknown_id_errors() {
        let (o, _) = add_phase(&Outline::default(), "A");
        for op in [
            OutlineOp::AddTask { phase_id: "09".into(), title: "t".into() },
            OutlineOp::Update { id: "00/07".into(), title: "t".into() },
            OutlineOp::Move { id: "05".into(), to_index: 0 },
            OutlineOp::Delete { id: "nope".into() },
        ] {
            let e = apply_outline_op(&o, op, "you", 2).unwrap_err();
            assert!(e.contains("unknown outline id"), "{e}");
        }
    }

    #[test]
    fn empty_title_errors() {
        let (o, _) = add_phase(&Outline::default(), "A");
        for op in [
            OutlineOp::AddPhase { title: "  ".into() },
            OutlineOp::AddTask { phase_id: "00".into(), title: "".into() },
            OutlineOp::Update { id: "00".into(), title: " \n".into() },
        ] {
            let e = apply_outline_op(&o, op, "you", 2).unwrap_err();
            assert!(e.contains("title must not be empty"), "{e}");
        }
    }

    #[test]
    fn delete_phase_removes_its_tasks() {
        let (o, _) = add_phase(&Outline::default(), "A");
        let (o, _) = add_task(&o, "00", "x");
        let (o, _) = add_task(&o, "00", "y");
        let (o, _) = apply_outline_op(&o, OutlineOp::Delete { id: "00".into() }, "you", 2).unwrap();
        assert!(o.phases.is_empty());
        assert!(apply_outline_op(&o, OutlineOp::Update { id: "00/01".into(), title: "z".into() }, "you", 3).is_err());
    }

    #[test]
    fn changed_outline_ids_detects_add_remove_rename_move() {
        let (a, _) = add_phase(&Outline::default(), "A");
        let (a, _) = add_phase(&a, "B");
        let (a, _) = add_phase(&a, "C");
        let (a, _) = add_task(&a, "00", "x");
        assert!(changed_outline_ids(&a, &a).is_empty());
        // add
        let (b, _) = add_task(&a, "01", "new");
        assert_eq!(changed_outline_ids(&a, &b), vec!["outline:01/01"]);
        // remove
        let (b, _) = apply_outline_op(&a, OutlineOp::Delete { id: "00/01".into() }, "you", 2).unwrap();
        assert_eq!(changed_outline_ids(&a, &b), vec!["outline:00/01"]);
        // rename
        let (b, _) = apply_outline_op(&a, OutlineOp::Update { id: "01".into(), title: "B2".into() }, "you", 2).unwrap();
        assert_eq!(changed_outline_ids(&a, &b), vec!["outline:01"]);
        // move: only the moved item is reported, not every shifted sibling
        let (b, _) = apply_outline_op(&a, OutlineOp::Move { id: "02".into(), to_index: 0 }, "you", 2).unwrap();
        assert_eq!(changed_outline_ids(&a, &b), vec!["outline:02"]);
        // deleting an earlier sibling shifts indices but is NOT a move of the later ones
        let (b, _) = apply_outline_op(&a, OutlineOp::Delete { id: "00".into() }, "you", 2).unwrap();
        assert_eq!(changed_outline_ids(&a, &b), vec!["outline:00", "outline:00/01"]);
    }

    #[test]
    fn outline_json_roundtrip() {
        let (o, _) = add_phase(&Outline::default(), "A");
        let (o, _) = add_phase(&o, "B");
        let (o, _) = add_task(&o, "00", "x");
        let (o, _) = apply_outline_op(&o, OutlineOp::Delete { id: "01".into() }, "you", 2).unwrap();
        let s = serde_json::to_string(&o).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["nextPhase"], json!(2));
        assert_eq!(v["phases"][0]["nextTask"], json!(2));
        assert_eq!(v["phases"][0]["addedIn"], json!(1));
        assert_eq!(v["phases"][0]["tasks"][0]["id"], json!("00/01"));
        let back: Outline = serde_json::from_str(&s).unwrap();
        assert_eq!(back, o);
        // An empty object (a v1 seed) deserializes to the empty outline.
        assert_eq!(serde_json::from_str::<Outline>("{}").unwrap(), Outline::default());
    }

    // ── task 04: diff ──

    #[test]
    fn diff_identical_is_empty() {
        let f = sample_fields();
        let o = sample_outline();
        let d = diff(&f, &o, &f, &o);
        assert!(d.is_empty());
        assert_eq!(d.fields.len(), 6, "one row per field, canonical order");
        assert_eq!(d.fields[2].field, "where");
        // A pure reorder of lines is not a change.
        let mut g = f.clone();
        g.why = "The planner needs structure.\nBriefs drift.".into();
        assert!(diff(&f, &o, &g, &o).is_empty());
    }

    #[test]
    fn diff_line_added_removed() {
        let a = sample_fields();
        let mut b = a.clone();
        b.why = "Briefs drift.\nAgents need a contract.".into();
        b.done_when = format!("{}\nnpx vitest run", a.done_when);
        let d = diff(&a, &Outline::default(), &b, &Outline::default());
        let why = d.fields.iter().find(|f| f.field == "why").unwrap();
        assert!(why.changed);
        assert_eq!(why.removed, vec!["The planner needs structure."]);
        assert_eq!(why.added, vec!["Agents need a contract."]);
        let done = d.fields.iter().find(|f| f.field == "done_when").unwrap();
        assert_eq!((done.removed.len(), done.added.clone()), (0, vec!["npx vitest run".to_string()]));
        assert!(!d.fields.iter().find(|f| f.field == "outcome").unwrap().changed);
        // Serialized camelCase for the web.
        let v = serde_json::to_value(&d).unwrap();
        assert!(v["outline"]["renamed"].is_array());
    }

    #[test]
    fn diff_where_items() {
        let a = sample_fields();
        let mut b = a.clone();
        b.where_ = vec!["desktop/src-tauri/src/services".into(), "worker/schema".into()];
        let d = diff(&a, &Outline::default(), &b, &Outline::default());
        let w = d.fields.iter().find(|f| f.field == "where").unwrap();
        assert!(w.changed);
        assert_eq!(w.removed, vec!["web/src/app/pages/plans"]);
        assert_eq!(w.added, vec!["worker/schema"]);
    }

    #[test]
    fn diff_outline_all_kinds() {
        let (a, _) = add_phase(&Outline::default(), "A");
        let (a, _) = add_phase(&a, "B");
        let (a, _) = add_phase(&a, "C");
        let (a, _) = add_task(&a, "00", "x");
        let (b, _) = apply_outline_op(&a, OutlineOp::Update { id: "00".into(), title: "A2".into() }, "you", 2).unwrap();
        let (b, _) = apply_outline_op(&b, OutlineOp::Delete { id: "00/01".into() }, "you", 3).unwrap();
        let (b, _) = add_task(&b, "01", "y");
        let (b, _) = apply_outline_op(&b, OutlineOp::Move { id: "02".into(), to_index: 0 }, "you", 4).unwrap();
        let d = diff(&BriefFields::default(), &a, &BriefFields::default(), &b);
        assert_eq!(d.outline.added, vec![OutlineItemRef { id: "01/01".into(), title: "y".into() }]);
        assert_eq!(d.outline.removed, vec![OutlineItemRef { id: "00/01".into(), title: "x".into() }]);
        assert_eq!(
            d.outline.renamed,
            vec![OutlineRename { id: "00".into(), before: "A".into(), after: "A2".into() }]
        );
        assert_eq!(d.outline.moved, vec!["02"]);
        assert!(d.fields.iter().all(|f| !f.changed));
    }

    #[test]
    fn diff_is_antisymmetric() {
        let a = sample_fields();
        let mut b = a.clone();
        b.why = "Briefs drift.\nNew reason.".into();
        b.where_.push("worker".into());
        let (oa, _) = add_phase(&Outline::default(), "A");
        let (ob, _) = add_phase(&oa, "B");
        let ab = diff(&a, &oa, &b, &ob);
        let ba = diff(&b, &ob, &a, &oa);
        for (x, y) in ab.fields.iter().zip(ba.fields.iter()) {
            assert_eq!(x.field, y.field);
            assert_eq!(x.added, y.removed, "{}", x.field);
            assert_eq!(x.removed, y.added, "{}", x.field);
            assert_eq!(x.changed, y.changed);
        }
        assert_eq!(ab.outline.added, ba.outline.removed);
        assert_eq!(ab.outline.removed, ba.outline.added);
    }

    // ── input limits (phase 02; follow-ups F-00-brief-store-2/3) ──

    #[test]
    fn over_cap_values_are_rejected_with_a_clear_error() {
        let f = BriefFields::default();
        let long = "x".repeat(MAX_FIELD_CHARS + 1);
        let e = apply_field_op(&f, BriefField::Why, FieldOp::Value(json!(long))).unwrap_err();
        assert!(e.contains("limit is 4000"), "{e}");
        assert!(apply_field_op(&f, BriefField::Why, FieldOp::Value(json!("x".repeat(MAX_FIELD_CHARS)))).is_ok());
        let item = "p".repeat(MAX_ITEM_CHARS + 1);
        assert!(apply_field_op(&f, BriefField::Where, FieldOp::Add(item)).unwrap_err().contains("limit is 300"));
        let many: Vec<String> = (0..=MAX_WHERE_ITEMS).map(|i| format!("w{i}")).collect();
        assert!(apply_field_op(&f, BriefField::Where, FieldOp::Value(json!(many))).unwrap_err().contains("limit is 50"));
        assert!(apply_field_op(&f, BriefField::Where, FieldOp::Add("a\nb".into())).unwrap_err().contains("single line"));
        assert!(apply_field_op(&f, BriefField::Why, FieldOp::Value(json!("bell\u{7}"))).unwrap_err().contains("control"));
        let o = Outline::default();
        assert!(apply_outline_op(&o, OutlineOp::AddPhase { title: "t".repeat(MAX_TITLE_CHARS + 1) }, "you", 1).is_err());
        let mut o = Outline::default();
        for i in 0..MAX_PHASES {
            o = apply_outline_op(&o, OutlineOp::AddPhase { title: format!("p{i}") }, "you", 1).unwrap().0;
        }
        assert!(apply_outline_op(&o, OutlineOp::AddPhase { title: "one more".into() }, "you", 2).unwrap_err().contains("limit"));
        let mut o2 = apply_outline_op(&Outline::default(), OutlineOp::AddPhase { title: "p".into() }, "you", 1).unwrap().0;
        for i in 0..MAX_TASKS_PER_PHASE {
            o2 = apply_outline_op(&o2, OutlineOp::AddTask { phase_id: "00".into(), title: format!("t{i}") }, "you", 1).unwrap().0;
        }
        assert!(apply_outline_op(&o2, OutlineOp::AddTask { phase_id: "00".into(), title: "x".into() }, "you", 2).is_err());
    }

    #[test]
    fn injected_title_cannot_add_a_heading_or_section() {
        let o = Outline::default();
        let e = apply_outline_op(&o, OutlineOp::AddPhase { title: "Store\n## Injected".into() }, "agent", 1).unwrap_err();
        assert!(e.contains("single line"), "{e}");
        // Text fields keep their words but lose their structure.
        let f = BriefFields {
            outcome: "Ship it.".into(),
            why: "## Injected section\n```\n---\nreal reason".into(),
            done_when: "# not a heading".into(),
            where_: vec!["a`b`".into()],
            ..Default::default()
        };
        let md = render_brief_md(&f, &o);
        let headings: Vec<&str> = md.lines().filter(|l| l.starts_with('#')).collect();
        assert_eq!(headings, vec!["# Outcome", "## Why", "## Where", "## Done when"], "{md}");
        assert!(md.contains("\\## Injected section"));
        assert!(md.contains("\\```"));
        assert!(md.contains("\\---"));
        assert!(md.contains("- \\# not a heading"));
        assert!(md.contains("- `a'b`"), "{md}");
    }
}
