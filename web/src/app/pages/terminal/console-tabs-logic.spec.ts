import { describe, it, expect } from 'vitest';
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { DEFAULT_PANE_TAB, type PaneTab } from './terminal-transcript-tab';
import {
  resolvePrimarySessionId,
  initiativeTabOf,
  rawAttachNeeded,
} from './console-tabs-logic';

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
