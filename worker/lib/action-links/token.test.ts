import { describe, it, expect } from 'vitest';
import {
  ACTION_LINK_AUD,
  ACTION_LINK_ISS,
  base64UrlEncode,
  signActionToken,
  verifyActionToken,
  type ActionClaims,
} from './token';
import { validateActionArgs, describeTarget } from './actions';

const SECRET = 'test-action-link-secret-0123456789abcdef';
const NOW = 1_800_000_000;

function claims(over: Partial<ActionClaims> = {}): ActionClaims {
  return {
    iss: ACTION_LINK_ISS,
    aud: ACTION_LINK_AUD,
    iat: NOW,
    exp: NOW + 120,
    jti: '11111111-2222-4333-8444-555555555555',
    tenant: 't',
    sub: 'u',
    act: 'answer',
    ...over,
  };
}

const enc = (v: unknown) => base64UrlEncode(new TextEncoder().encode(JSON.stringify(v)));

describe('action-link token', () => {
  it('valid → ok with the claims', async () => {
    const v = await verifyActionToken(await signActionToken(claims(), SECRET), SECRET, NOW + 1);
    expect(v.ok).toBe(true);
  });

  it('exp passed → expired (exp + 5 s skew is the last accepted second)', async () => {
    const t = await signActionToken(claims(), SECRET);
    expect((await verifyActionToken(t, SECRET, NOW + 125)).ok).toBe(true);
    expect(await verifyActionToken(t, SECRET, NOW + 126)).toMatchObject({ ok: false, code: 'expired' });
  });

  it('iat in the future beyond the skew → not_yet_valid', async () => {
    const t = await signActionToken(claims(), SECRET);
    expect(await verifyActionToken(t, SECRET, NOW - 6)).toMatchObject({ ok: false, code: 'not_yet_valid' });
  });

  it('a lifetime longer than 120 s is refused even when correctly signed', async () => {
    const t = await signActionToken(claims({ exp: NOW + 3600 }), SECRET);
    expect(await verifyActionToken(t, SECRET, NOW)).toMatchObject({ ok: false, code: 'bad_claims' });
  });

  it('wrong aud / iss → rejected', async () => {
    expect(await verifyActionToken(await signActionToken(claims({ aud: 'x' }), SECRET), SECRET, NOW)).toMatchObject({ code: 'wrong_aud' });
    expect(await verifyActionToken(await signActionToken(claims({ iss: 'x' }), SECRET), SECRET, NOW)).toMatchObject({ code: 'wrong_iss' });
  });

  it('tampered signature / payload / foreign key → bad_signature', async () => {
    const t = await signActionToken(claims(), SECRET);
    const [h, p, s] = t.split('.');
    const s2 = (s[0] === 'A' ? 'B' : 'A') + s.slice(1);
    expect(await verifyActionToken(`${h}.${p}.${s2}`, SECRET, NOW)).toMatchObject({ code: 'bad_signature' });
    expect(await verifyActionToken(`${h}.${enc(claims({ act: 'run_phase' }))}.${s}`, SECRET, NOW)).toMatchObject({ code: 'bad_signature' });
    const other = await signActionToken(claims(), 'a-completely-different-secret-0123456789');
    expect(await verifyActionToken(other, SECRET, NOW)).toMatchObject({ code: 'bad_signature' });
  });

  it('alg none / alg confusion headers → bad_header', async () => {
    const t = await signActionToken(claims(), SECRET);
    const [, p, s] = t.split('.');
    expect(await verifyActionToken(`${enc({ alg: 'none', typ: 'JWT' })}.${p}.`, SECRET, NOW)).toMatchObject({ code: 'bad_header' });
    expect(await verifyActionToken(`${enc({ alg: 'HS512', typ: 'JWT' })}.${p}.${s}`, SECRET, NOW)).toMatchObject({ code: 'bad_header' });
  });

  it('a non-allowlisted act in a signed token → not_allowlisted', async () => {
    const a = await signActionToken(claims({ act: 'shell' as any }), SECRET);
    expect(await verifyActionToken(a, SECRET, NOW)).toMatchObject({ code: 'not_allowlisted' });
  });

  it('a non-uuid jti or missing tenant/sub → bad_claims', async () => {
    expect(await verifyActionToken(await signActionToken(claims({ jti: 'x' }), SECRET), SECRET, NOW)).toMatchObject({ code: 'bad_claims' });
    expect(await verifyActionToken(await signActionToken(claims({ tenant: '' }), SECRET), SECRET, NOW)).toMatchObject({ code: 'bad_claims' });
    expect(await verifyActionToken(await signActionToken(claims({ sub: '' }), SECRET), SECRET, NOW)).toMatchObject({ code: 'bad_claims' });
  });

  it('malformed input never throws', async () => {
    for (const t of ['', 'a.b', 'a.b.c.d', 'x'.repeat(5000), 123, null, '!!.!!.!!']) {
      const v = await verifyActionToken(t as any, SECRET, NOW);
      expect(v.ok).toBe(false);
    }
  });

  it('a missing / short secret fails closed (throws, never verifies)', async () => {
    const t = await signActionToken(claims(), SECRET);
    await expect(verifyActionToken(t, undefined, NOW)).rejects.toThrow(/not configured/);
    await expect(verifyActionToken(t, 'short', NOW)).rejects.toThrow(/not configured/);
  });
});

describe('action allowlist', () => {
  it('accepts the four v1 actions with their exact args', () => {
    expect(validateActionArgs('answer', { sessionId: 's', reply: 'yes' }).ok).toBe(true);
    expect(validateActionArgs('approve_plan', { planId: 'p' }).ok).toBe(true);
    expect(validateActionArgs('run_phase', { planId: 'p', phaseId: '02-x' }).ok).toBe(true);
    expect(validateActionArgs('ack', {}).ok).toBe(true);
  });
  it('refuses anything else', () => {
    expect(validateActionArgs('delete_plan', {}).ok).toBe(false);
    expect(validateActionArgs('answer', { sessionId: 's' }).ok).toBe(false);
    expect(validateActionArgs('answer', { sessionId: 's', reply: 'x'.repeat(201) }).ok).toBe(false);
    expect(validateActionArgs('approve_plan', { planId: '../x' }).ok).toBe(false);
    expect(validateActionArgs('ack', []).ok).toBe(false);
    expect(validateActionArgs('answer', { sessionId: 's', reply: 'a\u001b[A' }).ok).toBe(false);
  });
  it('targets are short and derived from args', () => {
    expect(describeTarget('approve_plan', { planId: '0123456789abcdef' })).toBe('Accept plan 01234567 and start planning');
  });
});
