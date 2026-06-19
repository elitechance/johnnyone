import { describe, it, expect } from 'vitest';

describe('worker harness smoke', () => {
  it('runs a trivial passing test', () => {
    expect(1 + 1).toBe(2);
  });

  it('has vitest and cf pool available', () => {
    expect(typeof describe).toBe('function');
  });
});
