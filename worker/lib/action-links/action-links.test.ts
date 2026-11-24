/**
 * action-links.test.ts — the security properties of Discord action links, against a REAL D1
 * (vitest-pool-workers `env.DB`) with the real 0005 migration applied. The desktop relay is a
 * fake that records calls; nothing here reaches a desktop, Discord or a CLI.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { env } from 'cloudflare:test';
import migrationSql from '../../d1/migrations/0005_action_links.sql?raw';
import {
  ACTION_LINK_COPY,
  USED_COPY,
  relayFailureCode,
  mintActionLink,
  previewActionLink,
  redeemActionLink,
  type ActionLinkDeps,
} from './action-links';
import { signActionToken, base64UrlEncode, ACTION_LINK_AUD, ACTION_LINK_ISS, type ActionClaims } from './token';
import { validateLabel, validateReply } from './actions';
import createActionLink from '../../resolvers/ai/create-action-link';
import runActionLink from '../../resolvers/ai/run-action-link';
import previewActionLinkResolver from '../../resolvers/ai/preview-action-link';
import { authedCtx } from '../auth/test-authed-ctx';

// Test-only key (>= 32 chars). The real ACTION_LINK_SECRET is a wrangler secret, never in git.
const SECRET = 'test-action-link-secret-0123456789abcdef';
const TENANT = '00000000-0000-0000-0000-000000000001';
const USER = '00000000-0000-0000-0000-000000000002';
const OTHER_TENANT = '00000000-0000-0000-0000-0000000000ff';

const db = (env as any).DB as D1Database;

async function exec(sql: string) {
  const noComments = sql.replace(/--.*$/gm, '');
  for (const stmt of noComments.split(';').map((s) => s.trim()).filter(Boolean)) {
    await db.prepare(stmt).run();
  }
}

async function resetDb() {
  await exec(`
    CREATE TABLE IF NOT EXISTS tenants (tenant_id TEXT PRIMARY KEY, is_active INTEGER NOT NULL DEFAULT 1);
    CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'active');
    CREATE TABLE IF NOT EXISTS desktop_nodes (id TEXT PRIMARY KEY, tenant_id TEXT, user_id TEXT, status TEXT, is_deleted INTEGER DEFAULT 0, last_heartbeat_at TEXT)
  `);
  await exec(migrationSql);
  await exec(`
    DELETE FROM action_link_redemptions;
    DELETE FROM action_link_mints;

    DELETE FROM action_link_events;
    DELETE FROM tenants;
    DELETE FROM users
  `);
  await db.prepare('INSERT INTO tenants (tenant_id, is_active) VALUES (?, 1), (?, 1)').bind(TENANT, OTHER_TENANT).run();
  await db.prepare("INSERT INTO users (id, tenant_id, status) VALUES (?, ?, 'active')").bind(USER, TENANT).run();
}

interface RelayCall {
  identity: { tenantId: string; userId: string };
  method: string;
  params: Record<string, unknown>;
}

function makeDeps(over: Partial<ActionLinkDeps> & { nowS?: number } = {}) {
  const calls: RelayCall[] = [];
  let clock = over.nowS ?? 1_800_000_000;
  const deps: ActionLinkDeps = {
    db,
    secret: SECRET,
    now: () => clock,
    relay: async (identity, method, params) => {
      calls.push({ identity, method, params });
      return method === 'perform_action_link' ? { summary: 'Done by fake desktop.' } : true;
    },
    baseUrl: 'https://web.test',
    ...over,
  };
  return {
    deps,
    calls,
    performs: () => calls.filter((c) => c.method === 'perform_action_link'),
    advance: (s: number) => {
      clock += s;
    },
  };
}

const ID = { tenantId: TENANT, userId: USER };
const RUN_PHASE = { act: 'run_phase', args: JSON.stringify({ planId: 'plan-1', phaseId: '02-api' }), label: 'Run phase 02' };

function tokenOf(url: string): string {
  return new URL(url).searchParams.get('t')!;
}

async function mint(deps: ActionLinkDeps, input = RUN_PHASE) {
  const m = await mintActionLink(deps, ID, input);
  return { ...m, token: tokenOf(m.url) };
}

function baseClaims(nowS: number, over: Partial<ActionClaims> = {}): ActionClaims {
  return {
    iss: ACTION_LINK_ISS,
    aud: ACTION_LINK_AUD,
    iat: nowS,
    exp: nowS + 120,
    jti: crypto.randomUUID(),
    tenant: TENANT,
    sub: USER,
    act: 'ack',
    ...over,
  };
}

function claimsOf(token: string): ActionClaims {
  const p = token.split('.')[1].replace(/-/g, '+').replace(/_/g, '/');
  return JSON.parse(atob(p + '='.repeat((4 - (p.length % 4)) % 4)));
}

/** Re-sign a genuinely minted link's claims (its mint row exists) with some claims changed. */
async function resign(token: string, over: Partial<ActionClaims>): Promise<string> {
  return signActionToken({ ...claimsOf(token), ...over }, SECRET);
}

