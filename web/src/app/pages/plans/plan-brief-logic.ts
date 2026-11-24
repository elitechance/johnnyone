/**
 * Pure decisions of the plan detail's brief panel (Plans tab, initiative 8ad3ae71 phase 03 task 06).
 *
 * Which field changed in the latest version (the highlight), who/which version last touched each
 * field (provenance), the per-field undo target, check rendering, the Start/Queue gate (D11:
 * failures ask for confirmation, they never disable Start), phone badges, the outline cards, the
 * diff view and the page's load state (S11). DOM-free and Angular-free.
 */
import type {
  AgentPlan,
  BriefCheckItem,
  BriefDiff,
  BriefFieldKey,
  BriefFields,
  BriefOutline,
  BriefVersion,
  BriefVersionMeta,
} from '../../../../../ui/src/services/johnny-api.service';
import { relativeAgo } from './plans-list-logic';

/** The six fields in display order: storage key, the camelCase `BriefFields` key, label. */
export const BRIEF_FIELDS: { key: BriefFieldKey; wire: keyof BriefFields; label: string }[] = [
  { key: 'outcome', wire: 'outcome', label: 'Outcome' },
  { key: 'why', wire: 'why', label: 'Why / evidence' },
  { key: 'where', wire: 'where', label: 'Where' },
  { key: 'done_when', wire: 'doneWhen', label: 'Done when' },
  { key: 'out_of_scope', wire: 'outOfScope', label: 'Out of scope' },
  { key: 'decided', wire: 'decided', label: 'Already decided' },
];

/** The brief-check rule that reveals Queue planning. */
export const FILE_CONFLICT_RULE = 'file_conflict';

export interface FieldRow {
  key: BriefFieldKey;
  label: string;
  /** Text fields: the non-empty lines. */
  lines: string[];
  /** `where`: the items. */
  chips: string[];
  isList: boolean;
  /** Raw editable value (text fields) — what the inline editor starts from. */
  text: string;
  empty: boolean;
  who: 'you' | 'agent' | string;
  /** The version that last changed this field. */
  version: number;
  ago: string;
  /** Changed in the latest version → highlighted, undoable. */
  changed: boolean;
  canUndo: boolean;
  /** The version undo restores this field to (null = nothing to undo to). */
  undoTo: number | null;
}

/** `changed` keys of a version are the snake_case field keys plus `outline:<id>` ids. */
function fieldChanges(changed: string[] | null | undefined): string[] {
  return (changed ?? []).filter((c) => !c.startsWith('outline:'));
}

/** The most recent version (by number) whose `changed` names `field`, or null. */
function lastChangeOf(versions: BriefVersionMeta[], field: string): BriefVersionMeta | null {
  let best: BriefVersionMeta | null = null;
  for (const v of versions ?? []) {
    if ((v.changed ?? []).includes(field) && (!best || v.version > best.version)) best = v;
  }
  return best;
}

/**
 * Per-field undo target: the version just BEFORE the latest version that changed `field` (the server
 * then copies that version's value into a new version). Null when the field was only ever set by v1
 * (or never) — there is nothing older to go back to.
 */
export function undoTarget(versions: BriefVersionMeta[] | null | undefined, field: string): number | null {
  const last = lastChangeOf(versions ?? [], field);
  if (!last || last.version <= 1) return null;
  return last.version - 1;
}

/** A plan whose brief can no longer change: Start planning froze it. */
export function isFrozen(latest: Pick<BriefVersion, 'acceptedBriefVersion' | 'initiativeStatus'> | null | undefined): boolean {
  if (!latest) return false;
  return latest.acceptedBriefVersion != null || latest.initiativeStatus !== 'briefing';
}

function textLines(s: string | null | undefined): string[] {
  return (s ?? '')
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
}

