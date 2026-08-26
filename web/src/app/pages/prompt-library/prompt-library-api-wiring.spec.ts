import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const api = readFileSync(
  resolve(here, '../../../../../ui/src/services/johnny-api.service.ts'),
  'utf8',
);
const index = readFileSync(resolve(here, '../../../../../ui/src/index.ts'), 'utf8');

describe('prompt-library api wiring', () => {
  it('JohnnyApiService exposes listPromptLibrary with every catalog field', () => {
    expect(api).toMatch(/listPromptLibrary\(\)/);
    expect(api).toMatch(/query ListPromptLibrary/);
    expect(api).toMatch(/\.query</);
    expect(api).toMatch(/\.pipe\(map\(/);
    expect(api).toContain('usedCount');
    expect(api).toContain('customised');
    expect(api).toContain('readOnly');
    expect(api).toContain('engineReads');
    expect(api).toMatch(/name:/);
    expect(api).toMatch(/role:/);
    expect(api).toMatch(/scope:/);
    expect(api).toMatch(/version:/);
    expect(api).toContain('export interface PromptLibraryEntry');
  });

  it('re-exports PromptLibraryEntry from @johnnyone/ui', () => {
    expect(index).toMatch(/PromptLibraryEntry/);
  });

  it('does not add a duplicate prompt mutation', () => {
    expect(api).not.toMatch(/duplicatePlannerPrompt/);
  });
});
