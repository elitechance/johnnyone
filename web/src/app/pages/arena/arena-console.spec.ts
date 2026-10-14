// The console-side arena surfaces, MOUNTED and clicked with real MouseEvents in jsdom.
//
// Why these components are inline-template/inline-style standalone components rather than the
// page-with-templateUrl convention: `web/vitest.config.ts` omits @analogjs/vitest-angular, so a
// component whose template or styles live in a separate file cannot be compiled by TestBed — the
// measurement is in the report ("Component 'X' is not resolved"). Inline template + inline styles
// are JIT-compilable, which is the only way these behaviours get a failing-then-passing test
// instead of an assertion in a report.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import '../../../test-setup';
import { ArenaRowComponent } from './arena-row.component';
import { GateStripComponent } from './gate-strip.component';
import { NO_GATE_REASON } from './arena-logic';
import { DEFAULT_PANE_TAB, paneTabOf, type PaneTab } from '../terminal/terminal-transcript-tab';
import { consoleTabsWithArena, visibleConsoleTabs } from '../terminal/console-tabs-logic';

const click = (el: Element) => el.dispatchEvent(new MouseEvent('click', { bubbles: true }));

describe('ArenaRowComponent — step 1, on a phase that already exists', () => {
  beforeEach(() => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
  });

  const mount = (phase: Record<string, unknown>) => {
    const f = TestBed.createComponent(ArenaRowComponent);
    f.componentInstance.initiativeId = 'init-1';
    f.componentInstance.phase = phase as never;
    f.detectChanges();
    return f;
  };

  it('offers the arena on a phase that has a gate, linking to that initiative’s arena', () => {
    const f = mount({ phaseId: '04-leases', phaseTitle: 'The lease pool', effectiveTestCommands: ['cargo test --lib leases::'] });
    const el = f.nativeElement as HTMLElement;
    const enter = el.querySelector('[data-test="enter-arena"]') as HTMLAnchorElement;
    expect(enter).toBeTruthy();
    expect(enter.getAttribute('href')).toBe('/initiatives/init-1/arena?phaseId=04-leases');
    expect(el.textContent).toContain('cargo test --lib leases::');
  });

  it('keeps the control ON SCREEN and disabled with its reason when the phase has no gate', () => {
    const f = mount({ phaseId: '08-polish', phaseTitle: 'Plan-tab polish', effectiveTestCommands: [] });
    const el = f.nativeElement as HTMLElement;
    const disabled = el.querySelector('[data-test="enter-arena-disabled"]') as HTMLButtonElement;
    expect(disabled).toBeTruthy();
    expect(disabled.disabled).toBe(true);
    // The reason is TEXT ON THE SCREEN, not a tooltip: a hidden control teaches nobody.
    expect(el.textContent).toContain(NO_GATE_REASON);
    // ...and there is no live link to the arena at all.
    expect(el.querySelector('[data-test="enter-arena"]')).toBeNull();
  });

  // CORRECTED mid-round: this first asserted no `a[href*="arena"]` survived the click, which
  // contradicted the very next spec — the dead end's way out ("set one") is itself a link into the
  // setup's gate step. The invariant that matters is that the ENTRY control never appears.
  it('clicking the disabled control does not open the arena', () => {
    const f = mount({ phaseId: '08-polish', effectiveTestCommands: [] });
    const el = f.nativeElement as HTMLElement;
    const disabled = el.querySelector('[data-test="enter-arena-disabled"]') as HTMLButtonElement;
    click(disabled);
    f.detectChanges();
    expect(el.querySelector('[data-test="enter-arena"]')).toBeNull();
    const out = el.querySelector('[data-test="set-gate"]') as HTMLAnchorElement;
    expect(out.getAttribute('href')).toBe('/initiatives/init-1/arena?phaseId=08-polish&step=gate');
  });

  it('offers to SET a gate, so the dead end names its own way out', () => {
    const f = mount({ phaseId: '08-polish', effectiveTestCommands: [] });
    const set = (f.nativeElement as HTMLElement).querySelector('[data-test="set-gate"]');
    expect(set).toBeTruthy();
  });
});

