import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  clockOffsetMs,
  usedCopy,
  ACT_COPY,
  OPEN_IN_J1_PATH,
  PREVIEW_QUERY,
  RUN_MUTATION,
  canConfirm,
  confirmRequest,
  formatCountdown,
  graphqlField,
  offersOpenInJ1,
  previewRequest,
  readToken,
  secondsLeft,
  tick,
  viewFromPreview,
  viewFromResult,
  type ActView,
  type ActionLinkPreview,
} from './act-page-logic';

const TOKEN = 'eyJhbGciOiJIUzI1NiJ9.eyJqdGkiOiJ4In0.c2lnbmF0dXJl';
const T0 = 1_800_000_000; // seconds
const at = (s: number) => (T0 + s) * 1000; // ms

const valid: ActionLinkPreview = {
  state: 'valid',
  act: 'run_phase',
  label: 'Run phase 02',
  target: 'Run phase 02-api of plan 12345678',
  expiresAt: T0 + 120,
  message: null,
};

describe('act page: token', () => {
  it('reads a JWT-shaped token and refuses anything else', () => {
    expect(readToken(TOKEN)).toBe(TOKEN);
    expect(readToken(` ${TOKEN} `)).toBe(TOKEN);
    for (const bad of [null, undefined, '', 'abc', 'a.b', 'a.b.c.d', 'a.b.<script>', 'x'.repeat(3000)]) {
      expect(readToken(bad as any)).toBeNull();
    }
  });
});

describe('act page: countdown', () => {
  it('counts whole seconds down to 0 and never below', () => {
    expect(secondsLeft(T0 + 120, at(0))).toBe(120);
    expect(secondsLeft(T0 + 120, at(119.4))).toBe(0); // floor(0.6)
    expect(secondsLeft(T0 + 120, at(60.2))).toBe(59);
    expect(secondsLeft(T0 + 120, at(500))).toBe(0);
    expect(secondsLeft(null, at(0))).toBe(0);
  });

  it('formats m:ss', () => {
    expect(formatCountdown(120)).toBe('2:00');
    expect(formatCountdown(65)).toBe('1:05');
    expect(formatCountdown(9)).toBe('0:09');
    expect(formatCountdown(-3)).toBe('0:00');
  });

  it('a ready view ticks down and turns into expired at 0', () => {
    let v = viewFromPreview(valid, at(0));
    expect(v).toMatchObject({ kind: 'ready', countdown: '2:00', label: 'Run phase 02', action: 'Run a phase' });
    v = tick(v, valid.expiresAt, at(61));
    expect(v).toMatchObject({ kind: 'ready', countdown: '0:59' });
    v = tick(v, valid.expiresAt, at(120));
    expect(v).toEqual({ kind: 'expired', label: 'Run phase 02', target: valid.target, message: ACT_COPY.expired });
    expect(tick(v, valid.expiresAt, at(0))).toBe(v); // expired stays expired
  });
});

describe('act page: expired state', () => {
  it('a server-expired link shows its label, the expired copy and "Open in J1"', () => {
    const v = viewFromPreview({ ...valid, state: 'expired', message: ACT_COPY.expired }, at(200));
    expect(v).toMatchObject({ kind: 'expired', label: 'Run phase 02', message: ACT_COPY.expired });
    expect(canConfirm(v)).toBe(false);
    expect(offersOpenInJ1(v)).toBe(true);
    expect(OPEN_IN_J1_PATH).toBe('/initiatives');
  });

  it('a "valid" preview whose clock already ran out (slow phone) is expired, not confirmable', () => {
    const v = viewFromPreview(valid, at(121));
    expect(v.kind).toBe('expired');
    expect(confirmRequest(v, TOKEN)).toBeNull();
  });
});

describe('act page: confirm only on POST', () => {
  it('the preview is a POST that cannot consume (a query, not runActionLink)', () => {
    const r = previewRequest(TOKEN);
    expect(r.method).toBe('POST');
    expect(JSON.parse(r.body).query).toBe(PREVIEW_QUERY);
    expect(r.body).not.toContain('runActionLink');
  });

  it('confirm builds the runActionLink POST only from a ready view', () => {
    const ready = viewFromPreview(valid, at(5));
    const r = confirmRequest(ready, TOKEN)!;
    expect(r.method).toBe('POST');
    expect(JSON.parse(r.body)).toEqual({ query: RUN_MUTATION, variables: { t: TOKEN } });
    const notReady: ActView[] = [
      { kind: 'loading' },
      { kind: 'cancelled', message: ACT_COPY.cancelled },
      { kind: 'used', label: null, message: ACT_COPY.used },
      { kind: 'invalid', message: ACT_COPY.invalid },
      { kind: 'confirming', label: 'x', action: 'y', target: 'z' },
      { kind: 'done', summary: 'ok' },
      viewFromPreview(valid, at(130)),
    ];
    for (const v of notReady) expect(confirmRequest(v, TOKEN), v.kind).toBeNull();
  });

  it('neither request carries the session: no Authorization, no tenant, no cookies, no referrer', () => {
    for (const r of [previewRequest(TOKEN), confirmRequest(viewFromPreview(valid, at(0)), TOKEN)!]) {
      expect(Object.keys(r.headers).map((h) => h.toLowerCase())).not.toContain('authorization');
      expect(Object.keys(r.headers).map((h) => h.toLowerCase())).not.toContain('x-tenant-id');
      expect(r.credentials).toBe('omit');
      expect(r.referrerPolicy).toBe('no-referrer');
    }
  });
});

