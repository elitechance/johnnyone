import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import * as apiKeyMod from '../../lib/auth/api-key';

// Overhaul P4 (Phase 03): the four briefing mutations are thin desktopRpc pass-throughs to the
// Phase 01/02 host methods. These assertions pin each resolver's mapped host method + required
// auth scope (mirrors agent-plan-write-scope.test.ts). Behavior itself is tested on the host.

const mockCtx = {
  db: {} as any,
  env: { CHAT_RELAY_DO: {} as any },
  auth: { tenantId: 't1', userId: 'u1' },
};

describe('Briefing WRITE scope (overhaul P4 relay resolvers)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('each briefing resolver relays to the mapped host method with the correct scope', async () => {
    const cases = [
      {
        file: '../../resolvers/ai/create-briefing-initiative',
        method: 'create_briefing_run',
        scope: 'plans:write',
        args: {
          input: {
            workspacePath: '/w',
            brief: 'b',
            workerProvider: 'claude_code',
            reviewerProvider: 'claude_code',
          },
        },
      },
      {
        file: '../../resolvers/ai/accept-initiative-brief',
        method: 'accept_brief',
        scope: 'plans:write',
        args: { input: { initiativeId: 'i1', finalBrief: null } },
      },
      {
        file: '../../resolvers/ai/add-initiative-reference-path',
        method: 'add_initiative_reference_path',
        scope: 'plans:write',
        args: { initiativeId: 'i1', path: '/w/docs' },
      },
      {
        file: '../../resolvers/ai/initiative-upload-chunk',
        method: 'initiative_upload_chunk',
        scope: 'files:write',
        args: {
          initiativeId: 'i1',
          path: 'f.png',
          chunkIndex: 0,
          totalChunks: 1,
          dataBase64: 'AAA=',
          done: true,
        },
      },
    ] as const;

    for (const { file, method, scope, args } of cases) {
      const authSpy = vi
        .spyOn(apiKeyMod, 'authorizeForAltToken')
        .mockResolvedValueOnce(undefined as any);
      const rpcSpy = vi
        .spyOn(desktopRpcMod, 'desktopRpc')
        .mockResolvedValueOnce({} as any);
      const mod = await import(file);
      await mod.default(null, args, mockCtx as any);
      expect(authSpy).toHaveBeenCalledWith(mockCtx, scope);
      expect(rpcSpy).toHaveBeenCalledWith(mockCtx, method, expect.anything());
      vi.restoreAllMocks();
    }
  });

  it('initiativeUploadChunk relays the payload fields verbatim', async () => {
    vi.spyOn(apiKeyMod, 'authorizeForAltToken').mockResolvedValueOnce(undefined as any);
    const rpcSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: upload } = await import('../../resolvers/ai/initiative-upload-chunk');
    await upload(
      null,
      {
        initiativeId: 'i1',
        path: 'a.bin',
        chunkIndex: 2,
        totalChunks: 3,
        dataBase64: 'ZZ==',
        done: false,
      },
      mockCtx as any,
    );
    expect(rpcSpy).toHaveBeenCalledWith(mockCtx, 'initiative_upload_chunk', {
      initiativeId: 'i1',
      path: 'a.bin',
      chunkIndex: 2,
      totalChunks: 3,
      dataBase64: 'ZZ==',
      done: false,
    });
  });
});
