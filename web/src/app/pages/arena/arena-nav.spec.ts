import { describe, it, expect } from 'vitest';
import {
  arenaBackTarget,
  arenaReturnParams,
  parseReturnUrl,
  readReturnParam,
  returnLabel,
  stripReturnParam,
} from './arena-nav';

describe('parseReturnUrl', () => {
  it('splits a path into routerLink commands', () => {
    expect(parseReturnUrl('/initiatives')).toEqual({ link: ['/initiatives'], queryParams: {} });
    expect(parseReturnUrl('/workflows/w2/arena')).toEqual({
      link: ['/workflows', 'w2', 'arena'],
      queryParams: {},
    });
  });

  it('decodes the query the departure URL carried', () => {
    expect(parseReturnUrl('/initiatives?initiativeId=p1&tab=arena')).toEqual({
      link: ['/initiatives'],
      queryParams: { initiativeId: 'p1', tab: 'arena' },
    });
    expect(parseReturnUrl('/workflows?from=%2Finitiatives%3Ftab%3Darena')).toEqual({
      link: ['/workflows'],
      queryParams: { from: '/initiatives?tab=arena' },
    });
  });

  it('drops a fragment and tolerates a valueless parameter', () => {
    expect(parseReturnUrl('/workflows?x#frag')).toEqual({ link: ['/workflows'], queryParams: { x: '' } });
  });

  // `from` arrives in a query string, so a link someone sends can set it to anything.
  it('REFUSES anything that is not app-relative', () => {
    for (const bad of [
      'https://evil.example/x',
      '//evil.example/x',
      '/\\evil.example/x',
      '/\\/evil.example',
      'javascript:alert(1)',
      'initiatives',
      '/',
      '',
      null,
      undefined,
    ]) {
      expect(parseReturnUrl(bad as string), String(bad)).toBeNull();
    }
  });
});

describe('returnLabel', () => {
  it('names the place, not the direction', () => {
    expect(returnLabel('/initiatives')).toBe('the console');
    expect(returnLabel('/initiatives/p1/arena')).toBe('the arena setup');
    expect(returnLabel('/workflows')).toBe('workflows');
    expect(returnLabel('/workflows/w2')).toBe('the four parts');
    expect(returnLabel('/workflows/w2/arena')).toBe('the arena');
  });

  it('falls back to a generic word rather than a wrong one', () => {
    expect(returnLabel('/developer')).toBe('where you were');
  });
});

describe('arenaBackTarget', () => {
  const FALLBACK = { link: ['/workflows'], label: 'workflows' };

  it('spends ?from= when there is one', () => {
    const t = arenaBackTarget('/initiatives?initiativeId=p1&tab=arena', FALLBACK);
    expect(t.fromHistory).toBe(true);
    expect(t.link).toEqual(['/initiatives']);
    expect(t.queryParams).toEqual({ initiativeId: 'p1', tab: 'arena' });
    expect(t.label).toBe('‹ back to the console');
  });

  it('falls back to the page parent when there is no ?from=', () => {
    const t = arenaBackTarget(null, FALLBACK);
    expect(t.fromHistory).toBe(false);
    expect(t.link).toEqual(['/workflows']);
    expect(t.label).toBe('‹ back to workflows');
  });

  it('falls back when ?from= is hostile, instead of becoming an open redirect', () => {
    const t = arenaBackTarget('//evil.example', FALLBACK);
    expect(t.fromHistory).toBe(false);
    expect(t.link).toEqual(['/workflows']);
  });

  it('uses one phrasing for the gesture on every page', () => {
    const labels = [
      arenaBackTarget('/initiatives', FALLBACK).label,
      arenaBackTarget('/workflows', FALLBACK).label,
      arenaBackTarget('/workflows/w2', FALLBACK).label,
      arenaBackTarget(null, FALLBACK).label,
    ];
    for (const l of labels) expect(l).toMatch(/^‹ back to /);
  });
});

describe('arenaReturnParams', () => {
  it('carries the departure URL, identifying query and all', () => {
    expect(arenaReturnParams('/initiatives?initiativeId=p1&tab=arena')).toEqual({
      from: '/initiatives?initiativeId=p1&tab=arena',
    });
  });

  it('adds nothing when there is no usable current URL', () => {
    expect(arenaReturnParams('')).toEqual({});
    expect(arenaReturnParams('relative')).toEqual({});
  });
});

