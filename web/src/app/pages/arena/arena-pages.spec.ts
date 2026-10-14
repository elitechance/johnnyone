// The four routed arena screens, MOUNTED at a phone width and driven with real MouseEvent clicks.
// The stub API records every call, so "the UI refused" is proved by the mutation NOT being sent,
// not by a disabled attribute alone.
import '@angular/compiler';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { ActivatedRoute, convertToParamMap, provideRouter, Router } from '@angular/router';
import { of, throwError } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { ArenaApiService } from './arena-api.service';
import { ArenaSetupPage } from './arena-setup.page';
import { WorkflowsPage } from './workflows.page';
import { WorkflowDetailPage } from './workflow-detail.page';
import { WorkflowArenaPage } from './workflow-arena.page';

const PHONE = 400;
const click = (el: Element | null) => {
  if (!el) throw new Error('clicked a control that is not on the screen');
  el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
};
const type = (el: Element | null, value: string) => {
  const input = el as HTMLTextAreaElement;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
};

function phoneViewport(): void {
  // jsdom applies no stylesheets, so this does not prove CSS reachability — `arena-phone.spec.ts`
  // is the static guard that does. What it does prove is that the components render their content
  // at a phone width rather than branching on a width and rendering nothing.
  Object.defineProperty(window, 'innerWidth', { value: PHONE, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: 860, configurable: true });
  window.dispatchEvent(new Event('resize'));
}

const WF = {
  id: 'w2',
  repo: 'johnnyone',
  phaseType: 'dev-phase',
  family: 'glm-5.3-flash',
  version: 2,
  parentId: 'w1',
  role: 'You are the development worker.',
  instruction: 'Implement the phase.',
  contract: 'capture with .johnnyone/capture.sh',
  hop: '{"steps":["read conventions","implement"]}',
  fanOut: 2,
  isChampion: false,
  createdAt: '2026-10-14T00:00:00Z',
};
const CHAMP = {
  ...WF,
  id: 'w1',
  version: 1,
  parentId: null,
  contract: 'a screenshot of each screen you changed',
  hop: '{"steps":["implement"]}',
  fanOut: 1,
  isChampion: true,
};
const RUN_PASS = {
  id: 'r2',
  workflowId: 'w2',
  planId: 'p1',
  seed: 0,
  gateExit: 0,
  gateCollected: 23,
  seatsHeld: '{"glm-5.3-flash":2}',
  fleetBusy: '{"glm-5.3-flash":1}',
  startedAt: '2026-10-14T01:00:00Z',
  endedAt: '2026-10-14T01:06:40Z',
};
const RUN_FAIL = { ...RUN_PASS, id: 'r1', workflowId: 'w1', gateExit: 101, gateCollected: null };
const STATE_OK = {
  repo: 'johnnyone',
  phaseType: 'dev-phase',
  family: 'glm-5.3-flash',
  consecutiveRejections: 1,
  halted: false,
  strikeLimit: 3,
  recentRejections: ['too slow'],
  championWorkflowId: 'w1',
};
const STATE_HALTED = {
  ...STATE_OK,
  consecutiveRejections: 3,
  halted: true,
  recentRejections: ['no coverage check', 'ignores the cashier path', 'too slow'],
};
const PREF_OK = { id: 'pf1', winnerRunId: 'r2', loserRunId: 'r1', comparable: true, reason: null, createdAt: 'x' };
const PREF_MISMATCH = { ...PREF_OK, id: 'pf2', comparable: false };

