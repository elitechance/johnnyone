import { describe, it, expect, vi, beforeEach } from 'vitest';
import { verifySessionOwnership, invalidateSessionOwnership } from './session-ownership';
import * as relayRpcMod from './relay-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

describe('verifySessionOwnership (task 01)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('owned-passes: desktop reports the session → true', async () => {
    const spy = vi.spyOn(relayRpcMod, 'relayRpc').mockResolvedValueOnce({ id: 's1' } as any);
    const ok = await verifySessionOwnership(mockCtx as any, 's1');
    expect(ok).toBe(true);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'get_session', { id: 's1' });
  });

  it('foreign-rejected: desktop reports no session for tenant+user → false', async () => {
    vi.spyOn(relayRpcMod, 'relayRpc').mockRejectedValueOnce(new Error('session not found'));
    const ok = await verifySessionOwnership(mockCtx as any, 'foreign-s');
    expect(ok).toBe(false);
  });

  it('unknown-rejected: garbage sessionId → false', async () => {
    vi.spyOn(relayRpcMod, 'relayRpc').mockRejectedValueOnce(new Error('not found'));
    const ok = await verifySessionOwnership(mockCtx as any, 'does-not-exist-xyz');
    expect(ok).toBe(false);
  });

  it('tenant-scoping: same sessionId but different tenant/user → false', async () => {
    vi.spyOn(relayRpcMod, 'relayRpc').mockRejectedValueOnce(new Error('not yours'));
    const otherCtx = authedCtx({ tenantId: 'other-t', userId: 'other-u' });
    const ok = await verifySessionOwnership(otherCtx as any, 's1');
    expect(ok).toBe(false);
  });

  it('empty/invalid sessionId → false (no RPC)', async () => {
    const spy = vi.spyOn(relayRpcMod, 'relayRpc');
    expect(await verifySessionOwnership(mockCtx as any, '')).toBe(false);
    expect(await verifySessionOwnership(mockCtx as any, '   ')).toBe(false);
    expect(await verifySessionOwnership(mockCtx as any, null as any)).toBe(false);
    expect(spy).not.toHaveBeenCalled();
  });

  it('transport-failure: no online node / timeout → throws (fail-closed for caller)', async () => {
    vi.spyOn(relayRpcMod, 'relayRpc').mockRejectedValueOnce(
      new Error('No online desktop node found. Please ensure your desktop app is running and connected.')
    );
    await expect(verifySessionOwnership(mockCtx as any, 's-transport-1')).rejects.toThrow(/No online desktop node/);

    vi.spyOn(relayRpcMod, 'relayRpc').mockRejectedValueOnce(new Error('Desktop RPC timed out'));
    await expect(verifySessionOwnership(mockCtx as any, 's-transport-2')).rejects.toThrow(/timed out/);
  });

  it('cache-hit: repeated check within TTL calls relay once', async () => {
    const spy = vi.spyOn(relayRpcMod, 'relayRpc').mockResolvedValue({ id: 's-c' } as any);
    await verifySessionOwnership(mockCtx as any, 's-c');
    await verifySessionOwnership(mockCtx as any, 's-c');
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('cache per (tenant,user,session)', async () => {
    const spy = vi.spyOn(relayRpcMod, 'relayRpc').mockResolvedValue({} as any);
    await verifySessionOwnership(mockCtx as any, 's-per-tenant');
    const other = authedCtx({ tenantId: 't2', userId: 'u2' });
    await verifySessionOwnership(other, 's-per-tenant');
    expect(spy).toHaveBeenCalledTimes(2); // different keys, separate lookups
  });

  it('ttl-expiry with fake timers', async () => {
    vi.useFakeTimers();
    const spy = vi.spyOn(relayRpcMod, 'relayRpc').mockResolvedValue({ id: 's-ttl' } as any);
    await verifySessionOwnership(mockCtx as any, 's-ttl');
    expect(spy).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(6000);
    await verifySessionOwnership(mockCtx as any, 's-ttl');
    expect(spy).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  it('invalidates-on-session-delete', async () => {
    const spy = vi.spyOn(relayRpcMod, 'relayRpc').mockResolvedValue({} as any);
    await verifySessionOwnership(mockCtx as any, 's-del');
    invalidateSessionOwnership('s-del');
    await verifySessionOwnership(mockCtx as any, 's-del');
    expect(spy).toHaveBeenCalledTimes(2);
  });
});
