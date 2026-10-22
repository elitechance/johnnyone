import { describe, it, expect, vi, beforeEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import * as apiKeyMod from '../../lib/auth/api-key';
import { authedCtx } from '../auth/test-authed-ctx';

/**
 * The `runWorkflowRunPhase` resolver seam — the worker's only route to the desktop's
 * `execute_workflow_run` RPC, which is what lets a browser run a phase at all.
 *
 * Mirrors workflow-arena-scope.test.ts / agent-plan-write-scope.test.ts. Four things are pinned
 * here and nowhere else:
 *
 *  1. the exact host RPC method name and the exact param keys — nothing else in the worker tree
 *     drives `execute_workflow_run`, so a renamed key compiles, passes every other gate, and fails
 *     in production;
 *  2. that `verify` and `binary` are relayed ONLY when the caller supplied a non-empty string. The
 *     host owns the default for both; a worker-side default would be a second home for the same
 *     rule, and the copy on the wire is the one that wins. So an omitted / empty / whitespace-only
 *     value must leave the key ABSENT — a `null` here would read to the host as an explicit
 *     override with an empty command;
 *  3. the scope is `plans:write`, the same one `startWorkflowRun` authorizes with, because this
 *     executes work on the host;
 *  4. a failed authorization means NO RPC is attempted — fail closed, not fail through to the host.
 */
const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

/** A schema field's signature line, with the `#` doc comments above it skipped. */
function fieldLine(name: string): string | undefined {
  return schemaSource
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => !line.startsWith('#'))
    .find((line) => line.startsWith(`${name}(`) || line.startsWith(`${name}:`));
}

describe('runWorkflowRunPhase resolver seam', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('runWorkflowRunPhase relays the runId to the execute_workflow_run RPC', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({ gateExit: 0 } as any);
    const { default: runWorkflowRunPhase } = await import('../../resolvers/ai/run-workflow-run-phase');
    const res = await runWorkflowRunPhase(null, { runId: 'r1' }, mockCtx as any);
    // The caller's own ctx is relayed, so one tenant cannot run another tenant's phase.
    expect(spy).toHaveBeenCalledWith(mockCtx, 'execute_workflow_run', { runId: 'r1' });
    expect(res).toEqual({ gateExit: 0 });
  });

  it('runWorkflowRunPhase relays a supplied non-empty verify and binary', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
    const { default: runWorkflowRunPhase } = await import('../../resolvers/ai/run-workflow-run-phase');
    await runWorkflowRunPhase(
      null,
      { runId: 'r1', verify: 'cargo test', binary: './target/debug/johnnyone' },
      mockCtx as any,
    );
    expect(spy).toHaveBeenCalledWith(mockCtx, 'execute_workflow_run', {
      runId: 'r1',
      verify: 'cargo test',
      binary: './target/debug/johnnyone',
    });
  });

  it('runWorkflowRunPhase does NOT relay an omitted, empty or whitespace-only verify/binary', async () => {
    const { default: runWorkflowRunPhase } = await import('../../resolvers/ai/run-workflow-run-phase');
    const cases: Record<string, unknown>[] = [
      { runId: 'r1' },
      { runId: 'r1', verify: null, binary: null },
      { runId: 'r1', verify: '', binary: '' },
      { runId: 'r1', verify: '   ', binary: '\t\n' },
      { runId: 'r1', verify: undefined, binary: '   ' },
    ];
    for (const args of cases) {
      const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValueOnce({} as any);
      await runWorkflowRunPhase(null, args as any, mockCtx as any);
      expect(spy).toHaveBeenCalledTimes(1);
      const params = spy.mock.calls[0][2] as Record<string, unknown>;
      expect(params.runId, `runId must still relay for ${JSON.stringify(args)}`).toBe('r1');
      // ABSENT, not null: a null would reach the host as an explicit override carrying nothing,
      // which is a different and much worse claim than "the caller had no opinion".
      expect(
        Object.prototype.hasOwnProperty.call(params, 'verify'),
        `verify must be absent for ${JSON.stringify(args)}`,
      ).toBe(false);
      expect(
        Object.prototype.hasOwnProperty.call(params, 'binary'),
        `binary must be absent for ${JSON.stringify(args)}`,
      ).toBe(false);
      spy.mockRestore();
    }
  });

  it('runWorkflowRunPhase authorizes with startWorkflowRun\'s plans:write scope', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValue({} as any);
    const auth = vi.spyOn(apiKeyMod, 'authorizeForAltToken').mockResolvedValue(undefined);
    const { default: runWorkflowRunPhase } = await import('../../resolvers/ai/run-workflow-run-phase');
    const { default: startWorkflowRun } = await import('../../resolvers/ai/start-workflow-run');
    await runWorkflowRunPhase(null, { runId: 'r1' }, mockCtx as any);
    await startWorkflowRun(null, { workflowId: 'w1' }, mockCtx as any);
    const scopes = auth.mock.calls.map((c) => c[1]);
    expect(scopes).toEqual(['plans:write', 'plans:write']);
    expect(auth.mock.calls[0]![0]).toBe(mockCtx);
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it('runWorkflowRunPhase attempts no RPC when authorization fails', async () => {
    const spy = vi.spyOn(desktopRpcMod, 'desktopRpc').mockResolvedValue({} as any);
    vi.spyOn(apiKeyMod, 'authorizeForAltToken').mockRejectedValue(new Error('missing plans:write'));
    const { default: runWorkflowRunPhase } = await import('../../resolvers/ai/run-workflow-run-phase');
    await expect(runWorkflowRunPhase(null, { runId: 'r1' }, mockCtx as any)).rejects.toThrow(
      'missing plans:write',
    );
    expect(spy, 'a refused caller must not reach the host').not.toHaveBeenCalled();
  });

  it('the schema declares runWorkflowRunPhase and exposes gateExit', () => {
    const line = fieldLine('runWorkflowRunPhase');
    expect(line, 'runWorkflowRunPhase must be declared in the schema').toBeTruthy();
    expect(line).toContain('runId: ID!');
    expect(line, 'verify is optional: the host owns its default').toContain('verify: String');
    expect(line, 'binary is optional: the host owns its default').toContain('binary: String');
    expect(line).toContain('WorkflowRun!');
    // The result type must actually carry the score, or the caller cannot see what the run did.
    expect(schemaSource).toContain('gateExit: Float');
  });
});