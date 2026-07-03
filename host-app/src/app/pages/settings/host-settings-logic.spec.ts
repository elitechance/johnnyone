import { describe, it, expect } from 'vitest';
import { DEFAULT_STORE_PATHS, resolveStorePath, trimStorePaths } from './host-settings-logic';

// Pure spec (no Angular/Ionic) — runs under host-app/vitest.config.ts.
// Pins the store-path defaults + the empty→default resolution + trim (P9 / D2).

describe('DEFAULT_STORE_PATHS', () => {
  it('mirrors the Rust DEFAULT_INITIATIVES_DIR / DEFAULT_FILES_ROOT', () => {
    expect(DEFAULT_STORE_PATHS.initiativesDir).toBe(
      '/home/creepy/Documents/Workspace/.johnnyone/initiatives',
    );
    expect(DEFAULT_STORE_PATHS.filesRoot).toBe('/home/creepy/Documents/Workspace');
  });
});

describe('resolveStorePath', () => {
  const d = '/default/path';

  it('falls back on empty string', () => {
    expect(resolveStorePath('', d)).toBe(d);
  });

  it('falls back on whitespace-only', () => {
    expect(resolveStorePath('   ', d)).toBe(d);
  });

  it('falls back on null / undefined', () => {
    expect(resolveStorePath(null, d)).toBe(d);
    expect(resolveStorePath(undefined, d)).toBe(d);
  });

  it('keeps a non-empty value', () => {
    expect(resolveStorePath('/x', d)).toBe('/x');
  });
});

describe('trimStorePaths', () => {
  it('trims both fields', () => {
    expect(trimStorePaths({ initiativesDir: '  /a  ', filesRoot: ' /b ' })).toEqual({
      initiativesDir: '/a',
      filesRoot: '/b',
    });
  });
});
