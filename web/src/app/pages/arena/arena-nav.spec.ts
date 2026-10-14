import { describe, it, expect } from 'vitest';
import { arenaBackTarget, arenaReturnParams, parseReturnUrl, returnLabel } from './arena-nav';

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
  it('carries the departure URL whole, so the walk back chains', () => {
    expect(arenaReturnParams('/initiatives?initiativeId=p1&tab=arena')).toEqual({
      from: '/initiatives?initiativeId=p1&tab=arena',
    });
    const chained = arenaReturnParams('/workflows?from=%2Finitiatives%3Ftab%3Darena');
    expect(chained['from']).toBe('/workflows?from=%2Finitiatives%3Ftab%3Darena');
    // and the chained value survives a round trip
    expect(parseReturnUrl(chained['from'])?.queryParams['from']).toBe('/initiatives?tab=arena');
  });

  it('adds nothing when there is no usable current URL', () => {
    expect(arenaReturnParams('')).toEqual({});
    expect(arenaReturnParams('relative')).toEqual({});
  });
});