/** The six rows of the brief card. */
export function fieldRows(latest: BriefVersion, versions: BriefVersionMeta[] | null | undefined, nowIso: string): FieldRow[] {
  const metas = versions ?? [];
  const frozen = isFrozen(latest);
  const latestChanged = latest.version > 1 ? fieldChanges(latest.changed) : [];
  return BRIEF_FIELDS.map((f) => {
    const raw = latest.fields?.[f.wire];
    const isList = f.key === 'where';
    const chips = isList ? ((raw as string[] | undefined) ?? []).filter((x) => x.trim()) : [];
    const text = isList ? '' : ((raw as string | undefined) ?? '');
    const lines = isList ? [] : textLines(text);
    const prov = lastChangeOf(metas, f.key) ?? metas.find((m) => m.version === 1) ?? null;
    const changed = latestChanged.includes(f.key);
    const undoTo = undoTarget(metas, f.key);
    return {
      key: f.key,
      label: f.label,
      lines,
      chips,
      isList,
      text,
      empty: isList ? chips.length === 0 : lines.length === 0,
      who: prov?.author ?? latest.author,
      version: prov?.version ?? latest.version,
      ago: prov ? relativeAgo(prov.createdAt, nowIso) : '',
      changed,
      canUndo: changed && undoTo != null && !frozen,
      undoTo,
    };
  });
}

/** Phone badges: fields / outline items changed in the latest version. */
export function changedCounts(latest: Pick<BriefVersion, 'changed' | 'version'> | null | undefined): { brief: number; outline: number } {
  if (!latest || latest.version <= 1) return { brief: 0, outline: 0 };
  const all = latest.changed ?? [];
  return {
    brief: all.filter((c) => !c.startsWith('outline:')).length,
    outline: all.filter((c) => c.startsWith('outline:')).length,
  };
}

export interface CheckRow {
  rule: string;
  icon: '✓' | '!' | '✗';
  tone: 'ok' | 'warn' | 'fail';
  text: string;
}

export function checkRows(items: BriefCheckItem[] | null | undefined): CheckRow[] {
  return (items ?? []).map((i) => {
    const tone = i.status === 'fail' ? 'fail' : i.status === 'warn' ? 'warn' : 'ok';
    return { rule: i.rule, icon: tone === 'fail' ? '✗' : tone === 'warn' ? '!' : '✓', tone, text: i.detail };
  });
}

export interface StartGate {
  /** False ONLY when the brief is frozen — failures never disable Start (D11). */
  canStart: boolean;
  /** Any failed rule → Start first asks "Brief check has failures" with Start anyway / Keep editing. */
  needsConfirm: boolean;
  /** One human line per failed rule. */
  confirmLines: string[];
  /** A file conflict reveals Queue planning. */
  showQueue: boolean;
  /** The conflicting initiatives Queue planning waits on. */
  queueAfter: string[];
}

export function startGate(items: BriefCheckItem[] | null | undefined, initiativeStatus: string | null | undefined, frozen = false): StartGate {
  const list = items ?? [];
  const fails = list.filter((i) => i.status === 'fail');
  const conflict = list.filter((i) => i.rule === FILE_CONFLICT_RULE && i.status === 'warn');
  const queueAfter = Array.from(new Set(conflict.flatMap((i) => i.initiativeIds ?? [])));
  const live = !frozen && (initiativeStatus ?? '') === 'briefing';
  return {
    canStart: live,
    needsConfirm: live && fails.length > 0,
    confirmLines: fails.map((i) => i.detail),
    showQueue: live && queueAfter.length > 0,
    queueAfter,
  };
}

/** Sticky-bar summary: `Brief v5 · 1 warning` / `· 2 failed checks` / `· all checks pass`. */
export function summaryLine(version: number | null | undefined, items: BriefCheckItem[] | null | undefined): string {
  const list = items ?? [];
  const warn = list.filter((i) => i.status === 'warn').length;
  const fail = list.filter((i) => i.status === 'fail').length;
  const parts = [`Brief v${version ?? 0}`];
  if (fail) parts.push(`${fail} failed ${fail === 1 ? 'check' : 'checks'}`);
  if (warn) parts.push(`${warn} ${warn === 1 ? 'warning' : 'warnings'}`);
  if (!fail && !warn && list.length) parts.push('all checks pass');
  return parts.join(' · ');
}

