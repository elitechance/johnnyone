import { describe, it, expect } from 'vitest';
import {
  parseHostTimestamp,
  transcriptRowType,
  watchdogChipState,
} from './watchdog-logic';

/** Format an epoch ms the way the host writes it: SQLite `datetime('now')`, UTC, no zone. */
function hostTimestamp(epochMs: number): string {
  const d = new Date(epochMs);
  const p = (n: number) => n.toString().padStart(2, '0');
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ` +
    `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())}`;
}

describe('parseHostTimestamp', () => {
  it('treats a host timestamp as UTC, not local', () => {
    // Assert against Date.UTC, NOT a locally constructed new Date(...) — the latter
    // agrees with a local parse in UTC and the test would pass there and fail anywhere else.
    expect(parseHostTimestamp('2026-11-02 20:33:38')).toBe(Date.UTC(2026, 10, 2, 20, 33, 38));
    // A string already carrying its zone is parsed as-is, identically.
    expect(parseHostTimestamp('2026-11-02T20:33:38Z')).toBe(Date.UTC(2026, 10, 2, 20, 33, 38));
    expect(parseHostTimestamp('2026-11-02 20:33:38')).toBe(parseHostTimestamp('2026-11-02T20:33:38Z'));
    expect(parseHostTimestamp(null)).toBeNull();
    expect(parseHostTimestamp(undefined)).toBeNull();
    expect(parseHostTimestamp('')).toBeNull();
    expect(parseHostTimestamp('nonsense')).toBeNull();
  });

  it('a fresh nudge is never in the future', () => {
    const now = Date.now();
    const ts = hostTimestamp(now - 60_000);
    const parsed = parseHostTimestamp(ts);
    expect(parsed).not.toBeNull();
    const elapsedSecs = (now - (parsed as number)) / 1000;
    // Negative here is the live bug: the chip read "armed -17320s ago" and its
    // `nudgeSecsAgo < 60` gate was then permanently true.
    expect(elapsedSecs).toBeGreaterThan(0);
    // "Roughly 60" — the host string is second-granular, so the remainder of the
    // current second is real slack, not drift.
    expect(elapsedSecs).toBeGreaterThanOrEqual(59);
    expect(elapsedSecs).toBeLessThan(62);
  });
});

describe('watchdogChipState', () => {
  it('returns off when not enabled', () => {
    const state = watchdogChipState({
      enabled: false,
      lastNudgeAt: null,
      paneLastActiveAt: Date.now(),
      now: Date.now(),
      intervalSecs: 300,
    });
    expect(state).toBe('off');
  });

  it('returns armed-holding when pane recently active', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: null,
      paneLastActiveAt: now - 1000,
      now,
      intervalSecs: 300,
    });
    expect(state).toBe('holding');
  });

  it('returns quiet-when pane is inactive but not yet past interval', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: null,
      paneLastActiveAt: now - 60000,
      now,
      intervalSecs: 300,
    });
    expect(state).toBe('quiet');
  });

  it('returns quiet when pane last active exactly equals interval boundary', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: null,
      paneLastActiveAt: now - 300_000,
      now,
      intervalSecs: 300,
    });
    // Seconds elapsed = 300 which equals intervalSecs, so quiet
    expect(state).toBe('quiet');
  });

  it('returns quiet when pane last active exceeds interval', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: null,
      paneLastActiveAt: now - 310_000,
      now,
      intervalSecs: 300,
    });
    expect(state).toBe('quiet');
  });

  it('returns nudged when nudge within the last minute', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: new Date(now - 30000).toISOString(),
      paneLastActiveAt: now - 310_000,
      now,
      intervalSecs: 300,
    });
    expect(state).toBe('nudged');
  });

  it('returns quiet when nudge was older than 60 seconds', () => {
    const now = Date.now();
    const state = watchdogChipState({
      enabled: true,
      lastNudgeAt: new Date(now - 61000).toISOString(),
      paneLastActiveAt: now - 310_000,
      now,
      intervalSecs: 300,
    });
    expect(state).toBe('quiet');
  });
});

describe('transcriptRowType', () => {
  it('maps role=watchdog to watchdog', () => {
    expect(transcriptRowType({ role: 'watchdog', kind: 'text', language: null })).toBe('watchdog');
  });

  it('maps prompt kind to you', () => {
    expect(transcriptRowType({ role: 'user', kind: 'prompt', language: null })).toBe('you');
  });

  it('maps error kind to error', () => {
    expect(transcriptRowType({ role: 'agent', kind: 'error', language: null })).toBe('error');
  });

  it('maps markdown code to markdown', () => {
    expect(transcriptRowType({ role: 'agent', kind: 'code', language: 'markdown' })).toBe('markdown');
  });

  it('maps agent role to agent', () => {
    expect(transcriptRowType({ role: 'agent', kind: 'text', language: null })).toBe('agent');
  });
});