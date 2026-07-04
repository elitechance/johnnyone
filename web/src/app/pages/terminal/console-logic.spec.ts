import { describe, it, expect } from 'vitest';
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import {
  initiativeRows,
  lensSummary,
  lensSource,
  defaultLenses,
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

  it('collapses the planning + development runs of one initiative into a SINGLE row', () => {
    // The two runs share initiativeId = 'init-1' (the planning run's id IS the initiative id).
    const rows = initiativeRows(
      [
        plan({ id: 'dev-1', initiativeId: 'init-1', title: 'Feature', initiativeStatus: 'done', health: 'complete', updatedAt: '2026-07-04T10:20:00.000Z' }),
        plan({ id: 'init-1', initiativeId: 'init-1', title: 'Feature', initiativeStatus: 'planning', health: 'in-progress', updatedAt: '2026-07-04T10:00:00.000Z' }),
      ],
      null,
      NOW,
    );
    expect(rows).toHaveLength(1);
    // Represented by the run furthest along the lifecycle (the development run: done).
    expect(rows[0]).toMatchObject({ id: 'dev-1', status: 'done', health: 'complete' });
  });

  it('highlights the merged row when EITHER run id (or the initiativeId) is selected', () => {
    const runs = [
      plan({ id: 'dev-1', initiativeId: 'init-1', initiativeStatus: 'development' }),
      plan({ id: 'init-1', initiativeId: 'init-1', initiativeStatus: 'planning', updatedAt: '2026-07-04T09:00:00.000Z' }),
    ];
    expect(initiativeRows(runs, 'dev-1', NOW)[0].selected).toBe(true); // development run id
    expect(initiativeRows(runs, 'init-1', NOW)[0].selected).toBe(true); // planning run id == initiativeId
    expect(initiativeRows(runs, 'other', NOW)[0].selected).toBe(false);
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
