import { describe, it, expect } from 'vitest';
import { NAV_ITEMS } from './nav-items';

// Pins the single nav model shared by the widescreen rail and the mobile bottom
// nav (Overhaul P8 / phase 03). Guards both surfaces against drift (D4/D12).

describe('NAV_ITEMS', () => {
  it('has exactly the four destinations in order work/files/shells/settings', () => {
    expect(NAV_ITEMS.map((n) => n.id)).toEqual(['work', 'files', 'shells', 'settings']);
  });

  it('maps each entry to its existing route path', () => {
    const byId = Object.fromEntries(NAV_ITEMS.map((n) => [n.id, n]));
    expect(byId['work'].path).toBe('/initiatives');
    expect(byId['files'].path).toBe('/files');
    expect(byId['shells'].path).toBe('/shells');
    expect(byId['settings'].path).toBe('/settings');
  });

  it('labels each entry per the mock', () => {
    expect(NAV_ITEMS.map((n) => n.label)).toEqual(['Work', 'Files', 'Shells', 'Settings']);
  });

  it('gives every entry a non-empty registered icon name', () => {
    for (const item of NAV_ITEMS) {
      expect(item.icon).toMatch(/-outline$/);
    }
  });
});
