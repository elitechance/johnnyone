/**
 * Pure decisions of the `/plans` list (Plans tab, initiative 8ad3ae71 phase 03 task 04).
 *
 * DOM-free and Angular-free (the plugin-less web vitest pins it). The page only binds these view
 * models to Ionic templates. Relative times reuse the app's ONE wording (`formatRelTime`, also used
 * by the console's initiative rows) so "3m ago" reads the same everywhere.
 */
import type { DraftPlan } from '../../../../../ui/src/services/johnny-api.service';
import { formatRelTime } from '../shells/shells-page-logic';

export type Tone = 'ok' | 'warn' | 'fail' | 'none';

export interface CheckBadge {
  ok: number;
  warn: number;
  fail: number;
  /** e.g. `✓ 4 · ! 1` — empty parts are left out. */
  label: string;
  /** The worst status present: fail > warn > ok. */
  tone: Tone;
}

export interface PlanCard {
  id: string;
  title: string;
  workspacePath: string;
  /** Last two path segments, e.g. `apps/johnnyone`. */
  workspaceLabel: string;
  /** `v5 · agent · 3m ago` (or `no versions yet`). */
  versionLine: string;
  /** `you` | `agent` | `` — the template colours the author with it. */
  authorClass: string;
  check: CheckBadge;
  /** Start planning is queued behind other initiatives. */
  queued: boolean;
  /** Sort key (ms since epoch). */
  sortMs: number;
}

export type ListState = 'loading' | 'error' | 'empty' | 'ready';

/**
 * SQLite `datetime('now')` values (`2026-11-23 20:41:00`) carry no zone and are UTC. `Date.parse`
 * would read that form as LOCAL time, so normalise it to ISO-8601 UTC first. ISO input passes
 * through unchanged.
 */
export function toIsoUtc(ts: string | null | undefined): string {
  const s = (ts ?? '').trim();
  if (/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d+)?$/.test(s)) return `${s.replace(' ', 'T')}Z`;
  return s;
}

/** `3m ago` / `just now` / `2d ago` for a server timestamp (either form). */
export function relativeAgo(ts: string | null | undefined, nowIso: string): string {
  return formatRelTime(toIsoUtc(ts), nowIso);
}

/** The last two path segments (`/home/u/Workspace/personal/apps/johnnyone` → `apps/johnnyone`). */
export function workspaceLabel(path: string | null | undefined): string {
  const segs = (path ?? '').split('/').filter(Boolean);
  return segs.slice(-2).join('/') || (path ?? '');
}

export function checkBadge(summary: { ok: number; warn: number; fail: number } | null | undefined): CheckBadge {
  const ok = summary?.ok ?? 0;
  const warn = summary?.warn ?? 0;
  const fail = summary?.fail ?? 0;
  const parts: string[] = [];
  if (ok) parts.push(`✓ ${ok}`);
  if (warn) parts.push(`! ${warn}`);
  if (fail) parts.push(`✗ ${fail}`);
  const tone: Tone = fail ? 'fail' : warn ? 'warn' : ok ? 'ok' : 'none';
  return { ok, warn, fail, label: parts.join(' · '), tone };
}

function timeMs(ts: string | null | undefined): number {
  const n = Date.parse(toIsoUtc(ts));
  return Number.isNaN(n) ? 0 : n;
}

/** One card per draft plan, newest latest-version first (a never-versioned plan uses `updatedAt`). */
export function planCards(rows: DraftPlan[] | null | undefined, nowIso: string): PlanCard[] {
  return (rows ?? [])
    .map((r) => {
      const latest = r.latest ?? null;
      const when = latest?.createdAt ?? r.updatedAt;
      const versionLine = latest
        ? `v${latest.version} · ${latest.author} · ${relativeAgo(latest.createdAt, nowIso)}`
        : `no versions yet · ${relativeAgo(r.updatedAt, nowIso)}`;
      return {
        id: r.id,
        title: r.title?.trim() || 'Untitled plan',
        workspacePath: r.workspacePath,
        workspaceLabel: workspaceLabel(r.workspacePath),
        versionLine,
        authorClass: latest?.author === 'agent' ? 'agent' : latest ? 'you' : '',
        check: checkBadge(r.checkSummary),
        queued: (r.queuedAfter ?? []).length > 0,
        sortMs: timeMs(when),
      };
    })
    .sort((a, b) => b.sortMs - a.sortMs || a.title.localeCompare(b.title));
}

/** Create is enabled only with a non-blank title and a chosen workspace. */
export function newPlanValid(title: string | null | undefined, workspace: string | null | undefined): boolean {
  return (title ?? '').trim().length > 0 && (workspace ?? '').trim().length > 0;
}

/** Which body the list renders. An error wins over stale rows; loading only before the first rows. */
export function listState(loading: boolean, error: string | null | undefined, rows: unknown[] | null | undefined): ListState {
  if (error) return 'error';
  if (loading && !(rows ?? []).length) return 'loading';
  return (rows ?? []).length ? 'ready' : 'empty';
}

/** The header pill: `3 drafts` / `1 draft`. */
export function draftCountLabel(n: number): string {
  return `${n} ${n === 1 ? 'draft' : 'drafts'}`;
}