describe('GateStripComponent — step 5, the gate’s exit code and collected count', () => {
  beforeEach(() => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
  });

  const mount = (gate: Record<string, unknown> | null, pending = false) => {
    const f = TestBed.createComponent(GateStripComponent);
    f.componentInstance.gate = gate as never;
    f.componentInstance.pending = pending;
    f.detectChanges();
    return f;
  };

  it('renders a null collected count as "not reported" and NEVER as 0', () => {
    const el = mount({ gateDeclared: true, exitCode: 0, collected: null, command: 'npm run check', commandsRun: 1, output: '', truncated: false }).nativeElement as HTMLElement;
    const count = el.querySelector('[data-test="gate-collected"]') as HTMLElement;
    expect(count.textContent).toContain('not reported');
    expect(count.textContent).not.toContain('0');
  });

  it('renders a real 0 as 0 collected — the suite that discriminates nothing', () => {
    const el = mount({ gateDeclared: true, exitCode: 0, collected: 0, commandsRun: 1, output: '', truncated: false }).nativeElement as HTMLElement;
    expect((el.querySelector('[data-test="gate-collected"]') as HTMLElement).textContent).toContain('0 collected');
  });

  it('shows the exit code as the score, and no rating or percentage anywhere', () => {
    const el = mount({ gateDeclared: true, exitCode: 101, collected: 23, commandsRun: 1, output: '', truncated: false }).nativeElement as HTMLElement;
    expect((el.querySelector('[data-test="gate-exit"]') as HTMLElement).textContent).toContain('exit 101');
    expect(el.textContent).not.toMatch(/%|score:|rating/i);
  });

  it('says plainly when no gate is declared, instead of reporting exit 0 as a pass', () => {
    const el = mount({ gateDeclared: false, exitCode: 0, collected: null, commandsRun: 0, output: '', truncated: false }).nativeElement as HTMLElement;
    expect(el.textContent).toContain('No gate');
    expect(el.textContent).not.toContain('pass');
  });

  it('emits run on a real click of the run control, once', () => {
    const f = TestBed.createComponent(GateStripComponent);
    f.componentInstance.commands = ['npm run check'];
    let runs = 0;
    f.componentInstance.run.subscribe(() => (runs += 1));
    f.detectChanges();
    click((f.nativeElement as HTMLElement).querySelector('[data-test="run-gate"]')!);
    expect(runs).toBe(1);
  });

  it('does not offer to run while a run is in flight', () => {
    const f = mount(null, true);
    const btn = (f.nativeElement as HTMLElement).querySelector('[data-test="run-gate"]') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });
});

describe('the Arena tab is ADDED, never a replacement', () => {
  // CORRECTED mid-round. This first asserted on `visibleConsoleTabs` itself, which made five
  // SHIPPED specs fail (console-tabs-logic.spec.ts and checks-tab-logic.spec.ts assert its exact
  // arrays). Editing those was not an option — so the arena is appended by a separate function and
  // `visibleConsoleTabs` answers exactly what it answered before. Both are asserted here.
  it('keeps every tab that exists today, byte for byte, and appends arena beside them', () => {
    const local = { executorConfig: '{"mode":"local-small"}', initiativeStatus: 'planning' } as never;
    expect(visibleConsoleTabs(local, true, true, true)).toEqual(['raw', 'plan', 'checks', 'tasks', 'diff']);
    expect(consoleTabsWithArena(local, true, true, true)).toEqual([
      'raw', 'plan', 'checks', 'tasks', 'diff', 'arena',
    ]);
    const commercial = { executorConfig: null } as never;
    expect(visibleConsoleTabs(commercial, false, false, false)).toEqual(['raw', 'plan', 'diff']);
    expect(consoleTabsWithArena(commercial, false, false, false)).toEqual(['raw', 'plan', 'diff', 'arena']);
  });

  it('accepts arena as a persisted tab, and still defaults to raw for junk', () => {
    expect(paneTabOf({ a: 'arena' as PaneTab }, 'a')).toBe('arena');
    expect(paneTabOf({ a: 'nonsense' as PaneTab }, 'a')).toBe(DEFAULT_PANE_TAB);
  });
});

