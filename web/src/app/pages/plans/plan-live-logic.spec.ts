import { describe, it, expect } from 'vitest';
import { hasNewerVersion, isDeletedUpdate, pollEnabled, shouldRefetch, turnCompleted } from './plan-live-logic';

const shown = { brief: '# Outcome\nA', initiativeStatus: 'briefing' };
const upd = (planId: string, brief: string, extra: Record<string, unknown> = {}) => ({
  planId,
  deleted: false,
  run: { plan: { brief, initiativeStatus: 'briefing', status: 'draft', ...extra } },
});

describe('plan-live-logic', () => {
  it('refetches only this plan, on a brief change, a status change or a delete', () => {
    expect(shouldRefetch(upd('P', '# Outcome\nB'), 'P', shown)).toBe(true);
    expect(shouldRefetch(upd('P', '# Outcome\nA'), 'P', shown)).toBe(false);
    expect(shouldRefetch(upd('Q', '# Outcome\nB'), 'P', shown)).toBe(false);
    expect(shouldRefetch(upd('P', '# Outcome\nA', { initiativeStatus: 'planning' }), 'P', shown)).toBe(true);
    expect(shouldRefetch(upd('P', '# Outcome\nA', { status: 'closed' }), 'P', shown)).toBe(true);
    expect(shouldRefetch({ planId: 'P', deleted: true }, 'P', shown)).toBe(true);
    expect(shouldRefetch({ planId: 'P', deleted: false }, 'P', shown)).toBe(false);
    expect(shouldRefetch(null, 'P', shown)).toBe(false);
  });

  it('recognises a delete of this plan', () => {
    expect(isDeletedUpdate({ planId: 'P', deleted: true }, 'P')).toBe(true);
    expect(isDeletedUpdate(upd('P', '', { status: 'closed' }), 'P')).toBe(true);
    expect(isDeletedUpdate({ planId: 'Q', deleted: true }, 'P')).toBe(false);
    expect(isDeletedUpdate(upd('P', ''), 'P')).toBe(false);
  });

  it('polls only while the relay is down and the page is visible', () => {
    expect(pollEnabled(true, true)).toBe(false);
    expect(pollEnabled(true, false)).toBe(false);
    expect(pollEnabled(false, true)).toBe(true);
    expect(pollEnabled(false, false)).toBe(false);
  });

  it('detects a finished turn and a newer version', () => {
    expect(turnCompleted(true, false)).toBe(true);
    expect(turnCompleted(false, false)).toBe(false);
    expect(turnCompleted(true, true)).toBe(false);
    expect(hasNewerVersion(6, 5)).toBe(true);
    expect(hasNewerVersion(5, 5)).toBe(false);
    expect(hasNewerVersion(null, 5)).toBe(false);
  });
});
