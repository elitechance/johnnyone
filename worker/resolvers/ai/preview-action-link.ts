import { previewActionLink as preview } from '../../lib/action-links/action-links';
import { depsFromCtx, requestTenant, type ActionLinkCtx } from '../../lib/action-links/context';

/**
 * Describe an action link for the /act page WITHOUT consuming it. Token-authorized, no login:
 * the signed token is the only credential and it is verified (signature, aud, iss, exp ±5 s,
 * allowlist, tenant) before anything is returned. Returns only the label, the action, a short
 * target line and the expiry — never the args, tenant, user or jti.
 *
 * Never acts and never writes action_link_redemptions, so a link-unfurler, a prefetch or a
 * reload cannot use a link up.
 */
export default async function previewActionLink(
  _parent: unknown,
  args: { token: string },
  ctx: ActionLinkCtx,
) {
  return preview(depsFromCtx(ctx), args.token, requestTenant(ctx));
}
