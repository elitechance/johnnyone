/**
 * Plan agent in tmux: its shell page is view-only (review round 2: F-2-24, F-2-26, F-2-28).
 * `TerminalPage` cannot be TestBed-compiled under the plugin-less web vitest, so — like
 * `shell-rename-template.spec.ts` — this reads the template as TEXT.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const HTML = readFileSync(resolve(__dirname, 'terminal.page.html'), 'utf8');
const TS = readFileSync(resolve(__dirname, 'terminal.page.ts'), 'utf8');

function topbar(): string {
  const start = HTML.indexOf('<header class="plain-shell-topbar"');
  expect(start).toBeGreaterThan(-1);
  return HTML.slice(start, HTML.indexOf('</header>', start) + '</header>'.length);
}

describe('plan-agent shell page (template)', () => {
  it('derives view-only from the engine flag via isPlanAgentShell', () => {
    expect(HTML).toContain('@let topViewOnly = isPlanAgentShell(sh);');
    expect(topbar()).toContain('[class.pa-bar]="topViewOnly"');
    expect(HTML).toContain('@let shViewOnly = isPlanAgentShell(sh);');
  });

  it('hides close/detach, rename and the Watchdog view for a plan agent', () => {
    const bar = topbar();
    const close = bar.indexOf('class="ps-detach"');
    expect(bar.lastIndexOf('@if (sh && !topViewOnly) {', close)).toBeGreaterThan(-1);
    expect(bar.slice(bar.lastIndexOf('@if', close), close)).toContain('!topViewOnly');
    const wd = bar.indexOf("(click)=\"setShellView('watchdog')\"");
    expect(bar.slice(bar.lastIndexOf('@if', wd), wd)).toContain('@if (!topViewOnly) {');
    // A hand-typed /watchdog URL falls through to the raw view.
    expect(HTML).toContain("} @else if (sh && shellView() === 'watchdog' && !shViewOnly) {");
  });

  it('puts the view-only note and the terminal error IN the top bar flow, not over the pane', () => {
    const bar = topbar();
    expect(bar).toContain('class="ps-view-only" role="note"');
    expect(bar).toContain('class="ps-term-error" role="alert"');
    expect(HTML).not.toContain('pa-view-only');
    expect(HTML).not.toContain('pa-terminal-error');
  });

  it('disables input on the raw screen and hides the transcript composer', () => {
    expect(HTML).toContain('[disabled]="shViewOnly"');
    expect(HTML).toContain('[showInput]="!shViewOnly"');
    expect(HTML).toContain('@if (!shViewOnly) {\n          <form class="st-composer"');
  });

  it('clears a stale terminal error when a plan agent shell opens (keyed on the id)', () => {
    expect(TS).toContain('if (id === this.paErrorClearedFor) return;');
    expect(TS).toContain('if (isPlanAgentShell(s)) untracked(() => this.terminalError.set(null));');
  });
});