import { ArenaPanelComponent } from './arena-panel.component';

describe('ArenaPanelComponent — the Arena tab’s own body', () => {
  const PHASES = [
    { phaseId: '03-tables', phaseTitle: 'Migration 026', effectiveTestCommands: ['cargo test --lib db::'] },
    { phaseId: '04-leases', phaseTitle: 'The lease pool', effectiveTestCommands: ['cargo test --lib leases::'] },
    { phaseId: '08-polish', phaseTitle: 'Plan-tab polish', effectiveTestCommands: [] },
  ];

  const mount = (phases = PHASES) => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
    const f = TestBed.createComponent(ArenaPanelComponent);
    f.componentInstance.initiativeId = 'init-1';
    f.componentInstance.phases = phases;
    f.detectChanges();
    return f;
  };

  it('offers one arena row per phase, in the order the phases came in', () => {
    const el = mount().nativeElement as HTMLElement;
    const rows = el.querySelectorAll('johnny-arena-row');
    expect(rows).toHaveLength(3);
    expect(el.textContent).toContain('Migration 026');
    expect(el.textContent?.indexOf('Migration 026')).toBeLessThan(el.textContent!.indexOf('The lease pool'));
  });

  it('carries the disabled row’s reason for a phase with no gate', () => {
    const el = mount().nativeElement as HTMLElement;
    expect(el.textContent).toContain(NO_GATE_REASON);
    expect(el.querySelectorAll('[data-test="enter-arena"]')).toHaveLength(2);
    expect(el.querySelectorAll('[data-test="enter-arena-disabled"]')).toHaveLength(1);
  });

  it('says where the documents are, instead of cloning the document browser', () => {
    const el = mount().nativeElement as HTMLElement;
    expect(el.textContent).toMatch(/Plan tab/);
    expect(el.querySelector('johnny-markdown-view')).toBeNull();
  });

  it('links to the workflow list', () => {
    const el = mount().nativeElement as HTMLElement;
    expect((el.querySelector('[data-test="all-workflows"]') as HTMLAnchorElement).getAttribute('href')).toBe('/workflows');
  });

  it('says so plainly when the initiative has no phases yet', () => {
    const el = mount([]).nativeElement as HTMLElement;
    expect(el.textContent).toMatch(/no phases/i);
  });
});

// ── The two console fixes, each with a test that fails when the fix is reverted ────────────────
//
// Round 2 shipped both of these held by a screenshot and nothing else: QA reverted each one and the
// full suite stayed green, 60 files / 1030 passed, twice. These are the guards. Each was proven red
// by reverting the fix it covers — the failures are in the round-3 report.
import { tabNeedsPlanRun } from '../terminal/console-tabs-logic';
import { ArenaPanelComponent } from './arena-panel.component';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