function stubApi(over: Record<string, unknown> = {}) {
  const calls: { name: string; args: unknown[] }[] = [];
  const rec = <T>(name: string, value: T) =>
    (...args: unknown[]) => {
      calls.push({ name, args });
      return of(value);
    };
  const api = {
    calls,
    getAgentPlan: rec('getAgentPlan', {
      plan: {
        id: 'p1',
        runType: 'development',
        title: 'Arena',
        workspacePath: '/home/creepy/Documents/Workspace/personal/apps/johnnyone',
        initiativeId: 'init-1',
        testCommands: [],
      },
      phases: [
        { id: 'ph1', phaseId: '04-leases', phaseTitle: 'The lease pool', effectiveTestCommands: ['cargo test --lib leases::'] },
        { id: 'ph2', phaseId: '08-polish', phaseTitle: 'Plan-tab polish', effectiveTestCommands: [] },
      ],
      tasks: [],
      events: [],
    }),
    runAgentPlanTestGate: rec('runAgentPlanTestGate', {
      gateDeclared: true, exitCode: 0, collected: 23, command: 'cargo test --lib leases::',
      commandsRun: 1, output: 'test result: ok. 23 passed', truncated: false,
    }),
    updateAgentPlanTestCommands: rec('updateAgentPlanTestCommands', { plan: {}, phases: [], tasks: [], events: [] }),
    workflows: rec('workflows', [CHAMP, WF]),
    workflow: rec('workflow', WF),
    workflowRuns: rec('workflowRuns', [RUN_PASS]),
    workflowSearchState: rec('workflowSearchState', STATE_OK),
    createWorkflowVersion: rec('createWorkflowVersion', WF),
    startWorkflowRun: rec('startWorkflowRun', RUN_PASS),
    recordWorkflowRunGate: rec('recordWorkflowRunGate', RUN_PASS),
    recordWorkflowPref: rec('recordWorkflowPref', PREF_OK),
    updateWorkflowChampion: rec('updateWorkflowChampion', { ...WF, isChampion: true }),
    recordWorkflowRejection: rec('recordWorkflowRejection', STATE_HALTED),
    // Reading a preference back, rather than recording one to discover it (new in the API).
    workflowPrefs: rec('workflowPrefs', []),
    updateWorkflowSearchResumed: rec('updateWorkflowSearchResumed', { ...STATE_OK, consecutiveRejections: 0, halted: false, recentRejections: [] }),
  };
  // Overrides are WRAPPED so they record too. They did not, which made every `api.calls` assertion
  // in a test that overrode the method it was asserting about silently vacuous — found while
  // writing the preference test, which passed a non-recording stub and then asked whether the call
  // had been recorded.
  for (const [name, impl] of Object.entries(over)) {
    const fn = impl as (...a: unknown[]) => unknown;
    (api as Record<string, unknown>)[name] = (...args: unknown[]) => {
      calls.push({ name, args });
      return fn(...args);
    };
  }
  return api as typeof api & Record<string, (...a: unknown[]) => unknown>;
}

/** These pages load their data in an async ngOnInit, so a synchronous `detectChanges` paints the
 *  pre-data frame. Every mount and every click is followed by a real task-queue drain. */
async function flush(f: { detectChanges: () => void }): Promise<void> {
  // Microtasks only. A bare `setTimeout` here never settles under zone.js/testing's ProxyZone
  // outside fakeAsync (measured: every spec hit the 5s timeout), and the stub API is synchronous,
  // so a handful of microtask turns is all the awaited chain needs.
  // Each awaited API call costs a few microtask turns and these pages make up to six in
  // sequence, so the budget is generous rather than exact.
  for (let i = 0; i < 60; i += 1) await Promise.resolve();
  f.detectChanges();
}

async function mount<T>(cmp: new (...a: never[]) => T, api: unknown, params: Record<string, string>, query: Record<string, string> = {}) {
  phoneViewport();
  TestBed.resetTestingModule();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([]),
      // One stub object behind both services: the split is about what ships in which bundle, not
      // about two different fakes, and a test that can see every call in one list is easier to
      // read than two.
      { provide: JohnnyApiService, useValue: api },
      { provide: ArenaApiService, useValue: api },
      {
        provide: ActivatedRoute,
        useValue: {
          snapshot: { paramMap: convertToParamMap(params), queryParamMap: convertToParamMap(query) },
          paramMap: of(convertToParamMap(params)),
          queryParamMap: of(convertToParamMap(query)),
        },
      },
    ],
  });
  const f = TestBed.createComponent(cmp);
  await flush(f);
  await flush(f);
  return f;
}

