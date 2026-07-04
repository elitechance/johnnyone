import { describe, it, expect } from 'vitest';
import type { AgentPlanEvent } from '../../../../../ui/src/services/johnny-api.service';
import {
  initiativeTimeline,
  normalizeEventIso,
  lensOf,
  phaseLabelOf,
  stageOf,
} from './initiative-events-logic';

/** Minimal AgentPlanEvent factory — only the fields the timeline reads. */
function ev(over: Partial<AgentPlanEvent>): AgentPlanEvent {
  return {
    id: 'e1',
    planId: 'p1',
    eventType: 'agent_phase_started',
    actor: 'coordinator',
    category: 'phase',
    summary: 'Started phase work',
    payloadJson: '{}',
    createdAt: '2026-07-04 12:34:56',
    ...over,
  } as AgentPlanEvent;
}

describe('normalizeEventIso', () => {
  it('turns naive-UTC SQLite datetime into an unambiguous ISO-Z instant', () => {
    expect(normalizeEventIso('2026-07-04 12:34:56')).toBe('2026-07-04T12:34:56Z');
  });
  it('passes through an already-ISO instant with zone', () => {
    expect(normalizeEventIso('2026-07-04T12:34:56Z')).toBe('2026-07-04T12:34:56Z');
    expect(normalizeEventIso('2026-07-04T12:34:56+08:00')).toBe('2026-07-04T12:34:56+08:00');
  });
  it('is benign for empty/nullish', () => {
    expect(normalizeEventIso('')).toBe('');
    expect(normalizeEventIso(null)).toBe('');
    expect(normalizeEventIso(undefined)).toBe('');
  });
});

describe('lensOf', () => {
  it('extracts the lens name from payload JSON', () => {
    expect(lensOf('{"lens":"qa","verdict":"PASS"}')).toBe('qa');
  });
  it('returns null for no-lens / bad JSON / empty', () => {
    expect(lensOf('{"verdict":"PASS"}')).toBeNull();
    expect(lensOf('not json')).toBeNull();
    expect(lensOf('')).toBeNull();
    expect(lensOf(null)).toBeNull();
  });
});

describe('phaseLabelOf', () => {
  it('composes a 1-based phase label with title', () => {
    expect(phaseLabelOf(ev({ phaseIndex: 1, phaseTitle: 'Cashier atomicity' }))).toBe(
      'Phase 2 · Cashier atomicity',
    );
  });
  it('falls back to the number alone or null', () => {
    expect(phaseLabelOf(ev({ phaseIndex: 0, phaseTitle: '' }))).toBe('Phase 1');
    expect(phaseLabelOf(ev({ phaseIndex: undefined, phaseTitle: undefined }))).toBeNull();
  });
});

describe('stageOf', () => {
  it('maps lens/review events to review', () => {
    expect(stageOf('agent_lens_verdict', 'run')).toBe('review');
    expect(stageOf('agent_phase_review_started', 'phase')).toBe('review');
    expect(stageOf('agent_phase_gate_result', 'phase')).toBe('review');
  });
  it('maps planning / development / done', () => {
    expect(stageOf('planning_started', 'planning')).toBe('planning');
    expect(stageOf('agent_phase_started', 'phase')).toBe('development');
    expect(stageOf('agent_feedback_sent_to_worker', 'phase')).toBe('development');
    expect(stageOf('agent_plan_completed', 'run')).toBe('done');
  });

  it('maps genesis events to their opening stage (not "other")', () => {
    expect(stageOf('briefing_run_created', 'run')).toBe('planning');
    expect(stageOf('brief_accepted', 'run')).toBe('planning');
    expect(stageOf('agent_plan_created', 'run')).toBe('development');
  });
});

describe('initiativeTimeline', () => {
  it('projects rows, preserving order and flagging milestones', () => {
    const rows = initiativeTimeline([
      ev({ id: 'a', eventType: 'planning_started', category: 'planning', summary: 'Started planner pass' }),
      ev({
        id: 'b',
        eventType: 'agent_lens_verdict',
        category: 'run',
        summary: 'qa lens → NEEDS_CHANGES: missing rollback test',
        verdict: 'NEEDS_CHANGES',
        payloadJson: '{"lens":"qa","verdict":"NEEDS_CHANGES"}',
        phaseIndex: 1,
        phaseTitle: 'Cashier atomicity',
      }),
      ev({ id: 'c', eventType: 'agent_plan_completed', category: 'run', summary: 'Completed the full run' }),
    ]);
    expect(rows.map((r) => r.id)).toEqual(['a', 'b', 'c']);
    expect(rows[0]).toMatchObject({ stage: 'planning', milestone: true });
    expect(rows[1]).toMatchObject({
      stage: 'review',
      lens: 'qa',
      verdict: 'NEEDS_CHANGES',
      phaseLabel: 'Phase 2 · Cashier atomicity',
      milestone: false,
    });
    expect(rows[1].iso).toBe('2026-07-04T12:34:56Z');
    expect(rows[2]).toMatchObject({ stage: 'done', milestone: true });
  });

  it('flags a PASS gate as a milestone', () => {
    const [row] = initiativeTimeline([
      ev({ eventType: 'agent_phase_gate_result', verdict: 'PASS', summary: 'Phase review passed' }),
    ]);
    expect(row.milestone).toBe(true);
    expect(row.stage).toBe('review');
  });

  it('is total for null/empty', () => {
    expect(initiativeTimeline(null)).toEqual([]);
    expect(initiativeTimeline([])).toEqual([]);
  });
});
