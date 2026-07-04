import { describe, it, expect } from 'vitest';
import type { AgentPlan, GitDiffView } from '../../../../../ui/src/services/johnny-api.service';
import {
  initiativeRows,
  lensSummary,
  lensSource,
  defaultLenses,
  touchedFiles,
  consolePaneFor,
  CONSOLE_SEGMENTS,
} from './console-logic';

// Pure spec (no Angular/Ionic) — runs under the plugin-less web/vitest.config.ts. Pins the console's
// row/summary/file/segment projections (Overhaul P8 / phase 04, A1).

const NOW = '2026-07-03T12:00:00.000Z';

/** Minimal AgentPlan for row projection — only the fields initiativeRows reads. */
function plan(over: Partial<AgentPlan>): AgentPlan {
  return {
    id: 'p1',
    title: 'A plan',
    initiativeStatus: 'development',
    health: 'in-progress',
    updatedAt: '2026-07-03T11:58:00.000Z',
    ...over,
  } as AgentPlan;
}

describe('initiativeRows', () => {
  it('maps status/health meta, ago, and selection', () => {
    const rows = initiativeRows(
      [
        plan({ id: 'a', title: 'Alpha', initiativeStatus: 'development', health: 'in-progress', updatedAt: '2026-07-03T11:58:00.000Z' }),
        plan({ id: 'b', title: 'Beta', initiativeStatus: 'review', health: 'needs-attention', updatedAt: '2026-07-03T11:00:00.000Z' }),
      ],
      'b',
      NOW,
    );

    expect(rows[0]).toMatchObject({
      id: 'a',
      title: 'Alpha',
      status: 'development',
      health: 'in-progress',
      showHealth: false, // baseline health is hidden
      ago: '2m ago',
      selected: false,
    });
    expect(rows[0].statusMeta).toEqual({
      cssVar: '--jo-st-development',
      label: 'development',
      className: 'st-development',
    });

    expect(rows[1]).toMatchObject({
      id: 'b',
      title: 'Beta',
      showHealth: true, // needs-attention is noteworthy
      ago: '1h ago',
      selected: true,
    });
    expect(rows[1].healthMeta.className).toBe('health att');
  });

  it('falls back for a blank title and tolerates empty/nullish input', () => {
    expect(initiativeRows([plan({ id: 'x', title: '' })], null, NOW)[0].title).toBe('(untitled)');
    expect(initiativeRows([], null, NOW)).toEqual([]);
    expect(initiativeRows(null, null, NOW)).toEqual([]);
  });
});

describe('lensSummary', () => {
  it('returns the P7 default triad (product/qa/lead, blocking) for empty/null config', () => {
    for (const cfg of [null, undefined, '']) {
      const lenses = lensSummary(cfg);
      expect(lenses.map((l) => l.name)).toEqual(['product', 'qa', 'lead']);
      expect(lenses.every((l) => l.provider === 'claude_code' && l.blocking)).toBe(true);
    }
  });

  it('projects a custom config JSON to chip rows', () => {
    const cfg = JSON.stringify([
      { name: 'peer-review', provider: 'claude_code', model: 'opus-4.8', blocking: true },
      { name: 'security', provider: 'codex', model: 'gpt-5', blocking: false },
    ]);
    expect(lensSummary(cfg)).toEqual([
      { name: 'peer-review', provider: 'claude_code', model: 'opus-4.8', blocking: true },
      { name: 'security', provider: 'codex', model: 'gpt-5', blocking: false },
    ]);
  });
});

describe('validation is strictly per-initiative (isolation)', () => {
  const cfgA = JSON.stringify([
    { name: 'peer-review', provider: 'claude_code', model: 'opus-4.8', blocking: true },
  ]);
  const cfgB = JSON.stringify([
    { name: 'security', provider: 'codex', model: 'gpt-5', blocking: false },
    { name: 'a11y', provider: 'claude_code', model: '', blocking: true },
  ]);

  it('two distinct configs yield two distinct lens sets — A never yields B', () => {
    const a = lensSummary(cfgA);
    const b = lensSummary(cfgB);
    expect(a).not.toEqual(b);
    expect(a.map((l) => l.name)).toEqual(['peer-review']);
    expect(b.map((l) => l.name)).toEqual(['security', 'a11y']);
  });

  it('a configured initiative does NOT equal the default triad; an unconfigured one DOES', () => {
    const triad = defaultLenses().map((l) => l.name);
    expect(lensSummary(cfgA).map((l) => l.name)).not.toEqual(triad);
    expect(lensSummary(cfgB).map((l) => l.name)).not.toEqual(triad);
    // null / '' / invalid → the shared default template (the source of the "looks global" perception).
    for (const cfg of [null, undefined, '', '[]', 'not json']) {
      expect(lensSummary(cfg).map((l) => l.name)).toEqual(triad);
    }
  });
});

describe('lensSource', () => {
  it("is 'custom' for a valid non-empty config", () => {
    expect(lensSource(JSON.stringify([{ name: 'x', provider: 'codex', blocking: true }]))).toBe('custom');
  });

  it("is 'default' for null / '' / '[]' / malformed JSON (matches fromConfigJson's fallback boundary)", () => {
    for (const cfg of [null, undefined, '', '[]', '   ', '{bad json', '{"not":"array"}', '42']) {
      expect(lensSource(cfg)).toBe('default');
    }
  });
});

describe('touchedFiles', () => {
  const view = (files: GitDiffView['files'], over: Partial<GitDiffView> = {}): GitDiffView =>
    ({ repoRoot: '/r', branch: 'main', clean: false, files, ...over }) as GitDiffView;

  it('senses new (adds only) vs edited (has deletions)', () => {
    const rows = touchedFiles(
      view([
        { path: 'a.ts', additions: 10, deletions: 0, diff: '' },
        { path: 'b.ts', additions: 4, deletions: 3, diff: '' },
        { path: 'c.ts', additions: 0, deletions: 5, diff: '' },
      ] as GitDiffView['files']),
    );
    expect(rows).toEqual([
      { path: 'a.ts', adds: 10, dels: 0, badge: 'new' },
      { path: 'b.ts', adds: 4, dels: 3, badge: 'edited' },
      { path: 'c.ts', adds: 0, dels: 5, badge: 'edited' },
    ]);
  });

  it('is empty for null / clean / no-files views', () => {
    expect(touchedFiles(null)).toEqual([]);
    expect(touchedFiles(undefined)).toEqual([]);
    expect(touchedFiles(view([], { clean: true }))).toEqual([]);
    expect(touchedFiles(view([] as GitDiffView['files']))).toEqual([]);
  });
});

describe('consolePaneFor / CONSOLE_SEGMENTS', () => {
  it('lists the three segments in order', () => {
    expect([...CONSOLE_SEGMENTS]).toEqual(['console', 'files', 'validation']);
  });

  it('maps each segment to itself and falls back to console', () => {
    expect(consolePaneFor('console')).toBe('console');
    expect(consolePaneFor('files')).toBe('files');
    expect(consolePaneFor('validation')).toBe('validation');
    expect(consolePaneFor('nope')).toBe('console');
    expect(consolePaneFor(null)).toBe('console');
    expect(consolePaneFor(undefined)).toBe('console');
  });
});