const txt = (f: { nativeElement: HTMLElement }) => (f.nativeElement as HTMLElement).textContent ?? '';
const q = (f: { nativeElement: HTMLElement }, sel: string) => (f.nativeElement as HTMLElement).querySelector(sel);

describe('ArenaSetupPage — /initiatives/:id/arena, the 3-step setup', () => {
  beforeEach(() => vi.restoreAllMocks());

  it('step 1 shows the phase’s effective gate and will not advance before it has been run', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases' });
    expect(txt(f)).toContain('cargo test --lib leases::');
    expect((q(f, '[data-test="next-gate"]') as HTMLButtonElement).disabled).toBe(true);
    expect(api.calls.some((c) => c.name === 'runAgentPlanTestGate')).toBe(false);
  });

  it('runs the gate once, synchronously, and only then advances', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases' });
    click(q(f, '[data-test="run-gate"]'));
    await flush(f);
    expect(api.calls.filter((c) => c.name === 'runAgentPlanTestGate')).toHaveLength(1);
    expect(api.calls.find((c) => c.name === 'runAgentPlanTestGate')!.args).toEqual(['p1', '04-leases']);
    expect(txt(f)).toContain('23 collected');
    expect((q(f, '[data-test="next-gate"]') as HTMLButtonElement).disabled).toBe(false);
  });

  it('REFUSES a gate that collects nothing, and stays on step 1', async () => {
    const api = stubApi({
      runAgentPlanTestGate: () => of({ gateDeclared: true, exitCode: 0, collected: 0, command: 'npm t', commandsRun: 1, output: '0 passed', truncated: false }),
    });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases' });
    click(q(f, '[data-test="run-gate"]'));
    await flush(f);
    expect(txt(f)).toMatch(/collects nothing/i);
    expect((q(f, '[data-test="next-gate"]') as HTMLButtonElement).disabled).toBe(true);
  });

  it('REFUSES a command already red on untouched code', async () => {
    const api = stubApi({
      runAgentPlanTestGate: () => of({ gateDeclared: true, exitCode: 101, collected: 23, command: 'npm t', commandsRun: 1, output: 'FAILED', truncated: false }),
    });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases' });
    click(q(f, '[data-test="run-gate"]'));
    await flush(f);
    expect(txt(f)).toMatch(/already red/i);
    expect((q(f, '[data-test="next-gate"]') as HTMLButtonElement).disabled).toBe(true);
  });

  it('renders a null collected count as "not reported", never as 0', async () => {
    const api = stubApi({
      runAgentPlanTestGate: () => of({ gateDeclared: true, exitCode: 0, collected: null, command: 'npm run lint', commandsRun: 1, output: '', truncated: false }),
    });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases' });
    click(q(f, '[data-test="run-gate"]'));
    await flush(f);
    expect((q(f, '[data-test="gate-collected"]') as HTMLElement).textContent).toContain('not reported');
    // A lint is a legitimate gate: a null count must not block the setup.
    expect((q(f, '[data-test="next-gate"]') as HTMLButtonElement).disabled).toBe(false);
  });

  it('step 2 states plainly that leases are NOT measured, instead of drawing a meter', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'seats' });
    expect(txt(f)).toMatch(/not reported|not measured/i);
    expect(q(f, '[data-test="seats-unmeasured"]')).toBeTruthy();
  });

  it('step 3 refuses to start with any of the four parts blank', async () => {
    const api = stubApi({ workflows: () => of([]) });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    const start = q(f, '[data-test="start-search"]') as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    click(start);
    await flush(f);
    expect(api.calls.some((c) => c.name === 'createWorkflowVersion')).toBe(false);
    expect(txt(f)).toMatch(/four parts|blank/i);
  });

  it('seeds the four slots from the champion when one exists, and says where they came from', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    expect((q(f, '[data-test="slot-role"]') as HTMLTextAreaElement).value).toBe(CHAMP.role);
    expect((q(f, '[data-test="slot-contract"]') as HTMLTextAreaElement).value).toBe(CHAMP.contract);
    expect(txt(f)).toMatch(/v1|champion/i);
  });

  it('says plainly when there is NO champion to beat, rather than opening a blank editor silently', async () => {
    const api = stubApi({ workflows: () => of([]) });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    expect(q(f, '[data-test="no-baseline"]')).toBeTruthy();
    expect(txt(f)).toMatch(/baseline/i);
  });

  it('a HALTED search refuses to start a run, with the host’s reasons on screen', async () => {
    const api = stubApi({ workflowSearchState: () => of(STATE_HALTED) });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    expect(q(f, '[data-test="halt-block"]')).toBeTruthy();
    expect(txt(f)).toContain('ignores the cashier path');
    const start = q(f, '[data-test="start-search"]') as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    click(start);
    await flush(f);
    expect(api.calls.some((c) => c.name === 'startWorkflowRun')).toBe(false);
    expect(api.calls.some((c) => c.name === 'createWorkflowVersion')).toBe(false);
  });

  it('starts the search when everything is satisfied, creating a version and a run', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    click(q(f, '[data-test="start-search"]'));
    await flush(f);
    expect(api.calls.filter((c) => c.name === 'createWorkflowVersion')).toHaveLength(1);
    expect(api.calls.filter((c) => c.name === 'startWorkflowRun')).toHaveLength(1);
  });

  it('never invents a seat budget or a contention map — the host measures both', async () => {
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    click(q(f, '[data-test="start-search"]'));
    await flush(f);
    const args = api.calls.find((c) => c.name === 'startWorkflowRun')!.args[0] as Record<string, unknown>;
    expect(Object.keys(args).sort()).toEqual(['planId', 'seats', 'seed', 'workflowId']);
    expect(JSON.stringify(args)).not.toContain('fleetBusy');
    expect(JSON.stringify(args)).not.toContain('seatsHeld');
  });

  it('does NOT claim a run is under way that nothing will ever score', async () => {
    // Measured hole: `recordWorkflowRunGate` has no caller outside a spec stub, and nothing on the
    // host links a `workflow_runs` row to plan execution — so a run created here keeps
    // `gateExit = null` forever, which blocks promotion. The screen has to say that.
    const api = stubApi();
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    const note = q(f, '[data-test="no-executor-note"]');
    expect(note).toBeTruthy();
    expect(note!.textContent).toMatch(/executor/i);
    expect(note!.textContent).toMatch(/never ran|nothing (?:executes|has scored)|no worker/i);
    expect(txt(f)).not.toContain('Start the search');
  });

  it('shows a host refusal instead of pretending the run started', async () => {
    const api = stubApi({
      startWorkflowRun: () => throwError(() => new Error('the search for this family is halted')),
    });
    const f = await mount(ArenaSetupPage, api, { id: 'p1' }, { phaseId: '04-leases', step: 'prompt' });
    click(q(f, '[data-test="start-search"]'));
    await flush(f);
    expect(txt(f)).toContain('halted');
  });
});

