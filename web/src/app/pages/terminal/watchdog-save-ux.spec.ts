// Watchdog save UX — three fixes to the page's save path, all in the pure seam.
//
//  1. `runWatchdogAction` used to drop the server's reason. The API client throws
//     `GraphQLRequestError`, an `Error` whose `.message` is the server's text, so the strip
//     could name the refusal instead of just "the server refused".
//  2. The prompt is a textarea but the host rejects any control character except tab, so a
//     single Enter made every Save fail. `normalizeWatchdogPrompt` flattens it before it goes
//     on the wire.
//  3. The footnote claimed "armed 792h ago" off the session's `created_at`. No armed-at time is
//     stored, so `watchdogLastNudgeText` reports the last nudge — or says there hasn't been one.
//
// TerminalPage itself cannot be TestBed-compiled here (`web/vitest.config.ts` has no
// `@analogjs/vitest-angular`, so a `templateUrl` component cannot be instantiated), so these
// drive the pure seam directly — the same idiom as `watchdog-nudge-now.spec.ts`.
import { describe, it, expect } from 'vitest';
import {
  normalizeWatchdogPrompt,
  runWatchdogAction,
  watchdogLastNudgeText,
} from './watchdog-logic';

describe('runWatchdogAction error surface', () => {
  const FAILURE = 'Could not save the watchdog settings — the server refused.';

  it('appends the server message when the action throws an Error', async () => {
    const msg = await runWatchdogAction(async () => {
      throw new Error('Watchdog prompt contains control characters');
    }, FAILURE);
    expect(msg).toContain('Could not save the watchdog settings');
    expect(msg).toContain('Watchdog prompt contains control characters');
  });

  it('returns exactly the failure text for a thrown non-Error', async () => {
    const msg = await runWatchdogAction(async () => {
      // eslint-disable-next-line no-throw-literal
      throw 'Watchdog prompt contains control characters';
    }, FAILURE);
    expect(msg).toBe(FAILURE);
  });

  it('returns exactly the failure text for an Error with an empty message', async () => {
    const msg = await runWatchdogAction(async () => {
      throw new Error('   ');
    }, FAILURE);
    expect(msg).toBe(FAILURE);
  });

  it('returns null on success', async () => {
    await expect(runWatchdogAction(async () => true, FAILURE)).resolves.toBeNull();
  });
});

describe('normalizeWatchdogPrompt', () => {
  it('turns a newline into a single space', () => {
    expect(normalizeWatchdogPrompt('a\nb')).toBe('a b');
  });

  it('collapses a run of control characters and trailing spaces', () => {
    expect(normalizeWatchdogPrompt('a\r\n\r\nb  ')).toBe('a b');
  });

  it('keeps a tab', () => {
    expect(normalizeWatchdogPrompt('a\tb')).toBe('a\tb');
  });

  it('leaves a long single-line prompt untouched', () => {
    const long = 'x'.repeat(220);
    expect(normalizeWatchdogPrompt(long)).toBe(long);
  });
});

describe('watchdogLastNudgeText', () => {
  const NOW = Date.UTC(2026, 10, 2, 20, 33, 38);

  it('says "off" when the watchdog is disabled', () => {
    expect(watchdogLastNudgeText(false, NOW - 90_000, NOW)).toBe('off');
  });

  it('says "no nudge yet" when enabled with no nudge recorded', () => {
    expect(watchdogLastNudgeText(true, null, NOW)).toBe('no nudge yet');
  });

  it('formats minutes under an hour', () => {
    expect(watchdogLastNudgeText(true, NOW - 90_000, NOW)).toBe('last nudge 1m ago');
  });

  it('formats hours over an hour', () => {
    expect(watchdogLastNudgeText(true, NOW - 2 * 3_600_000, NOW)).toBe('last nudge 2h ago');
  });
});