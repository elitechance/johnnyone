import { describe, it, expect } from 'vitest';
import type { AgentPlan } from '../../../../../ui/src/services/johnny-api.service';
import { resolveSelectedInitiative, selectionState } from './console-selection-logic';

/** Minimal AgentPlan factory — only the fields the seam reads (`id`, `initiativeId`). */
function plan(id: string, initiativeId: string): AgentPlan {
  return { id, initiativeId } as AgentPlan;
}

const PLANS = [
  plan('dev-1', 'init-1'), // development run of initiative init-1
  plan('dev-2', 'init-2'),
];

describe('resolveSelectedInitiative', () => {
  it('matches by plan-run id', () => {
    expect(resolveSelectedInitiative(PLANS, 'dev-2')?.id).toBe('dev-2');
  });
  it('matches by group initiativeId when the id does not match (deep-link path)', () => {
    expect(resolveSelectedInitiative(PLANS, 'init-1')?.id).toBe('dev-1');
  });
  it('prefers an id match over an initiativeId match', () => {
    // A plan whose id collides with another plan's initiativeId must resolve by id first.
    const plans = [plan('x', 'shared'), plan('shared', 'other')];
    expect(resolveSelectedInitiative(plans, 'shared')?.id).toBe('shared');
  });
  it('returns null when neither id nor initiativeId matches, or for empty/null inputs', () => {
    expect(resolveSelectedInitiative(PLANS, 'nope')).toBeNull();
    expect(resolveSelectedInitiative(PLANS, '')).toBeNull();
    expect(resolveSelectedInitiative(PLANS, null)).toBeNull();
    expect(resolveSelectedInitiative(null, 'dev-1')).toBeNull();
  });
});

describe('selectionState', () => {
  it('recoverableEmpty is true when a selection is set but unresolved (the strand condition)', () => {
    const s = selectionState(PLANS, 'ghost');
    expect(s).toEqual({ hasSelection: true, resolved: null, recoverableEmpty: true });
  });
  it('recoverableEmpty is false when the selection resolves (by id or initiativeId)', () => {
    expect(selectionState(PLANS, 'dev-1').recoverableEmpty).toBe(false);
    expect(selectionState(PLANS, 'init-2').recoverableEmpty).toBe(false);
  });
  it('recoverableEmpty is false when there is no selection', () => {
    const s = selectionState(PLANS, null);
    expect(s).toEqual({ hasSelection: false, resolved: null, recoverableEmpty: false });
  });
});
