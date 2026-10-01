import { describe, expect, it } from 'vitest';
import {
  COMPOSER_MAX_PX,
  composerHeightPx,
  enterAction,
  enterSendsForPointer,
} from './composer-input';

describe('composer Enter handling', () => {
  it('sends on Enter with a real keyboard', () => {
    expect(enterSendsForPointer(false)).toBe(true);
    expect(enterAction({ shiftKey: false, enterSends: true })).toBe('send');
  });

  // The whole reason this logic exists: a soft keyboard has no Shift+Enter, so if Enter sent on
  // touch there would be no way to type a newline on a phone at all.
  it('inserts a newline on Enter on a touch device', () => {
    expect(enterSendsForPointer(true)).toBe(false);
    expect(enterAction({ shiftKey: false, enterSends: false })).toBe('newline');
  });

  it('treats Shift+Enter as a newline on every surface', () => {
    expect(enterAction({ shiftKey: true, enterSends: true })).toBe('newline');
    expect(enterAction({ shiftKey: true, enterSends: false })).toBe('newline');
  });
});

describe('composer auto-grow', () => {
  it('grows with its content', () => {
    expect(composerHeightPx(60)).toBe(60);
  });

  it('caps so a long message cannot eat the transcript', () => {
    expect(composerHeightPx(9000)).toBe(COMPOSER_MAX_PX);
  });
});