async function redemptionRows() {
  const r = await db.prepare('SELECT COUNT(*) AS n FROM action_link_redemptions').first<{ n: number }>();
  return r?.n ?? 0;
}

describe('action links: mint', () => {
  beforeEach(resetDb);

  it('valid mint → a /act URL carrying a 120 s token; audit has the jti and never the token', async () => {
    const { deps } = makeDeps();
    const m = await mint(deps);
    expect(m.url.startsWith('https://web.test/act?t=')).toBe(true);
    expect(m.expiresAt).toBe(1_800_000_120);
    const events = await db.prepare('SELECT * FROM action_link_events').all<any>();
    expect(events.results.map((e) => e.kind)).toEqual(['mint']);
    expect(events.results[0].jti).toBe(m.jti);
    expect(JSON.stringify(events.results)).not.toContain(m.token);
  });

  it('a non-allowlisted act is rejected at mint (core and resolver)', async () => {
    const { deps } = makeDeps();
    await expect(mintActionLink(deps, ID, { act: 'delete_plan', args: '{}', label: 'x' })).rejects.toThrow(/not allowlisted/);
    const ctx = authedCtx({ tenantId: TENANT, userId: USER, db, env: { ACTION_LINK_SECRET: SECRET } });
    await expect(
      createActionLink(null, { input: { act: 'shell', args: '{}', label: 'x' } }, ctx),
    ).rejects.toThrow(/not allowlisted/);
    const rej = await db.prepare("SELECT COUNT(*) AS n FROM action_link_events WHERE kind='reject'").first<{ n: number }>();
    expect(rej?.n).toBe(2);
  });

  it('bad args are rejected at mint (unknown key, control chars, // key directive, phase traversal)', async () => {
    const { deps } = makeDeps();
    const bad = [
      { act: 'run_phase', args: JSON.stringify({ planId: 'p', phaseId: '02', extra: 'x' }) },
      { act: 'answer', args: JSON.stringify({ sessionId: 's1', reply: 'yes\nrm -rf /' }) },
      { act: 'answer', args: JSON.stringify({ sessionId: 's1', reply: '//key C-c' }) },
      { act: 'run_phase', args: JSON.stringify({ planId: 'p', phaseId: '..' }) },
      { act: 'approve_plan', args: 'not json' },
    ];
    for (const b of bad) {
      await expect(mintActionLink(deps, ID, { ...b, label: 'x' })).rejects.toThrow(/createActionLink/);
    }
  });

  it('createActionLink needs an identity, and the scope the action needs', async () => {
    const unauth: any = { db, env: { ACTION_LINK_SECRET: SECRET }, request: new Request('http://x'), auth: {} };
    await expect(createActionLink(null, { input: RUN_PHASE }, unauth)).rejects.toThrow(/UNAUTHENTICATED/);
    // An API-key caller granted only plans:read may mint `ack` but not `run_phase`.
    const readOnlyKey = authedCtx({
      tenantId: TENANT,
      userId: USER,
      db,
      env: { ACTION_LINK_SECRET: SECRET },
      apiKey: { id: 'k', scopes: ['plans:read'] },
    });
    await expect(createActionLink(null, { input: RUN_PHASE }, readOnlyKey)).rejects.toThrow(/missing required scope: plans:write/);
    const res = await createActionLink(null, { input: { act: 'ack', args: '{}', label: 'Got it' } }, readOnlyKey);
    expect(res.url).toContain('/act?t=');
  });

  it('every link fits a Discord link button (≤ 512 chars) even at the longest label and reply', async () => {
    const { deps } = makeDeps({ baseUrl: 'https://johnnyone.pages.dev' });
    const m = await mintActionLink(deps, ID, {
      act: 'answer',
      args: JSON.stringify({ sessionId: 'x'.repeat(128), reply: 'y'.repeat(200) }),
      label: 'z'.repeat(120),
    });
    expect(m.url.length).toBeLessThanOrEqual(512);
    const typical = await mintActionLink(deps, ID, {
      act: 'answer',
      args: JSON.stringify({ sessionId: crypto.randomUUID(), reply: 'yes' }),
      label: 'Proceed with migration 0021?',
    });
    expect(typical.url.length).toBeLessThan(420); // ~405 with the prod base URL
    // and the token itself carries no args or label
    const claims = claimsOf(tokenOf(typical.url));
    expect(Object.keys(claims).sort()).toEqual(['act', 'aud', 'exp', 'iat', 'iss', 'jti', 'sub', 'tenant']);
  });

  it('minting fails closed when ACTION_LINK_SECRET is missing or too short', async () => {
    await expect(mintActionLink(makeDeps({ secret: undefined }).deps, ID, RUN_PHASE)).rejects.toThrow(/not configured/);
    await expect(mintActionLink(makeDeps({ secret: 'short' }).deps, ID, RUN_PHASE)).rejects.toThrow(/not configured/);
    const mints = await db.prepare('SELECT COUNT(*) AS n FROM action_link_mints').first<{ n: number }>();
    expect(mints?.n).toBe(0);
  });
});

