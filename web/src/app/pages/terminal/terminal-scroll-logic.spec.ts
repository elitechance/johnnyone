import { describe, it, expect } from 'vitest';
import {
  viewportAnchorFromBottom,
  restoreTargetTop,
  shouldPreserveReadingPosition,
  nextPinState,
  consoleCaptureLines,
  CONSOLE_CAPTURE_LINES,
} from '../../../../../ui/src/components/terminal-screen/terminal-scroll-logic';

describe('viewportAnchorFromBottom', () => {
  it('computes lines-from-bottom for the current viewport', () => {
    expect(viewportAnchorFromBottom(1000, 800, 40)).toBe(160);
  });
  it('clamps to 0 when the viewport is at/near the bottom', () => {
    expect(viewportAnchorFromBottom(1000, 980, 40)).toBe(0);
    expect(viewportAnchorFromBottom(40, 0, 40)).toBe(0);
  });
});

describe('restoreTargetTop', () => {
  it('computes the target top row for a given anchor', () => {
    expect(restoreTargetTop(1000, 40, 160)).toBe(800);
  });
  it('clamps to 0 when the buffer is too short', () => {
    expect(restoreTargetTop(30, 40, 160)).toBe(0);
  });
});

describe('anchor round-trip', () => {
  it('restores the same viewport top for an in-range position', () => {
    const len = 1000;
    const rows = 40;
    const y = 800;
    const anchor = viewportAnchorFromBottom(len, y, rows);
    expect(restoreTargetTop(len, rows, anchor)).toBe(y);
  });
  it('stays in range at the raised capture depth (C2b — 1500-line buffer)', () => {
    const len = CONSOLE_CAPTURE_LINES;
    const rows = 40;
    // For every plausible anchor, the restore target is clamped to [0, len - rows].
    for (const anchor of [0, 1, 500, 1460, 1461, 5000]) {
      const top = restoreTargetTop(len, rows, anchor);
      expect(top).toBeGreaterThanOrEqual(0);
      expect(top).toBeLessThanOrEqual(len - rows); // 1460
    }
  });
});

describe('shouldPreserveReadingPosition', () => {
  it('preserves position when the user is unpinned (scrolled up)', () => {
    expect(shouldPreserveReadingPosition(false)).toBe(true);
  });
  it('does not preserve (snaps to latest) when pinned', () => {
    expect(shouldPreserveReadingPosition(true)).toBe(false);
  });
});

describe('nextPinState', () => {
  it('re-pins when at the bottom', () => {
    expect(nextPinState(true, false, false)).toBe(true);
  });
  it('unpins on a deliberate upward scroll', () => {
    expect(nextPinState(false, true, true)).toBe(false);
  });
  it('keeps the current state when neither at-bottom nor upward', () => {
    expect(nextPinState(false, false, true)).toBe(true);
    expect(nextPinState(false, false, false)).toBe(false);
  });
  it('at-bottom wins over an upward flag', () => {
    expect(nextPinState(true, true, false)).toBe(true);
  });
});

describe('consoleCaptureLines (C2b)', () => {
  it('returns the raised primary-pane capture depth (materially > 200)', () => {
    expect(consoleCaptureLines()).toBe(1500);
    expect(consoleCaptureLines()).toBeGreaterThan(200);
  });
});
