import { requireIdentity } from '../../lib/auth/require-identity';
import { requireScope } from '../../lib/auth/scopes';
import { actionScope, isActionKind } from '../../lib/action-links/actions';
import { auditActionLink, mintActionLink, MintError } from '../../lib/action-links/action-links';
import { depsFromCtx, type ActionLinkCtx } from '../../lib/action-links/context';

/**
 * Mint a Discord action link (signed, 120 s, single-use). Called by the desktop host's
 * `mintActionLink` on 127.0.0.1:7788 with its relay credential; the worker signs, so
 * ACTION_LINK_SECRET never leaves it. Tenant and user come from the verified identity, never
 * from the input. The caller must hold the scope the action needs (actions.ts `actionScope`),
 * so a link can never do more than the identity that minted it.
 */
export default async function createActionLink(
  _parent: unknown,
  args: { input: { act: string; args: string; label: string; group?: string | null; targetTitle?: string | null } },
  ctx: ActionLinkCtx,
) {
  const identity = await requireIdentity(ctx);
  const input = args.input;
  if (!isActionKind(input?.act)) {
    await auditActionLink(ctx.db, {
      kind: 'reject',
      jti: null,
      tenantId: identity.tenantId,
      act: String(input?.act ?? '').slice(0, 32),
      reason: 'mint: action is not allowlisted',
    });
    throw new MintError('createActionLink: action is not allowlisted');
  }
  requireScope(ctx as any, actionScope(input.act));
  return mintActionLink(depsFromCtx(ctx), identity, input);
}
