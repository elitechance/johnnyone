import { describe, it, expect } from 'vitest';
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { DEFAULT_PANE_TAB, type PaneTab } from './terminal-transcript-tab';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  resolvePrimarySessionId,
  initiativeTabOf,
  rawAttachNeeded,
  resolvedPaneTab,
  visibleConsoleTabs,
  consoleTabsWithArena,
} from './console-tabs-logic';

const here = dirname(fileURLToPath(import.meta.url));

// Pure spec (no Angular/Ionic/DOM) — runs under the plugin-less web/vitest.config.ts. Pins the
// per-initiative tab shell's primary-session resolution, tab default/lookup, and Raw-attach predicate
// (Overhaul P9 / phase P1, T01).

/** Minimal AgentPlan carrying only the session-id fields resolvePrimarySessionId reads. */
function plan(over: Partial<AgentPlan>): AgentPlan {
  return { id: 'p1', ...over } as AgentPlan;
}

describe('resolvePrimarySessionId', () => {
  it('prefers worker over reviewer over briefing', () => {
    expect(
      resolvePrimarySessionId(
        plan({ workerSessionId: 'w', reviewerSessionId: 'r', briefingSessionId: 'b' }),
      ),
    ).toBe('w');
  });

  it('falls to reviewer when no worker session', () => {
    expect(
      resolvePrimarySessionId(plan({ reviewerSessionId: 'r', briefingSessionId: 'b' })),
    ).toBe('r');
  });

  it('falls to briefing when no worker/reviewer session', () => {
    expect(resolvePrimarySessionId(plan({ briefingSessionId: 'b' }))).toBe('b');
  });

  it('returns null when the initiative has no session', () => {
    expect(resolvePrimarySessionId(plan({}))).toBe(null);
  });

  it('returns null for a null/undefined initiative', () => {
    expect(resolvePrimarySessionId(null)).toBe(null);
    expect(resolvePrimarySessionId(undefined)).toBe(null);
  });
});

describe('initiativeTabOf', () => {
  it('returns the stored tab for a known initiative', () => {
    const tabs: Record<string, PaneTab> = { i1: 'diff', i2: 'raw' };
    expect(initiativeTabOf(tabs, 'i1')).toBe('diff');
    expect(initiativeTabOf(tabs, 'i2')).toBe('raw');
  });

  it("defaults to 'raw' for an unknown id or empty map (Transcript removed)", () => {
    expect(initiativeTabOf({}, 'nope')).toBe('raw');
    expect(initiativeTabOf({ i1: 'plan' }, 'other')).toBe(DEFAULT_PANE_TAB);
    expect(DEFAULT_PANE_TAB).toBe('raw');
  });

  it('resolvedPaneTab falls back when the stored tab is hidden', () => {
    expect(resolvedPaneTab('tasks', ['raw', 'plan', 'diff'])).toBe(DEFAULT_PANE_TAB);
    expect(resolvedPaneTab('tasks', ['raw', 'plan', 'tasks', 'diff'])).toBe('tasks');
  });
});

describe('rawAttachNeeded', () => {
  it('is true when there is no primary session', () => {
    expect(rawAttachNeeded(null, false)).toBe(true);
    expect(rawAttachNeeded(null, true)).toBe(true);
    expect(rawAttachNeeded(undefined, true)).toBe(true);
    expect(rawAttachNeeded('', true)).toBe(true);
  });

  it('is true when a primary session exists but no screen is attached', () => {
    expect(rawAttachNeeded('s1', false)).toBe(true);
  });

  it('is false when both a primary session and an attached screen exist', () => {
    expect(rawAttachNeeded('s1', true)).toBe(false);
  });
});