describe('WorkflowsPage — /workflows, the list and the fleet strip', () => {
  it('lists champions and challengers, marking the one champion per family', async () => {
    const api = stubApi();
    const f = await mount(WorkflowsPage, api, {});
    expect(txt(f)).toContain('dev-phase');
    expect(txt(f)).toContain('glm-5.3-flash');
    expect(q(f, '[data-test="wf-row-w1"] [data-test="champion-badge"]')).toBeTruthy();
    expect(q(f, '[data-test="wf-row-w2"] [data-test="champion-badge"]')).toBeNull();
  });

  it('links each row to its four slots and to its arena', async () => {
    const f = await mount(WorkflowsPage, stubApi(), {});
    expect((q(f, '[data-test="wf-link-w2"]') as HTMLAnchorElement).getAttribute('href')).toBe('/workflows/w2');
    expect((q(f, '[data-test="wf-arena-w2"]') as HTMLAnchorElement).getAttribute('href')).toBe('/workflows/w2/arena');
  });

  it('the fleet strip reports what it actually knows — no invented seat meter', async () => {
    const f = await mount(WorkflowsPage, stubApi(), {});
    const strip = q(f, '[data-test="fleet-strip"]') as HTMLElement;
    expect(strip).toBeTruthy();
    expect(strip.textContent).toMatch(/not measured|not reported/i);
    expect(strip.querySelector('progress')).toBeNull();
  });

  it('says so when there are no workflows at all', async () => {
    const f = await mount(WorkflowsPage, stubApi({ workflows: () => of([]) }), {});
    expect(txt(f)).toMatch(/no workflows/i);
  });
});