describe('action links: preview', () => {
  beforeEach(resetDb);

  it('valid → shows label, act, target and expiry only; does NOT consume', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const p1 = await previewActionLink(f.deps, m.token, null);
    const p2 = await previewActionLink(f.deps, m.token, null);
    expect(p1).toEqual({
      state: 'valid',
      act: 'run_phase',
      label: 'Run phase 02',
      target: 'Run phase 02-api of plan plan-1',
      expiresAt: m.expiresAt,
      message: null,
      outcome: null,
      serverNow: 1_800_000_000,
    });
    expect(p2.state).toBe('valid');
    expect(await redemptionRows()).toBe(0);
    expect(f.performs()).toHaveLength(0);
    // ...and the link still redeems after any number of previews.
    const r = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(r.ok).toBe(true);
  });

  it('preview leaks nothing beyond label/target: no tenant, user, jti or raw args', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const s = JSON.stringify(await previewActionLink(f.deps, m.token, null));
    expect(s).not.toContain(TENANT);
    expect(s).not.toContain(USER);
    expect(s).not.toContain(m.jti);
    expect(s).not.toContain('planId');
  });

  it('expired → state expired with the label (for "Open in J1"); tampered → invalid with nothing', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    f.advance(126);
    const p = await previewActionLink(f.deps, m.token, null);
    expect(p.state).toBe('expired');
    expect(p.label).toBe('Run phase 02');
    expect(p.message).toBe(ACTION_LINK_COPY.expired);
    const tampered = m.token.slice(0, -2) + (m.token.endsWith('AA') ? 'BB' : 'AA');
    const t = await previewActionLink(f.deps, tampered, null);
    expect(t).toEqual({
      state: 'invalid',
      act: null,
      label: null,
      target: null,
      expiresAt: null,
      message: ACTION_LINK_COPY.invalid,
      outcome: null,
      serverNow: 1_800_000_126,
    });
  });

  it('after a redeem, preview says used', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    await redeemActionLink(f.deps, m.token, null, 'POST');
    expect((await previewActionLink(f.deps, m.token, null)).state).toBe('used');
  });
});

