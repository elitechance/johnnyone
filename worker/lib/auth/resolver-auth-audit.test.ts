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
  'resolvers/ai/list-session-reports.ts',
  'resolvers/ai/list-tmux-sessions.ts',
  'resolvers/ai/list-workspace-files.ts',
  'resolvers/ai/on-desktop-node-status.ts',
  'resolvers/ai/on-relay-chat-delta.ts',
  'resolvers/ai/on-relay-chat-message.ts',
  'resolvers/ai/read-host-file.ts',
  'resolvers/ai/register-desktop-node.ts',
  'resolvers/ai/retry-agent-reviewer.ts',
  'resolvers/ai/run-agent-plan-test-gate.ts',
  'resolvers/ai/run-initiative-from-phase.ts',
  'resolvers/ai/send-agent-feedback-to-worker.ts',
  'resolvers/ai/send-relay-chat-message.ts',
  'resolvers/ai/start-agent-plan.ts',
  'resolvers/ai/update-agent-phase-manual-pass.ts',
  'resolvers/ai/update-agent-plan-amend.ts',
  'resolvers/ai/update-agent-plan-app-scope.ts',
  'resolvers/ai/update-agent-plan-blocked.ts',
  'resolvers/ai/unblock-agent-plan.ts',
  'resolvers/ai/update-agent-plan-dev-providers.ts',
  'resolvers/ai/update-agent-plan-phases-refresh.ts',
  'resolvers/ai/update-agent-plan-stopped.ts',
  'resolvers/ai/update-agent-plan-test-commands.ts',
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
  // --- session watchdog (desktop migrations 027-029) ---
  'resolvers/ai/send-session-watchdog-nudge.ts',
  'resolvers/ai/update-session-watchdog.ts',
  'resolvers/ai/update-session-watchdog-prompt.ts',
  'resolvers/ai/update-session-watchdog-triggers.ts',
  // --- the workflow arena (desktop migration 026) ---
  'resolvers/ai/run-workflow-run-phase.ts',
  'resolvers/ai/create-workflow-version.ts',
  'resolvers/ai/record-workflow-pref.ts',
  'resolvers/ai/record-workflow-rejection.ts',
  'resolvers/ai/record-workflow-run-gate.ts',
  'resolvers/ai/record-workflow-run-rescore.ts',
  'resolvers/ai/run-workflow-run-rescore.ts',
  'resolvers/ai/repo-identity.ts',
  'resolvers/ai/run-workflow-run-gate.ts',
  'resolvers/ai/start-workflow-run.ts',
  'resolvers/ai/update-workflow-champion.ts',
  'resolvers/ai/update-workflow-search-resumed.ts',
  'resolvers/ai/workflow-runs.ts',
  'resolvers/ai/workflow-prefs.ts',
  'resolvers/ai/workflow-search-state.ts',
  'resolvers/ai/workflow.ts',
  'resolvers/ai/workflows.ts',
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
  // Plans tab (initiative 8ad3ae71, phase 01): draft plans + versioned brief, all relay-to-host.
  'resolvers/ai/list-draft-plans.ts',
  'resolvers/ai/get-brief-version.ts',
  'resolvers/ai/list-brief-versions.ts',
  'resolvers/ai/get-brief-diff.ts',
  'resolvers/ai/get-brief-check.ts',
  'resolvers/ai/create-draft-plan.ts',
  'resolvers/ai/create-draft-plan-duplicate.ts',
  'resolvers/ai/update-brief-field.ts',
  'resolvers/ai/reset-brief-to-version.ts',
  'resolvers/ai/add-outline-phase.ts',
  'resolvers/ai/add-outline-task.ts',
  'resolvers/ai/update-outline-item.ts',
  'resolvers/ai/update-outline-item-position.ts',
  'resolvers/ai/delete-outline-item.ts',
  'resolvers/ai/update-planning-queue.ts',
  // Discord action links: mint is identity-gated like every other write. Preview and run are
  // TOKEN-AUTHORIZED instead — see TOKEN_AUTHORIZED below.
  'resolvers/ai/create-action-link.ts',
];

/**
 * Resolvers authorized by a signed, single-action token INSTEAD of a session identity. A phone
 * tap on a Discord link has no login, by design. They reach D1 and the relay only through
 * lib/action-links (which verifies the token before anything else), so the grep classifier does
 * not see them as protected. Because that is also how a resolver could slip PAST the classifier,
 * the action-link wiring is fenced here instead:
 *  - only create-action-link (identity-gated) and the entries below may import it;
 *  - each entry's only work is the one pinned call into lib/action-links;
 *  - it never reads or writes ctx.auth (the identity it acts as comes from the verified token and
 *    is passed to the relay explicitly — memoising it onto ctx would let a second field of the
 *    same GraphQL document run as the minter).
 */
const TOKEN_AUTHORIZED: Record<string, RegExp> = {
  'resolvers/ai/preview-action-link.ts': /return preview\(depsFromCtx\(ctx\), args\.token, requestTenant\(ctx\)\);/,
  'resolvers/ai/run-action-link.ts': /return redeemActionLink\(depsFromCtx\(ctx\), args\.token, requestTenant\(ctx\), requestMethod\(ctx\)\);/,
};

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

  it('token-authorized resolvers keep their pinned shape and never touch ctx.auth', () => {
    for (const [rel, shape] of Object.entries(TOKEN_AUTHORIZED)) {
      const hit = Object.entries(resolverSources).find(([k]) => relativeResolverPath(k) === rel);
      expect(hit, `missing ${rel}`).toBeTruthy();
      const src = hit![1].replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
      expect(src, rel).toMatch(shape);
      expect(src, rel).not.toMatch(/ctx\.auth|ctx\.db|CHAT_RELAY_DO|desktopRpc|relayRpc|requireIdentity/);
    }
    const wired = Object.entries(resolverSources)
      .filter(([, src]) => src.includes('lib/action-links/'))
      .map(([k]) => relativeResolverPath(k))
      .sort();
    expect(wired).toEqual(['resolvers/ai/create-action-link.ts', ...Object.keys(TOKEN_AUTHORIZED)].sort());
    const create = Object.entries(resolverSources).find(([k]) => relativeResolverPath(k) === 'resolvers/ai/create-action-link.ts')![1];
    expect(create).toMatch(/default async function createActionLink\([^)]*\) \{\s*const identity = await requireIdentity\(ctx\);/);
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
