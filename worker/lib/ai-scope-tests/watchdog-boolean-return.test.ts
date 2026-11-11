import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import * as apiKeyMod from '../../lib/auth/api-key';
import { authedCtx } from '../auth/test-authed-ctx';

// Regression pin for the watchdog `Boolean!` return bug.
//
// The four watchdog mutations relay to the desktop host with `desktopRpc`, and the host answers
// `{ ok: true }`. The GraphQL schema declares each field `Boolean!`. The resolvers used to
// `return desktopRpc(...)` — the OBJECT — and graphql-js rejected it with
//   "Boolean cannot represent a non boolean value: { ok: true }"
// AFTER the write had already landed, so the client reported failure for a successful write.
// They now `await desktopRpc(...)` and `return true`.
//
// Two things this pins, per resolver:
//   1. the resolved value is EXACTLY `true` (`toBe`, not `toBeTruthy` — the object `{ok:true}` is
//      truthy, which is precisely the bug);
//   2. a host-side rejection still rejects the resolver (mirrored bug: dropping the `await` would
//      swallow the failure and report success for a failed write);
//   3. `desktopRpc` is called once with the resolver's mapped host method.

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

const HOST_OK = { ok: true };

const CASES = [
  {
    name: 'updateSessionWatchdog',
    file: '../../resolvers/ai/update-session-watchdog',
    method: 'update_session_watchdog',
    args: { sessionId: 's1', enabled: true, intervalSecs: 300 },
  },
  {
    name: 'updateSessionWatchdogPrompt',
    file: '../../resolvers/ai/update-session-watchdog-prompt',
    method: 'update_session_watchdog_prompt',
    args: { sessionId: 's1', prompt: 'Still there? Nudge prompt.' },
  },
  {
    name: 'updateSessionWatchdogTriggers',
    file: '../../resolvers/ai/update-session-watchdog-triggers',
    method: 'update_session_watchdog_triggers',
    args: { sessionId: 's1', onBlocked: true, onStaleReport: false },
  },
  {
    name: 'sendSessionWatchdogNudge',
    file: '../../resolvers/ai/send-session-watchdog-nudge',
    method: 'nudge_session_watchdog',
    args: { sessionId: 's1' },
  },
] as const;

function stubAuth() {
  return vi.spyOn(apiKeyMod, 'authorizeForAltToken').mockResolvedValue(undefined as any);
}

describe('Watchdog mutations return Boolean! (not the host {ok:true} payload)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  for (const { name, file, method, args } of CASES) {
    it(`${name} resolves to exactly true when the host answers { ok: true }`, async () => {
      stubAuth();
      vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
      const { default: resolver } = await import(file);
      const result = await resolver(null, { ...args }, mockCtx as any);
      // `toBe(true)` on purpose: `{ ok: true }` is truthy, so `toBeTruthy` would pass the bug.
      expect(result).toBe(true);
    });

    it(`${name} rejects when the host call rejects (no success for a failed write)`, async () => {
      stubAuth();
      vi.spyOn(desktopRpcMod, 'desktopRpc').mockRejectedValueOnce(new Error('host offline'));
      const { default: resolver } = await import(file);
      await expect(resolver(null, { ...args }, mockCtx as any)).rejects.toThrow('host offline');
    });

    it(`${name} relays once to the mapped host method '${method}'`, async () => {
      stubAuth();
      const rpcSpy = vi
        .spyOn(desktopRpcMod, 'desktopRpc')
        .mockResolvedValueOnce(HOST_OK as any);
      const { default: resolver } = await import(file);
      await resolver(null, { ...args }, mockCtx as any);
      expect(rpcSpy).toHaveBeenCalledTimes(1);
      expect(rpcSpy).toHaveBeenCalledWith(mockCtx, method, expect.anything());
    });
  }

  it('updateSessionWatchdog relays the payload fields verbatim', async () => {
    stubAuth();
    const rpcSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
    const { default: resolver } = await import('../../resolvers/ai/update-session-watchdog');
    await resolver(
      null,
      { sessionId: 's9', enabled: false, intervalSecs: 90 },
      mockCtx as any,
    );
    expect(rpcSpy).toHaveBeenCalledWith(mockCtx, 'update_session_watchdog', {
      sessionId: 's9',
      enabled: false,
      intervalSecs: 90,
    });
  });

  it('updateSessionWatchdogPrompt relays the payload fields verbatim', async () => {
    stubAuth();
    const rpcSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
    const { default: resolver } = await import(
      '../../resolvers/ai/update-session-watchdog-prompt'
    );
    await resolver(null, { sessionId: 's9', prompt: 'Check in.' }, mockCtx as any);
    // The host handler `rpc_update_session_watchdog_prompt` reads `id` (unlike its three watchdog
    // siblings, which read `sessionId`), so the Worker maps `sessionId -> id` here. This expectation
    // used to assert `sessionId: 's9'` — that encoded the bug and made the console's prompt save
    // always fail with `Missing 'id' parameter`.
    expect(rpcSpy).toHaveBeenCalledWith(mockCtx, 'update_session_watchdog_prompt', {
      id: 's9',
      prompt: 'Check in.',
    });
  });

  it('updateSessionWatchdogTriggers relays the payload fields verbatim', async () => {
    stubAuth();
    const rpcSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
    const { default: resolver } = await import(
      '../../resolvers/ai/update-session-watchdog-triggers'
    );
    await resolver(
      null,
      { sessionId: 's9', onBlocked: false, onStaleReport: true },
      mockCtx as any,
    );
    expect(rpcSpy).toHaveBeenCalledWith(mockCtx, 'update_session_watchdog_triggers', {
      sessionId: 's9',
      onBlocked: false,
      onStaleReport: true,
    });
  });

  it('sendSessionWatchdogNudge relays the payload fields verbatim', async () => {
    stubAuth();
    const rpcSpy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
    const { default: resolver } = await import('../../resolvers/ai/send-session-watchdog-nudge');
    await resolver(null, { sessionId: 's9' }, mockCtx as any);
    expect(rpcSpy).toHaveBeenCalledWith(mockCtx, 'nudge_session_watchdog', {
      sessionId: 's9',
    });
  });

  it('each watchdog resolver authorizes with the sessions:write scope', async () => {
    for (const { file, method, args } of CASES) {
      const authSpy = stubAuth();
      vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce(HOST_OK as any);
      const { default: resolver } = await import(file);
      await resolver(null, { ...args }, mockCtx as any);
      expect(authSpy).toHaveBeenCalledWith(mockCtx, 'sessions:write');
      expect(vi.mocked(desktopRpcMod.desktopRpc)).toHaveBeenCalledWith(
        mockCtx,
        method,
        expect.anything(),
      );
      vi.restoreAllMocks();
    }
  });
});