import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string }; request?: Request; [key: string]: unknown }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// The workflow arena's list read (desktop migration 026). Query, not Mutation: lokal infers the
// GraphQL field type from the resolver FILENAME's leading verb, and a prefix-less name defaults to
// Query — same convention note as files-list-dir.ts.
//
// All three filters are optional and independent; absent means "any". Nulls are passed explicitly
// so the host's param-puller sees a key rather than inferring a default.
export default async function workflows(
  _parent: unknown,
  args: { repo?: string | null; phaseType?: string | null; family?: string | null },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'plans:read');
  return desktopRpc<unknown[]>(ctx, 'list_workflows', {
    repo: args.repo ?? null,
    phaseType: args.phaseType ?? null,
    family: args.family ?? null,
  });
}
