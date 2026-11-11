/**
 * Regression guard for the watchdog On/Off switch.
 *
 * Commit 6940b2c restyled the watchdog strip and, in doing so, turned the On/Off
 * control from a `(click)="toggleWatchdogEnabled()"` button into a bare
 * `<label class="wd-toggle-ctrl">` — a label with no input and no handler, i.e.
 * a switch that renders but does nothing. The same commit dropped the
 * `@if (wdEnabled())` guard around the Disarm button, so on an *off* watchdog
 * the always-visible "Disarm" button actually armed it.
 *
 * `toggleWatchdogEnabled()` itself was never broken, so no component instance is
 * needed to prove the fix — the template is the bug and the template is the fix.
 * These specs read `terminal.page.html` as text, the same way the other
 * template-wiring specs in this folder load their subjects.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// Same loader the sibling template specs use: vitest serves modules over http, so
// `import.meta.url` is not a file: URL — derive the directory from __dirname instead.
const here = resolve(__dirname);

const TEMPLATE = readFileSync(resolve(here, 'terminal.page.html'), 'utf8');

/** The opening tag of the element carrying the `wd-toggle-ctrl` class. */
function toggleOpenTag(template: string): string {
  const idx = template.indexOf('wd-toggle-ctrl');
  expect(idx, 'the wd-toggle-ctrl control exists at all').toBeGreaterThan(-1);
  const open = template.lastIndexOf('<', idx);
  const close = template.indexOf('>', idx);
  return template.slice(open, close + 1);
}

/** The condition of the nearest `@if (` at or before `before`, e.g. `wdEnabled()`. */
function nearestIfConditionBefore(template: string, before: number): string {
  const idx = template.lastIndexOf('@if (', before);
  expect(idx, 'there is an @if block before this point').toBeGreaterThan(-1);
  const open = idx + '@if ('.length;
  // Balance the parens — the condition itself calls functions, so the first ')'
  // closes `wdEnabled(` rather than the `@if (`.
  let depth = 1;
  let close = open;
  for (; close < template.length; close++) {
    if (template[close] === '(') depth++;
    else if (template[close] === ')' && --depth === 0) break;
  }
  expect(depth, 'the @if condition is closed').toBe(0);
  return template.slice(open, close).trim();
}

describe('watchdog On/Off switch is a real control', () => {
  it('renders the toggle as a <button>', () => {
    expect(toggleOpenTag(TEMPLATE)).toMatch(/^<button\b/);
  });

  it('wires the toggle to toggleWatchdogEnabled()', () => {
    expect(toggleOpenTag(TEMPLATE)).toContain('(click)="toggleWatchdogEnabled()"');
  });

  it('exposes the toggle as a switch with an aria-checked state', () => {
    const tag = toggleOpenTag(TEMPLATE);
    expect(tag).toContain('role="switch"');
    expect(tag).toContain('[attr.aria-checked]="wdEnabled()"');
  });

  it('disables the toggle while a save is in flight', () => {
    expect(toggleOpenTag(TEMPLATE)).toContain('[disabled]="wdSaving()"');
  });

  it('keeps the track + label spans as the button children', () => {
    const start = TEMPLATE.indexOf('wd-toggle-ctrl');
    const body = TEMPLATE.slice(start, TEMPLATE.indexOf('</button>', start));
    expect(body).toContain('<span class="wd-toggle-track"');
    expect(body).toContain('[class.on]="wdEnabled()"');
    expect(body).toContain('<span class="wd-toggle-label">');
    expect(body).toContain("wdEnabled() ? 'On' : 'Off'");
  });

  it('leaves no <label class="wd-toggle-ctrl"> behind', () => {
    expect(TEMPLATE).not.toContain('<label class="wd-toggle-ctrl"');
  });
});

describe('Disarm button only renders for an armed watchdog', () => {
  const disarm = TEMPLATE.indexOf('>Disarm</button>');

  it('still has a Disarm button', () => {
    expect(disarm).toBeGreaterThan(-1);
  });

  it('sits inside an @if (wdEnabled()) block', () => {
    expect(nearestIfConditionBefore(TEMPLATE, disarm)).toBe('wdEnabled()');
  });

  it('is the only Disarm button, so the guard cannot be bypassed', () => {
    expect(TEMPLATE.indexOf('>Disarm</button>', disarm + 1)).toBe(-1);
  });
});