describe('action links: redeem (runActionLink)', () => {
  beforeEach(resetDb);

  it('valid → ok; relays perform_action_link to the MINTER (identity from the token)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const r = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(r).toEqual({ ok: true, code: null, summary: 'Done by fake desktop.' });
    expect(f.performs()).toEqual([
      {
        identity: ID,
        method: 'perform_action_link',
        params: { jti: m.jti, act: 'run_phase', args: { planId: 'plan-1', phaseId: '02-api' }, exp: m.expiresAt },
      },
    ]);
    const row = await db.prepare('SELECT status FROM action_link_redemptions WHERE jti = ?').bind(m.jti).first<any>();
    expect(row.status).toBe('done');
  });

  it('exp passed (beyond the 5 s skew) → rejected, nothing relayed, nothing claimed', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    f.advance(125); // exp + 5: still inside the skew
    f.advance(1);
    const r = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(r).toEqual({ ok: false, code: 'expired', summary: ACTION_LINK_COPY.expired });
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
  });

  it('within the skew (exp + 5 s) → still ok', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    f.advance(125);
    expect((await redeemActionLink(f.deps, m.token, null, 'POST')).ok).toBe(true);
  });

  it('a second redeem of the same jti → rejected "already used"; acted exactly once', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    expect((await redeemActionLink(f.deps, m.token, null, 'POST')).ok).toBe(true);
    const again = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(again).toEqual({ ok: false, code: 'used', summary: ACTION_LINK_COPY.used });
    expect(f.performs()).toHaveLength(1);
  });

  it('concurrent redeems of one link → exactly one acts (insert-before-act on the PK)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const results = await Promise.all(Array.from({ length: 6 }, () => redeemActionLink(f.deps, m.token, null, 'POST')));
    expect(results.filter((r) => r.ok)).toHaveLength(1);
    expect(results.filter((r) => r.code === 'used')).toHaveLength(5);
    expect(f.performs()).toHaveLength(1);
  });

  it('tampered signature → rejected', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const [h, p, s] = m.token.split('.');
    const flipped = s[0] === 'A' ? 'B' + s.slice(1) : 'A' + s.slice(1);
    expect((await redeemActionLink(f.deps, `${h}.${p}.${flipped}`, null, 'POST')).code).toBe('invalid');
    // payload swapped for another action under the original signature
    const evil = base64UrlEncode(new TextEncoder().encode(JSON.stringify(baseClaims(1_800_000_000, { act: 'run_phase', args: { planId: 'other', phaseId: '01' } }))));
    expect((await redeemActionLink(f.deps, `${h}.${evil}.${s}`, null, 'POST')).code).toBe('invalid');
    // signed with a different key
    const foreign = await signActionToken(baseClaims(1_800_000_000), 'some-other-secret-0123456789abcdefgh');
    expect((await redeemActionLink(f.deps, foreign, null, 'POST')).code).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
  });

  it('wrong aud → rejected (a real minted link, re-signed with another audience)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const t = await resign(m.token, { aud: 'j1-login' });
    expect((await redeemActionLink(f.deps, t, null, 'POST')).code).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
  });

  it('wrong tenant → rejected (request names another tenant)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    expect((await redeemActionLink(f.deps, m.token, OTHER_TENANT, 'POST')).code).toBe('invalid');
    expect((await previewActionLink(f.deps, m.token, OTHER_TENANT)).state).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
    // the same link is fine for its own tenant
    expect((await redeemActionLink(f.deps, m.token, TENANT, 'POST')).ok).toBe(true);
  });

  it('wrong tenant → rejected (the minter is no longer an active member of the token tenant)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    await db.prepare("UPDATE users SET status = 'disabled' WHERE id = ?").bind(USER).run();
    expect((await redeemActionLink(f.deps, m.token, null, 'POST')).code).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
  });

  it('a signed token whose claims disagree with its mint row → rejected (tenant, act or exp swapped)', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    for (const over of [{ tenant: OTHER_TENANT }, { act: 'ack' as const }, { exp: claimsOf(m.token).exp - 1 }]) {
      expect((await redeemActionLink(f.deps, await resign(m.token, over), null, 'POST')).code).toBe('invalid');
    }
    expect(f.performs()).toHaveLength(0);
  });

  it('a correctly signed token with no mint row (e.g. a leaked key) → rejected', async () => {
    const f = makeDeps();
    const t = await signActionToken(baseClaims(1_800_000_000), SECRET);
    expect((await redeemActionLink(f.deps, t, null, 'POST')).code).toBe('invalid');
    expect((await previewActionLink(f.deps, t, null)).state).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
  });

  it('a non-allowlisted act inside a correctly signed token → rejected', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    const t = await resign(m.token, { act: 'delete_plan' as any });
    expect((await redeemActionLink(f.deps, t, null, 'POST')).code).toBe('invalid');
    expect(f.performs()).toHaveLength(0);
  });

  it('GET never acts: a non-POST redeem is refused before the token is looked at', async () => {
    const f = makeDeps({ nowS: Math.floor(Date.now() / 1000) });
    const m = await mint(f.deps);
    for (const method of ['GET', 'HEAD', null]) {
      expect((await redeemActionLink(f.deps, m.token, null, method)).ok).toBe(false);
    }
    expect(f.performs()).toHaveLength(0);
    expect(await redemptionRows()).toBe(0);
    // the resolver reads the method from the request
    const ctx: any = { db, env: { ACTION_LINK_SECRET: SECRET }, request: new Request('https://w/graphql?query=x', { method: 'GET' }) };
    expect((await runActionLink(null, { token: m.token }, ctx)).ok).toBe(false);
    expect(await redemptionRows()).toBe(0);
  });

  it('desktop offline → link is used up (fail closed) with the "unavailable" copy', async () => {
    const f = makeDeps({
      relay: async () => {
        throw new Error('No online backend app found');
      },
    });
    const m = await mint(f.deps);
    const r = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(r).toEqual({ ok: false, code: 'unavailable', summary: ACTION_LINK_COPY.unavailable });
    expect((await redeemActionLink(f.deps, m.token, null, 'POST')).code).toBe('used');
  });

  it('desktop refusal text never reaches the page', async () => {
    const f = makeDeps({
      relay: async () => {
        throw new Error('Agent plan not found: /home/secret/path');
      },
    });
    const m = await mint(f.deps);
    const r = await redeemActionLink(f.deps, m.token, null, 'POST');
    expect(r.code).toBe('failed');
    expect(r.summary).not.toContain('/home/secret');
  });

  it('runActionLink never reads or sets ctx.auth (a session credential on the request is ignored)', async () => {
    const f = makeDeps({ nowS: Math.floor(Date.now() / 1000) });
    const m = await mint(f.deps);
    const auth = { isAuthenticated: false };
    const ctx: any = {
      db,
      env: { ACTION_LINK_SECRET: SECRET, CHAT_RELAY_DO: { idFromName: () => 'x', get: () => ({ fetch: async () => new Response(JSON.stringify({ success: true, data: { summary: 'ok' } })) }) } },
      request: new Request('https://w/graphql', { method: 'POST' }),
      auth,
    };
    await db.prepare("INSERT OR REPLACE INTO desktop_nodes (id, tenant_id, user_id, status, is_deleted, last_heartbeat_at) VALUES ('n1', ?, ?, 'online', 0, datetime('now'))").bind(TENANT, USER).run();
    const r = await runActionLink(null, { token: m.token }, ctx);
    expect(r.ok).toBe(true);
    expect(ctx.auth).toEqual({ isAuthenticated: false });
    expect(Object.getOwnPropertySymbols(ctx)).toEqual([]);
  });

  it('every outcome is audited with the jti, never the token', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    await previewActionLink(f.deps, m.token, null);
    await redeemActionLink(f.deps, m.token, null, 'POST');
    await redeemActionLink(f.deps, m.token, null, 'POST');
    const rows = (await db.prepare('SELECT kind, jti, reason FROM action_link_events ORDER BY rowid').all<any>()).results;
    expect(rows.map((r) => r.kind)).toEqual(['mint', 'preview', 'redeem', 'reject']);
    expect(rows.every((r) => r.jti === m.jti)).toBe(true);
    expect(rows[3].reason).toBe('redeem: reused_jti');
    expect(JSON.stringify(rows)).not.toContain(m.token.split('.')[2]);
  });

  it('preview resolver: token-authorized, no login required, GET allowed because it never acts', async () => {
    const f = makeDeps({ nowS: Math.floor(Date.now() / 1000) });
    const m = await mint(f.deps);
    const ctx: any = { db, env: { ACTION_LINK_SECRET: SECRET, CHAT_RELAY_DO: {} }, request: new Request('https://w/graphql', { method: 'GET' }) };
    const p = await previewActionLinkResolver(null, { token: m.token }, ctx);
    expect(p.state).toBe('valid');
    expect(p.label).toBe('Run phase 02');
    expect(await redemptionRows()).toBe(0);
  });
});