export interface OutlineTaskRow {
  id: string;
  /** The task number within the phase (`01`). */
  num: string;
  title: string;
  agentTag: string | null;
  added: boolean;
  first: boolean;
  last: boolean;
}

export interface OutlinePhaseCard {
  id: string;
  title: string;
  agentTag: string | null;
  added: boolean;
  first: boolean;
  last: boolean;
  tasks: OutlineTaskRow[];
}

/**
 * The draft outline as cards. `agentTag` (`agent v4`) marks items the agent added; `added`
 * highlights items changed in the latest version (`outline:<id>` in its `changed`).
 */
export function outlineCards(outline: BriefOutline | null | undefined, latestChanged: string[] | null | undefined): OutlinePhaseCard[] {
  const hot = new Set((latestChanged ?? []).filter((c) => c.startsWith('outline:')).map((c) => c.slice('outline:'.length)));
  const tag = (author: string, addedIn: number) => (author === 'agent' ? `agent v${addedIn}` : null);
  const phases = outline?.phases ?? [];
  return phases.map((p, pi) => ({
    id: p.id,
    title: p.title,
    agentTag: tag(p.author, p.addedIn),
    added: hot.has(p.id),
    first: pi === 0,
    last: pi === phases.length - 1,
    tasks: (p.tasks ?? []).map((t, ti) => ({
      id: t.id,
      num: t.id.split('/').pop() ?? t.id,
      title: t.title,
      agentTag: tag(t.author, t.addedIn),
      added: hot.has(t.id),
      first: ti === 0,
      last: ti === p.tasks.length - 1,
    })),
  }));
}

export interface DiffSection {
  title: string;
  del: string[];
  ins: string[];
}

export interface DiffView {
  sections: DiffSection[];
  /** `4 fields unchanged` (empty when every field changed). */
  unchangedNote: string;
  empty: boolean;
}

const LABEL_OF: Record<string, string> = Object.fromEntries(BRIEF_FIELDS.map((f) => [f.key, f.label]));

export function diffView(diff: BriefDiff | null | undefined): DiffView {
  const fields = diff?.fields ?? [];
  const sections: DiffSection[] = fields
    .filter((f) => f.changed)
    .map((f) => ({
      title: LABEL_OF[f.field] ?? f.field,
      del: f.removed.length ? f.removed : f.added.length ? ['(empty)'] : [],
      ins: f.added,
    }));
  const o = diff?.outline;
  if (o) {
    const ins = [
      ...o.added.map((a) => `${a.id} ${a.title}`),
      ...o.renamed.map((r) => `${r.id}: ${r.after}`),
      ...o.moved.map((m) => `${m} moved`),
    ];
    const del = [...o.removed.map((r) => `${r.id} ${r.title}`), ...o.renamed.map((r) => `${r.id}: ${r.before}`)];
    if (ins.length || del.length) sections.push({ title: 'Outline', del, ins });
  }
  const unchanged = fields.filter((f) => !f.changed).length;
  return {
    sections,
    unchangedNote: unchanged ? `${unchanged} ${unchanged === 1 ? 'field' : 'fields'} unchanged` : '',
    empty: sections.length === 0,
  };
}

export interface VersionChip {
  version: number;
  label: string;
  author: string;
  latest: boolean;
}

/** The version strip: `v1 you` … `v5 agent`; first and latest carry the author, the rest only `vN`. */
export function versionChips(versions: BriefVersionMeta[] | null | undefined): VersionChip[] {
  const list = [...(versions ?? [])].sort((a, b) => a.version - b.version);
  return list.map((v, i) => {
    const edge = i === 0 || i === list.length - 1;
    return {
      version: v.version,
      label: edge ? `v${v.version} ${v.author}` : `v${v.version}`,
      author: v.author,
      latest: i === list.length - 1,
    };
  });
}

