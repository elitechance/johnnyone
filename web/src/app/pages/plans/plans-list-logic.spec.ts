import { describe, it, expect } from 'vitest';
import {
  checkBadge,
  draftCountLabel,
  listState,
  newPlanValid,
  planCards,
  relativeAgo,
  toIsoUtc,
  workspaceLabel,
} from './plans-list-logic';

const NOW = '2026-11-23T12:00:00Z';

const row = (id: string, created: string | null, extra: Record<string, unknown> = {}) => ({
  id,
  title: `Plan ${id}`,
  workspacePath: '/home/u/Workspace/personal/apps/johnnyone',
  updatedAt: '2026-11-20 10:00:00',
  latest: created ? { version: 5, author: 'agent', createdAt: created } : null,
  checkSummary: { ok: 4, warn: 1, fail: 0 },
  queuedAfter: [],
  ...extra,
});

describe('plans-list-logic', () => {
  it('normalises SQLite UTC timestamps before measuring age', () => {
    expect(toIsoUtc('2026-11-23 11:57:00')).toBe('2026-11-23T11:57:00Z');
    expect(toIsoUtc('2026-11-23T11:57:00Z')).toBe('2026-11-23T11:57:00Z');
    expect(relativeAgo('2026-11-23 11:57:00', NOW)).toBe('3m ago');
    expect(relativeAgo('2026-11-23 11:59:40', NOW)).toBe('just now');
    expect(relativeAgo('2026-11-21 11:00:00', NOW)).toBe('2d ago');
    expect(relativeAgo('', NOW)).toBe('');
  });

  it('formats the version line as "v5 · agent · 3m ago"', () => {
    const [c] = planCards([row('a', '2026-11-23 11:57:00')], NOW);
    expect(c.versionLine).toBe('v5 · agent · 3m ago');
    expect(c.authorClass).toBe('agent');
    const [y] = planCards([row('b', '2026-11-23 11:59:59', { latest: { version: 1, author: 'you', createdAt: '2026-11-23 11:59:59' } })], NOW);
    expect(y.versionLine).toBe('v1 · you · just now');
    expect(y.authorClass).toBe('you');
    const [n] = planCards([row('c', null)], NOW);
    expect(n.versionLine).toBe('no versions yet · 3d ago');
  });

  it('sorts by latest version time, newest first (never-versioned rows by updatedAt)', () => {
    const cards = planCards(
      [row('old', '2026-11-21 11:00:00'), row('new', '2026-11-23 11:57:00'), row('none', null), row('mid', '2026-11-22 11:00:00')],
      NOW,
    );
    expect(cards.map((c) => c.id)).toEqual(['new', 'mid', 'old', 'none']);
  });

  it('labels the workspace with its last two segments', () => {
    expect(workspaceLabel('/home/u/Workspace/personal/apps/johnnyone')).toBe('apps/johnnyone');
    expect(workspaceLabel('/srv')).toBe('srv');
    expect(workspaceLabel('')).toBe('');
  });

  it('colours the check badge by the worst status: fail > warn > ok', () => {
    expect(checkBadge({ ok: 4, warn: 1, fail: 0 })).toEqual({ ok: 4, warn: 1, fail: 0, label: '✓ 4 · ! 1', tone: 'warn' });
    expect(checkBadge({ ok: 2, warn: 1, fail: 2 }).tone).toBe('fail');
    expect(checkBadge({ ok: 2, warn: 1, fail: 2 }).label).toBe('✓ 2 · ! 1 · ✗ 2');
    expect(checkBadge({ ok: 5, warn: 0, fail: 0 }).tone).toBe('ok');
    expect(checkBadge(null)).toEqual({ ok: 0, warn: 0, fail: 0, label: '', tone: 'none' });
  });

  it('marks queued plans and falls back to a title', () => {
    const [c] = planCards([row('q', '2026-11-23 11:00:00', { queuedAfter: ['x'], title: '  ' })], NOW);
    expect(c.queued).toBe(true);
    expect(c.title).toBe('Untitled plan');
  });

  it('decides the list state: error wins, loading only before rows, then empty/ready', () => {
    expect(listState(true, null, [])).toBe('loading');
    expect(listState(true, null, [1])).toBe('ready');
    expect(listState(false, 'No online backend app found', [1])).toBe('error');
    expect(listState(false, null, [])).toBe('empty');
    expect(listState(false, null, null)).toBe('empty');
    expect(listState(false, null, [1, 2])).toBe('ready');
  });

  it('enables Create only with a title and a workspace', () => {
    expect(newPlanValid('Plans tab', '/w')).toBe(true);
    expect(newPlanValid('   ', '/w')).toBe(false);
    expect(newPlanValid('Plans tab', '')).toBe(false);
    expect(newPlanValid(null, null)).toBe(false);
  });

  it('pluralises the draft count', () => {
    expect(draftCountLabel(1)).toBe('1 draft');
    expect(draftCountLabel(3)).toBe('3 drafts');
  });
});