describe('the Arena tab needs the plan loaded, exactly as the Plan tab does', () => {
  it('names both tabs, and only those two', () => {
    expect(tabNeedsPlanRun('plan')).toBe(true);
    expect(tabNeedsPlanRun('arena')).toBe(true);
    for (const tab of ['raw', 'diff', 'checks', 'tasks'] as PaneTab[]) {
      expect(tabNeedsPlanRun(tab), `${tab} should not trigger a plan load`).toBe(false);
    }
  });

  it('is what the console actually calls — the effect does not re-spell the condition', () => {
    // MEASURED defect this guards: the Arena tab read `planRun`, which only the PLAN tab's effect
    // ever loaded, so the tab said "This initiative has no phases yet" on an initiative with three
    // until the Plan tab had been opened once
    // (.johnnyone/reports/arena-screens/before2-console-arena-tab-no-phases.png).
    const ts = readFileSync(join(process.cwd(), 'web/src/app/pages/terminal/terminal.page.ts'), 'utf8');
    expect(ts).toContain('tabNeedsPlanRun(this.activeTab())');
    // …and the condition is not ALSO written out by hand next to it, which is how two spellings
    // drift apart.
    expect(ts).not.toMatch(/activeTab\(\) === 'plan'\s*\|\|\s*this\.activeTab\(\) === 'arena'/);
  });
});

describe('the Arena tab panel is its own scroller', () => {
  it('computes to a scrollable, full-height host', () => {
    // The console's `.terminal-surface` is `height: 100%; overflow: hidden` — it hands each tab a
    // box and expects the TAB to scroll inside it. Without this the third phase card was cut off
    // with no way to reach it (before3-console-arena-tab-clipped.png). jsdom applies a component's
    // own inline styles, so this is a COMPUTED value, not a source grep.
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
    const f = TestBed.createComponent(ArenaPanelComponent);
    f.detectChanges();
    const cs = getComputedStyle(f.nativeElement as HTMLElement);
    expect(cs.overflowY, 'the panel must scroll inside the console surface').toBe('auto');
    expect(cs.height).toBe('100%');
    expect(cs.display).toBe('block');
  });

  it('keeps the bottom padding clear of the gesture bar', () => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
    const f = TestBed.createComponent(ArenaPanelComponent);
    f.detectChanges();
    const panel = (f.nativeElement as HTMLElement).querySelector('.arena-panel') as HTMLElement;
    expect(getComputedStyle(panel).paddingBottom).toContain('safe-area-inset-bottom');
  });
});

// ── Tap targets, measured on mounted components rather than grepped ───────────────────────────
//
// REPLACES `expect(ARENA_CSS).toMatch(/min-height:\s*40px/)` in `arena-phone.spec.ts` — the same
// substring shape I removed for `overflow-wrap` in round 3, which survived in the one assertion
// that was not rewritten. It passed while four real controls were 20px: `score-phase` is a
// `<select class="a-input">` and the rule enumerated `textarea.a-input, input.a-input` only, and
// the three `family-*` radios carried no class at all. iOS asks 44px, Android 48px.
import { ArenaSetupPage } from './arena-setup.page';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { ArenaApiService } from './arena-api.service';
import { ActivatedRoute, convertToParamMap } from '@angular/router';
import { of } from 'rxjs';

const TAP_MIN = 40;

