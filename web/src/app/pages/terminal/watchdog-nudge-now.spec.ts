// "Nudge now" must put a NUDGE on the wire, not a settings save.
//
// The bug this pins: `nudgeWatchdog()` called `updateSessionWatchdog(id, true, interval)` — a
// mutation that re-saves `enabled`/`intervalSecs` and delivers nothing. The button looked busy,
// the counter never moved, and the only code that ever typed a nudge was the 20s sweep.
//
// TerminalPage itself cannot be TestBed-compiled here (`web/vitest.config.ts` has no
// `@analogjs/vitest-angular`, so a `templateUrl` component cannot be instantiated), so this drives
// the REAL `JohnnyApiService` with a stub `GraphQLClient` — the same idiom as
// `arena/arena-api.spec.ts` — and inspects the operation the client actually received. That is the
// layer where "which mutation does the button send" is decided.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom, of } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';
import { runWatchdogAction } from './watchdog-logic';

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

describe('watchdog nudge wire', () => {
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

  it('nudge now sends the nudge mutation, not a settings save', async () => {
    gql.payload = { sendSessionWatchdogNudge: true };

    // Exactly what TerminalPage.nudgeWatchdog() does: one action, wrapped so a refusal still
    // reaches `wdError`.
    const error = await runWatchdogAction(
      async () => firstValueFrom(api.sendSessionWatchdogNudge('s1')),
      'Could not send the nudge — the server refused.',
    );
    expect(error).toBeNull();

    expect(gql.sent.length, 'the button sends exactly one operation').toBe(1);
    const { op, variables } = last();
    expect(op).toContain('sendSessionWatchdogNudge');
    expect(op).toContain('$sessionId');
    expect(variables).toEqual({ sessionId: 's1' });

    // THE assertion: the settings mutation must not be on the wire at all. Re-saving
    // enabled/intervalSecs is what the button used to do, and it delivered nothing.
    expect(op).not.toContain('updateSessionWatchdog');
    expect(JSON.stringify(variables)).not.toContain('intervalSecs');
    expect(JSON.stringify(variables)).not.toContain('enabled');
  });

  it('still surfaces a refused nudge as an error message', async () => {
    gql.payload = { sendSessionWatchdogNudge: false };
    // A falsy result is not a rejection, so the refusal path is a thrown observable.
    await expect(
      runWatchdogAction(async () => {
        throw new Error('boom');
      }, 'Could not send the nudge — the server refused.'),
    ).resolves.toBe('Could not send the nudge — the server refused.');
  });
});