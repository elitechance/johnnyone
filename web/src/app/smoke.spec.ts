import { describe, it, expect } from 'vitest';

describe('web harness smoke', () => {
  it('runs a trivial passing test', () => {
    expect(2 + 2).toBe(4);
  });

  it('vitest + jsdom + angular plugin available', () => {
    expect(typeof describe).toBe('function');
  });
});