describe('act page: error copy', () => {
  it('maps each refusal code to its copy', () => {
    expect(viewFromResult({ ok: false, code: 'expired', summary: '' })).toMatchObject({ kind: 'expired', message: 'This link expired, ask J1 for a new one.' });
    expect(viewFromResult({ ok: false, code: 'used', summary: '' })).toMatchObject({ kind: 'used', message: ACT_COPY.used });
    expect(viewFromResult({ ok: false, code: 'unavailable', summary: '' })).toMatchObject({ kind: 'failed', message: ACT_COPY.unavailable });
    expect(viewFromResult({ ok: false, code: 'failed', summary: 'Agent plan not found: /home/x' })).toEqual({ kind: 'failed', message: ACT_COPY.failed });
    expect(viewFromResult({ ok: false, code: 'whatever', summary: '' })).toMatchObject({ kind: 'invalid', message: ACT_COPY.invalid });
    expect(viewFromResult({ ok: true, code: null, summary: 'Phase 02-api is starting.' })).toEqual({ kind: 'done', summary: 'Phase 02-api is starting.' });
  });

  it('used / invalid previews show their copy and offer Open in J1, never Confirm', () => {
    for (const p of [
      { ...valid, state: 'used' as const, message: ACT_COPY.used },
      { state: 'invalid' as const, act: null, label: null, target: null, expiresAt: null, message: ACT_COPY.invalid },
    ]) {
      const v = viewFromPreview(p, at(0));
      expect(canConfirm(v)).toBe(false);
      expect(offersOpenInJ1(v)).toBe(true);
      expect((v as any).message).toBe(p.message);
    }
  });

  it('graphqlField tolerates errors and bad shapes', () => {
    expect(graphqlField({ data: { previewActionLink: valid } }, 'previewActionLink')).toEqual(valid);
    expect(graphqlField({ errors: [{ message: 'x' }] }, 'previewActionLink')).toBeNull();
    expect(graphqlField(null, 'x')).toBeNull();
    expect(graphqlField({ data: { x: 'str' } }, 'x')).toBeNull();
  });
});

describe('act page: review round 1', () => {
  it('H-1-10: the countdown runs on the server clock, not the phone clock', () => {
    // The phone is 30 s fast: its Date.now() reads T0+30 when the server says T0.
    const offset = clockOffsetMs(T0, at(30));
    expect(offset).toBe(-30_000);
    const v = viewFromPreview({ ...valid, serverNow: T0 }, at(30) + offset);
    expect(v).toMatchObject({ kind: 'ready', countdown: '2:00' });
    // without the offset the fast phone would show 1:30 — and hide Confirm 30 s early
    expect(viewFromPreview(valid, at(30))).toMatchObject({ countdown: '1:30' });
    expect(clockOffsetMs(null, at(0))).toBe(0);
    expect(PREVIEW_QUERY).toContain('serverNow');
  });

  it('H-1-5: a used link says how it ended; "Already done" offers no Open in J1', () => {
    expect(usedCopy('done')).toBe('Already done ✓');
    expect(usedCopy('failed')).toBe('Tried, but J1 could not do it, so check J1.');
    expect(usedCopy('superseded')).toBe('Another answer was already chosen.');
    expect(usedCopy('uncertain')).toBe(ACT_COPY.uncertain);
    expect(usedCopy(null)).toBe(ACT_COPY.used);
    const done = viewFromPreview({ ...valid, state: 'used', outcome: 'done' }, at(0));
    expect(done).toMatchObject({ kind: 'used', message: 'Already done ✓' });
    expect(offersOpenInJ1(done)).toBe(false);
    expect(offersOpenInJ1(viewFromPreview({ ...valid, state: 'used', outcome: 'failed' }, at(0)))).toBe(true);
    expect(PREVIEW_QUERY).toContain('outcome');
  });

  it('H-1-1 / H-1-9: uncertain and restarted have their own copy', () => {
    expect(viewFromResult({ ok: false, code: 'uncertain', summary: '' })).toEqual({
      kind: 'failed',
      message: 'It may have gone through, so check J1 before trying again.',
    });
    expect(viewFromResult({ ok: false, code: 'restarted', summary: '' })).toEqual({
      kind: 'failed',
      message: 'J1 restarted since this link was made, so ask for a new link.',
    });
  });

  it('H-1-19: Pages serves /act with Referrer-Policy: no-referrer from the first byte', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const headers = readFileSync(resolve(here, '../../../../public/_headers'), 'utf8');
    expect(headers).toMatch(/^\/act\n\s+Referrer-Policy: no-referrer$/m);
  });
});

describe('act page: review round 2', () => {
  it('H-2-25 / H-2-24b: superseded and answered results have their own copy', () => {
    expect(viewFromResult({ ok: false, code: 'superseded', summary: '' })).toMatchObject({
      kind: 'used',
      message: 'Another answer was already chosen.',
    });
    expect(viewFromResult({ ok: false, code: 'answered', summary: '' })).toMatchObject({
      kind: 'used',
      message: 'The agent already got an answer, so nothing was sent.',
    });
    expect(usedCopy('answered')).toBe(ACT_COPY.answered);
  });
});

describe('act page: follow-ups round 1', () => {
  it('C-1-1: an answer link whose agent session ended (archived) says so, on Confirm and on reuse', () => {
    // The worker's `ended` code: the desktop refused with `ended: session … is archived`.
    expect(viewFromResult({ ok: false, code: 'ended', summary: '' })).toEqual({
      kind: 'used',
      label: null,
      message: "That agent's session has ended, so nothing was sent.",
      outcome: 'ended',
    });
    expect(usedCopy('ended')).toBe("That agent's session had ended, so nothing was sent.");
    expect(
      viewFromPreview(
        { state: 'used', act: 'answer', label: 'Yes', target: null, expiresAt: null, message: null, outcome: 'ended' },
        0,
      ),
    ).toMatchObject({ kind: 'used', message: "That agent's session had ended, so nothing was sent." });
  });
});
