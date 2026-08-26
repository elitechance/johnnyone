import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

describe('AI session WRITE scope (tenant isolation)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('createAiSession: uses call ctx.auth for node; different tenant cannot create under A', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ id: 'new-s' } as any);
    const { default: create } = await import('../../resolvers/ai/create-ai-session');
    await create(null, { input: { provider: 'ollama', workingDirectory: '~' } }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 't1', userId: 'u1' }) }), 'create_session', expect.anything());

    const b = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ id: 'b-s' } as any);
    const { default: createB } = await import('../../resolvers/ai/create-ai-session');
    await createB(null, { input: { provider: 'ollama' } }, b as any);
    expect(spyB).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }), 'create_session', expect.anything());
  });

  it('updateAiSessionTitle: ctx.auth + only id+value in args', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: fn } = await import('../../resolvers/ai/update-ai-session-title');
    await fn(null, { id: 's1', title: 'new' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_session_title', { id: 's1', title: 'new' });
  });

  it('updateAiSessionWorkingDirectory: ctx.auth scopes', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: fn } = await import('../../resolvers/ai/update-ai-session-working-directory');
    await fn(null, { id: 's1', workingDirectory: '/tmp' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_session_working_directory', { id: 's1', workingDirectory: '/tmp' });
  });

  it('updateAiSessionProvider + archived: ctx only', async () => {
    let spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: prov } = await import('../../resolvers/ai/update-ai-session-provider');
    await prov(null, { id: 's1', provider: 'claude' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'update_session_provider', { id: 's1', provider: 'claude' });

    spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: arch } = await import('../../resolvers/ai/update-ai-session-archived');
    await arch(null, { id: 's1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'archive_session', { id: 's1' });
  });

  it('deleteAiSession: only own node; cross tenant denied', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(true);
    const { default: del } = await import('../../resolvers/ai/delete-ai-session');
    await del(null, { id: 's1' }, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'delete_session', { id: 's1' });

    const b = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(false);
    const { default: delB } = await import('../../resolvers/ai/delete-ai-session');
    await delB(null, { id: 's1' }, b as any);
    expect(spyB).toHaveBeenCalledWith(expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }), 'delete_session', { id: 's1' });
  });
});