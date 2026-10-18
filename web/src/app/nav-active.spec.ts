import { describe, it, expect } from 'vitest';
import { NAV_ITEMS, litNavItemIds, navItemIsActive, navLinkQueryParams } from './nav-items';

/**
 * The lit-nav half of the nav-seam fix. Every route listed here was measured in real Chrome at
 * 400x860 against the built bundle before this spec existed; `arena-nav-harness.mjs` re-measures
 * the same claim through `routerLinkActive`'s real resolution, and this spec is the fast guard on
 * the model it resolves from.
 */
describe('which nav destination a URL lights', () => {
  // The defect, by route: these three lit NOTHING. `nav.botnav a.b.on` was the empty set on all
  // three, measured in Chrome at 400x860. The 3-step setup wizard at /initiatives/:id/arena was NOT
  // among them — it already lit Work, correctly, and still does.
  const ARENA = ['/workflows', '/workflows/w2', '/workflows/w2/arena'];

  it('lights exactly Arena on every top-level arena route', () => {
    for (const url of ARENA) {
      expect(litNavItemIds(url), url).toEqual(['arena']);
    }
  });

  it('lights exactly one destination on the arena routes that carry query state', () => {
    expect(litNavItemIds('/workflows/w2/arena?from=%2Finitiatives')).toEqual(['arena']);
    // The wizard belongs to one INITIATIVE: its route is under /initiatives/:id and its back link
    // returns to that initiative's console, so Work is the destination that names where you are.
    expect(litNavItemIds('/initiatives/p1/arena?phaseId=04-leases&step=seats')).toEqual(['work']);
    expect(litNavItemIds('/initiatives?initiativeId=p1&tab=arena')).toEqual(['work']);
  });

  it('still lights exactly one destination on each nav path', () => {
    for (const item of NAV_ITEMS) {
      expect(litNavItemIds(item.path), item.path).toEqual([item.id]);
    }
  });

  it('keeps the destinations the rail already lit', () => {
    expect(litNavItemIds('/settings/prompts')).toEqual(['settings']);
    expect(litNavItemIds('/briefing/new')).toEqual(['work']);
    expect(litNavItemIds('/workflows')).toEqual(['arena']);
    expect(litNavItemIds('/terminal')).toEqual(['work']);
    expect(litNavItemIds('/initiatives/p1/validation')).toEqual(['work']);
    expect(litNavItemIds('/shells/s1/raw')).toEqual(['shells']);
  });

  it('never lights two destinations at once, so no two `owns` lists overlap', () => {
    const urls = [
      ...ARENA, '/', '/login', '/integration', '/developer', '/install', '/files', '/shells',
      '/settings', '/settings/prompts', '/initiatives', '/briefing/new', '/terminal',
    ];
    for (const url of urls) {
      expect(litNavItemIds(url).length, url).toBeLessThanOrEqual(1);
    }
  });

  it('matches on a segment boundary, not a bare string prefix', () => {
    const files = NAV_ITEMS.find((n) => n.id === 'files')!;
    expect(navItemIsActive(files, '/files')).toBe(true);
    expect(navItemIsActive(files, '/files/a/b')).toBe(true);
    expect(navItemIsActive(files, '/filesystem')).toBe(false);
    const work = NAV_ITEMS.find((n) => n.id === 'work')!;
    expect(navItemIsActive(work, '/workflowsomething')).toBe(false);
  });

  it('lights nothing on the routes that are outside every destination', () => {
    // Unchanged behaviour, pinned so a future `owns` entry cannot quietly claim them.
    for (const url of ['/login', '/integration', '/developer', '/install']) {
      expect(litNavItemIds(url), url).toEqual([]);
    }
  });

  it('tolerates a trailing slash and a bare root', () => {
    expect(litNavItemIds('/workflows/')).toEqual(['arena']);
    expect(litNavItemIds('/')).toEqual([]);
  });
});

describe('what the global nav entry carries', () => {
  const byId = Object.fromEntries(NAV_ITEMS.map((n) => [n.id, n]));

  it('carries the console URL into the arena, so the way IN is not a way to lose your place', () => {
    expect(navLinkQueryParams(byId['arena'], '/initiatives?initiativeId=p2&tab=arena')).toEqual({
      from: '/initiatives?initiativeId=p2&tab=arena',
    });
    expect(navLinkQueryParams(byId['arena'], '/initiatives/p2/arena?phaseId=04-leases')).toEqual({
      from: '/initiatives/p2/arena?phaseId=04-leases',
    });
  });

  it('carries nothing from outside the area the arena belongs to', () => {
    for (const url of ['/files', '/shells', '/settings', '/workflows/w2', '/']) {
      expect(navLinkQueryParams(byId['arena'], url), url).toEqual({});
    }
  });

  it('carries nothing on any other destination — Files and Shells are separate areas', () => {
    for (const id of ['work', 'files', 'shells', 'settings']) {
      expect(navLinkQueryParams(byId[id], '/initiatives?initiativeId=p2&tab=arena'), id).toEqual({});
    }
  });
});
