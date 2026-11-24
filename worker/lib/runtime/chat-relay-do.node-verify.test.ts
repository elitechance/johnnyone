/**
 * Hotfix live-chat-2: the plan chat was still not live after #83. The desktop's relay socket
 * authenticates with its stored `jk_` API key, and L-1-2 skipped every publish from an API-key
 * socket, so nothing reached the web's subscriptions. Publishing is now tied to the VERIFIED desktop
 * node: the `nodeId` the desktop registered (and sends at upgrade) must be the node the server
 * resolves for this tenant+user from `desktop_nodes`. JWT or API key alike; nothing else publishes.
 *
 * REAL Workers DO runtime (runInDurableObject) + real D1, like chat-relay-do.upgrade.test.ts.
 */
import { describe, it, expect, vi } from 'vitest';
import { ChatRelayDO, mayPublishForSubscriptions, verifiedDesktopNode } from './chat-relay-do';
import { getSocketAuth } from '../auth/socket-auth-context';
import { env, runInDurableObject } from 'cloudflare:test';
import { signJwt } from 'modules/auth/auth-middleware';
import { generateApiKey, sha256Hex } from '../auth/api-key';

const SECRET = 'test-secret-for-phase-00';

// Default tenant/user from seeds (satisfy FKs for desktop_nodes)
const DEFAULT_TENANT = '00000000-0000-0000-0000-000000000001';
const DEFAULT_USER = '00000000-0000-0000-0000-000000000002';

const MINIMAL_MIGRATIONS_SQL = `
CREATE TABLE IF NOT EXISTS tenants (
  tenant_id TEXT PRIMARY KEY,
  preferred_tenant_id TEXT NOT NULL UNIQUE,
  display_name TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  plan TEXT NOT NULL DEFAULT 'free',
  settings TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL,
  email TEXT NOT NULL,
  display_name TEXT,
  password_hash TEXT,
  status TEXT NOT NULL DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS desktop_nodes (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  hostname TEXT NOT NULL,
  os TEXT,
  arch TEXT,
  version TEXT,
  status TEXT NOT NULL DEFAULT 'offline',
  capabilities TEXT NOT NULL DEFAULT '[]',
  last_heartbeat_at TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS api_keys (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  name TEXT NOT NULL,
  key_hash TEXT NOT NULL,
  key_prefix TEXT NOT NULL,
  scopes TEXT NOT NULL DEFAULT '[]',
  last_used_at TEXT,
  expires_at TEXT,
  revoked_at TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
`;

// Seed helper using REAL D1 (no mocks). Idempotent.
// Uses prepare/run per statement to avoid D1 .exec parser issues with multi-line strings.
async function seedTestData(db: D1Database, nodeId: string, tenantId = DEFAULT_TENANT, userId = DEFAULT_USER) {
  // Split the migration SQL into individual statements (handle newlines and ;)
  const statements = MINIMAL_MIGRATIONS_SQL
    .split(';')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && !s.startsWith('--'));

  for (const stmt of statements) {
    try {
      await db.prepare(stmt + ';').run();
    } catch (e: any) {
      // Ignore "already exists" type errors for IF NOT EXISTS cases
      if (!/already exists|duplicate/i.test(String(e))) {
        throw e;
      }
    }
  }

  await db.batch([
    db.prepare(
      `INSERT OR IGNORE INTO tenants (tenant_id, preferred_tenant_id, display_name, is_active, plan, settings, created_at, updated_at)
       VALUES (?, 'default', 'Test Tenant', 1, 'pro', '{}', datetime('now'), datetime('now'))`
    ).bind(tenantId),
    db.prepare(
      `INSERT OR IGNORE INTO users (id, tenant_id, email, display_name, password_hash, status, created_at, updated_at)
       VALUES (?, ?, 'test@local', 'Test User', '', 'active', datetime('now'), datetime('now'))`
    ).bind(userId, tenantId),
    db.prepare(
      `INSERT OR IGNORE INTO desktop_nodes (id, tenant_id, user_id, hostname, status, is_deleted, last_heartbeat_at, created_at, updated_at)
       VALUES (?, ?, ?, 'test-host', 'online', 0, datetime('now'), datetime('now'), datetime('now'))`
    ).bind(nodeId, tenantId, userId),
  ]);
}

