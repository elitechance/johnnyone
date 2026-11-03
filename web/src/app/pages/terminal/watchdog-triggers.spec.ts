// The two "Also nudge when" checkboxes must PERSIST, and hydrate back.
//
// The bug this pins: both `(change)` handlers were `wdBlockedNudge.set(!wdBlockedNudge())` — a local
// signal flip that wrote nothing anywhere. The checkboxes unchecked themselves on the next page load,
// and the sweep never read a flag because no flag existed. The first label promised "fires at once,
// ignores the interval", a promise no code kept.
//
// TerminalPage itself cannot be TestBed-compiled here (`web/vitest.config.ts` has no
// `@analogjs/vitest-angular`, so a `templateUrl` component cannot be instantiated), so this drives the
// REAL `JohnnyApiService` with a stub `GraphQLClient` — the same idiom as
// `watchdog-nudge-now.spec.ts` — and inspects the operation the client actually received.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom, of } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';
import {
  WATCHDOG_SIGNAL_DEFAULTS,
  runWatchdogAction,
  watchdogSettingsFromSession,
} from './watchdog-logic';

class StubClient {
  sent: { op: string; variables: Record<string, unknown> | undefined }[] = [];
  payload: unknown = {};
  query<T>(op: string, variables?: Record<string, unknown>) {
    this.sent.push({ op, variables });
    return of(this.payload as T);
  }
  queryPreferLocalHost<T>(workerOp: string, _localOp: string, variables?: Record<string, unknown>) {
    this.sent.push({ op: workerOp, variables });
    return of(this.payload as T);
  }
  mutate<T>(op: string, variables?: Record<string, unknown>) {
    this.sent.push({ op, variables });
    return of(this.payload as T);
  }
}

describe('watchdog trigger checkboxes', () => {
  let api: JohnnyApiService;
  let gql: StubClient;

  beforeEach(() => {
    gql = new StubClient();
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [JohnnyApiService, { provide: GraphQLClient, useValue: gql }],
    });
    api = TestBed.inject(JohnnyApiService);
  });

  const last = () => gql.sent[gql.sent.length - 1];

  it('persists the Also nudge when checkboxes', async () => {
    gql.payload = { updateSessionWatchdogTriggers: true };

    // Exactly what TerminalPage's two `(change)` handlers do: flip one flag, keep the sibling as it
    // stands, and persist the PAIR through `runWatchdogAction` so a refusal reaches `wdError`.
    const error = await runWatchdogAction(
      async () => firstValueFrom(api.updateSessionWatchdogTriggers('s1', true, false)),
      'Could not save the nudge triggers — the server refused.',
    );
    expect(error, 'a accepted save reports no error').toBeNull();

    expect(gql.sent.length, 'one tap sends exactly one operation').toBe(1);
    const { op, variables } = last();

    // The mutation NAME — a document that mentions only `updateSessionWatchdog` would re-save
    // enabled/intervalSecs and persist neither checkbox.
    expect(op, 'the document must name the triggers mutation').toContain('updateSessionWatchdogTriggers');
    expect(op).toContain('$sessionId');
    expect(op).toContain('$onBlocked: Boolean!');
    expect(op).toContain('$onStaleReport: Boolean!');

    // BOTH booleans ride along, under the names the resolver reads.
    expect(variables).toEqual({ sessionId: 's1', onBlocked: true, onStaleReport: false });
    expect(JSON.stringify(variables)).toContain('onBlocked');
    expect(JSON.stringify(variables)).toContain('onStaleReport');

    // And the OTHER combination persists too — the pair, not one flag at a time.
    gql.sent = [];
    await firstValueFrom(api.updateSessionWatchdogTriggers('s1', false, true));
    expect(last().variables).toEqual({ sessionId: 's1', onBlocked: false, onStaleReport: true });
  });

  it('surfaces a refused trigger save as an error message', async () => {
    await expect(
      runWatchdogAction(async () => {
        throw new Error('offline node');
      }, 'Could not save the nudge triggers — the server refused.'),
    ).resolves.toBe('Could not save the nudge triggers — the server refused.');
  });

  it('hydrates the Also nudge when checkboxes', () => {
    // A row with both triggers armed must seed both signals — without this they read false on every
    // page load, which was defect 3 of the original watchdog bug.
    const armed = watchdogSettingsFromSession({
      watchdogEnabled: true,
      watchdogIntervalSecs: 300,
      watchdogPrompt: 'Report.',
      watchdogOnBlocked: true,
      watchdogOnStaleReport: true,
    });
    expect(armed.onBlocked, 'the blocked checkbox must hydrate as checked').toBe(true);
    expect(armed.onStaleReport, 'the stale-report checkbox must hydrate as checked').toBe(true);

    // Independently: one on, one off must not blur together.
    const one = watchdogSettingsFromSession({ watchdogOnBlocked: true });
    expect(one.onBlocked).toBe(true);
    expect(one.onStaleReport, 'the sibling flag must not be inferred').toBe(false);

    // Nullable on the wire (deploy skew) and on rows predating migration 029 ⇒ unchecked, never
    // `undefined` leaking into a signal.
    const cold = watchdogSettingsFromSession({ watchdogEnabled: true });
    expect(cold.onBlocked).toBe(false);
    expect(cold.onStaleReport).toBe(false);
    expect(watchdogSettingsFromSession(null)).toEqual(WATCHDOG_SIGNAL_DEFAULTS);
    expect(WATCHDOG_SIGNAL_DEFAULTS.onBlocked).toBe(false);
    expect(WATCHDOG_SIGNAL_DEFAULTS.onStaleReport).toBe(false);
  });
});