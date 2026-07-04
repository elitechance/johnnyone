import { describe, it, expect } from 'vitest';
import type {
  AgentPlanRun,
  AgentPlanPhase,
  AgentPlanTask,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  planCounts,
  docNavModel,
  phaseCards,
  planDocPath,
  taskStatusLabel,
} from './plan-tab-logic';

// Pure spec (no Angular/Ionic/DOM) — runs under the plugin-less web/vitest.config.ts. Pins the Plan
// tab's counts, docnav model, phase cards, safe path builder, and status-chip mapping
// (Overhaul P9 / phase P2, T01).

function phase(over: Partial<AgentPlanPhase>): AgentPlanPhase {
  return { phaseId: 'P', phaseTitle: 'Phase', phaseIndex: 0, status: 'planned', ...over } as AgentPlanPhase;
}
function task(over: Partial<AgentPlanTask>): AgentPlanTask {
  return { phaseId: 'P', taskId: 'T', taskTitle: 'Task', taskIndex: 0, status: 'planned', ...over } as AgentPlanTask;
}
function run(over: Partial<AgentPlanRun>): AgentPlanRun {
  return { plan: {} as any, phases: [], tasks: [], events: [], ...over } as AgentPlanRun;
}

const SAMPLE = run({
  phases: [
    phase({ phaseId: 'P1-schema', phaseTitle: 'Schema & storage', phaseIndex: 0, status: 'done' }),
    phase({ phaseId: 'P2-runner', phaseTitle: 'Parallel runner', phaseIndex: 1, status: 'in-progress' }),
  ],
  tasks: [
    task({ phaseId: 'P1-schema', taskId: 'T01', taskTitle: 'Add column', status: 'done' }),
    task({ phaseId: 'P1-schema', taskId: 'T02', taskTitle: 'Migration', status: 'completed' }),
    task({ phaseId: 'P2-runner', taskId: 'T01', taskTitle: 'Fan-out', status: 'in-progress' }),
    task({ phaseId: 'P2-runner', taskId: 'T02', taskTitle: 'Gate logic', status: 'planned' }),
    task({ phaseId: 'P2-runner', taskId: 'T03', taskTitle: 'Wiring', status: 'planned' }),
  ],
});

describe('planCounts', () => {
  it('counts phases, tasks, and done (incl. host aliases like completed)', () => {
    expect(planCounts(SAMPLE)).toEqual({ phases: 2, tasks: 5, done: 2 });
  });
  it('is zeroed for a null/empty run', () => {
    expect(planCounts(null)).toEqual({ phases: 0, tasks: 0, done: 0 });
    expect(planCounts(run({}))).toEqual({ phases: 0, tasks: 0, done: 0 });
  });
});

describe('docNavModel', () => {
  it('lists overview.md, status.md, then phases in host order, each with its task rows', () => {
    const nav = docNavModel(SAMPLE);
    expect(nav.map((e) => e.id)).toEqual(['overview.md', 'status.md', 'P1-schema', 'P2-runner']);
    expect(nav[0]).toEqual({ kind: 'file', id: 'overview.md', label: 'overview.md' });
    expect(nav[1]).toEqual({ kind: 'file', id: 'status.md', label: 'status.md' });
    const p2 = nav[3];
    expect(p2.kind).toBe('phase');
    if (p2.kind === 'phase') {
      expect(p2.label).toBe('Parallel runner');
      expect(p2.status).toBe('in-progress');
      expect(p2.tasks).toEqual([
        { id: 'T01', title: 'Fan-out', status: 'in-progress' },
        { id: 'T02', title: 'Gate logic', status: 'planned' },
        { id: 'T03', title: 'Wiring', status: 'planned' },
      ]);
    }
  });
  it('returns just the two fixed files for a null/empty run', () => {
    expect(docNavModel(null).map((e) => e.id)).toEqual(['overview.md', 'status.md']);
  });
});

