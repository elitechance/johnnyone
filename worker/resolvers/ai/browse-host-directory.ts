import { desktopRpc } from '../../lib/runtime/desktop-rpc';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

export default async function browseHostDirectory(_parent: unknown, args: { path: string }, ctx: ResolverContext) {
  return desktopRpc<unknown[]>(ctx, 'browse_host_directory', { path: args.path });
}
