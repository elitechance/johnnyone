import { describe, it, expect } from 'vitest';
import { GraphQLError } from 'graphql';
import { ChatRelayDO } from './chat-relay-do';
import { getSocketAuth } from '../auth/socket-auth-context';
import { env, runInDurableObject } from 'cloudflare:test';
import { signJwt, verifyJwtWithFallback } from 'modules/auth/auth-middleware';
import { generateApiKey, sha256Hex } from '../auth/api-key';

const SECRET = 'test-secret-for-phase-00';
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

async function seedIdentity(db: D1Database, tenantId = DEFAULT_TENANT, userId = DEFAULT_USER) {
  const statements = MINIMAL_MIGRATIONS_SQL
    .split(';')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && !s.startsWith('--'));
  for (const stmt of statements) {
    try {
      await db.prepare(stmt + ';').run();
    } catch (e: any) {
      if (!/already exists|duplicate/i.test(String(e))) throw e;
    }
  }
  await db.batch([
    db
      .prepare(
        `INSERT OR IGNORE INTO tenants (tenant_id, preferred_tenant_id, display_name, is_active, plan, settings, created_at, updated_at)
         VALUES (?, ?, 'Test Tenant', 1, 'pro', '{}', datetime('now'), datetime('now'))`,
      )
      .bind(tenantId, `pref-${tenantId}`),
    db
      .prepare(
        `INSERT OR IGNORE INTO users (id, tenant_id, email, display_name, password_hash, status, created_at, updated_at)
         VALUES (?, ?, 'test@local', 'Test User', '', 'active', datetime('now'), datetime('now'))`,
      )
      .bind(userId, tenantId),
  ]);
}

async function seedApiKey(
  db: D1Database,
  keyId: string,
  secret: string,
  tenantId = DEFAULT_TENANT,
  userId = DEFAULT_USER,
) {
  const keyHash = await sha256Hex(secret);
  await db
    .prepare(
      `INSERT OR IGNORE INTO api_keys (id, tenant_id, user_id, name, key_hash, key_prefix, scopes, revoked_at, expires_at, is_deleted, created_at, updated_at)
       VALUES (?, ?, ?, 'test-jk', ?, ?, ?, NULL, NULL, 0, datetime('now'), datetime('now'))`,
    )
    .bind(keyId, tenantId, userId, keyHash, `jk_${keyId}`, JSON.stringify(['terminal:read', 'terminal:write']))
    .run();
}

