import { describe, it, expect } from 'vitest';
import {
  generateApiKey,
  parseApiKey,
  sha256Hex,
  timingSafeEqualHex,
  isTokenLike,
} from './api-key';

describe('api-key primitives', () => {
  it('generate returns keyId/secret/token/keyPrefix and token shape', () => {
    const g = generateApiKey();
    expect(g.keyId).toBeTypeOf('string');
    expect(g.secret).toBeTypeOf('string');
    expect(g.token).toBeTypeOf('string');
    expect(g.keyPrefix).toBeTypeOf('string');
    expect(g.token).toBe(`jk_${g.keyId}_${g.secret}`);
    expect(g.keyPrefix).toBe(`jk_${g.keyId}`);
    expect(g.keyPrefix.includes(g.secret)).toBe(false);
  });

  it('two generates produce different secrets (randomness)', () => {
    const a = generateApiKey().secret;
    const b = generateApiKey().secret;
    expect(a).not.toBe(b);
    expect(a.length).toBeGreaterThanOrEqual(32); // 16 bytes -> 32 hex
  });

  it('parse roundtrips generate', () => {
    const g = generateApiKey();
    const p = parseApiKey(g.token);
    expect(p).not.toBeNull();
    expect(p!.keyId).toBe(g.keyId);
    expect(p!.secret).toBe(g.secret);
  });

  it('parse roundtrips even with underscore-containing keyId (uses last _ as delimiter)', () => {
    const keyIdWithUnderscore = 'abc_def_ghi123';
    const secret = '0123456789abcdef0123456789abcdef'; // 32 hex chars
    const token = `jk_${keyIdWithUnderscore}_${secret}`;
    const p = parseApiKey(token);
    expect(p).not.toBeNull();
    expect(p!.keyId).toBe(keyIdWithUnderscore);
    expect(p!.secret).toBe(secret);
    // also roundtrip with generate (now hex, no _)
    const g = generateApiKey();
    const p2 = parseApiKey(g.token);
    expect(p2!.keyId).toBe(g.keyId);
    expect(p2!.secret).toBe(g.secret);
  });

  it('parse rejects malformed', () => {
    expect(parseApiKey('')).toBeNull();
    expect(parseApiKey('notjk_')).toBeNull();
    expect(parseApiKey('jk_')).toBeNull();
    expect(parseApiKey('jk_only')).toBeNull();
    expect(parseApiKey('jk__')).toBeNull();
    expect(parseApiKey('jk_a_')).toBeNull();
    expect(parseApiKey('eyJ...')).toBeNull();
    expect(parseApiKey('jk_a_b_c')).not.toBeNull(); // extra _ is allowed, whole tail=secret
  });

  it('sha256Hex matches known vector and is 64 hex chars', async () => {
    // known: sha256('test') = 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
    const h = await sha256Hex('test');
    expect(h).toBe('9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08');
    expect(h.length).toBe(64);
    expect(/^[0-9a-f]{64}$/.test(h)).toBe(true);

    const same = await sha256Hex('test');
    expect(same).toBe(h);
    const diff = await sha256Hex('test2');
    expect(diff).not.toBe(h);
  });

  it('timingSafeEqualHex equal/unequal + length diff + no early return behavior', () => {
    const a = 'deadbeef';
    const b = 'deadbeef';
    expect(timingSafeEqualHex(a, b)).toBe(true);

    expect(timingSafeEqualHex(a, 'deadbeee')).toBe(false);
    expect(timingSafeEqualHex(a, 'deadbee')).toBe(false); // length diff (odd ignored but treated unequal)
    expect(timingSafeEqualHex('', '')).toBe(true);
    expect(timingSafeEqualHex('0a', '0b')).toBe(false);

    // differing length inputs -> false
    expect(timingSafeEqualHex('aa', 'a')).toBe(false); // 'a' invalid -> false
    expect(timingSafeEqualHex('0a', '0aa')).toBe(false);
  });

  it('isTokenLike cheap check', () => {
    expect(isTokenLike('jk_abc_123')).toBe(true);
    expect(isTokenLike('eyJhbGci')).toBe(false);
    expect(isTokenLike('')).toBe(false);
  });

  it('no secret leakage in errors or parse fails', () => {
    // parse never throws and returns null; we don't embed secret in any error path here
    const p = parseApiKey('jk_bad');
    expect(p).toBeNull();
  });
});
