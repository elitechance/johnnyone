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
    expect(HTML).toContain('@let topViewOnly = isReadOnlyShell(sh);');
    expect(topbar()).toContain('[class.pa-bar]="topViewOnly || !!jobEndedNotice(terminalError()) || !!(sh && paneNotices()[sh.id])"');
    expect(HTML).toContain('@let shViewOnly = isReadOnlyShell(sh);');
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

  it('a /watchdog URL on a plan agent highlights Raw (the view it renders)', () => {
    const bar = topbar();
    expect(bar).toContain('@let shownView = visibleShellView(shellView(), topViewOnly);');
    expect(bar).toContain(`[class.on]="shownView === 'raw'"`);
    expect(bar).toContain(`[class.on]="shownView === 'transcript'"`);
    expect(bar).not.toContain(`[class.on]="shellView() === 'raw'"`);
  });

  it('shows "job has ended, transcript only" instead of a blank Raw screen', () => {
    const bar = topbar();
    expect(bar).toContain('@let topJobEnded = (sh ? paneNotices()[sh.id] : null) || jobEndedNotice(terminalError());');
    expect(bar).toContain('class="ps-view-only ps-job-ended" role="note"');
    const ended = HTML.indexOf("} @else if (sh && rawPaneView(sh.id, !!shScreen, paneNotices(), terminalError(), shViewOnly && !isPlanAgentShell(sh)).kind === 'ended') {");
    const raw = HTML.indexOf('<johnny-terminal-screen\n          class="terminal-surface plain-shell-screen mirror"');
    expect(ended).toBeGreaterThan(-1);
    expect(raw).toBeGreaterThan(ended); // checked BEFORE the raw screen renders
    expect(HTML.slice(ended, raw)).toContain("(click)=\"setShellView('transcript')\"");
  });

  it('shows a refused close as the engine sentence, without "Failed to kill terminal: Error:"', () => {
    expect(TS).not.toContain('`Failed to kill terminal: ${String(err)}`');
    expect(TS).not.toContain('`Failed to close terminal: ${String(err)}`');
    expect(TS.match(/this\.terminalError\.set\(closeRefusalText\(err\)\)/g)?.length).toBe(2);
  });

  it('clears a stale terminal error when a plan agent shell opens (keyed on the id)', () => {
    expect(TS).toContain('if (id === this.paErrorClearedFor) return;');
    expect(TS).toContain('if (isPlanAgentShell(s)) untracked(() => this.terminalError.set(null));');
  });
});
