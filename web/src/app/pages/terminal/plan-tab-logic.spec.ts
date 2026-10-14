import { describe, it, expect } from 'vitest';
import type {
  AgentPlanRun,
  AgentPlanPhase,
  AgentPlanTask,
} from '../../../../../ui/src/services/johnny-api.service';
import {
  planCounts,
  docNavModel,
  defaultPlanDoc,
  phaseCards,
  planDocPath,
  taskStatusLabel,
  artifactGroups,
  artifactDirsFor,
  artifactOpenMode,
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
  it('lists brief/plan, then (when phases exist) overview/status + phases in host order', () => {
    const nav = docNavModel(SAMPLE);
    expect(nav.map((e) => e.id)).toEqual([
      'brief.md',
      'plan.md',
      'overview.md',
      'status.md',
      'P1-schema',
      'P2-runner',
    ]);
    expect(nav[0]).toEqual({ kind: 'file', id: 'brief.md', label: 'brief.md' });
    expect(nav[1]).toEqual({ kind: 'file', id: 'plan.md', label: 'plan.md' });
    const p2 = nav[5];
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
  it('returns just the planning artifacts (brief/plan) for a null/phase-less run', () => {
    expect(docNavModel(null).map((e) => e.id)).toEqual(['brief.md', 'plan.md']);
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

describe('defaultPlanDoc', () => {
  it('opens plan.md while planning (no phases) and overview.md once decomposed', () => {
    expect(defaultPlanDoc(null)).toBe('plan.md');
    expect(defaultPlanDoc({ phases: [] } as unknown as AgentPlanRun)).toBe('plan.md');
    expect(defaultPlanDoc(SAMPLE)).toBe('overview.md');
  });
});

describe('planDocPath', () => {
  const BASE = '/store/.johnnyone/initiatives/abc/plan';

  it('builds fixed-suffix paths for planning + development files and a phase id', () => {
    expect(planDocPath(BASE, 'brief.md')).toBe(`${BASE}/brief.md`);
    expect(planDocPath(BASE, 'plan.md')).toBe(`${BASE}/plan.md`);
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

// ── Artifacts in the Plan tab (mass-artifacts) ───────────────────────────────────────────────────
// `planDocPath` is the SINGLE allowlist. It now also accepts artifact sub-paths under the plan dir
// (`artifacts/…` and `phases/<id>/artifacts/…`); everything else keeps the old fixed-suffix shape.
// These traversal cases were written and run RED before the widening landed.

describe('planDocPath — artifact sub-paths', () => {
  const BASE = '/store/.johnnyone/initiatives/abc/plan';

  it('accepts the plan artifacts dir and files/dirs beneath it', () => {
    expect(planDocPath(BASE, 'artifacts')).toBe(`${BASE}/artifacts`);
    expect(planDocPath(BASE, 'artifacts/run-resume-mock.html')).toBe(
      `${BASE}/artifacts/run-resume-mock.html`,
    );
    expect(planDocPath(BASE, 'artifacts/mocks')).toBe(`${BASE}/artifacts/mocks`);
    expect(planDocPath(BASE, 'artifacts/mocks/connections.html')).toBe(
      `${BASE}/artifacts/mocks/connections.html`,
    );
    expect(planDocPath(BASE, 'artifacts/ui/e2e-s10-after-apply.png')).toBe(
      `${BASE}/artifacts/ui/e2e-s10-after-apply.png`,
    );
  });

  it("accepts a phase's artifacts dir and its contents", () => {
    expect(planDocPath(BASE, 'phases/01-console-run-resume-ui/artifacts')).toBe(
      `${BASE}/phases/01-console-run-resume-ui/artifacts`,
    );
    expect(planDocPath(BASE, 'phases/P2-plan-tab/artifacts/vitest.log')).toBe(
      `${BASE}/phases/P2-plan-tab/artifacts/vitest.log`,
    );
    expect(planDocPath(BASE, 'phases/P2-plan-tab/artifacts/screenshots/p2.png')).toBe(
      `${BASE}/phases/P2-plan-tab/artifacts/screenshots/p2.png`,
    );
  });

  it('normalizes a trailing slash on planPath for artifact paths too', () => {
    expect(planDocPath(`${BASE}//`, 'artifacts/a.md')).toBe(`${BASE}/artifacts/a.md`);
  });

  it('rejects `..` anywhere in an artifact sub-path', () => {
    expect(planDocPath(BASE, 'artifacts/../../../etc/passwd')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/..')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/../brief.md')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/mocks/../../../../secret')).toBe(null);
    expect(planDocPath(BASE, 'phases/P1/artifacts/../../../../etc/shadow')).toBe(null);
    expect(planDocPath(BASE, 'phases/../artifacts/x')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/..%2f..%2fetc')).toBe(null);
  });

  it('rejects backslash traversal and Windows-style absolute artifact paths', () => {
    expect(planDocPath(BASE, 'artifacts\\..\\..\\secret')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/mocks\\evil.html')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/C:\\Windows\\win.ini')).toBe(null);
    expect(planDocPath(BASE, 'phases/P1\\artifacts/x')).toBe(null);
  });

  it('rejects an absolute or UNC artifact path', () => {
    expect(planDocPath(BASE, '/artifacts/x.html')).toBe(null);
    expect(planDocPath(BASE, '/etc/passwd')).toBe(null);
    expect(planDocPath(BASE, '//evil.host/share/x')).toBe(null);
    expect(planDocPath(BASE, 'artifacts//x')).toBe(null);
    expect(planDocPath(BASE, 'artifacts/./x')).toBe(null);
  });

  it('does not treat a non-artifact nested path as allowed', () => {
    // Only `artifacts/…` and `phases/<id>/artifacts/…` are nested-capable; nothing else is.
    expect(planDocPath(BASE, 'phases/P1/overview.md')).toBe(null);
    expect(planDocPath(BASE, 'phases/P1/notes/secret.md')).toBe(null);
    expect(planDocPath(BASE, 'brief.md/../../x')).toBe(null);
    expect(planDocPath(BASE, 'secrets/key.pem')).toBe(null);
  });

  it('still returns null for an empty planPath on an artifact path', () => {
    expect(planDocPath('', 'artifacts/x.md')).toBe(null);
    expect(planDocPath(null, 'artifacts/x.md')).toBe(null);
  });

  it('never yields a path outside the plan dir, for any selection', () => {
    const sels = [
      'brief.md', 'plan.md', 'overview.md', 'status.md', 'P1',
      'artifacts', 'artifacts/a.md', 'artifacts/mocks/b.html',
      'phases/P1/artifacts', 'phases/P1/artifacts/deep/c.png',
      '../x', 'a/b', 'artifacts/../..', 'artifacts\\x', '/etc/passwd',
      'artifacts/./x', 'artifacts//x', 'phases/../artifacts/x',
    ];
    for (const sel of sels) {
      const p = planDocPath(BASE, sel);
      if (p === null) continue;
      expect(p.includes('..')).toBe(false);
      expect(p.includes('\\')).toBe(false);
      expect(p.startsWith(`${BASE}/`)).toBe(true);
    }
  });
});

describe('artifactGroups', () => {
  const RUN = run({
    phases: [
      phase({ phaseId: '00-backend', phaseTitle: 'Backend run-from-phase', phaseIndex: 0 }),
      phase({ phaseId: '01-console', phaseTitle: 'Console run/resume UI', phaseIndex: 1 }),
    ],
  });

  it('is empty when nothing has been listed yet', () => {
    expect(artifactGroups(RUN, {}, {})).toEqual([]);
  });

  it('omits a listed-but-empty directory entirely (no empty shell)', () => {
    expect(artifactGroups(RUN, { artifacts: [], 'phases/00-backend/artifacts': [] }, {})).toEqual([]);
  });

  it('lists the plan artifacts dir, dirs collapsed, with plan-relative ids', () => {
    const groups = artifactGroups(
      RUN,
      {
        artifacts: [
          { name: 'mocks', kind: 'dir' },
          { name: 'ui-impact.md', kind: 'file' },
        ],
      },
      {},
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe('plan');
    expect(groups[0].dirRel).toBe('artifacts');
    expect(groups[0].nodes).toEqual([
      { id: 'artifacts/mocks', name: 'mocks', kind: 'dir', depth: 0, expanded: false },
      { id: 'artifacts/ui-impact.md', name: 'ui-impact.md', kind: 'file', depth: 0, expanded: false },
    ]);
  });

  it('splices an expanded subdirectory in place, one depth deeper', () => {
    const groups = artifactGroups(
      RUN,
      {
        artifacts: [
          { name: 'mocks', kind: 'dir' },
          { name: 'ui-impact.md', kind: 'file' },
        ],
        'artifacts/mocks': [
          { name: 'connections.html', kind: 'file' },
          { name: 'nested', kind: 'dir' },
        ],
      },
      { 'artifacts/mocks': true },
    );
    expect(groups[0].nodes.map((n) => [n.id, n.depth, n.kind, n.expanded])).toEqual([
      ['artifacts/mocks', 0, 'dir', true],
      ['artifacts/mocks/connections.html', 1, 'file', false],
      ['artifacts/mocks/nested', 1, 'dir', false],
      ['artifacts/ui-impact.md', 0, 'file', false],
    ]);
  });

  it('does not splice children of a listed-but-collapsed subdirectory', () => {
    const groups = artifactGroups(
      RUN,
      {
        artifacts: [{ name: 'mocks', kind: 'dir' }],
        'artifacts/mocks': [{ name: 'connections.html', kind: 'file' }],
      },
      {},
    );
    expect(groups[0].nodes.map((n) => n.id)).toEqual(['artifacts/mocks']);
  });

  it('adds one group per phase that has artifacts, labelled by phase title, in host order', () => {
    const groups = artifactGroups(
      RUN,
      {
        'phases/01-console/artifacts': [{ name: 'vitest.log', kind: 'file' }],
        'phases/00-backend/artifacts': [{ name: 'verify.txt', kind: 'file' }],
      },
      {},
    );
    expect(groups.map((g) => [g.dirRel, g.label])).toEqual([
      ['phases/00-backend/artifacts', 'Backend run-from-phase'],
      ['phases/01-console/artifacts', 'Console run/resume UI'],
    ]);
    expect(groups[1].nodes[0].id).toBe('phases/01-console/artifacts/vitest.log');
  });

  it('puts the plan group before the phase groups', () => {
    const groups = artifactGroups(
      RUN,
      {
        'phases/00-backend/artifacts': [{ name: 'verify.txt', kind: 'file' }],
        artifacts: [{ name: 'a.md', kind: 'file' }],
      },
      {},
    );
    expect(groups.map((g) => g.dirRel)).toEqual(['artifacts', 'phases/00-backend/artifacts']);
  });

  it('ignores a listing for a phase the run does not have', () => {
    expect(artifactGroups(RUN, { 'phases/99-ghost/artifacts': [{ name: 'x', kind: 'file' }] }, {}))
      .toEqual([]);
  });

  it('drops an entry whose name would not survive the path allowlist', () => {
    const groups = artifactGroups(
      RUN,
      { artifacts: [{ name: '..', kind: 'dir' }, { name: 'a/b', kind: 'file' }, { name: 'ok.md', kind: 'file' }] },
      {},
    );
    expect(groups[0].nodes.map((n) => n.name)).toEqual(['ok.md']);
  });

  it('is null-safe on the run', () => {
    expect(artifactGroups(null, { artifacts: [{ name: 'a.md', kind: 'file' }] }, {})).toEqual([
      { label: 'plan', dirRel: 'artifacts', nodes: [
        { id: 'artifacts/a.md', name: 'a.md', kind: 'file', depth: 0, expanded: false },
      ] },
    ]);
  });
});

describe('artifactDirsFor', () => {
  it('is the plan artifacts dir plus one per phase', () => {
    expect(artifactDirsFor(SAMPLE)).toEqual([
      'artifacts',
      'phases/P1-schema/artifacts',
      'phases/P2-runner/artifacts',
    ]);
  });
  it('is just the plan artifacts dir for a null/phase-less run', () => {
    expect(artifactDirsFor(null)).toEqual(['artifacts']);
  });
  it('skips a phase id that would not survive the allowlist', () => {
    const bad = run({ phases: [phase({ phaseId: '../escape' }), phase({ phaseId: 'ok' })] });
    expect(artifactDirsFor(bad)).toEqual(['artifacts', 'phases/ok/artifacts']);
  });
});

describe('artifactOpenMode', () => {
  it('routes markdown to the in-page markdown view', () => {
    expect(artifactOpenMode('artifacts/ui-impact.md')).toBe('markdown');
    expect(artifactOpenMode('artifacts/A.MD')).toBe('markdown');
    expect(artifactOpenMode('artifacts/x.markdown')).toBe('markdown');
  });
  it('routes everything else to the file viewer (which picks html/image/text itself)', () => {
    expect(artifactOpenMode('artifacts/mock.html')).toBe('viewer');
    expect(artifactOpenMode('artifacts/ui/a.png')).toBe('viewer');
    expect(artifactOpenMode('phases/P1/artifacts/verify.txt')).toBe('viewer');
    expect(artifactOpenMode('artifacts/noext')).toBe('viewer');
  });
});
