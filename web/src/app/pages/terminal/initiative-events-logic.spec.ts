import { describe, it, expect } from 'vitest';
import type { AgentPlanEvent } from '../../../../../ui/src/services/johnny-api.service';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  initiativeTimeline,
  normalizeEventIso,
  lensOf,
  phaseLabelOf,
  stageOf,
  actorLabel,
  windowTimeline,
} from './initiative-events-logic';

const here = dirname(fileURLToPath(import.meta.url));

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

  it('maps human_comment to development (or planning per category), never "other"', () => {
    expect(stageOf('human_comment', 'phase')).toBe('development');
    expect(stageOf('human_comment', 'planning')).toBe('planning');
  });

  it('prefix-buckets check/replan events, never other', () => {
    expect(stageOf('planning_check_failed', 'run')).toBe('planning');
    expect(stageOf('planning_check_phase', 'run')).toBe('planning');
    expect(stageOf('agent_phase_replan_started', 'run')).toBe('development');
    expect(stageOf('agent_phase_preflight_failed', 'phase')).toBe('development');
  });

  it('maps agent_phase_task_* to development even when category is run', () => {
    expect(stageOf('agent_phase_task_done', 'phase')).toBe('development');
    expect(stageOf('agent_phase_task_failed', 'phase')).toBe('development');
    expect(stageOf('agent_phase_task_escalated', 'phase')).toBe('development');
    expect(stageOf('agent_phase_task_failed', 'run')).toBe('development');
  });
});

describe('initiativeTimeline — kloo task events (S1–S2)', () => {
  it('marks task_failed as a milestone and task_done as not', () => {
    const [failed] = initiativeTimeline([
      ev({ id: 'f', eventType: 'agent_phase_task_failed', category: 'phase', summary: 'Task 01-add failed (x)' }),
    ]);
    const [done] = initiativeTimeline([
      ev({ id: 'd', eventType: 'agent_phase_task_done', category: 'phase', summary: 'Task 01-add passed (qwen3-coder)' }),
    ]);
    expect(failed.milestone).toBe(true);
    expect(failed.stage).toBe('development');
    expect(done.milestone).toBe(false);
    expect(done.stage).toBe('development');
  });
});

describe('actorLabel', () => {
  it('renders a human actor as "you" and passes others through', () => {
    expect(actorLabel('human')).toBe('you');
    expect(actorLabel('coordinator')).toBe('coordinator');
    expect(actorLabel('t2')).toBe('t2');
    expect(actorLabel('')).toBe('');
    expect(actorLabel(null)).toBe('');
  });
});

describe('initiativeTimeline — human_comment', () => {
  it('projects a Resume-with-comment as a development human row (chip data + label)', () => {
    const [row] = initiativeTimeline([
      ev({
        id: 'hc',
        eventType: 'human_comment',
        category: 'phase',
        actor: 'human',
        summary: 'Resume · P02 + comment: fix it',
      }),
    ]);
    expect(row).toMatchObject({
      stage: 'development',
      actor: 'human',
      actorLabel: 'you',
      title: 'Resume · P02 + comment: fix it',
      milestone: false,
    });
  });

  it('honors a planning-stage human_comment', () => {
    const [row] = initiativeTimeline([
      ev({ eventType: 'human_comment', category: 'planning', actor: 'human', summary: 'Comment: looks good' }),
    ]);
    expect(row.stage).toBe('planning');
    expect(row.actorLabel).toBe('you');
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

  it('prefers summary over raw type for preflight_failed', () => {
    const [row] = initiativeTimeline([
      ev({
        eventType: 'agent_phase_preflight_failed',
        summary: 'Phase 04 preflight failed — 4 violations.',
      }),
    ]);
    expect(row.title).toBe('Phase 04 preflight failed — 4 violations.');
    expect(row.title).not.toBe('agent_phase_preflight_failed');
  });
});

describe('windowTimeline', () => {
  it('500 task-done + 1 failed stays ≤160 and keeps the failed id', () => {
    const events = [
      ...Array.from({ length: 500 }, (_, i) =>
        ev({ id: `d${i}`, eventType: 'agent_phase_task_done', summary: `done ${i}` }),
      ),
      ev({ id: 'fail-new', eventType: 'agent_phase_task_failed', summary: 'boom' }),
    ];
    const t0 = performance.now();
    const rows = windowTimeline(initiativeTimeline(events));
    expect(performance.now() - t0).toBeLessThan(100);
    expect(rows.length).toBeLessThanOrEqual(160);
    expect(rows.some((r) => r.id === 'fail-new')).toBe(true);
  });

  it('milestone-heavy 120 failures caps at 160 and keeps the latest failed', () => {
    const events = [
      ...Array.from({ length: 120 }, (_, i) =>
        ev({ id: `f${i}`, eventType: 'agent_phase_task_failed', summary: `fail ${i}` }),
      ),
      ev({ id: 'c1', eventType: 'planning_check_failed', category: 'planning' }),
      ev({ id: 'p1', eventType: 'agent_phase_preflight_failed' }),
      ...Array.from({ length: 378 }, (_, i) =>
        ev({ id: `o${i}`, eventType: 'agent_phase_task_done' }),
      ),
    ];
    const rows = windowTimeline(initiativeTimeline(events));
    expect(rows.length).toBeLessThanOrEqual(160);
    expect(rows.some((r) => r.id === 'f119')).toBe(true);
  });
});

describe('wiring — events rail', () => {
  it('page uses windowTimeline', () => {
    const html = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
    const ts = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
    expect(html + ts).toMatch(/windowTimeline/);
  });
});