/** The previous version to diff a chip against (null for v1). */
export function diffBase(version: number): number | null {
  return version > 1 ? version - 1 : null;
}

export type DetailState = 'loading' | 'error' | 'not-found' | 'closed' | 'frozen' | 'ready';

/**
 * S11: what the detail page renders. A not-found error or no row → not-found; a CLOSED row (the
 * engine soft-deletes by closing, so the row and its title still exist) → `closed`, a read-only
 * "This plan is closed" (live UI bug 5: it said "Plan not found … the link is wrong" under the real
 * title); a row that left briefing → the frozen (S9) view.
 */
export function detailState(
  loading: boolean,
  error: string | null | undefined,
  plan: Pick<AgentPlan, 'status' | 'initiativeStatus'> | null | undefined,
): DetailState {
  if (error) return /not found|no such|unknown plan/i.test(error) ? 'not-found' : 'error';
  if (loading && !plan) return 'loading';
  if (!plan) return loading ? 'loading' : 'not-found';
  if (plan.status === 'closed') return 'closed';
  if (plan.initiativeStatus !== 'briefing') return 'frozen';
  return 'ready';
}

/** Parse a `where` chip input: one item per non-empty line or comma-free entry. */
export function whereItems(raw: string | null | undefined): string[] {
  return (raw ?? '')
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);
}

/** The closed-plan card's copy (bug 5). Human, says what happened and what is still there (L7). */
export const CLOSED_PLAN_COPY = {
  title: 'This plan is closed',
  body: "It was closed or deleted, so it can't be edited, and its chat is archived.",
} as const;

/**
 * The initiative a CLOSED plan can still link to (U-1-7), else `null`. A planning run IS its own
 * initiative (`initiativeId === id`), so a link there lands back on this closed plan; and a closed
 * initiative is a dead end too. `initiative` is that initiative's own row, when it was fetched.
 */
export function closedPlanInitiativeLink(
  plan: Pick<AgentPlan, 'id' | 'initiativeId'> | null | undefined,
  initiative: Pick<AgentPlan, 'id' | 'status'> | null | undefined,
): string | null {
  const target = (plan?.initiativeId ?? '').trim();
  if (!plan || !target || target === plan.id) return null;
  if (!initiative || initiative.id !== target || initiative.status === 'closed') return null;
  return target;
}

/**
 * A LEGACY plan (approved before the structured brief, #76): it has no brief versions, only the
 * markdown `brief` on the plan row. Returns that markdown to render read-only, else `null` (live UI
 * bug 6: such plans showed six empty fields and "4 failed checks", the check grading an empty brief).
 * A plan still in briefing is never legacy: a missing version there is a load problem, not history.
 */
export function legacyBriefMarkdown(
  plan: Pick<AgentPlan, 'brief' | 'initiativeStatus'> | null | undefined,
  latest: BriefVersion | null | undefined,
  versions: BriefVersionMeta[] | null | undefined,
): string | null {
  if (!plan || latest || (versions ?? []).length > 0) return null;
  if ((plan.initiativeStatus ?? '') === 'briefing') return null;
  const md = (plan.brief ?? '').trim();
  return md || null;
}

/** The note above a legacy brief. */
export const LEGACY_BRIEF_NOTE =
  'This plan was approved before briefs had separate fields, so its brief is shown as it was written. It is read-only.';

/**
 * The frozen banner's text (U-1-8). A plan frozen at a real brief version says so; a legacy plan
 * (no versions, `frozenAt` 0) said "Frozen at v0", so it just says planning started.
 */
export function frozenBanner(frozenAt: number | null | undefined): { lead: string; tail: string } {
  const v = frozenAt ?? 0;
  return v > 0 ? { lead: `Frozen at v${v}`, tail: ' — planning started.' } : { lead: 'Planning started.', tail: '' };
}
