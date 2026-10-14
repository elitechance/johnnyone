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
