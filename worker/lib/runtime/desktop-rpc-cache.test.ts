import { describe, it, expect } from 'vitest';
import {
  readCachedRpc,
  writeCachedRpc,
  invalidateCachedRpc,
  invalidatedReadsFor,
} from './desktop-rpc-cache';

const auth = { tenantId: 't1', userId: 'u1' } as any;
const other = { tenantId: 't1', userId: 'u2' } as any;

describe('desktop RPC cache — write invalidation (F2)', () => {
  it('stops serving a cached list_sessions after an archive write', () => {
    writeCachedRpc(auth, 'list_sessions', { status: 'active' }, [{ id: 'a' }]);
    expect(readCachedRpc(auth, 'list_sessions', { status: 'active' })).not.toBeNull();

    invalidateCachedRpc(auth, 'archive_session');

    expect(readCachedRpc(auth, 'list_sessions', { status: 'active' })).toBeNull();
  });

  it('invalidates EVERY list_sessions variant, not just the one key we happened to guess', () => {
    // The cache key includes the params, so `status:'active'` and `status:null` are different
    // entries. A clear only writes one of them, but both are stale afterwards.
    writeCachedRpc(auth, 'list_sessions', { status: 'active' }, [{ id: 'a' }]);
    writeCachedRpc(auth, 'list_sessions', { status: null }, [{ id: 'a' }]);
    writeCachedRpc(auth, 'list_sessions', { status: 'archived' }, []);

    invalidateCachedRpc(auth, 'archive_session');

    expect(readCachedRpc(auth, 'list_sessions', { status: 'active' })).toBeNull();
    expect(readCachedRpc(auth, 'list_sessions', { status: null })).toBeNull();
    expect(readCachedRpc(auth, 'list_sessions', { status: 'archived' })).toBeNull();
  });

  it('does not touch another user’s cached sessions', () => {
    writeCachedRpc(auth, 'list_sessions', { status: 'active' }, [{ id: 'a' }]);
    writeCachedRpc(other, 'list_sessions', { status: 'active' }, [{ id: 'b' }]);

    invalidateCachedRpc(auth, 'archive_session');

    expect(readCachedRpc(auth, 'list_sessions', { status: 'active' })).toBeNull();
    expect(readCachedRpc(other, 'list_sessions', { status: 'active' })).not.toBeNull();
  });

  it('does not invalidate unrelated cached reads', () => {
    writeCachedRpc(auth, 'get_setting', { key: 'theme' }, 'dark');
    writeCachedRpc(auth, 'detect_cli_tools', {}, ['claude']);

    invalidateCachedRpc(auth, 'archive_session');

    expect(readCachedRpc(auth, 'get_setting', { key: 'theme' })).toBe('dark');
    expect(readCachedRpc(auth, 'detect_cli_tools', {})).not.toBeNull();
  });

  it('is a no-op for a read method and for a write with nothing to invalidate', () => {
    writeCachedRpc(auth, 'list_sessions', { status: 'active' }, [{ id: 'a' }]);
    invalidateCachedRpc(auth, 'list_sessions');
    invalidateCachedRpc(auth, 'some_unrelated_write');
    expect(readCachedRpc(auth, 'list_sessions', { status: 'active' })).not.toBeNull();
  });

  it('every session-WRITE rpc invalidates list_sessions (so a new one is not forgotten)', () => {
    for (const method of [
      'archive_session',
      'delete_session',
      'create_session',
      'update_session_title',
      'update_session_working_directory',
      'update_session_provider',
    ]) {
      expect(invalidatedReadsFor(method), method).toContain('list_sessions');
    }
    expect(invalidatedReadsFor('list_sessions')).toEqual([]);
  });
});

// The invalidation is wired into the ONE rpc seam every session resolver already calls, rather than
// repeated per resolver. Pinned against the real source so the wiring cannot be dropped silently —
// an unwired `invalidateCachedRpc` would leave every test above green and the bug back.
const rpcSources = import.meta.glob(['./desktop-rpc.ts', './relay-rpc.ts'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

describe('invalidation is wired into the rpc seam', () => {
  it('both rpc helpers invalidate after a successful call', () => {
    const entries = Object.entries(rpcSources);
    expect(entries.length).toBe(2);
    for (const [path, src] of entries) {
      expect(src, path).toContain('invalidateCachedRpc(identity, method)');
    }
  });

  it('the session write resolvers route through those helpers', () => {
    const resolverSources = import.meta.glob(
      [
        '../../resolvers/ai/update-ai-session-archived.ts',
        '../../resolvers/ai/delete-ai-session.ts',
        '../../resolvers/ai/create-ai-session.ts',
      ],
      { query: '?raw', import: 'default', eager: true },
    ) as Record<string, string>;
    const entries = Object.entries(resolverSources);
    expect(entries.length).toBe(3);
    for (const [path, src] of entries) {
      expect(src, path).toMatch(/desktopRpc|relayRpc/);
    }
  });
});
