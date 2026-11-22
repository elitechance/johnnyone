/**
 * Plain-shell top bar rename (initiative 6672dfab, task 04).
 *
 * The complaint: an open shell shows its name in the top bar (`.ps-name`) but it cannot be renamed,
 * and the only rename code (an `on…Renamed` handler) was uncalled and used a blocking native prompt.
 * `TerminalPage` cannot be TestBed-compiled under the plugin-less web vitest, so — like
 * `watchdog-toggle-template.spec.ts` — this reads the template, source and stylesheet as TEXT.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// vitest serves modules over http, so `import.meta.url` is not a file: URL — use __dirname.
const here = resolve(__dirname);
const HTML = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');
const TS = readFileSync(resolve(here, 'terminal.page.ts'), 'utf8');
const SCSS = readFileSync(resolve(here, 'terminal.page.scss'), 'utf8');

/** The `<header class="plain-shell-topbar">…</header>` block. */
function topbar(): string {
  const start = HTML.indexOf('<header class="plain-shell-topbar">');
  expect(start, 'the plain-shell top bar exists').toBeGreaterThan(-1);
  return HTML.slice(start, HTML.indexOf('</header>', start) + '</header>'.length);
}

/** `renameCurrentShell`'s body, from its signature to the next 2-space-indented `}`. */
function methodBody(sig: string): string {
  const start = TS.indexOf(sig);
  expect(start, `${sig} exists`).toBeGreaterThan(-1);
  return TS.slice(start, TS.indexOf('\n  }\n', start) + 4);
}

/** The body of a top-level SCSS rule `selector {` inside `.plain-shell-topbar` (brace-balanced). */
function scssRule(selector: string): string {
  const bar = SCSS.indexOf('.plain-shell-topbar {');
  expect(bar).toBeGreaterThan(-1);
  const start = SCSS.indexOf(`${selector} {`, bar);
  expect(start, `${selector} rule exists inside .plain-shell-topbar`).toBeGreaterThan(-1);
  let depth = 0;
  for (let i = SCSS.indexOf('{', start); i < SCSS.length; i++) {
    if (SCSS[i] === '{') depth++;
    else if (SCSS[i] === '}' && --depth === 0) return SCSS.slice(start, i + 1);
  }
  return SCSS.slice(start);
}

describe('top bar ✎ (template)', () => {
  it('sits inside @if (sh) directly after the ps-name span and before the provider ps-dim span', () => {
    const bar = topbar();
    const nameEnd = bar.indexOf('</span>', bar.indexOf('class="ps-name"')) + '</span>'.length;
    const after = bar.slice(nameEnd).trimStart();
    expect(after.startsWith('@if (sh) {'), 'the next thing after .ps-name is the @if (sh) guard').toBe(true);
    const afterGuard = after.slice('@if (sh) {'.length).trimStart();
    expect(afterGuard.startsWith('<button type="button" class="ps-rename"'), 'then the ✎ button').toBe(true);
    const renameAt = bar.indexOf('class="ps-rename"');
    expect(renameAt).toBeLessThan(bar.indexOf('@if (sh?.provider)'));
    expect(renameAt).toBeLessThan(bar.indexOf('class="ps-dim"'));
  });

  it('is an accessible button, disabled while a rename is open, wired to renameCurrentShell', () => {
    const bar = topbar();
    const at = bar.indexOf('class="ps-rename"');
    const tag = bar.slice(bar.lastIndexOf('<', at), bar.indexOf('>', at) + 1);
    expect(tag).toContain('type="button"');
    expect(tag).toContain('title="Rename shell"');
    expect(tag).toContain('aria-label="Rename shell"');
    expect(tag).toContain('[disabled]="renamingShell()"');
    expect(tag).toContain('(click)="renameCurrentShell()"');
    expect(HTML.split('title="Rename shell"').length - 1, 'exactly one ✎ in this template').toBe(1);
  });
});

describe('TerminalPage.renameCurrentShell (source)', () => {
  it('the dead native-prompt rename is gone', () => {
    // Split literal so the phase's repo-wide dead-code grep gate does not match this spec.
    expect(TS).not.toContain('onSession' + 'Renamed');
    const native = 'window.' + 'prompt(';
    const call = TS.indexOf('updateSessionTitle(');
    expect(call).toBeGreaterThan(-1);
    expect(TS.slice(Math.max(0, call - 400), call + 400)).not.toContain(native);
    // The only remaining native prompt is the out-of-scope working-directory one.
    const remaining = TS.split(native).slice(1).map((rest) => rest.slice(0, 40));
    expect(remaining.length).toBe(1);
    expect(remaining[0].startsWith("'Working directory'")).toBe(true);
  });

  it('takes the guard BEFORE the prompt and releases it in a finally', () => {
    const body = methodBody('async renameCurrentShell(');
    const take = body.indexOf('this.renamingShell.set(true)');
    expect(take).toBeGreaterThan(-1);
    expect(take).toBeLessThan(body.indexOf('promptSessionRename('));
    const fin = body.indexOf('finally');
    expect(body.indexOf('this.renamingShell.set(false)', fin)).toBeGreaterThan(fin);
  });

  it('goes prompt → renameIntent → updateSessionTitle → upsertSession + currentSession.set', () => {
    const body = methodBody('async renameCurrentShell(');
    const order = [
      'promptSessionRename(',
      'renameIntent(',
      'updateSessionTitle(',
      'this.upsertSession(',
      'this.currentSession.set(',
    ].map((s) => body.indexOf(s));
    for (const i of order) expect(i).toBeGreaterThan(-1);
    expect([...order].sort((x, y) => x - y)).toEqual(order);
  });

  it('a failure is a danger "Rename failed: " toast and changes nothing', () => {
    const body = methodBody('async renameCurrentShell(');
    const c = body.indexOf('catch (');
    const catchBlock = body.slice(c, body.indexOf('finally', c));
    expect(catchBlock).toContain('showShellRenameToast(');
    expect(catchBlock).not.toContain('upsertSession(');
    expect(catchBlock).not.toContain('currentSession.set(');
    const toast = methodBody('private async showShellRenameToast(');
    expect(toast).toContain('Rename failed: ');
    expect(toast).toContain("color: 'danger'");
    expect(toast).toContain("position: 'bottom'");
    expect(TS.split('Rename failed: ').length - 1).toBe(1);
  });
});

describe('top bar ✎ (styles, 360px)', () => {
  it('.ps-name truncates with an ellipsis instead of wrapping the bar', () => {
    const rule = scssRule('.ps-name');
    expect(rule).toContain('text-overflow: ellipsis');
    expect(rule).toContain('min-width: 0');
    expect(rule).toContain('white-space: nowrap');
    expect(rule).toContain('overflow: hidden');
  });

  it('.ps-rename is a fixed-size, non-shrinking icon button', () => {
    const rule = scssRule('.ps-rename');
    expect(rule).toContain('flex: none');
    expect(rule).toContain('width: 22px');
    expect(rule).toContain('[disabled]');
  });
});
