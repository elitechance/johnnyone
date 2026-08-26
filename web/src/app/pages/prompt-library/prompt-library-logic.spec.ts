import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { PromptLibraryEntry } from '@johnnyone/ui';
import {
  editorHref,
  editorQueryParams,
  filterRows,
  formatUsed,
  loadView,
  newHref,
  rowAction,
  rowActionLabel,
  scopeLabel,
} from './prompt-library-logic';

function row(
  partial: Partial<PromptLibraryEntry> & Pick<PromptLibraryEntry, 'key' | 'role'>,
): PromptLibraryEntry {
  return {
    id: partial.key,
    name: partial.name ?? partial.key,
    scope: 'builtin',
    version: 'v1',
    usedCount: 0,
    customised: false,
    readOnly: true,
    engineReads: true,
    ...partial,
  };
}

const catalog: PromptLibraryEntry[] = [
  row({ key: 'planning.planner', name: 'Planning planner', role: 'planner' }),
  row({ key: 'planning.reviewer', name: 'Planning reviewer', role: 'lens' }),
  row({ key: 'planning.amendPlanner', name: 'Planning amend planner', role: 'planner' }),
  row({ key: 'planning.amendReviewer', name: 'Planning amend reviewer', role: 'lens' }),
  row({ key: 'development.worker', name: 'Development worker', role: 'worker' }),
  row({ key: 'development.reviewer', name: 'Development reviewer', role: 'lens' }),
  row({ key: 'smallMode.planner', name: 'Small-mode planner', role: 'planner' }),
  row({ key: 'smallMode.reviewer', name: 'Small-mode reviewer', role: 'lens' }),
  row({ key: 'smallMode.leafWrapper', name: 'Leaf wrapper', role: 'worker' }),
  row({ key: 'smallMode.amendPlanner', name: 'Amend planner', role: 'planner' }),
];

describe('prompt-library-logic', () => {
  it('has no Angular / Ionic import', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), 'prompt-library-logic.ts'),
      'utf8',
    );
    const imports = src
      .split('\n')
      .filter((line) => /^\s*import\b/.test(line))
      .join('\n');
    expect(imports).not.toMatch(/@angular\/|@ionic\//);
  });

  it('empty role keeps all; role lens drops non-lens', () => {
    expect(filterRows(catalog, { role: '' })).toHaveLength(10);
    const lens = filterRows(catalog, { role: 'lens' });
    expect(lens.length).toBeGreaterThan(0);
    expect(lens.every((r) => r.role === 'lens')).toBe(true);
  });

  it('empty scope keeps all; scope project on default catalog is empty', () => {
    expect(filterRows(catalog, { scope: '' })).toHaveLength(10);
    expect(filterRows(catalog, { scope: 'project' })).toEqual([]);
  });

  it('search planning.planner keeps that key; zzzz returns empty', () => {
    const hit = filterRows(catalog, { query: 'planning.planner' });
    expect(hit.map((r) => r.key)).toEqual(['planning.planner']);
    expect(filterRows(catalog, { query: 'zzzz' })).toEqual([]);
  });

  it('combined role and query', () => {
    const hit = filterRows(catalog, { role: 'lens', query: 'reviewer' });
    expect(hit.every((r) => r.role === 'lens')).toBe(true);
    expect(hit.some((r) => r.key === 'planning.reviewer')).toBe(true);
    expect(hit.some((r) => r.role === 'planner')).toBe(false);
  });

  it('rowAction is duplicate for built-in and open for customised', () => {
    expect(rowAction(catalog[0])).toEqual({ kind: 'duplicate', key: 'planning.planner' });
    expect(
      rowAction({
        key: 'development.reviewer',
        customised: true,
        readOnly: false,
      }),
    ).toEqual({ kind: 'open', key: 'development.reviewer' });
  });

  it('editorHref and newHref', () => {
    expect(editorHref('planning.planner')).toBe('/settings?prompt=planning.planner');
    expect(newHref()).toBe('/settings');
    expect(editorQueryParams('planning.planner')).toEqual({ prompt: 'planning.planner' });
    expect(rowActionLabel(catalog[0])).toBe('Duplicate to edit');
    expect(
      rowActionLabel({ key: 'development.reviewer', customised: true, readOnly: false }),
    ).toBe('Open editor');
  });

  it('formatUsed never uses a placeholder', () => {
    expect(formatUsed(0)).toBe('0 runs');
    expect(formatUsed(1)).toBe('1 run');
    expect(formatUsed(2)).toBe('2 runs');
  });

  it('scopeLabel maps builtin', () => {
    expect(scopeLabel('builtin')).toBe('built-in');
    expect(scopeLabel('workspace')).toBe('workspace');
    expect(scopeLabel('project')).toBe('project');
  });

  it('loadView covers loading error empty ready', () => {
    expect(loadView('loading', 0)).toBe('loading');
    expect(loadView('idle', 0)).toBe('loading');
    expect(loadView('load-error', 0)).toBe('error');
    expect(loadView('ready', 0)).toBe('empty');
    expect(loadView('ready', 3)).toBe('ready');
  });
});
