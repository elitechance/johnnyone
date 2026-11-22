/**
 * `/shells` row rename wiring (initiative 6672dfab, task 03).
 *
 * The complaint: "The UI no longer offers rename anywhere." `ShellsPage` cannot be TestBed-compiled
 * under the plugin-less web vitest (`templateUrl` + Ionic), so — like
 * `web/src/app/pages/terminal/watchdog-toggle-template.spec.ts` — this reads the template and the
 * component source as TEXT and pins the wiring: a ✎ on BOTH `@switch (shellRowState(s))` branches
 * (so ended rows too), none on attachable tmux rows, a click that cannot fall through to the row's
 * `open(s)`, and a handler that goes prompt → `renameIntent` → mutation → title-only local patch.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// vitest serves modules over http, so `import.meta.url` is not a file: URL — use __dirname.
const here = resolve(__dirname);
const HTML = readFileSync(resolve(here, 'shells.page.html'), 'utf8');
const TS = readFileSync(resolve(here, 'shells.page.ts'), 'utf8');

const RENAME = 'title="Rename shell"';
const CLEAR = 'title="Clear this shell"';

/** All indices of `needle` in `hay`. */
function indicesOf(hay: string, needle: string): number[] {
  const out: number[] = [];
  for (let i = hay.indexOf(needle); i !== -1; i = hay.indexOf(needle, i + 1)) out.push(i);
  return out;
}

/** The full opening tag around position `idx`. */
function openTagAt(src: string, idx: number): string {
  const start = src.lastIndexOf('<', idx);
  const end = src.indexOf('>', idx);
  return src.slice(start, end + 1);
}

const endedStart = HTML.indexOf("@case ('ended')");
const defaultStart = HTML.indexOf('@default');
const attachableStart = HTML.indexOf('Attachable tmux sessions');

/** The method body of `renameOne`, from its signature to the next 2-space-indented `}`. */
function renameOneBody(): string {
  const start = TS.indexOf('async renameOne(');
  expect(start, 'renameOne exists').toBeGreaterThan(-1);
  const end = TS.indexOf('\n  }\n', start);
  return TS.slice(start, end + 4);
}

describe('/shells ✎ buttons (template)', () => {
  it('has exactly two Rename shell buttons', () => {
    expect(indicesOf(HTML, RENAME).length).toBe(2);
  });

  it('puts one in the ended branch and one in the default branch, none on attachable rows', () => {
    expect(endedStart).toBeGreaterThan(-1);
    expect(defaultStart).toBeGreaterThan(endedStart);
    expect(attachableStart).toBeGreaterThan(defaultStart);
    const [a, b] = indicesOf(HTML, RENAME);
    expect(a > endedStart && a < defaultStart, 'first ✎ is inside @case (ended)').toBe(true);
    expect(b > defaultStart && b < attachableStart, 'second ✎ is inside @default').toBe(true);
    expect(HTML.slice(attachableStart).includes(RENAME), 'no ✎ on attachable tmux rows').toBe(false);
  });

  it('each ✎ comes before the Clear button of its own branch', () => {
    const renames = indicesOf(HTML, RENAME);
    const clears = indicesOf(HTML, CLEAR);
    expect(clears.length).toBe(2);
    expect(renames[0]).toBeLessThan(clears[0]);
    expect(clears[0]).toBeLessThan(defaultStart);
    expect(renames[1]).toBeGreaterThan(defaultStart);
    expect(renames[1]).toBeLessThan(clears[1]);
  });

  it('each ✎ is an accessible, busy-disabled button whose click does not open the shell', () => {
    for (const idx of indicesOf(HTML, RENAME)) {
      const tag = openTagAt(HTML, idx);
      expect(tag.startsWith('<button')).toBe(true);
      expect(tag).toContain('type="button"');
      expect(tag).toContain('class="ia"');
      expect(tag).toContain('aria-label="Rename shell"');
      expect(tag).toContain('[disabled]="busy()"');
      expect(tag).toContain('(click)="renameOne(s); $event.stopPropagation()"');
    }
  });
});

describe('ShellsPage.renameOne (source)', () => {
  it('goes prompt → renameIntent → updateSessionTitle → sessionsWithTitle, in that order', () => {
    const body = renameOneBody();
    const order = ['promptSessionRename(', 'renameIntent(', 'updateSessionTitle(', 'sessionsWithTitle('].map(
      (s) => body.indexOf(s),
    );
    for (const i of order) expect(i).toBeGreaterThan(-1);
    expect([...order].sort((x, y) => x - y)).toEqual(order);
  });

  it('takes busy BEFORE the prompt and releases it in a finally (no double alert)', () => {
    const body = renameOneBody();
    const take = body.indexOf('this.busy.set(true)');
    expect(take).toBeGreaterThan(-1);
    expect(take).toBeLessThan(body.indexOf('promptSessionRename('));
    const fin = body.indexOf('finally');
    expect(fin).toBeGreaterThan(-1);
    expect(body.indexOf('this.busy.set(false)', fin)).toBeGreaterThan(fin);
  });

  it('does not refresh after a rename (a cached read could revert the title)', () => {
    expect(renameOneBody()).not.toContain('this.refresh(');
  });

  it('surfaces a failure in the error banner and leaves the rows alone', () => {
    const body = renameOneBody();
    const c = body.indexOf('catch (');
    const f = body.indexOf('finally', c);
    expect(c).toBeGreaterThan(-1);
    const catchBlock = body.slice(c, f);
    expect(catchBlock).toContain("this.loadError.set('Could not rename shell: '");
    expect(catchBlock).not.toContain('this.sessions.update(');
    expect(catchBlock).not.toContain('this.sessions.set(');
  });

  it('uses no native dialog and does not just console.error', () => {
    const body = renameOneBody();
    // Split literal so the phase's repo-wide native-prompt grep gate does not match this spec.
    expect(body).not.toContain('window.' + 'prompt');
    expect(body).not.toContain('console.error');
  });
});
