import { describe, it, expect } from 'vitest';
import {
  DEFAULT_WATCHDOG_PROMPT,
  WATCHDOG_SIGNAL_DEFAULTS,
  runWatchdogAction,
  shouldHydrateWatchdogSignals,
  watchdogPromptAfterEnable,
  watchdogSettingsFromSession,
} from './watchdog-logic';
import { sessionStateFromApi } from './session-map';
import type { AiSession } from '../../../../../ui/src/services/johnny-api.service';

/** A wire session with the watchdog fields the console needs. */
function apiSession(over: Partial<AiSession> = {}): AiSession {
  return {
    id: 's1',
    title: 'kord',
    provider: 'claude_code',
    model: '',
    status: 'active',
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostCents: 0,
    createdAt: '2024-01-15T10:00:00Z',
    updatedAt: '2024-01-15T10:00:00Z',
    ...over,
  };
}

describe('watchdog hydration from the session', () => {
  it('hydrates the watchdog toggle from the session', () => {
    // An armed session seeds all three signals — before this, the page showed
    // false/300/'' no matter what the host had, so the first click DISARMED a running watchdog.
    const armed = watchdogSettingsFromSession({
      watchdogEnabled: true,
      watchdogIntervalSecs: 600,
      watchdogPrompt: 'ping me',
    });
    expect(armed).toEqual({ enabled: true, intervalSecs: 600, prompt: 'ping me' });

    // No session at all (nothing selected yet) → the signals' own initial values.
    expect(watchdogSettingsFromSession(null)).toEqual(WATCHDOG_SIGNAL_DEFAULTS);
    expect(watchdogSettingsFromSession(undefined)).toEqual(WATCHDOG_SIGNAL_DEFAULTS);
    expect(watchdogSettingsFromSession(undefined).enabled).toBe(false);
    expect(watchdogSettingsFromSession(null).intervalSecs).toBe(300);
    expect(watchdogSettingsFromSession(null).prompt).toBe('');

    // A null interval (deploy skew / a row predating migration 027) falls back to 300.
    // `undefined` reaching the signal instead would render "NaN min" in the picker.
    const nullInterval = watchdogSettingsFromSession({ watchdogEnabled: true, watchdogIntervalSecs: null });
    expect(nullInterval.intervalSecs).toBe(300);
    expect(Number.isNaN(nullInterval.intervalSecs)).toBe(false);
    expect(nullInterval.enabled).toBe(true);
    expect(nullInterval.prompt).toBe('');

    // Hydration is keyed on the id CHANGING, so a re-set of the same session (which
    // `selectSession`/`loadSessions` do constantly) must NOT re-seed — that would erase the
    // keystrokes of someone typing in the prompt textarea.
    expect(shouldHydrateWatchdogSignals(null, 's1')).toBe(true);
    expect(shouldHydrateWatchdogSignals('s1', 's1')).toBe(false);
    expect(shouldHydrateWatchdogSignals('s1', 's2')).toBe(true);
    expect(shouldHydrateWatchdogSignals('s1', null)).toBe(true);
    expect(shouldHydrateWatchdogSignals('s1', undefined)).toBe(true);
    expect(shouldHydrateWatchdogSignals(null, null)).toBe(false);
  });

  it('maps the watchdog fields through to state', () => {
    const state = sessionStateFromApi(apiSession({
      workingDirectory: '/repo',
      attachedTmux: true,
      watchdogEnabled: true,
      watchdogIntervalSecs: 900,
      watchdogPrompt: 'where are you at?',
      watchdogLastNudgeAt: '2024-01-15T10:05:00Z',
      watchdogNudgeCount: 3,
    }));

    // The point of the assertion is the VALUES, and `watchdogEnabled` above all: a mapper that
    // strips it makes the chip read "Off" forever even though the wire carries `true`.
    expect(state.watchdogEnabled).toBe(true);
    expect(state.watchdogIntervalSecs).toBe(900);
    expect(state.watchdogPrompt).toBe('where are you at?');
    expect(state.watchdogLastNudgeAt).toBe('2024-01-15T10:05:00Z');
    expect(state.watchdogNudgeCount).toBe(3);

    // The twelve legacy fields still land, snake_case as the templates expect.
    expect(state.id).toBe('s1');
    expect(state.title).toBe('kord');
    expect(state.provider).toBe('claude_code');
    expect(state.model).toBe('');
    expect(state.working_directory).toBe('/repo');
    expect(state.status).toBe('active');
    expect(state.total_input_tokens).toBe(0);
    expect(state.total_output_tokens).toBe(0);
    expect(state.total_cost_cents).toBe(0);
    expect(state.created_at).toBe('2024-01-15T10:00:00Z');
    expect(state.updated_at).toBe('2024-01-15T10:00:00Z');
    expect(state.attached_tmux).toBe(true);

    // A session that never had a watchdog is disarmed, not "undefined-ish armed".
    const bare = sessionStateFromApi(apiSession());
    expect(bare.watchdogEnabled).toBeUndefined();
    expect(bare.attached_tmux).toBe(false);
    expect(bare.working_directory).toBe('');
  });

  it('surfaces a rejected watchdog save', async () => {
    const FAILURE = 'Could not save the watchdog settings — the server refused.';

    // Success → no message, so the handler clears/leaves `wdError` empty and the strip stays quiet.
    let sent = 0;
    await expect(runWatchdogAction(async () => {
      sent += 1;
      return true;
    }, FAILURE)).resolves.toBeNull();
    expect(sent).toBe(1);

    // Rejection → the message the view renders. Previously this was `catch { /* nop */ }`, so a
    // refused save was invisible and the user kept clicking Save on a toggle that never moved.
    await expect(runWatchdogAction(async () => {
      throw new Error('Invalid parent path');
    }, FAILURE)).resolves.toBe(FAILURE);

    // A rejected *promise* (not a thrown one) is caught too — that is what `firstValueFrom`
    // hands the handlers when the relay answers with a GraphQL error.
    await expect(runWatchdogAction(
      () => Promise.reject(new Error('No online backend app found')),
      FAILURE,
    )).resolves.toBe(FAILURE);

    // And the error is the handler's string, not the exception's — the raw text is not the UI's.
    const msg = await runWatchdogAction(() => Promise.reject(new Error('stack noise')), 'nope');
    expect(msg).toBe('nope');
  });

  it('enabling with no prompt is not silently inert', () => {
    // The sweep only picks up rows with `watchdog_prompt != ''`, and migration 027 defaults every
    // row to ''. Arming such a session used to flip the flag and change nothing. Both now yield
    // the text the host backfills, so the box shows what will actually be typed.
    for (const empty of ['', undefined, null]) {
      const seeded = watchdogPromptAfterEnable(empty);
      expect(seeded).not.toBe('');
      expect(seeded.length).toBeGreaterThan(0);
      expect(seeded).toContain('{quiet}');
      expect(seeded).toContain('{name}');
      expect(seeded).toContain('{nudges}');
      expect(seeded).toBe(DEFAULT_WATCHDOG_PROMPT);
    }

    // A prompt the user wrote is returned untouched — arming never overwrites their words.
    expect(watchdogPromptAfterEnable('my own words')).toBe('my own words');
    expect(watchdogPromptAfterEnable('  keep mine  ')).toBe('  keep mine  ');
  });
});