describe('action links: review round 1', () => {
  beforeEach(resetDb);

  async function eventCount() {
    return (await db.prepare('SELECT COUNT(*) AS n FROM action_link_events').first<{ n: number }>())?.n ?? 0;
  }

  it('H-1-16: unsigned garbage and GET redeems write no audit row, only a log line', async () => {
    const lines: string[] = [];
    const f = makeDeps({ log: (l) => lines.push(l) });
    const m = await mint(f.deps);
    const before = await eventCount();
    const [h, p, sig] = m.token.split('.');
    const tampered = `${h}.${p}.${(sig[0] === 'A' ? 'B' : 'A') + sig.slice(1)}`;
    for (const t of [tampered, 'garbage', 'a.b.c']) {
      await previewActionLink(f.deps, t, null);
      await redeemActionLink(f.deps, t, null, 'POST');
    }
    await redeemActionLink(f.deps, m.token, null, 'GET');
    expect(await eventCount()).toBe(before);
    expect(lines.length).toBe(7);
    expect(lines.join('\n')).not.toContain(sig);
    // a SIGNED rejection is still audited
    await redeemActionLink(f.deps, m.token, OTHER_TENANT, 'POST');
    expect(await eventCount()).toBe(before + 1);
  });

  it('H-1-1: a relay timeout is "uncertain" (it may have acted), and the preview says so', async () => {
    const f = makeDeps({
      relay: async (_i, method) => {
        if (method === 'perform_action_link') throw new Error('timed out');
        return true;
      },
    });
    const m = await mint(f.deps);
    expect(await redeemActionLink(f.deps, m.token, null, 'POST')).toEqual({
      ok: false,
      code: 'uncertain',
      summary: 'It may have gone through, so check J1 before trying again.',
    });
    const p = await previewActionLink(f.deps, m.token, null);
    expect(p).toMatchObject({ state: 'used', outcome: 'uncertain', message: ACTION_LINK_COPY.uncertain });
    expect(relayFailureCode('Backend RPC failed: perform_action_link timeout')).toBe('uncertain');
  });

  it('H-1-9: a host with no record of the link (restarted) gets its own code', async () => {
    const f = makeDeps({
      relay: async (_i, method) => {
        if (method === 'perform_action_link') throw new Error('perform_action_link: unknown_link: this host has no record');
        return true;
      },
    });
    const m = await mint(f.deps);
    expect(await redeemActionLink(f.deps, m.token, null, 'POST')).toEqual({
      ok: false,
      code: 'restarted',
      summary: ACTION_LINK_COPY.restarted,
    });
  });

  it('H-1-5: a used link previews how it ended (done / failed)', async () => {
    const ok = makeDeps();
    const a = await mint(ok.deps);
    await redeemActionLink(ok.deps, a.token, null, 'POST');
    expect(await previewActionLink(ok.deps, a.token, null)).toMatchObject({ state: 'used', outcome: 'done', message: 'Already done ✓' });
    const bad = makeDeps({
      relay: async (_i, method) => {
        if (method === 'perform_action_link') throw new Error('Phase not found');
        return true;
      },
    });
    const b = await mint(bad.deps);
    await redeemActionLink(bad.deps, b.token, null, 'POST');
    expect(await previewActionLink(bad.deps, b.token, null)).toMatchObject({ state: 'used', outcome: 'failed', message: USED_COPY.failed });
  });

  const ANSWER = (reply: string) => ({
    act: 'answer',
    args: JSON.stringify({ sessionId: 'sess-1', reply }),
    label: 'Proceed with migration 0021?',
    group: 'q-0021',
  });

  it('H-1-2: redeeming one link of a group supersedes its siblings', async () => {
    const f = makeDeps();
    const yes = await mint(f.deps, ANSWER('yes'));
    const no = await mint(f.deps, ANSWER('no'));
    const other = await mint(f.deps, { ...ANSWER('no'), group: 'another-question' });
    expect((await redeemActionLink(f.deps, yes.token, null, 'POST')).ok).toBe(true);
    expect(await previewActionLink(f.deps, no.token, null)).toMatchObject({
      state: 'used',
      outcome: 'superseded',
      message: 'Another answer was already chosen.',
    });
    expect((await redeemActionLink(f.deps, no.token, null, 'POST')).code).toBe('superseded');
    expect((await previewActionLink(f.deps, other.token, null)).state).toBe('valid');
    expect(f.performs().map((c) => (c.params.args as any).reply)).toEqual(['yes']);
  });

  it('H-1-2: a Yes and a No tapped at once → exactly one acts', async () => {
    const f = makeDeps();
    const yes = await mint(f.deps, ANSWER('yes'));
    const no = await mint(f.deps, ANSWER('no'));
    const results = await Promise.all([yes, no, yes, no].map((l) => redeemActionLink(f.deps, l.token, null, 'POST')));
    expect(results.filter((r) => r.ok)).toHaveLength(1);
    expect(f.performs()).toHaveLength(1);
  });

  it('H-1-2: a group never reaches across users or tenants', async () => {
    const f = makeDeps();
    await db.prepare("INSERT INTO users (id, tenant_id, status) VALUES ('u-other', ?, 'active')").bind(TENANT).run();
    const mine = await mint(f.deps, ANSWER('yes'));
    const theirs = await mintActionLink(f.deps, { tenantId: TENANT, userId: 'u-other' }, ANSWER('no'));
    await redeemActionLink(f.deps, mine.token, null, 'POST');
    expect((await previewActionLink(f.deps, tokenOf(theirs.url), null)).state).toBe('valid');
  });

  it('H-1-2: an invalid group is refused at mint', async () => {
    const f = makeDeps();
    await expect(mintActionLink(f.deps, ID, { ...ANSWER('yes'), group: 'bad group!' })).rejects.toThrow(/group/);
  });

  it('H-1-3: the target shows the human title from the mint, ids only as a fallback', async () => {
    const f = makeDeps();
    const m = await mintActionLink(f.deps, ID, { ...RUN_PHASE, targetTitle: 'Discord action links · API surface' });
    expect((await previewActionLink(f.deps, tokenOf(m.url), null)).target).toBe('Run phase 02-api: Discord action links · API surface');
    const a = await mintActionLink(f.deps, ID, { ...ANSWER('yes'), targetTitle: 'T2 worker (worker) · Discord links' });
    expect((await previewActionLink(f.deps, tokenOf(a.url), null)).target).toBe('Reply "yes" to T2 worker (worker) · Discord links');
  });

  it('H-1-15a: replies a CLI would read as a command are refused at mint', async () => {
    const f = makeDeps();
    for (const reply of ['!rm -rf ~', ' !ls', '/compact', '# remember', '//key C-c']) {
      await expect(mintActionLink(f.deps, ID, ANSWER(reply))).rejects.toThrow(/must not start with/);
    }
    await expect(mintActionLink(f.deps, ID, ANSWER('use option 2'))).resolves.toBeTruthy();
  });

  it('H-1-17: mint prunes rows a day past exp and events after 30 days', async () => {
    const f = makeDeps();
    const old = 1_800_000_000 - 86_400 - 1;
    await db.prepare("INSERT INTO action_link_mints (jti, tenant_id, user_id, act, args_json, label, exp) VALUES ('old', ?, ?, 'ack', '{}', 'x', ?)").bind(TENANT, USER, old).run();
    await db.prepare("INSERT INTO action_link_redemptions (jti, tenant_id, user_id, act, exp) VALUES ('old', ?, ?, 'ack', ?)").bind(TENANT, USER, old).run();
    await db.prepare("INSERT INTO action_link_events (id, kind, created_at) VALUES ('e-old', 'mint', datetime('now', '-31 days'))").run();
    await db.prepare("INSERT INTO action_link_events (id, kind, created_at) VALUES ('e-new', 'mint', datetime('now', '-29 days'))").run();
    await mint(f.deps);
    expect(await db.prepare("SELECT jti FROM action_link_mints WHERE jti = 'old'").first()).toBeNull();
    expect(await db.prepare("SELECT jti FROM action_link_redemptions WHERE jti = 'old'").first()).toBeNull();
    expect(await db.prepare("SELECT id FROM action_link_events WHERE id = 'e-old'").first()).toBeNull();
    expect(await db.prepare("SELECT id FROM action_link_events WHERE id = 'e-new'").first()).not.toBeNull();
  });

  it('H-1-17: a link expired more than 10 minutes ago previews nothing about itself', async () => {
    const f = makeDeps();
    const m = await mint(f.deps);
    f.advance(120 + 600);
    expect((await previewActionLink(f.deps, m.token, null)).label).toBe('Run phase 02');
    f.advance(1);
    expect(await previewActionLink(f.deps, m.token, null)).toMatchObject({ state: 'expired', label: null, target: null });
  });

  it('H-1-10: the preview carries the server clock', async () => {
    const f = makeDeps({ nowS: 1_800_000_042 });
    const m = await mint(f.deps);
    expect((await previewActionLink(f.deps, m.token, null)).serverNow).toBe(1_800_000_042);
  });
});


