import { describe, it, expect } from 'vitest';
import { classifyResolverSource } from './resolver-auth-classifier';

const resolverSources = import.meta.glob('../../resolvers/**/*.ts', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const helperSources = import.meta.glob(
  ['../runtime/desktop-rpc.ts', '../runtime/relay-rpc.ts', './api-key.ts'],
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;

function sourceBySuffix(map: Record<string, string>, suffix: string): string {
  const hit = Object.entries(map).find(([k]) => k.replace(/\\/g, '/').endsWith(suffix));
  if (!hit) throw new Error(`missing raw source for ${suffix}`);
  return hit[1];
}

function relativeResolverPath(key: string): string {
  const norm = key.replace(/\\/g, '/');
  const idx = norm.lastIndexOf('/resolvers/');
  return idx === -1 ? norm : norm.slice(idx + 1);
}

const PROTECTED_SNAPSHOT = [
  'resolvers/ai/accept-initiative-brief.ts',
  'resolvers/ai/add-initiative-reference-path.ts',
  'resolvers/ai/browse-host-directory.ts',
  'resolvers/ai/cancel-ai-generation.ts',
  'resolvers/ai/capture-terminal.ts',
  'resolvers/ai/create-agent-plan.ts',
  'resolvers/ai/create-ai-session.ts',
  'resolvers/ai/create-briefing-initiative.ts',
  'resolvers/ai/delete-agent-plan.ts',
  'resolvers/ai/delete-ai-session.ts',
  'resolvers/ai/delete-provider-config.ts',
  'resolvers/ai/files-delete.ts',
  'resolvers/ai/files-list-dir.ts',
  'resolvers/ai/files-mkdir.ts',
  'resolvers/ai/files-read.ts',
  'resolvers/ai/files-rename.ts',
  'resolvers/ai/files-upload-chunk.ts',
  'resolvers/ai/files-write.ts',
  'resolvers/ai/get-agent-plan.ts',
  'resolvers/ai/get-ai-session.ts',
  'resolvers/ai/get-kloo-doctor.ts',
  'resolvers/ai/get-kloo-probe.ts',
  'resolvers/ai/get-plan-check.ts',
  'resolvers/ai/get-planner-prompt-settings.ts',
  'resolvers/ai/get-setting.ts',
  'resolvers/ai/get-task-run.ts',
  'resolvers/ai/get-workspace-file-diff.ts',
  'resolvers/ai/git-diff.ts',
  'resolvers/ai/git-file-view.ts',
  'resolvers/ai/initiative-upload-chunk.ts',
  'resolvers/ai/list-agent-plans.ts',
  'resolvers/ai/list-ai-messages.ts',
  'resolvers/ai/list-ai-sessions.ts',
  'resolvers/ai/list-desktop-nodes.ts',
  'resolvers/ai/list-detected-cli-tools.ts',
  'resolvers/ai/list-initiative-events.ts',
  'resolvers/ai/list-prompt-library.ts',
  'resolvers/ai/list-provider-configs.ts',
  'resolvers/ai/list-tmux-sessions.ts',
  'resolvers/ai/list-workspace-files.ts',
  'resolvers/ai/on-desktop-node-status.ts',
  'resolvers/ai/on-relay-chat-delta.ts',
  'resolvers/ai/on-relay-chat-message.ts',
  'resolvers/ai/read-host-file.ts',
  'resolvers/ai/register-desktop-node.ts',
  'resolvers/ai/retry-agent-reviewer.ts',
  'resolvers/ai/run-initiative-from-phase.ts',
  'resolvers/ai/send-agent-feedback-to-worker.ts',
  'resolvers/ai/send-relay-chat-message.ts',
  'resolvers/ai/start-agent-plan.ts',
  'resolvers/ai/update-agent-phase-manual-pass.ts',
  'resolvers/ai/update-agent-plan-amend.ts',
  'resolvers/ai/update-agent-plan-app-scope.ts',
  'resolvers/ai/update-agent-plan-blocked.ts',
  'resolvers/ai/update-agent-plan-phases-refresh.ts',
  'resolvers/ai/update-agent-plan-stopped.ts',
  'resolvers/ai/update-agent-plan-title.ts',
  'resolvers/ai/update-agent-plan-validation-config.ts',
  'resolvers/ai/update-ai-session-archived.ts',
  'resolvers/ai/update-ai-session-provider.ts',
  'resolvers/ai/update-ai-session-title.ts',
  'resolvers/ai/update-ai-session-working-directory.ts',
  'resolvers/ai/update-desktop-node-status.ts',
  'resolvers/ai/update-planner-prompt-settings.ts',
  'resolvers/ai/update-setting.ts',
  'resolvers/ai/upsert-provider-config.ts',
  'resolvers/ai/validate-workspace-plan.ts',
  'resolvers/channels/create-channel-webhook.ts',
  'resolvers/channels/link-channel.ts',
  'resolvers/channels/list-channel-bindings.ts',
  'resolvers/channels/unlink-channel.ts',
  'resolvers/create-api-key.ts',
  'resolvers/create-chat-attachment.ts',
  'resolvers/delete-api-key.ts',
  'resolvers/delete-chat-attachment.ts',
  'resolvers/get-chat-attachment.ts',
  'resolvers/list-api-keys.ts',
  'resolvers/revoke-api-key.ts',
  'resolvers/update-git-action.ts',
];

describe('resolver auth audit', () => {
  const desktopSrc = sourceBySuffix(helperSources, '/desktop-rpc.ts');
  const relaySrc = sourceBySuffix(helperSources, '/relay-rpc.ts');
  const apiKeySrc = sourceBySuffix(helperSources, '/api-key.ts');
  const helpers = {
    desktopRpcHasRequire: desktopSrc.includes('requireIdentity'),
    relayRpcHasRequire: relaySrc.includes('requireIdentity'),
  };

  it('current tree: every protected resolver is classified safe', () => {
    const unsafe: string[] = [];
    const protectedPaths: string[] = [];
    for (const [key, src] of Object.entries(resolverSources)) {
      const rel = relativeResolverPath(key);
      const { protected: prot, safe } = classifyResolverSource(src, helpers);
      if (prot) protectedPaths.push(rel);
      if (prot && !safe) unsafe.push(rel);
    }
    expect(unsafe, `unprotected resolvers: ${unsafe.join(', ')}`).toEqual([]);
    expect(protectedPaths.length).toBeGreaterThanOrEqual(78);
    expect(protectedPaths.sort()).toEqual([...PROTECTED_SNAPSHOT].sort());
  });

  it('classifier rejects a sample that uses ctx.db without a choke-point call', () => {
    const sample = 'export default async function x(_p, _a, ctx) { return ctx.db.prepare("select 1") }';
    const { protected: prot, safe } = classifyResolverSource(sample, helpers);
    expect(prot).toBe(true);
    expect(safe).toBe(false);
  });

  it('classifier accepts a sample that calls requireIdentity then ctx.db', () => {
    const sample = `
      import { requireIdentity } from '../lib/auth/require-identity';
      export default async function x(_p, _a, ctx) {
        const id = await requireIdentity(ctx);
        return ctx.db.prepare('select 1').bind(id.tenantId);
      }
    `;
    const { protected: prot, safe } = classifyResolverSource(sample, helpers);
    expect(prot).toBe(true);
    expect(safe).toBe(true);
  });

  it('classifier accepts a sample that only calls desktopRpc<T>( when desktop-rpc.ts contains requireIdentity', () => {
    const sample = `
      import { desktopRpc } from '../../lib/runtime/desktop-rpc';
      export default async function x(_p, a, ctx) {
        return desktopRpc<unknown>(ctx, 'list_sessions', {});
      }
    `;
    expect(classifyResolverSource(sample, { desktopRpcHasRequire: true, relayRpcHasRequire: false }).safe).toBe(true);
    expect(classifyResolverSource(sample, { desktopRpcHasRequire: false, relayRpcHasRequire: false }).safe).toBe(false);
  });

  it('classifier treats const { db } = ctx and const { auth } = ctx as protected', () => {
    expect(classifyResolverSource('const { db } = ctx; db.prepare("x")', helpers).protected).toBe(true);
    expect(classifyResolverSource('const { auth } = ctx; return auth.tenantId;', helpers).protected).toBe(true);
    expect(classifyResolverSource('const { db } = ctx;', helpers).safe).toBe(false);
  });

  it('classifier treats on-relay-chat-delta shape as protected and unsafe without requireIdentity', () => {
    const sample = `export default { subscribe: (_p, a, ctx) => ctx.pubsub.subscribe('relay-chat-delta:' + a.relayId) }`;
    const { protected: prot, safe } = classifyResolverSource(sample, helpers);
    expect(prot).toBe(true);
    expect(safe).toBe(false);
  });

  it('helper files contain requireIdentity', () => {
    expect(desktopSrc).toContain('requireIdentity');
    expect(relaySrc).toContain('requireIdentity');
    expect(apiKeySrc).toContain('requireIdentity');
    expect(apiKeySrc).toMatch(/authorizeForAltToken[\s\S]*requireIdentity/);
  });
});
