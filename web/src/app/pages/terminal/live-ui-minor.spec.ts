/**
 * Live UI bug 7 (minor):
 *  - the plan agent's Transcript showed a "Watchdog off ⚙" bar; a view-only shell has no watchdog.
 *  - Ionic `loadIcon` threw "Failed to construct 'URL': Invalid base URL": the chat composer's
 *    `send` / `close` glyphs (and /shells' `help-circle-outline`) were never registered with
 *    `addIcons`, so Ionicons tried to load the name as a URL. The Plans chat Send button was blank.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { showsWatchdog } from './pane-notice-logic';

const ROOT = resolve(__dirname, '../../../../..');
const read = (p: string) => readFileSync(resolve(ROOT, p), 'utf8');

/** Glyph names a component registers: `addIcons({ 'a-b': aB, send, ... })`. */
function registered(ts: string): Set<string> {
  const out = new Set<string>();
  for (const m of ts.matchAll(/addIcons\(\s*\{([\s\S]*?)\}\s*\)/g)) {
    for (const k of m[1].matchAll(/'([a-z0-9-]+)'\s*:/g)) out.add(k[1]);
    for (const k of m[1].matchAll(/(?:^|[,{\s])([a-z][A-Za-z0-9]*)\s*(?=,|\s*$)/g)) {
      out.add(k[1].replace(/([A-Z0-9])/g, (c) => `-${c.toLowerCase()}`));
    }
  }
  return out;
}

/** Static `<ion-icon name="...">` names in a template. */
function used(html: string): string[] {
  return [...html.matchAll(/<ion-icon[^>]*?\sname="([^"]+)"/g)].map((m) => m[1]);
}

describe('no watchdog bar on a view-only shell (bug 7)', () => {
  it('plan agents and ended sessions get no chip; a live shell does', () => {
    expect(showsWatchdog({ planAgent: true, status: 'active' })).toBe(false);
    expect(showsWatchdog({ status: 'archived' })).toBe(false);
    expect(showsWatchdog({ status: 'active' })).toBe(true);
    expect(showsWatchdog(null)).toBe(false);
    expect(read('web/src/app/pages/terminal/terminal.page.ts')).toContain('if (!sh || !showsWatchdog(sh)) return null;');
  });
});

describe('every static ion-icon on the touched surfaces is registered (bug 7)', () => {
  const app = registered(read('web/src/app/app.component.ts'));
  const cases: Array<[string, string]> = [
    ['ui/src/components/message-composer/message-composer.component.html', 'ui/src/components/message-composer/message-composer.component.ts'],
    ['ui/src/components/chat-window/chat-window.component.html', 'ui/src/components/chat-window/chat-window.component.ts'],
    ['web/src/app/pages/shells/shells.page.html', 'web/src/app/pages/shells/shells.page.ts'],
    ['web/src/app/pages/plans/plan-detail.page.html', 'web/src/app/pages/plans/plan-detail.page.ts'],
  ];
  for (const [html, ts] of cases) {
    it(html, () => {
      const known = new Set([...app, ...registered(read(ts))]);
      const missing = used(read(html)).filter((n) => !known.has(n));
      expect(missing).toEqual([]);
    });
  }
});
