import { describe, it, expect } from 'vitest';

// Deep path into the ui library — the pure module is barrel-exported from `@johnnyone/ui`, but the
// web vitest config has no tsconfig-paths plugin (mirrors transcript-view.spec.ts). The pure module
// carries no Angular import, so it loads cleanly in jsdom.
import {
  parseUnifiedDiff,
  langForPath,
  fileTotals,
} from '../../../../../ui/src/components/diff-view/diff-parse';
import type { GitDiffFile } from '../../../../../ui/src/services/johnny-api.service';

describe('parseUnifiedDiff', () => {
  const sample = [
    'diff --git a/x.ts b/x.ts',
    'index 111..222 100644',
    '--- a/x.ts',
    '+++ b/x.ts',
    '@@ -1,3 +1,4 @@ export function f() {',
    ' const a = 1;',
    '-const b = 2;',
    '+const b = 3;',
    '+const c = 4;',
  ].join('\n');

  it('classifies hunk / meta / add / del / context lines', () => {
    const lines = parseUnifiedDiff(sample);
    const kinds = lines.map((l) => l.kind);
    expect(kinds).toEqual([
      'meta', // diff --git
      'meta', // index
      'meta', // --- a/
      'meta', // +++ b/
      'hunk', // @@
      'context', // ' const a'
      'del', // -const b
      'add', // +const b
      'add', // +const c
    ]);
  });

  it('preserves the raw line text including its marker', () => {
    const lines = parseUnifiedDiff(sample);
    const add = lines.find((l) => l.kind === 'add');
    expect(add?.text).toBe('+const b = 3;');
    const del = lines.find((l) => l.kind === 'del');
    expect(del?.text).toBe('-const b = 2;');
  });

  it('returns [] for empty input', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
  });
});

describe('langForPath', () => {
  it('maps known extensions to hljs language ids', () => {
    expect(langForPath('x/y.ts')).toBe('typescript');
    expect(langForPath('lib/foo.rs')).toBe('rust');
    expect(langForPath('a.tsx')).toBe('typescript');
    expect(langForPath('style.scss')).toBe('css');
    expect(langForPath('page.html')).toBe('xml');
  });

  it('returns "" for unknown / extensionless paths', () => {
    expect(langForPath('README')).toBe('');
    expect(langForPath('data.weirdext')).toBe('');
  });
});

describe('fileTotals', () => {
  it('sums additions and deletions across files', () => {
    const files: GitDiffFile[] = [
      { path: 'a.ts', additions: 18, deletions: 4, binary: false, diff: '', oldPath: null },
      { path: 'b.ts', additions: 14, deletions: 0, binary: false, diff: '', oldPath: null },
      { path: 'c.ts', additions: 2, deletions: 2, binary: false, diff: '', oldPath: null },
    ];
    expect(fileTotals(files)).toEqual({ count: 3, additions: 34, deletions: 6 });
  });

  it('is zero for an empty list', () => {
    expect(fileTotals([])).toEqual({ count: 0, additions: 0, deletions: 0 });
  });
});