describe('WorkflowDetailPage — /workflows/:id, the four slots and the lineage', () => {
  it('shows all four parts and the seat cost', async () => {
    const f = await mount(WorkflowDetailPage, stubApi(), { id: 'w2' });
    for (const slot of ['role', 'instruction', 'contract', 'hop']) {
      expect(q(f, `[data-test="slot-${slot}"]`)).toBeTruthy();
    }
    expect(txt(f)).toContain(WF.contract);
    expect(txt(f)).toMatch(/width 2|2 seats/i);
  });

  it('shows the lineage oldest-first, so a rejected version is still visible', async () => {
    const f = await mount(WorkflowDetailPage, stubApi(), { id: 'w2' });
    const items = (f.nativeElement as HTMLElement).querySelectorAll('[data-test="lineage-item"]');
    expect(items).toHaveLength(2);
    expect(items[0].textContent).toContain('v1');
    expect(items[1].textContent).toContain('v2');
  });

  it('never shows a score, a rating or a percentage for a workflow', async () => {
    const f = await mount(WorkflowDetailPage, stubApi(), { id: 'w2' });
    expect(txt(f)).not.toMatch(/\bscore\b|\brating\b|%/i);
  });
});

describe('WorkflowArenaPage — /workflows/:id/arena, champion vs challenger', () => {
  it('shows both sides with the gate as the only score, and greys the timing', async () => {
    const api = stubApi({
      workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    expect(txt(f)).toContain('exit 101');
    expect(txt(f)).toContain('exit 0');
    expect(txt(f)).toMatch(/never scores|does not score/i);
  });

  it('renders the champion run’s null collected count as "not reported", not 0', async () => {
    const api = stubApi({ workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]) });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    const cell = q(f, '[data-test="champion-collected"]') as HTMLElement;
    expect(cell.textContent).toContain('not reported');
    expect(cell.textContent).not.toContain('0');
  });

  it('shows WHICH of the four parts changed, not a text diff of the whole prompt', async () => {
    const f = await mount(WorkflowArenaPage, stubApi(), { id: 'w2' });
    const changed = (f.nativeElement as HTMLElement).querySelectorAll('[data-test="changed-slot"]');
    expect([...changed].map((c) => c.getAttribute('data-slot'))).toEqual(['contract', 'hop', 'width']);
  });

  // CORRECTED mid-round. These two specs first assumed the page would hold a preference as soon
  // as it loaded. It cannot: the schema has queries for workflows, runs and the search state, but
  // NONE for `workflow_prefs`, so the only way a client obtains the host's comparability verdict is
  // to record the head-to-head. Recording a row as a side effect of merely opening a page is wrong,
  // so comparing is an explicit control and promotion is blocked until it has happened. The gap is
  // in the report, not papered over by re-deriving comparability in the client.
  it('promotion needs an explicit confirm, and the confirm states the blast radius', async () => {
    const api = stubApi({ workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]) });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    // Promotion is blocked before a comparable head-to-head exists.
    expect((q(f, '[data-test="promote"]') as HTMLButtonElement).disabled).toBe(true);
    click(q(f, '[data-test="compare"]'));
    await flush(f);
    expect(api.calls.find((c) => c.name === 'recordWorkflowPref')!.args).toEqual(['r2', 'r1', null]);
    click(q(f, '[data-test="promote"]'));
    await flush(f);
    // Nothing is promoted by opening the confirm.
    expect(api.calls.some((c) => c.name === 'updateWorkflowChampion')).toBe(false);
    const confirm = q(f, '[data-test="promote-confirm"]') as HTMLElement;
    expect(confirm.textContent).toContain('johnnyone');
    expect(confirm.textContent).toContain('dev-phase');
    expect(confirm.textContent).toContain('glm-5.3-flash');
    click(q(f, '[data-test="promote-do"]'));
    await flush(f);
    const call = api.calls.find((c) => c.name === 'updateWorkflowChampion')!;
    expect(call.args).toEqual(['r2', 'pf1', true]);
  });

  it('BLOCKS promotion when the seat budgets did not match — blocked, not warned', async () => {
    const api = stubApi({
      recordWorkflowPref: () => of(PREF_MISMATCH),
      workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    click(q(f, '[data-test="compare"]'));
    await flush(f);
    const promote = q(f, '[data-test="promote"]') as HTMLButtonElement;
    expect(promote.disabled).toBe(true);
    expect(txt(f)).toMatch(/seat/i);
    click(promote);
    await flush(f);
    expect(q(f, '[data-test="promote-confirm"]')).toBeNull();
    expect(api.calls.some((c) => c.name === 'updateWorkflowChampion')).toBe(false);
  });

  it('refuses a rejection with no reason — there is no bare no', async () => {
    const api = stubApi();
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    click(q(f, '[data-test="reject"]'));
    await flush(f);
    const send = q(f, '[data-test="reject-send"]') as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    click(send);
    await flush(f);
    expect(api.calls.some((c) => c.name === 'recordWorkflowRejection')).toBe(false);
    expect(txt(f)).toMatch(/reason/i);

    type(q(f, '[data-test="reject-reason"]'), '   ');
    await flush(f);
    expect((q(f, '[data-test="reject-send"]') as HTMLButtonElement).disabled).toBe(true);
  });

  it('records a rejection with its reason, and says so when the search halts', async () => {
    const api = stubApi();
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    click(q(f, '[data-test="reject"]'));
    await flush(f);
    type(q(f, '[data-test="reject-reason"]'), 'the gate never touches the cashier path');
    await flush(f);
    click(q(f, '[data-test="reject-send"]'));
    await flush(f);
    expect(api.calls.find((c) => c.name === 'recordWorkflowRejection')!.args).toEqual([
      'r2',
      'the gate never touches the cashier path',
    ]);
    // The host answered with a halted state: the UI must say so and refuse another run.
    expect(q(f, '[data-test="halt-block"]')).toBeTruthy();
    expect(txt(f)).toContain('no coverage check');
    expect((q(f, '[data-test="replay"]') as HTMLButtonElement).disabled).toBe(true);
  });

  it('a search already halted on load refuses a replay before anything is clicked', async () => {
    const api = stubApi({ workflowSearchState: () => of(STATE_HALTED) });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    expect(q(f, '[data-test="halt-block"]')).toBeTruthy();
    click(q(f, '[data-test="replay"]'));
    await flush(f);
    expect(api.calls.some((c) => c.name === 'startWorkflowRun')).toBe(false);
  });

  it('a replay asks the host for a run and invents no budget', async () => {
    const api = stubApi({ workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]) });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    click(q(f, '[data-test="replay"]'));
    await flush(f);
    const args = api.calls.find((c) => c.name === 'startWorkflowRun')!.args[0] as Record<string, unknown>;
    expect(args['seed']).toBe(1);
    expect(Object.keys(args).sort()).toEqual(['planId', 'seats', 'seed', 'workflowId']);
  });

  it('READS the preference back on load — no write is needed to discover one', async () => {
    const api = stubApi({
      workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]),
      workflowPrefs: () => of([PREF_OK]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    expect(api.calls.some((c) => c.name === 'workflowPrefs')).toBe(true);
    // Promotion is available without recording anything first…
    expect((q(f, '[data-test="promote"]') as HTMLButtonElement).disabled).toBe(false);
    expect(api.calls.some((c) => c.name === 'recordWorkflowPref')).toBe(false);
    // …and the Compare control is not offered, because there is nothing left to record.
    expect(q(f, '[data-test="compare"]')).toBeNull();
  });

  it('ignores a preference that is about other runs', async () => {
    const api = stubApi({
      workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]),
      workflowPrefs: () => of([{ ...PREF_OK, id: 'pfX', winnerRunId: 'rZZ', loserRunId: 'rYY' }]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    expect((q(f, '[data-test="promote"]') as HTMLButtonElement).disabled).toBe(true);
    expect(q(f, '[data-test="compare"]')).toBeTruthy();
  });

  it('says plainly that a run with no score is one nothing has scored yet', async () => {
    const api = stubApi({
      workflowRuns: () => of([{ ...RUN_PASS, gateExit: null, gateCollected: null, endedAt: null }]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    const note = q(f, '[data-test="no-executor-note"]');
    expect(note).toBeTruthy();
    expect(note!.textContent).toMatch(/executor/i);
    expect(note!.textContent).toMatch(/never ran|nothing (?:executes|has scored)|no worker/i);
    expect((q(f, '[data-test="promote"]') as HTMLButtonElement).disabled).toBe(true);
  });

  it('lifts a halt only through its own confirmed decision, with a visible reason', async () => {
    const api = stubApi({ workflowSearchState: () => of(STATE_HALTED) });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    expect(q(f, '[data-test="halt-block"]')).toBeTruthy();
    click(q(f, '[data-test="resume"]'));
    await flush(f);
    const send = q(f, '[data-test="resume-send"]') as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    // A reason made only of invisible characters is not a reason.
    type(q(f, '[data-test="resume-reason"]'), '\u200b\ufeff');
    await flush(f);
    expect((q(f, '[data-test="resume-send"]') as HTMLButtonElement).disabled).toBe(true);
    click(q(f, '[data-test="resume-send"]'));
    await flush(f);
    expect(api.calls.some((c) => c.name === 'updateWorkflowSearchResumed')).toBe(false);

    type(q(f, '[data-test="resume-reason"]'), 'added a coverage threshold to the gate');
    await flush(f);
    click(q(f, '[data-test="resume-send"]'));
    await flush(f);
    expect(api.calls.find((c) => c.name === 'updateWorkflowSearchResumed')!.args).toEqual([
      'johnnyone', 'dev-phase', 'glm-5.3-flash', 'added a coverage threshold to the gate', true,
    ]);
    // The host's new state comes back un-halted, so the banner goes.
    expect(q(f, '[data-test="halt-block"]')).toBeNull();
  });

  it('refuses to promote while the search is halted, as the host now does', async () => {
    const api = stubApi({
      workflowSearchState: () => of(STATE_HALTED),
      workflowRuns: (id: unknown) => of(id === 'w1' ? [RUN_FAIL] : [RUN_PASS]),
      workflowPrefs: () => of([PREF_OK]),
    });
    const f = await mount(WorkflowArenaPage, api, { id: 'w2' });
    const promote = q(f, '[data-test="promote"]') as HTMLButtonElement;
    expect(promote.disabled).toBe(true);
    click(promote);
    await flush(f);
    expect(q(f, '[data-test="promote-confirm"]')).toBeNull();
    expect(api.calls.some((c) => c.name === 'updateWorkflowChampion')).toBe(false);
  });

  it('links back to the Plan tab to read the work, rather than cloning the document browser', async () => {
    const f = await mount(WorkflowArenaPage, stubApi(), { id: 'w2' });
    const read = q(f, '[data-test="read-the-work"]') as HTMLAnchorElement;
    expect(read.getAttribute('href')).toContain('/initiatives');
    expect(read.getAttribute('href')).toContain('tab=plan');
  });
});