describe('action links: review round 2', () => {
  beforeEach(resetDb);

  const ANSWER2 = (reply: string, group?: string) => ({
    act: 'answer',
    args: JSON.stringify({ sessionId: 'sess-1', reply }),
    label: 'Proceed?',
    ...(group ? { group } : {}),
  });

  it('H-2-31: Unicode format characters are refused anywhere in a reply or label', async () => {
    const f = makeDeps();
    for (const reply of ['\u200B!rm -rf ~', 'ye\u200Bs', 'yes\u2060', '\uFEFFyes', 'a\u202Eb', '\u{E0041}x', '\u00A0!ls']) {
      await expect(mintActionLink(f.deps, ID, ANSWER2(reply)), JSON.stringify(reply)).rejects.toThrow(/createActionLink/);
    }
    for (const label of ['Proceed\u200B?', '\u2066Proceed', 'x\u00ADy']) {
      await expect(mintActionLink(f.deps, ID, { ...ANSWER2('yes'), label }), JSON.stringify(label)).rejects.toThrow(/label/);
    }
    await expect(mintActionLink(f.deps, ID, { ...ANSWER2('ça va ✓'), label: 'Proceed? 日本 ✓' })).resolves.toBeTruthy();
  });

  it('H-3-48: non-Cf invisible characters (Hangul fillers, braille blank) are refused too', async () => {
    const f = makeDeps();
    for (const c of ['\u115F', '\u1160', '\u3164', '\uFFA0', '\u2800', '\u034F', '\u17B4', '\u17B5']) {
      expect(validateReply(`${c}!ls`), JSON.stringify(c)).not.toBeNull();
      expect(validateReply(`ye${c}s`), JSON.stringify(c)).not.toBeNull();
      expect(validateLabel(`Proceed${c}?`), JSON.stringify(c)).not.toBeNull();
    }
    await expect(mintActionLink(f.deps, ID, ANSWER2('\u3164!ls'))).rejects.toThrow(/createActionLink/);
    await expect(mintActionLink(f.deps, ID, { ...ANSWER2('yes'), label: 'Proceed\u3164?' })).rejects.toThrow(/label/);
    // C-1-2: invisible is its own message, not "control characters".
    expect(validateReply('ye\u3164s')).toBe('reply must not contain invisible characters');
    expect(validateReply('ye\u200Bs')).toBe('reply must not contain invisible characters');
    expect(validateLabel('Proceed\u2800?')).toBe('label must not contain invisible characters');
    expect(validateReply('yes\rno')).toBe('reply must be one line with no control characters');
    expect(validateReply('한국어 ㅎ 네')).toBeNull();
    expect(validateLabel('⠁⠃ braille text')).toBeNull();
    const m = await mintActionLink(f.deps, ID, { ...RUN_PHASE, targetTitle: 'Disc\u3164ord\u2800 links' });
    expect((await previewActionLink(f.deps, tokenOf(m.url), null)).target).toBe('Run phase 02-api: Discord links');
  });

  it('H-2-31: a format character in the desktop-supplied target title is dropped, not shown', async () => {
    const f = makeDeps();
    const m = await mintActionLink(f.deps, ID, { ...RUN_PHASE, targetTitle: 'Disc\u200Bord\u202E links' });
    expect((await previewActionLink(f.deps, tokenOf(m.url), null)).target).toBe('Run phase 02-api: Discord links');
  });

  it('H-2-29: replaying an used grouped link supersedes nothing', async () => {
    const f = makeDeps();
    const yes = await mint(f.deps, ANSWER2('yes', 'g1'));
    expect((await redeemActionLink(f.deps, yes.token, null, 'POST')).ok).toBe(true);
    // a sibling that exists after the redeem (written directly: minting into the group is refused)
    await db
      .prepare("INSERT INTO action_link_mints (jti, tenant_id, user_id, act, args_json, label, exp, group_id) VALUES ('late', ?, ?, 'answer', '{}', 'x', ?, 'g1')")
      .bind(TENANT, USER, 1_800_000_120)
      .run();
    expect((await redeemActionLink(f.deps, yes.token, null, 'POST')).code).toBe('used');
    expect(await db.prepare("SELECT jti FROM action_link_redemptions WHERE jti = 'late'").first()).toBeNull();
  });

  it('H-2-29: minting into a group that was already answered is refused', async () => {
    const f = makeDeps();
    const yes = await mint(f.deps, ANSWER2('yes', 'g2'));
    const no = await mint(f.deps, ANSWER2('no', 'g2'));
    // still open: siblings may be added until one is redeemed
    await mint(f.deps, ANSWER2('later', 'g2'));
    await redeemActionLink(f.deps, yes.token, null, 'POST');
    await expect(mintActionLink(f.deps, ID, ANSWER2('maybe', 'g2'))).rejects.toThrow(/already answered/);
    expect((await previewActionLink(f.deps, no.token, null)).outcome).toBe('superseded');
  });

  it('H-2-25: confirming a superseded sibling says another answer was chosen', async () => {
    const f = makeDeps();
    const yes = await mint(f.deps, ANSWER2('yes', 'g3'));
    const no = await mint(f.deps, ANSWER2('no', 'g3'));
    await redeemActionLink(f.deps, yes.token, null, 'POST');
    expect(await redeemActionLink(f.deps, no.token, null, 'POST')).toEqual({
      ok: false,
      code: 'superseded',
      summary: 'Another answer was already chosen.',
    });
    expect(f.performs()).toHaveLength(1);
  });

  it('H-2-24b: the desktop\'s "answered" refusal becomes its own code and used-outcome', async () => {
    const f = makeDeps({
      relay: async (_i, method) => {
        if (method === 'perform_action_link') throw new Error('perform_action_link: answered: the agent already got an answer');
        return true;
      },
    });
    const m = await mint(f.deps, ANSWER2('yes'));
    expect(await redeemActionLink(f.deps, m.token, null, 'POST')).toEqual({
      ok: false,
      code: 'answered',
      summary: 'The agent already got an answer, so nothing was sent.',
    });
    expect(await previewActionLink(f.deps, m.token, null)).toMatchObject({
      state: 'used',
      outcome: 'answered',
      message: 'The agent already got an answer, so nothing was sent.',
    });
  });
});
