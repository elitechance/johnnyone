import { describe, it, expect } from 'vitest';
import { watchdogChipState, transcriptRowType } from './watchdog-logic';

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