// Helper to seed api key row for jk_ tests. Uses real D1.
async function seedApiKey(db: D1Database, keyId: string, secret: string, tenantId = DEFAULT_TENANT, userId = DEFAULT_USER, scopes = ['terminal:read', 'terminal:write'], revokedAt: string | null = null, expiresAt: string | null = null) {
  const keyHash = await sha256Hex(secret);
  const keyPrefix = `jk_${keyId}`;
  await db.prepare(
    `INSERT OR IGNORE INTO api_keys (id, tenant_id, user_id, name, key_hash, key_prefix, scopes, revoked_at, expires_at, is_deleted, created_at, updated_at)
     VALUES (?, ?, ?, 'test-jk', ?, ?, ?, ?, ?, 0, datetime('now'), datetime('now'))`
  ).bind(keyId, tenantId, userId, keyHash, keyPrefix, JSON.stringify(scopes), revokedAt, expiresAt).run();
}

// Helper to compute the real non-forgeable internal marker for tests (HMAC using same logic/secret as server).
async function computeInternalMarker(nodeId: string, userId: string, secret: string): Promise<string> {
  const input = `${nodeId}|${userId}`;
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    'raw',
    enc.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign']
  );
  const signature = await crypto.subtle.sign('HMAC', key, enc.encode(input));
  const bytes = new Uint8Array(signature);
  let binary = '';
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}


const DELTA = JSON.stringify({ type: 'chat_delta', data: { relayId: 'r1', sessionId: 's1', delta: 'hi', chunkType: 'text', isFinal: false } });

function stubFor(name: string) {
  const ns = (env as any).CHAT_RELAY_DO;
  return ns.get(ns.idFromName(name));
}

/** Upgrade on the accepting (forwarded) path, as the forwarder sends it, then report the socket. */
async function upgradeAndSend(opts: {
  name: string;
  token: string;
  clientType: string;
  claimedNodeId: string | null;
  resolvedNodeId: string;
  userId: string;
}) {
  let out: { status: number; verified?: string; published: boolean } = { status: 0, published: false };
  await runInDurableObject(stubFor(opts.name), async (instance: ChatRelayDO, state: DurableObjectState) => {
    const marker = await computeInternalMarker(opts.resolvedNodeId, opts.userId, SECRET);
    const q = new URLSearchParams({ clientType: opts.clientType, token: opts.token, nodeId: opts.resolvedNodeId });
    if (opts.claimedNodeId !== null) q.set('claimedNodeId', opts.claimedNodeId);
    const res = await (instance as any).handleWebSocketUpgrade(
      new Request(`https://x/websocket-resolved?${q}`, { headers: { Upgrade: 'websocket', 'x-internal-forward': marker } }),
    );
    out.status = res.status;
    if (res.status !== 101) return;
    const ws = state.getWebSockets().at(-1)!;
    const auth = getSocketAuth(ws, state);
    out.verified = auth?.desktopNodeId;
    const spy = vi.spyOn(instance as any, 'publishForSubscriptions').mockResolvedValue(undefined);
    await instance.webSocketMessage(ws, DELTA);
    out.published = spy.mock.calls.length > 0;
    spy.mockRestore();
  });
  return out;
}

