import { describe, it, expect } from 'vitest';
import { sessionAfterWatchdogToggle } from './watchdog-logic';

/** A session exactly as `sessionStateFromApi` hands it to `currentSession()`. */
function loaded(over: {
  watchdogEnabled?: boolean | null;
  watchdogPrompt?: string | null;
} = {}) {
  return {
    id: 's1',
    title: 'kord',
    provider: 'claude_code',
    watchdogEnabled: false,
    watchdogPrompt: 'my own words',
    ...over,
  };
}

describe('sessionAfterWatchdogToggle', () => {
  it('enabling sets watchdogEnabled and the given prompt', () => {
    const next = sessionAfterWatchdogToggle(loaded(), true, 'ping me every 5m');
    expect(next.watchdogEnabled).toBe(true);
    expect(next.watchdogPrompt).toBe('ping me every 5m');

    // Arming with the backfilled default is the same path — whatever `wdPrompt` holds is
    // what the host just stored, so the banner and the textarea must agree on it.
    const armed = sessionAfterWatchdogToggle(loaded({ watchdogPrompt: '' }), true, 'DEFAULT TEXT');
    expect(armed.watchdogEnabled).toBe(true);
    expect(armed.watchdogPrompt).toBe('DEFAULT TEXT');

    // A session whose flag was null/undefined on the wire (deploy skew, a row predating
    // migration 027) still ends up a hard boolean, never `undefined`.
    expect(sessionAfterWatchdogToggle(loaded({ watchdogEnabled: null }), true, 'p').watchdogEnabled).toBe(true);
    expect(sessionAfterWatchdogToggle({}, true, 'p').watchdogEnabled).toBe(true);
  });

  it('disabling sets watchdogEnabled false and KEEPS the old prompt', () => {
    const next = sessionAfterWatchdogToggle(
      { watchdogEnabled: true, watchdogPrompt: 'my own words' },
      false,
      'a prompt that was never saved',
    );
    expect(next.watchdogEnabled).toBe(false);
    // Disarming does not touch the stored prompt — the host leaves it there, and the next
    // arm offers the user's own words back rather than a fresh default.
    expect(next.watchdogPrompt).toBe('my own words');
    expect(next.watchdogPrompt).not.toBe('a prompt that was never saved');

    // Passing the current prompt through on a disarm is a no-op on the prompt.
    const same = sessionAfterWatchdogToggle(loaded({ watchdogEnabled: true }), false, 'my own words');
    expect(same.watchdogEnabled).toBe(false);
    expect(same.watchdogPrompt).toBe('my own words');
  });

  it('returns a new object and never mutates the input', () => {
    const before = loaded();
    const snapshot = { ...before };
    const next = sessionAfterWatchdogToggle(before, true, 'ping me every 5m');

    // The signal and the `sessions()` list hold the SAME object, so mutating it in place
    // would both skip change detection and corrupt the loaded row.
    expect(next).not.toBe(before);
    expect(before).toEqual(snapshot);
    expect(before.watchdogEnabled).toBe(false);
    expect(before.watchdogPrompt).toBe('my own words');

    // Disarming is copy-on-write too.
    const off = sessionAfterWatchdogToggle(before, false, 'ignored');
    expect(off).not.toBe(before);
    expect(before.watchdogEnabled).toBe(false);
  });

  it('copies every other field unchanged', () => {
    const session = {
      id: 's1',
      title: 'kord',
      provider: 'claude_code',
      model: 'kloo-1',
      working_directory: '/repo',
      status: 'active' as const,
      total_input_tokens: 12,
      total_output_tokens: 34,
      total_cost_cents: 56,
      created_at: '2024-01-15T10:00:00Z',
      updated_at: '2024-01-15T10:05:00Z',
      attached_tmux: true,
      watchdogEnabled: false,
      watchdogIntervalSecs: 900,
      watchdogPrompt: 'my own words',
      watchdogLastNudgeAt: '2024-01-15T10:04:00Z',
      watchdogNudgeCount: 3,
      watchdogHeldCount: 5,
      watchdogOnBlocked: true,
      watchdogOnStaleReport: false,
    };
    const next = sessionAfterWatchdogToggle(session, true, 'ping me every 5m');

    // The unrelated fields survive the copy — a toggle that rebuilt the session from scratch
    // would blank the counters the chip and footnote render.
    expect(next.title).toBe('kord');
    expect(next.id).toBe('s1');
    expect(next.provider).toBe('claude_code');
    expect(next.model).toBe('kloo-1');
    expect(next.working_directory).toBe('/repo');
    expect(next.status).toBe('active');
    expect(next.total_input_tokens).toBe(12);
    expect(next.total_output_tokens).toBe(34);
    expect(next.total_cost_cents).toBe(56);
    expect(next.created_at).toBe('2024-01-15T10:00:00Z');
    expect(next.updated_at).toBe('2024-01-15T10:05:00Z');
    expect(next.attached_tmux).toBe(true);
    expect(next.watchdogIntervalSecs).toBe(900);
    expect(next.watchdogLastNudgeAt).toBe('2024-01-15T10:04:00Z');
    expect(next.watchdogNudgeCount).toBe(3);
    expect(next.watchdogHeldCount).toBe(5);
    expect(next.watchdogOnBlocked).toBe(true);
    expect(next.watchdogOnStaleReport).toBe(false);

    // And the ONLY deltas are the two watchdog fields arming writes.
    expect(next.watchdogEnabled).toBe(true);
    expect(next.watchdogPrompt).toBe('ping me every 5m');
    expect(Object.keys(next).sort()).toEqual(Object.keys(session).sort());
  });
});