describe('every control the arena shows is big enough to hit', () => {
  const api = {
    getAgentPlan: () =>
      of({
        plan: { id: 'p1', runType: 'development', workspacePath: '/w/johnnyone', currentPhaseId: '04-leases' },
        phases: [
          { id: 'ph1', phaseId: '04-leases', phaseTitle: 'The lease pool', effectiveTestCommands: ['cargo test'] },
          { id: 'ph2', phaseId: '08-polish', phaseTitle: 'Polish', effectiveTestCommands: [] },
        ],
        tasks: [],
        events: [],
      }),
    runAgentPlanTestGate: () => of({ gateDeclared: true, exitCode: 0, collected: 1, commandsRun: 1, output: '', truncated: false }),
    updateAgentPlanTestCommands: () => of({ plan: {}, phases: [], tasks: [], events: [] }),
    repoIdentity: () => of('github.com/lokalcamp/johnnyone'),
    workflows: () => of([]),
    workflowSearchState: () =>
      of({ repo: 'r', phaseType: 'dev-phase', family: 'glm-5.3-flash', consecutiveRejections: 0, halted: false, strikeLimit: 3, recentRejections: [], championWorkflowId: null }),
    createWorkflowVersion: () => of({}),
    startWorkflowRun: () => of({}),
  };

  const mountSetup = async (step: string) => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([]),
        { provide: JohnnyApiService, useValue: api },
        { provide: ArenaApiService, useValue: api },
        {
          provide: ActivatedRoute,
          useValue: {
            snapshot: {
              paramMap: convertToParamMap({ id: 'p1' }),
              queryParamMap: convertToParamMap({ phaseId: '04-leases', step }),
            },
          },
        },
      ],
    });
    const f = TestBed.createComponent(ArenaSetupPage);
    f.detectChanges();
    for (let i = 0; i < 60; i += 1) await Promise.resolve();
    f.detectChanges();
    return f;
  };

  /**
   * Every element a finger can hit, measured from the stylesheet the component actually ships.
   *
   * A radio or checkbox BOX is deliberately small — 22px, the platform convention — and what has to
   * be 44px is the label row around it, which is asserted separately below. Everything else is in.
   */
  const tappable = (root: HTMLElement) =>
    [...root.querySelectorAll<HTMLElement>('button, a[href], select, textarea, input, [role="button"]')]
      .filter((el) => !(el instanceof HTMLInputElement && (el.type === 'radio' || el.type === 'checkbox')));

  const px = (v: string) => (v.endsWith('px') ? Number.parseFloat(v) : NaN);

  it('the seats step: the family radio ROWS are 44px and every other control clears 40px', async () => {
    const f = await mountSetup('seats');
    const labels = [...(f.nativeElement as HTMLElement).querySelectorAll<HTMLElement>('label')]
      .filter((l) => l.querySelector('input[type="radio"]'));
    expect(labels.length, 'the family radios are not on screen').toBeGreaterThanOrEqual(3);
    for (const label of labels) {
      const h = px(getComputedStyle(label).minHeight);
      expect(h >= 44, `a family radio row is ${h}px`).toBe(true);
    }
    const els = tappable(f.nativeElement as HTMLElement);
    expect(els.length).toBeGreaterThan(3);
    for (const el of els) {
      const cs = getComputedStyle(el);
      const h = px(cs.minHeight);
      expect(
        Number.isFinite(h) && h >= TAP_MIN,
        `${el.tagName.toLowerCase()}[data-test=${el.getAttribute('data-test') ?? '—'}] has min-height ${cs.minHeight}`,
      ).toBe(true);
    }
  });

  it('the prompt step: the slot textareas and the start control clear 40px', async () => {
    const f = await mountSetup('prompt');
    for (const el of tappable(f.nativeElement as HTMLElement)) {
      const h = px(getComputedStyle(el).minHeight);
      expect(h >= TAP_MIN, `${el.tagName.toLowerCase()} min-height ${h}`).toBe(true);
    }
  });

  it('the gate step, including the phase select when one is shown', async () => {
    const f = await mountSetup('gate');
    for (const el of tappable(f.nativeElement as HTMLElement)) {
      const h = px(getComputedStyle(el).minHeight);
      expect(h >= TAP_MIN, `${el.tagName.toLowerCase()} min-height ${h}`).toBe(true);
    }
  });

  it('the arena row’s controls, enabled and disabled alike', () => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({ providers: [provideRouter([])] });
    for (const phase of [
      { phaseId: '04-leases', effectiveTestCommands: ['cargo test'] },
      { phaseId: '08-polish', effectiveTestCommands: [] },
    ]) {
      const f = TestBed.createComponent(ArenaRowComponent);
      f.componentInstance.initiativeId = 'init-1';
      f.componentInstance.phase = phase as never;
      f.detectChanges();
      for (const el of tappable(f.nativeElement as HTMLElement)) {
        const h = px(getComputedStyle(el).minHeight);
        expect(h >= TAP_MIN, `${el.getAttribute('data-test')} min-height ${h}`).toBe(true);
      }
    }
  });
});
