import { describe, it, expect, vi } from 'vitest';
import { resolveOnlineNode } from './resolve-online-node';

function makeStubD1(returnRow: { id: string } | null) {
  const bind = vi.fn().mockReturnThis();
  const first = vi.fn().mockResolvedValue(returnRow);
  const prepare = vi.fn(() => ({ bind, first }));
  return { prepare, bind, first } as unknown as D1Database;
}

describe('resolveOnlineNode', () => {
  it('returns the node when found', async () => {
    const db = makeStubD1({ id: 'node-xyz' });
    const res = await resolveOnlineNode(db, { tenantId: 't1', userId: 'u1' });
    expect(res).toEqual({ id: 'node-xyz' });
    expect(db.prepare).toHaveBeenCalled();
    // exact bind order tenant then user
    expect((db as any).bind).toHaveBeenCalledWith('t1', 'u1');
  });

  it('returns null when no online node', async () => {
    const db = makeStubD1(null);
    const res = await resolveOnlineNode(db, { tenantId: 't2', userId: 'u2' });
    expect(res).toBeNull();
  });

  it('binds both tenant and user (tenant isolation)', async () => {
    const db = makeStubD1({ id: 'n' });
    await resolveOnlineNode(db, { tenantId: 'tenant-only-this', userId: 'user-only-this' });
    expect((db as any).bind).toHaveBeenCalledWith('tenant-only-this', 'user-only-this');
  });
});