async function computeInternalMarker(nodeId: string, userId: string, secret: string): Promise<string> {
  const input = `${nodeId}|${userId}`;
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey('raw', enc.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, [
    'sign',
  ]);
  const signature = await crypto.subtle.sign('HMAC', key, enc.encode(input));
  const bytes = new Uint8Array(signature);
  let binary = '';
  for (let i = 0; i < bytes.byteLength; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

async function getRealStub() {
  const ns = (env as any).CHAT_RELAY_DO;
  if (!ns || typeof ns.idFromName !== 'function') {
    throw new Error('env.CHAT_RELAY_DO namespace not available from cloudflare:test');
  }
  return ns.get(ns.idFromName('relay-compose-e2e'));
}

function jwtCtx(db: D1Database, token: string, tenantId = DEFAULT_TENANT, userId = DEFAULT_USER) {
  return {
    db,
    request: {
      headers: {
        get(k: string) {
          return String(k).toLowerCase() === 'authorization' ? `Bearer ${token}` : null;
        },
      },
    },
    auth: {
      isAuthenticated: false,
      tenantId: 'spoof-tenant',
      userId: 'spoof-user',
      verifyJwt: (t: string) => verifyJwtWithFallback(t, SECRET),
    },
  };
}

function jkCtx(db: D1Database, token: string) {
  return {
    db,
    request: {
      headers: {
        get(k: string) {
          return String(k).toLowerCase() === 'authorization' ? `Bearer ${token}` : null;
        },
      },
    },
    auth: { isAuthenticated: false },
  };
}

describe('relay e2e compose: register + upgrade + heartbeat', () => {
  it('JWT: register → upgrade 101 → heartbeat accepted', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedIdentity(db);

    const token = await signJwt(
      { sub: DEFAULT_USER, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) + 3600 },
      SECRET,
    );
    const { default: registerDesktopNode } = await import('../../resolvers/ai/register-desktop-node');
    const registered = await registerDesktopNode(
      null,
      { input: { hostname: 'compose-jwt-host' } },
      jwtCtx(db, token) as any,
    );
    expect(registered.id).toBeTruthy();
    expect(registered.status).toBe('online');

    const row = await db
      .prepare(
        `SELECT id, status, tenant_id, user_id FROM desktop_nodes WHERE id = ? AND is_deleted = 0`,
      )
      .bind(registered.id)
      .first<any>();
    expect(row).toBeTruthy();
    expect(row.status).toBe('online');
    expect(row.tenant_id).toBe(DEFAULT_TENANT);
    expect(row.user_id).toBe(DEFAULT_USER);

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const marker = await computeInternalMarker(registered.id, DEFAULT_USER, SECRET);
      const req = new Request(`https://x/websocket-resolved?clientType=desktop&token=${token}`, {
        headers: { Upgrade: 'websocket', 'x-internal-forward': marker },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(101);

      const sockets = state.getWebSockets('desktop');
      expect(sockets.length).toBeGreaterThanOrEqual(1);
      const ws = sockets[sockets.length - 1];
      const auth = getSocketAuth(ws, state);
      expect(auth).not.toBeNull();
      expect(auth!.tenantId).toBe(DEFAULT_TENANT);
      expect(auth!.userId).toBe(DEFAULT_USER);

      await db
        .prepare(`UPDATE desktop_nodes SET last_heartbeat_at = '2000-01-01 00:00:00' WHERE id = ?`)
        .bind(registered.id)
        .run();

      await instance.webSocketMessage(
        ws,
        JSON.stringify({
          type: 'heartbeat',
          data: { timestamp: new Date().toISOString(), nodeId: registered.id },
        }),
      );
      expect(state.getWebSockets('desktop').length).toBeGreaterThanOrEqual(1);

      const afterHb = await db
        .prepare(`SELECT last_heartbeat_at, status FROM desktop_nodes WHERE id = ?`)
        .bind(registered.id)
        .first<any>();
      expect(afterHb.status).toBe('online');
      expect(afterHb.last_heartbeat_at).toBeTruthy();
      expect(String(afterHb.last_heartbeat_at)).not.toBe('2000-01-01 00:00:00');
    });
  });

  it('JWT register with expired token → UNAUTHENTICATED (real verifier)', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedIdentity(db);
    const expired = await signJwt(
      { sub: DEFAULT_USER, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) - 60 },
      SECRET,
    );
    const { default: registerDesktopNode } = await import('../../resolvers/ai/register-desktop-node');
    try {
      await registerDesktopNode(
        null,
        { input: { hostname: 'expired-jwt-host' } },
        jwtCtx(db, expired) as any,
      );
      throw new Error('expected UNAUTHENTICATED');
    } catch (e) {
      expect(e).toBeInstanceOf(GraphQLError);
      expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
    }
  });

  it('jk_: register → upgrade 101', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    const tenantId = '00000000-0000-0000-0000-00000000JK01';
    const userId = '00000000-0000-0000-0000-00000000JK02';
    await seedIdentity(db, tenantId, userId);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, tenantId, userId);

    const { default: registerDesktopNode } = await import('../../resolvers/ai/register-desktop-node');
    const registered = await registerDesktopNode(
      null,
      { input: { hostname: 'compose-jk-host' } },
      jkCtx(db, g.token) as any,
    );
    expect(registered.id).toBeTruthy();
    expect(registered.status).toBe('online');

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const marker = await computeInternalMarker(registered.id, userId, SECRET);
      const req = new Request(`https://x/websocket-resolved?clientType=desktop&token=${g.token}`, {
        headers: { Upgrade: 'websocket', 'x-internal-forward': marker },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(101);
      const sockets = state.getWebSockets('desktop');
      expect(sockets.length).toBeGreaterThanOrEqual(1);
      const auth = getSocketAuth(sockets[sockets.length - 1], state);
      expect(auth).not.toBeNull();
      expect(auth!.tenantId).toBe(tenantId);
      expect(auth!.userId).toBe(userId);
    });
  });

  it('register without identity → UNAUTHENTICATED', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedIdentity(db);
    const { default: registerDesktopNode } = await import('../../resolvers/ai/register-desktop-node');
    try {
      await registerDesktopNode(
        null,
        { input: { hostname: 'spoof-host' } },
        {
          db,
          request: new Request('http://localhost'),
          auth: { isAuthenticated: false, tenantId: DEFAULT_TENANT, userId: DEFAULT_USER },
        } as any,
      );
      throw new Error('expected UNAUTHENTICATED');
    } catch (e) {
      expect(e).toBeInstanceOf(GraphQLError);
      expect((e as GraphQLError).extensions.code).toBe('UNAUTHENTICATED');
    }
  });
});