describe('the from trail does not nest', () => {
  // THE DEFECT, measured in Chrome at phone width on c24c54d, entering by the global Arena item:
  //   len  51  /workflows?from=%2Finitiatives
  //   len  78  /workflows/w1?from=%2Fworkflows%3Ffrom%3D%252Finitiatives
  //   len 119  /workflows/w1/arena?from=%2Fworkflows%2Fw1%3Ffrom%3D%252Fworkflows%253Ffrom%253D…
  // Each hop encoded the whole previous address, `from` and all, into the next one, gaining a level
  // of escaping every time.

  it('strips from, and leaves every other parameter alone', () => {
    expect(stripReturnParam('/workflows?from=%2Finitiatives')).toBe('/workflows');
    expect(stripReturnParam('/initiatives?initiativeId=p2&tab=arena')).toBe('/initiatives?initiativeId=p2&tab=arena');
    expect(stripReturnParam('/initiatives/p2/arena?phaseId=04-leases&from=%2Fx&step=seats')).toBe(
      '/initiatives/p2/arena?phaseId=04-leases&step=seats',
    );
    expect(stripReturnParam('/workflows/w1?from=%2Fa#frag')).toBe('/workflows/w1#frag');
    expect(stripReturnParam('/workflows')).toBe('/workflows');
  });

  it('reads the from a URL already carries', () => {
    expect(readReturnParam('/workflows?from=%2Finitiatives%3FinitiativeId%3Dp2%26tab%3Darena')).toBe(
      '/initiatives?initiativeId=p2&tab=arena',
    );
    expect(readReturnParam('/workflows')).toBeNull();
    expect(readReturnParam('/workflows?x=1')).toBeNull();
  });

  it('forwards an origin unchanged instead of wrapping it', () => {
    const origin = '/initiatives?initiativeId=p2&tab=arena';
    const first = arenaReturnParams(origin);
    expect(first['from']).toBe(origin);
    // Now on /workflows carrying that origin: the next link must forward the SAME value.
    const onWorkflows = `/workflows?from=${encodeURIComponent(origin)}`;
    expect(arenaReturnParams(onWorkflows)['from']).toBe(origin);
    const onDetail = `/workflows/w1?from=${encodeURIComponent(origin)}`;
    expect(arenaReturnParams(onDetail)['from']).toBe(origin);
  });

  it('is bounded over a long wander, which a nesting bug cannot be', () => {
    // Six hops, each one the link a page would render, fed the URL the previous hop produced.
    const origin = '/initiatives?initiativeId=p2&tab=arena';
    const paths = ['/workflows', '/workflows/w1', '/workflows/w1/arena', '/workflows/w2', '/workflows/w2/arena', '/workflows'];
    let url = origin;
    const lengths: number[] = [];
    const queries: string[] = [];
    for (const path of paths) {
      const q = arenaReturnParams(url);
      url = `${path}?from=${encodeURIComponent(q['from'])}`;
      lengths.push(url.length);
      queries.push(url.slice(url.indexOf('?')));
      // The origin is still intact and still parses, hop after hop.
      expect(readReturnParam(url)).toBe(origin);
      expect(parseReturnUrl(readReturnParam(url)!)).toEqual({
        link: ['/initiatives'],
        queryParams: { initiativeId: 'p2', tab: 'arena' },
      });
    }
    // A STATED CONSTANT, and six hops is enough that the measured nesting bug blows through it: it
    // reached 119 chars by hop 3 and more than doubles per hop after that.
    const MAX_URL = 128;
    for (const [i, len] of lengths.entries()) {
      expect(len, `hop ${i + 1} produced a ${len}-char URL: ${url}`).toBeLessThanOrEqual(MAX_URL);
    }
    // THE PROPERTY, stated exactly: the query is byte-identical on every hop. Only the path
    // differs, so nothing accumulates. Under the nesting bug this set had six distinct members,
    // each longer than the last.
    expect(new Set(queries).size, JSON.stringify(queries)).toBe(1);
  });

  it('drops a hostile origin rather than forwarding it', () => {
    const onWorkflows = `/workflows?from=${encodeURIComponent('//evil.example')}`;
    // The bad value is refused, so the page's own identity becomes the origin instead.
    expect(arenaReturnParams(onWorkflows)['from']).toBe('/workflows');
  });
});