describe('phaseCards', () => {
  it('maps each phase to done/total + task rows', () => {
    const cards = phaseCards(SAMPLE);
    expect(cards).toEqual([
      {
        phaseId: 'P1-schema', phaseTitle: 'Schema & storage', done: 2, total: 2,
        tasks: [
          { id: 'T01', title: 'Add column', status: 'done' },
          { id: 'T02', title: 'Migration', status: 'completed' },
        ],
      },
      {
        phaseId: 'P2-runner', phaseTitle: 'Parallel runner', done: 0, total: 3,
        tasks: [
          { id: 'T01', title: 'Fan-out', status: 'in-progress' },
          { id: 'T02', title: 'Gate logic', status: 'planned' },
          { id: 'T03', title: 'Wiring', status: 'planned' },
        ],
      },
    ]);
  });
  it('is empty for a null run', () => {
    expect(phaseCards(null)).toEqual([]);
  });
});

describe('planDocPath', () => {
  const BASE = '/store/.johnnyone/initiatives/abc/plan';

  it('builds fixed-suffix paths for the two files and a phase id', () => {
    expect(planDocPath(BASE, 'overview.md')).toBe(`${BASE}/overview.md`);
    expect(planDocPath(BASE, 'status.md')).toBe(`${BASE}/status.md`);
    expect(planDocPath(BASE, 'P2-runner')).toBe(`${BASE}/phases/P2-runner/overview.md`);
  });
  it('normalizes a trailing slash on planPath (no double slash)', () => {
    expect(planDocPath(`${BASE}/`, 'overview.md')).toBe(`${BASE}/overview.md`);
    expect(planDocPath(`${BASE}///`, 'P1')).toBe(`${BASE}/phases/P1/overview.md`);
  });
  it('returns null for an empty/blank planPath', () => {
    expect(planDocPath('', 'overview.md')).toBe(null);
    expect(planDocPath('   ', 'overview.md')).toBe(null);
    expect(planDocPath(null, 'overview.md')).toBe(null);
  });
  it('rejects traversal / path-separator ids and empty selection', () => {
    expect(planDocPath(BASE, '../secret')).toBe(null);
    expect(planDocPath(BASE, 'a/b')).toBe(null);
    expect(planDocPath(BASE, 'a\\b')).toBe(null);
    expect(planDocPath(BASE, '..')).toBe(null);
    expect(planDocPath(BASE, '')).toBe(null);
    expect(planDocPath(BASE, '   ')).toBe(null);
    expect(planDocPath(BASE, null)).toBe(null);
  });
  it('never yields a path containing ..', () => {
    for (const sel of ['overview.md', 'status.md', 'P1', '../x', 'a/b']) {
      const p = planDocPath(BASE, sel);
      if (p !== null) expect(p.includes('..')).toBe(false);
    }
  });
});

describe('taskStatusLabel', () => {
  it('maps the known statuses to label + token', () => {
    expect(taskStatusLabel('planned')).toEqual({ label: 'PLANNED', token: 'planned' });
    expect(taskStatusLabel('in-progress')).toEqual({ label: 'IN-PROGRESS', token: 'in-progress' });
    expect(taskStatusLabel('in_progress')).toEqual({ label: 'IN-PROGRESS', token: 'in-progress' });
    expect(taskStatusLabel('done')).toEqual({ label: 'DONE', token: 'done' });
    expect(taskStatusLabel('completed')).toEqual({ label: 'DONE', token: 'done' });
  });
  it('uppercases an unknown raw status and tokenizes it', () => {
    expect(taskStatusLabel('blocked')).toEqual({ label: 'BLOCKED', token: 'blocked' });
    expect(taskStatusLabel('needs-changes')).toEqual({ label: 'NEEDS-CHANGES', token: 'needs-changes' });
    expect(taskStatusLabel('not-started')).toEqual({ label: 'NOT-STARTED', token: 'not-started' });
  });
});
