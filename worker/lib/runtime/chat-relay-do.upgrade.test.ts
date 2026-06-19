import { describe, it, expect } from 'vitest';
import { ChatRelayDO } from './chat-relay-do';
import { getSocketAuth } from '../auth/socket-auth-context';
import { env, runInDurableObject, applyD1Migrations } from 'cloudflare:test';
import { signJwt } from 'modules/auth/auth-middleware';
import { generateApiKey, sha256Hex } from '../auth/api-key';

// REAL integration test using the Workers Durable Object runtime (vitest-pool-workers).
// - Uses runInDurableObject + cfEnv (no hand-mocked state, no vi.fn for acceptWebSocket/getWebSockets).
// - Seeds real D1 (tables + online desktop_node).
// - Signs tokens with the builtin signJwt from the platform auth module.
// - Calls handleWebSocketUpgrade (or .fetch) on the REAL ChatRelayDO instance.
// If runInDurableObject or env bindings are unavailable, the error is surfaced (config will be fixed instead of mocking).

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

describe('Phase 00 upgrade matrix (REAL Workers DO runtime via runInDurableObject)', () => {
  // Node id used for success cases (JWT for default tenant/user must resolve to an online node)
  const SUCCESS_NODE_ID = 'real-do-resolved-node-1';

  async function getRealStub() {
    // Use the real DO namespace from the Workers test runtime (provided by vitest-pool-workers + main export + wrangler config)
    const ns = (env as any).CHAT_RELAY_DO;
    if (!ns || typeof ns.idFromName !== 'function') {
      throw new Error('env.CHAT_RELAY_DO namespace not available from cloudflare:test (check vitest pool miniflare durableObjects + main export in config; accessing DO ns from test spec context may require additional pool setup)');
    }
    // Use a stable name; resolution of node happens inside from JWT anyway
    const id = ns.idFromName('entry-for-real-test');
    return ns.get(id);
  }

  it('401 with no JWT using real runInDurableObject + real Request', async () => {
    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request('https://x/websocket?clientType=desktop', {
        headers: { Upgrade: 'websocket' },
      });
      // Call the upgrade handler directly on the REAL instance (no mock state)
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      // socket must NOT be accepted when no token is presented
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('401 with an INVALID token (signed with the wrong secret) — verifier rejects, socket NOT accepted', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding');
    await seedTestData(db, SUCCESS_NODE_ID);
    // Token is well-formed but signed with a DIFFERENT secret, so verifyJwtWithFallback must reject it.
    // This exercises the actual verifier path (the no-token case short-circuits before the verifier).
    const forged = await signJwt(
      { sub: DEFAULT_USER, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) + 3600 },
      'a-completely-different-wrong-secret'
    );
    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=desktop&token=${forged}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('401 with an EXPIRED token — verifier rejects, socket NOT accepted', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding');
    await seedTestData(db, SUCCESS_NODE_ID);
    const expired = await signJwt(
      { sub: DEFAULT_USER, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) - 60 },
      SECRET
    );
    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=desktop&token=${expired}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('upgrade accepted (101) with valid JWT (builtin signJwt) + node resolved from JWT + real accept', async () => {
    // Seed using the REAL D1 from the test runtime (populated via wrangler + miniflare in pool config)
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding (fix vitest config d1Databases + main)');
    await seedTestData(db, SUCCESS_NODE_ID);

    const token = await signJwt(
      {
        sub: DEFAULT_USER,
        tenantId: DEFAULT_TENANT,
        exp: Math.floor(Date.now() / 1000) + 3600,
      },
      SECRET
    );

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      // Use the internal resolved path (x-internal-forward) so accept happens in *this* real instance
      // (avoids observing accept in a forwarded stub while still exercising resolve from JWT)
      const marker = await computeInternalMarker(SUCCESS_NODE_ID, DEFAULT_USER, SECRET);
      const req = new Request(
        `https://x/websocket-resolved?clientType=desktop&token=${token}`,
        {
          headers: {
            Upgrade: 'websocket',
            'x-internal-forward': marker,
          },
        }
      );
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(101);

      // Security: a client forging the old plain "resolved" value (or any wrong marker) must be rejected (403)
      // and not take the accept-here branch.
      const badReq = new Request(
        `https://x/websocket-resolved?clientType=desktop&token=${token}`,
        {
          headers: {
            Upgrade: 'websocket',
            'x-internal-forward': 'resolved',
          },
        }
      );
      const badRes = await (instance as any).handleWebSocketUpgrade(badReq);
      expect(badRes.status).toBe(403);

      // Real proof using the actual DurableObjectState (no mocks, no vi.fn)
      const sockets = state.getWebSockets('desktop');
      expect(sockets.length).toBe(1);

      const auth = getSocketAuth(sockets[0], state);
      expect(auth).not.toBeNull();
      expect(auth!.tenantId).toBe(DEFAULT_TENANT);
      expect(auth!.userId).toBe(DEFAULT_USER);
      expect(auth!.clientType).toBe('desktop');
    });
  });

  it('clientType=desktop + client-supplied nodeId is rejected for cross-tenant (JWT identity has no node; supplied nodeId ignored)', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding (fix vitest config d1Databases + main)');
    // Seed the good node (so that the supplied nodeId exists in table), but do not create any row for the bad tenant
    await seedTestData(db, SUCCESS_NODE_ID);

    // JWT for a tenant that has NO desktop node row (cross-tenant attempt)
    const badTenant = '00000000-0000-0000-0000-00000000BEEF';
    const badUser = '00000000-0000-0000-0000-00000000DEAD';
    const token = await signJwt(
      {
        sub: badUser,
        tenantId: badTenant,
        exp: Math.floor(Date.now() / 1000) + 3600,
      },
      SECRET
    );

    // Note: the bad identity has no row, even though we pass a real nodeId from another tenant
    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      // Client tries to supply a known good nodeId (from default tenant) + desktop type
      const req = new Request(
        `https://x/websocket?nodeId=${SUCCESS_NODE_ID}&clientType=desktop&token=${token}`,
        { headers: { Upgrade: 'websocket' } }
      );
      const res = await (instance as any).handleWebSocketUpgrade(req);
      // Must be rejected because resolve is *only* from JWT, not client nodeId
      expect(res.status).toBe(503);
      // and no socket may be accepted on the cross-tenant rejection
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('mobile clientType with a valid JWT is accepted (101) and tagged mobile (parity with desktop)', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding');
    await seedTestData(db, SUCCESS_NODE_ID);
    const token = await signJwt(
      { sub: DEFAULT_USER, tenantId: DEFAULT_TENANT, exp: Math.floor(Date.now() / 1000) + 3600 },
      SECRET
    );
    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const marker = await computeInternalMarker(SUCCESS_NODE_ID, DEFAULT_USER, SECRET);
      const req = new Request(`https://x/websocket-resolved?clientType=mobile&token=${token}`, {
        headers: { Upgrade: 'websocket', 'x-internal-forward': marker },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(101);
      // tag-filtered so this is independent of any desktop socket left by earlier cases
      const sockets = state.getWebSockets('mobile');
      expect(sockets.length).toBe(1);
      const auth = getSocketAuth(sockets[0], state);
      expect(auth).not.toBeNull();
      expect(auth!.clientType).toBe('mobile');
    });
  });

  it('valid JWT for desktop resolves node and accepts (via real fetch path exercising re-dispatch)', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available for real test seeding (fix vitest config d1Databases + main)');
    await seedTestData(db, SUCCESS_NODE_ID);

    const token = await signJwt(
      {
        sub: DEFAULT_USER,
        tenantId: DEFAULT_TENANT,
        exp: Math.floor(Date.now() / 1000) + 3600,
      },
      SECRET
    );

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const req = new Request(
        `https://x/websocket?nodeId=client-lied&clientType=desktop&token=${token}`,
        { headers: { Upgrade: 'websocket' } }
      );
      // Using .fetch (the public DurableObject entry) exercises full path including resolve + possible ns re-dispatch
      const res = await instance.fetch(req);
      expect(res.status).toBe(101);

      // Even after re-dispatch, we can still inspect this entry instance or the state in general.
      // For direct accept evidence on a resolved instance we rely on the previous test using /websocket-resolved.
      // Here we mainly confirm the overall flow with real JWT + resolve did not 4xx/5xx.
      // Additionally, calling getWebSockets on this state may be 0 or 1 depending on which instance accepted.
      // The key is no error and 101.
    });
  });

  // --- jk_ API key upgrade tests (Task 06) ---

  it('valid jk_ upgrade accepted (101) + tagged api + authCtx has apiKey info', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedTestData(db, SUCCESS_NODE_ID);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, DEFAULT_USER, ['terminal:read', 'terminal:write']);

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const marker = await computeInternalMarker(SUCCESS_NODE_ID, DEFAULT_USER, SECRET);
      const req = new Request(`https://x/websocket-resolved?clientType=api&token=${g.token}`, {
        headers: { Upgrade: 'websocket', 'x-internal-forward': marker },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(101);

      const sockets = state.getWebSockets('api');
      expect(sockets.length).toBe(1);
      const auth = getSocketAuth(sockets[0], state);
      expect(auth).not.toBeNull();
      expect(auth!.tenantId).toBe(DEFAULT_TENANT);
      expect(auth!.userId).toBe(DEFAULT_USER);
      expect(auth!.clientType).toBe('api');
      expect(auth!.apiKey).toBeDefined();
      expect(auth!.apiKey!.id).toBe(g.keyId);
    });
  });

  it('revoked jk_ rejected 401', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedTestData(db, SUCCESS_NODE_ID);
    const g = generateApiKey();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, DEFAULT_USER, ['terminal:read'], new Date().toISOString(), null); // revoked

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=api&token=${g.token}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('expired jk_ rejected 401', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedTestData(db, SUCCESS_NODE_ID);
    const g = generateApiKey();
    const past = new Date(Date.now() - 10000).toISOString();
    await seedApiKey(db, g.keyId, g.secret, DEFAULT_TENANT, DEFAULT_USER, ['terminal:read'], null, past);

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=api&token=${g.token}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });
  });

  it('unknown jk_ id / hash-mismatch rejected 401 (generic, no leak)', async () => {
    const db = (env as any).DB as D1Database;
    if (!db) throw new Error('env.DB not available');
    await seedTestData(db, SUCCESS_NODE_ID);
    // token whose id not present
    const g = generateApiKey();
    // do not seed it

    const stub = await getRealStub();
    await runInDurableObject(stub, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=api&token=${g.token}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });

    // hash mismatch case: seed but wrong hash by using different secret in token
    const g2 = generateApiKey();
    await seedApiKey(db, g2.keyId, 'wrongsecretforhash0123456789abcdef', DEFAULT_TENANT, DEFAULT_USER); // hash won't match
    const stub2 = await getRealStub();
    await runInDurableObject(stub2, async (instance: ChatRelayDO, state: DurableObjectState) => {
      const before = state.getWebSockets().length;
      const req = new Request(`https://x/websocket?clientType=api&token=${g2.token}`, {
        headers: { Upgrade: 'websocket' },
      });
      const res = await (instance as any).handleWebSocketUpgrade(req);
      expect(res.status).toBe(401);
      expect(state.getWebSockets().length).toBe(before);
    });
  });
});