describe('visibleConsoleTabs', () => {
  it('commercial planning is raw|plan|diff', () => {
    expect(
      visibleConsoleTabs(plan({ initiativeStatus: 'planning' }), false, false, false),
    ).toEqual(['raw', 'plan', 'diff']);
  });
  it('local-small planning adds checks', () => {
    expect(
      visibleConsoleTabs(
        plan({ initiativeStatus: 'planning', executorConfig: '{"mode":"local-small"}' }),
        false,
        false,
        false,
      ),
    ).toEqual(['raw', 'plan', 'checks', 'diff']);
  });
  it('local-small development + preflight no task-run is checks only extra', () => {
    expect(
      visibleConsoleTabs(
        plan({ initiativeStatus: 'development', executorConfig: '{"mode":"local-small"}' }),
        false,
        false,
        true,
      ),
    ).toEqual(['raw', 'plan', 'checks', 'diff']);
  });
  it('local-small development with task-run adds tasks and optional checks', () => {
    expect(
      visibleConsoleTabs(
        plan({ initiativeStatus: 'development', executorConfig: '{"mode":"local-small"}' }),
        true,
        false,
        true,
      ),
    ).toEqual(['raw', 'plan', 'checks', 'tasks', 'diff']);
  });
});

// REPLACED (with the coordinator's authorisation, round 3). This was named a wiring test and
// asserted that `terminal.page.{html,ts}` CONTAINED the string "visibleConsoleTabs" — which a
// comment satisfies, and which would stay green if someone replaced the delegation with a
// hand-rolled array that quietly dropped a tab. That is the exact drift the two-function design
// exists to prevent, so the wiring test now asserts the DELEGATION over the same fixtures the file
// already uses, plus the one thing a pure test cannot see: which function the page calls.
describe('wiring — the strip the console renders is the strip this file decides, plus arena', () => {
  const FIXTURES: { name: string; init: AgentPlan; taskRun: boolean; planCheck: boolean; preflight: boolean }[] = [
    { name: 'commercial planning', init: plan({ initiativeStatus: 'planning' }), taskRun: false, planCheck: false, preflight: false },
    { name: 'commercial development', init: plan({ initiativeStatus: 'development' }), taskRun: true, planCheck: true, preflight: true },
    { name: 'local-small planning', init: plan({ initiativeStatus: 'planning', executorConfig: '{"mode":"local-small"}' }), taskRun: false, planCheck: false, preflight: false },
    { name: 'local-small development + preflight', init: plan({ initiativeStatus: 'development', executorConfig: '{"mode":"local-small"}' }), taskRun: false, planCheck: false, preflight: true },
    { name: 'local-small development + task-run', init: plan({ initiativeStatus: 'development', executorConfig: '{"mode":"local-small"}' }), taskRun: true, planCheck: false, preflight: true },
  ];

  for (const f of FIXTURES) {
    it(`${f.name}: the rendered strip is exactly visibleConsoleTabs + arena`, () => {
      const base = visibleConsoleTabs(f.init, f.taskRun, f.planCheck, f.preflight);
      expect(consoleTabsWithArena(f.init, f.taskRun, f.planCheck, f.preflight)).toEqual([
        ...base,
        'arena',
      ]);
    });
  }

  it('appends arena exactly once, and never drops or reorders what was already there', () => {
    for (const f of FIXTURES) {
      const withArena = consoleTabsWithArena(f.init, f.taskRun, f.planCheck, f.preflight);
      expect(withArena.filter((t) => t === 'arena')).toHaveLength(1);
      expect(withArena[withArena.length - 1]).toBe('arena');
      expect(new Set(withArena).size).toBe(withArena.length);
    }
  });

  it('the console calls the delegating function, not a hand-rolled list', () => {
    const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
    expect(ts).toMatch(/visibleTabs\s*=\s*computed\([^;]*consoleTabsWithArena\(/);
    // A second, literal tab array in the page would be the drift this guards against — in either
    // quote style, because `["raw", "plan", …]` evaded the single-quote-only version.
    expect(ts).not.toMatch(/\[\s*(['"`])raw\1\s*,\s*(['"`])plan\2/);
  });
});
