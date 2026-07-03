import { describe, it, expect } from 'vitest';
import {
  defaultLenses,
  addLens,
  removeLens,
  moveLens,
  providerOptions,
  toConfigJson,
  fromConfigJson,
  type LensDraft,
} from './validation-config-logic';

describe('defaultLenses', () => {
  it('is product/qa/lead, all blocking and all non-vision', () => {
    const d = defaultLenses();
    expect(d.map((l) => l.name)).toEqual(['product', 'qa', 'lead']);
    expect(d.every((l) => l.blocking === true)).toBe(true);
    expect(d.every((l) => l.vision === false)).toBe(true);
    expect(d.every((l) => l.provider === 'claude_code')).toBe(true);
    expect(d.every((l) => l.model === '')).toBe(true);
  });
});

describe('providerOptions', () => {
  it('is the real registry minus shell (no kloo)', () => {
    expect(providerOptions()).toEqual(['claude_code', 'codex', 'cline', 'ollama', 'grok']);
    expect(providerOptions()).not.toContain('shell');
    expect(providerOptions()).not.toContain('kloo');
  });
});

describe('addLens', () => {
  it('appends a new lens (new array, len+1, vision:false, blocking:true)', () => {
    const start = defaultLenses();
    const next = addLens(start);
    expect(next).not.toBe(start);
    expect(next.length).toBe(start.length + 1);
    const added = next[next.length - 1];
    expect(added.vision).toBe(false);
    expect(added.blocking).toBe(true);
    expect(added.provider).toBe('claude_code');
    expect(added.name).toBe('lens-4');
  });
});

describe('removeLens', () => {
  it('removes at index (new array)', () => {
    const start = defaultLenses();
    const next = removeLens(start, 1);
    expect(next).not.toBe(start);
    expect(next.map((l) => l.name)).toEqual(['product', 'lead']);
  });

  it('is a no-op at length 1 (min 1)', () => {
    const one: LensDraft[] = [
      { name: 'solo', provider: 'grok', model: '', vision: false, blocking: true },
    ];
    expect(removeLens(one, 0)).toEqual(one);
    expect(removeLens(one, 0).length).toBe(1);
  });

  it('is a no-op for an out-of-range index', () => {
    const d = defaultLenses();
    expect(removeLens(d, 9).length).toBe(3);
  });
});

describe('moveLens', () => {
  it('reorders (new array)', () => {
    const d = defaultLenses(); // product, qa, lead
    expect(moveLens(d, 0, 2).map((l) => l.name)).toEqual(['qa', 'lead', 'product']);
    expect(moveLens(d, 2, 0).map((l) => l.name)).toEqual(['lead', 'product', 'qa']);
  });

  it('clamps to and no-ops out-of-range from', () => {
    const d = defaultLenses();
    expect(moveLens(d, 0, 99).map((l) => l.name)).toEqual(['qa', 'lead', 'product']);
    expect(moveLens(d, 9, 0)).toBe(d); // from out of range → unchanged ref
  });
});

describe('toConfigJson / fromConfigJson round-trip', () => {
  it('omits empty model but keeps a set model, and preserves vision both ways', () => {
    const lenses: LensDraft[] = [
      { name: 'product', provider: 'claude_code', model: '', vision: false, blocking: true },
      { name: 'security', provider: 'grok', model: 'grok-build', vision: true, blocking: false },
    ];
    const json = toConfigJson(lenses);
    const wire = JSON.parse(json);
    // model omitted when empty…
    expect(wire[0].model).toBeUndefined();
    // …but present when set.
    expect(wire[1].model).toBe('grok-build');
    // vision is always emitted (a bool).
    expect(wire[0].vision).toBe(false);
    expect(wire[1].vision).toBe(true);

    const back = fromConfigJson(json);
    expect(back[0].model).toBe('');
    expect(back[1].model).toBe('grok-build');
    expect(back[0].vision).toBe(false);
    expect(back[1].vision).toBe(true);
    expect(back[1].blocking).toBe(false);
  });

  it('a wire lens missing the vision key parses to vision:false', () => {
    const json = JSON.stringify([{ name: 'x', provider: 'codex', blocking: true }]);
    const back = fromConfigJson(json);
    expect(back[0].vision).toBe(false);
    expect(back[0].model).toBe('');
  });

  it('null / empty / invalid / empty-array → defaultLenses()', () => {
    const names = ['product', 'qa', 'lead'];
    expect(fromConfigJson(null).map((l) => l.name)).toEqual(names);
    expect(fromConfigJson('').map((l) => l.name)).toEqual(names);
    expect(fromConfigJson('nonsense').map((l) => l.name)).toEqual(names);
    expect(fromConfigJson('[]').map((l) => l.name)).toEqual(names);
  });
});