describe('hotfix live-chat-2: only the verified desktop node feeds the chat subscriptions', () => {
  it('a jk_-authenticated registered desktop node publishes', async () => {
    const db = (env as any).DB as D1Database;
    const user = 'user-hotfix-a';
    await seedTestData(db, 'node-a', DEFAULT_TENANT, user);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, user, ['terminal:read', 'terminal:write']);
    const r = await upgradeAndSend({ name: 'hf-a', token: g.token, clientType: 'desktop', claimedNodeId: 'node-a', resolvedNodeId: 'node-a', userId: user });
    expect(r.status).toBe(101);
    expect(r.verified).toBe('node-a');
    expect(r.published).toBe(true);
  });

  it('a JWT-authenticated registered desktop node publishes too', async () => {
    const db = (env as any).DB as D1Database;
    const user = 'user-hotfix-j';
    await seedTestData(db, 'node-j', DEFAULT_TENANT, user);
    const token = await signJwt({ sub: user, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) + 3600 }, SECRET);
    const r = await upgradeAndSend({ name: 'hf-j', token, clientType: 'desktop', claimedNodeId: 'node-j', resolvedNodeId: 'node-j', userId: user });
    expect(r.verified).toBe('node-j');
    expect(r.published).toBe(true);
  });

  it('a jk_ client that is not the registered node does not publish', async () => {
    const db = (env as any).DB as D1Database;
    const user = 'user-hotfix-b';
    await seedTestData(db, 'node-b', DEFAULT_TENANT, user);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, user, ['terminal:read', 'terminal:write']);
    // `clientType=desktop` with no / a wrong node id, and a correct node id on a non-desktop socket.
    for (const [clientType, claimed] of [['desktop', null], ['desktop', 'node-forged'], ['api', 'node-b'], ['mobile', 'node-b']] as const) {
      const r = await upgradeAndSend({ name: `hf-b-${clientType}-${claimed}`, token: g.token, clientType, claimedNodeId: claimed, resolvedNodeId: 'node-b', userId: user });
      expect(r.status, `${clientType}/${claimed}`).toBe(101);
      expect(r.verified, `${clientType}/${claimed}`).toBeUndefined();
      expect(r.published, `${clientType}/${claimed}`).toBe(false);
    }
  });

  it("another user's node does not publish for this user", async () => {
    const db = (env as any).DB as D1Database;
    await seedTestData(db, 'node-victim', DEFAULT_TENANT, 'user-victim');
    const attacker = 'user-attacker';
    await seedTestData(db, 'node-attacker', DEFAULT_TENANT, attacker);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, attacker, ['terminal:read', 'terminal:write']);
    // The attacker claims the victim's node id; the server resolves the ATTACKER's node.
    const r = await upgradeAndSend({ name: 'hf-x', token: g.token, clientType: 'desktop', claimedNodeId: 'node-victim', resolvedNodeId: 'node-attacker', userId: attacker });
    expect(r.verified).toBeUndefined();
    expect(r.published).toBe(false);
  });

  it('the forwarder carries the claimed node id and overwrites a client-supplied claimedNodeId', async () => {
    const db = (env as any).DB as D1Database;
    const user = 'user-hotfix-f';
    await seedTestData(db, 'node-f', DEFAULT_TENANT, user);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, user, ['terminal:read', 'terminal:write']);
    const forwarded: Request[] = [];
    const fakeNs = {
      idFromName: (n: string) => n,
      get: () => ({ fetch: async (req: Request) => (forwarded.push(req), new Response('fwd')) }),
    };
    const relay = new ChatRelayDO({ acceptWebSocket: vi.fn(), getTags: () => [], getWebSockets: () => [] } as any, {
      JWT_SECRET: SECRET, DB: db, CHAT_RELAY_DO: fakeNs,
    });
    await (relay as any).handleWebSocketUpgrade(
      new Request(`https://x/websocket?clientType=desktop&nodeId=node-f&claimedNodeId=node-evil&token=${g.token}`, { headers: { Upgrade: 'websocket' } }),
    );
    expect(forwarded.length).toBe(1);
    const q = new URL(forwarded[0].url).searchParams;
    expect(q.getAll('claimedNodeId')).toEqual(['node-f']);
    expect(q.get('nodeId')).toBe('node-f');
  });

  it('the pure rules', () => {
    expect(verifiedDesktopNode('desktop', 'n', 'n')).toBe('n');
    expect(verifiedDesktopNode('desktop', 'n', 'm')).toBeUndefined();
    expect(verifiedDesktopNode('api', 'n', 'n')).toBeUndefined();
    expect(verifiedDesktopNode('desktop', '', 'n')).toBeUndefined();
    expect(mayPublishForSubscriptions({ clientType: 'desktop', desktopNodeId: 'n' })).toBe(true);
    expect(mayPublishForSubscriptions({ clientType: 'desktop' })).toBe(false);
    expect(mayPublishForSubscriptions({ clientType: 'api', desktopNodeId: 'n' })).toBe(false);
    expect(mayPublishForSubscriptions(null)).toBe(false);
  });
});
