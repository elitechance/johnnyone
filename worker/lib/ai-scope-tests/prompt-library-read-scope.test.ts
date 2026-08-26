import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import { authedCtx } from '../auth/test-authed-ctx';

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

describe('listPromptLibrary READ scope', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('listPromptLibrary calls desktopRpc with list_prompt_library and authed ctx', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    const { default: list } = await import('../../resolvers/ai/list-prompt-library');
    await list(null, {}, mockCtx as any);
    expect(spy).toHaveBeenCalledWith(mockCtx, 'list_prompt_library');

    const b = authedCtx({ tenantId: 'tB', userId: 'uB' });
    const spyB = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce([] as any);
    await list(null, {}, b as any);
    expect(spyB).toHaveBeenCalledWith(
      expect.objectContaining({ auth: expect.objectContaining({ tenantId: 'tB', userId: 'uB' }) }),
      'list_prompt_library',
    );
  });
});
