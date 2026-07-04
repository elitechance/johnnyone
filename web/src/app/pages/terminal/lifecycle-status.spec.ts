import { describe, it, expect } from 'vitest';

// Deep path into the ui library — the pure module is barrel-exported from `@johnnyone/ui`, but the
// web vitest config has no tsconfig-paths plugin (mirrors diff-parse.spec.ts). The pure module
// carries no Angular import, so it loads cleanly in jsdom.
import {
  statusMeta,
  healthMeta,
  stageIndex,
  LIFECYCLE_STAGES,
} from '../../../../../ui/src/lib/lifecycle-status';

describe('statusMeta', () => {
  it('maps every lifecycle stage to its token/label/class', () => {
    expect(statusMeta('briefing')).toEqual({
      cssVar: '--jo-st-briefing',
      label: 'briefing',
      className: 'st-briefing',
    });
    expect(statusMeta('planning')).toEqual({
      cssVar: '--jo-st-planning',
      label: 'planning',
      className: 'st-planning',
    });
    expect(statusMeta('development')).toEqual({
      cssVar: '--jo-st-development',
      label: 'development',
      className: 'st-development',
    });
    expect(statusMeta('review')).toEqual({
      cssVar: '--jo-st-review',
      label: 'review',
      className: 'st-review',
    });
    expect(statusMeta('done')).toEqual({
      cssVar: '--jo-st-done',
      label: 'done',
      className: 'st-done',
    });
  });

  it('is case- and whitespace-tolerant', () => {
    expect(statusMeta('  Development  ')).toEqual(statusMeta('development'));
    expect(statusMeta('REVIEW')).toEqual(statusMeta('review'));
  });

  it('falls back benignly for unknown / empty / nullish input (never throws)', () => {
    expect(statusMeta('wat')).toEqual({
      cssVar: '--jo-fg-muted',
      label: 'wat',
      className: 'st-unknown',
    });
    expect(statusMeta('')).toEqual({ cssVar: '--jo-fg-muted', label: '', className: 'st-unknown' });
    expect(statusMeta(null)).toEqual({ cssVar: '--jo-fg-muted', label: '', className: 'st-unknown' });
    expect(statusMeta(undefined)).toEqual({
      cssVar: '--jo-fg-muted',
      label: '',
      className: 'st-unknown',
    });
  });
});

describe('healthMeta', () => {
  it('maps the three health values to token/label/class', () => {
    expect(healthMeta('in-progress')).toEqual({
      cssVar: '--jo-good',
      label: 'in-progress',
      className: 'health',
    });
    expect(healthMeta('needs-attention')).toEqual({
      cssVar: '--jo-warn',
      label: 'needs-attention',
      className: 'health att',
    });
    expect(healthMeta('blocked')).toEqual({
      cssVar: '--jo-bad',
      label: 'blocked',
      className: 'health blk',
    });
  });

  it('maps the terminal complete/done health to the done token', () => {
    expect(healthMeta('complete')).toEqual({
      cssVar: '--jo-st-done',
      label: 'complete',
      className: 'health done',
    });
    // Backend may report either 'complete' or 'done' on the health axis.
    expect(healthMeta('done')).toEqual(healthMeta('complete'));
  });

  it('is case-tolerant and falls back for unknown', () => {
    expect(healthMeta('Blocked')).toEqual(healthMeta('blocked'));
    expect(healthMeta('exploded')).toEqual({
      cssVar: '--jo-fg-muted',
      label: 'exploded',
      className: 'st-unknown',
    });
    expect(healthMeta(null)).toEqual({ cssVar: '--jo-fg-muted', label: '', className: 'st-unknown' });
  });
});

describe('stageIndex / LIFECYCLE_STAGES', () => {
  it('orders the five stages briefing→done', () => {
    expect([...LIFECYCLE_STAGES]).toEqual([
      'briefing',
      'planning',
      'development',
      'review',
      'done',
    ]);
  });

  it('returns the ordinal for known stages and -1 for unknown', () => {
    expect(stageIndex('briefing')).toBe(0);
    expect(stageIndex('development')).toBe(2);
    expect(stageIndex('done')).toBe(4);
    expect(stageIndex('  Review ')).toBe(3);
    expect(stageIndex('nope')).toBe(-1);
    expect(stageIndex('')).toBe(-1);
    expect(stageIndex(null)).toBe(-1);
  });
});
