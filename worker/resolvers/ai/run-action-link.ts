import { redeemActionLink } from '../../lib/action-links/action-links';
import { depsFromCtx, requestMethod, requestTenant, type ActionLinkCtx } from '../../lib/action-links/context';

/**
 * Redeem an action link: the /act page's Confirm. (The brief calls this `redeemActionLink`; the
 * lokal CLI infers Query vs Mutation from the filename verb and has no `redeem-` verb, so it is
 * `run-` here, which it already classifies as a Mutation.)
 *
 * Token-authorized, no login. The token is verified, its jti is claimed atomically in D1
 * (single use), and only then is the action relayed to the minter's desktop — as the identity
 * IN THE TOKEN, passed explicitly; the session identity on the context is never read or set
 * (see context.ts for why).
 *
 * Only a POST may redeem: GraphQL-over-GET is refused by yoga for mutations already, and again
 * here. CSRF: nothing about this request is authorized by ambient browser state (J1 sends no
 * cookies; its session is a Bearer header the /act page does not send). The only credential is
 * the token in the body, which a cross-site page would have to already hold — and holding it is
 * exactly the authority the link grants.
 */
export default async function runActionLink(
  _parent: unknown,
  args: { token: string },
  ctx: ActionLinkCtx,
) {
  return redeemActionLink(depsFromCtx(ctx), args.token, requestTenant(ctx), requestMethod(ctx));
}
