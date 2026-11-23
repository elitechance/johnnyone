import { describe, it, expect } from 'vitest';
import {
  absolutePath,
  breadcrumbs,
  canChoose,
  dirEntries,
  displayPath,
  parentPath,
} from './host-dir-picker-logic';

const listing = (path: string) => ({
  root: '/home/u/Workspace/',
  path,
  entries: [
    { name: 'apps', path: path ? `${path}/apps` : 'apps', kind: 'dir' },
    { name: 'README.md', path: 'README.md', kind: 'file' },
    { name: 'kord', path: path ? `${path}/kord` : 'kord', kind: 'dir' },
  ],
});

describe('host-dir-picker-logic', () => {
  it('lists folders only', () => {
    expect(dirEntries(listing('')).map((e) => e.name)).toEqual(['apps', 'kord']);
    expect(dirEntries(null)).toEqual([]);
  });

  it('computes the parent of a relative path, null at the root', () => {
    expect(parentPath('')).toBeNull();
    expect(parentPath(null)).toBeNull();
    expect(parentPath('apps')).toBe('');
    expect(parentPath('personal/apps')).toBe('personal');
    expect(parentPath('personal/apps/')).toBe('personal');
  });

  it('joins root + relative path without doubled slashes', () => {
    expect(absolutePath(listing(''))).toBe('/home/u/Workspace');
    expect(absolutePath(listing('personal/apps'))).toBe('/home/u/Workspace/personal/apps');
    expect(absolutePath(null)).toBe('');
    expect(displayPath(null)).toBe('…');
    expect(displayPath(listing('x'))).toBe('/home/u/Workspace/x');
  });

  it('builds clickable breadcrumbs from the root', () => {
    expect(breadcrumbs(listing('personal/apps'))).toEqual([
      { label: 'Workspace', relPath: '' },
      { label: 'personal', relPath: 'personal' },
      { label: 'apps', relPath: 'personal/apps' },
    ]);
    expect(breadcrumbs({ root: '/', path: '', entries: [] })).toEqual([{ label: '/', relPath: '' }]);
    expect(breadcrumbs(null)).toEqual([]);
  });

  it('allows choosing only a loaded, error-free listing', () => {
    expect(canChoose(listing(''), false, null)).toBe(true);
    expect(canChoose(listing(''), true, null)).toBe(false);
    expect(canChoose(listing(''), false, 'boom')).toBe(false);
    expect(canChoose(null, false, null)).toBe(false);
  });
});
