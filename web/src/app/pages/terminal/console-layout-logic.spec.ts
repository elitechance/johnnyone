import { describe, it, expect } from 'vitest';
import {
  RAIL_MIN,
  RAIL_MAX,
  RAIL_DEFAULT,
  RIGHT_MIN,
  RIGHT_MAX,
  RIGHT_DEFAULT,
  DIVIDER_PX,
  clampRailWidth,
  clampRightWidth,
  consoleColumns,
  parseStoredWidth,
} from './console-layout-logic';

// Pure spec (no Angular/Ionic/DOM) — runs under the plugin-less web/vitest.config.ts. Pins the
// resizable-divider clamp / grid-template / persistence-parse helpers (Overhaul P9 / phase P5, T01).

describe('clampRailWidth', () => {
  it('clamps to the rail bounds and passes mid-range through', () => {
    expect(clampRailWidth(RAIL_MIN - 50)).toBe(RAIL_MIN);
    expect(clampRailWidth(RAIL_MAX + 50)).toBe(RAIL_MAX);
    expect(clampRailWidth(300)).toBe(300);
    expect(clampRailWidth(RAIL_MIN)).toBe(RAIL_MIN);
    expect(clampRailWidth(RAIL_MAX)).toBe(RAIL_MAX);
  });

  it('returns the default for a non-finite value', () => {
    expect(clampRailWidth(NaN)).toBe(RAIL_DEFAULT);
    expect(clampRailWidth(Infinity)).toBe(RAIL_DEFAULT);
    expect(clampRailWidth(-Infinity)).toBe(RAIL_DEFAULT);
  });
});

describe('clampRightWidth', () => {
  it('clamps to the right-column bounds and passes mid-range through', () => {
    expect(clampRightWidth(RIGHT_MIN - 50)).toBe(RIGHT_MIN);
    expect(clampRightWidth(RIGHT_MAX + 999)).toBe(RIGHT_MAX);
    expect(clampRightWidth(360)).toBe(360);
  });

  it('returns the default for a non-finite value', () => {
    expect(clampRightWidth(NaN)).toBe(RIGHT_DEFAULT);
    expect(clampRightWidth(Infinity)).toBe(RIGHT_DEFAULT);
  });
});

describe('consoleColumns', () => {
  it('renders rail | divider | center | divider | right, with both divider tracks + minmax', () => {
    expect(consoleColumns(216, 262)).toBe(`216px ${DIVIDER_PX}px minmax(0, 1fr) ${DIVIDER_PX}px 262px`);
    expect(consoleColumns(300, 400)).toBe(`300px 6px minmax(0, 1fr) 6px 400px`);
  });

  it('contains both divider tracks and the flexible center', () => {
    const cols = consoleColumns(200, 300);
    expect(cols).toContain('minmax(0, 1fr)');
    expect(cols.match(/6px/g)?.length).toBe(2);
  });

  it('honors a custom divider width', () => {
    expect(consoleColumns(216, 262, 8)).toBe('216px 8px minmax(0, 1fr) 8px 262px');
  });
});

describe('parseStoredWidth', () => {
  it('parses a valid in-range string unchanged', () => {
    expect(parseStoredWidth('300', clampRailWidth)).toBe(300);
    expect(parseStoredWidth('360', clampRightWidth)).toBe(360);
  });

  it('clamps an out-of-range numeric string to the bound', () => {
    expect(parseStoredWidth('9999', clampRailWidth)).toBe(RAIL_MAX);
    expect(parseStoredWidth('10', clampRailWidth)).toBe(RAIL_MIN);
    expect(parseStoredWidth('9999', clampRightWidth)).toBe(RIGHT_MAX);
  });

  it('returns the clamp default for junk / empty / null / undefined', () => {
    expect(parseStoredWidth('junk', clampRailWidth)).toBe(RAIL_DEFAULT);
    expect(parseStoredWidth('', clampRailWidth)).toBe(RAIL_DEFAULT);
    expect(parseStoredWidth('   ', clampRailWidth)).toBe(RAIL_DEFAULT);
    expect(parseStoredWidth(null, clampRailWidth)).toBe(RAIL_DEFAULT);
    expect(parseStoredWidth(undefined, clampRightWidth)).toBe(RIGHT_DEFAULT);
  });
});
