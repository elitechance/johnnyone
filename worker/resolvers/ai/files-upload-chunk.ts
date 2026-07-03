import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext { db: D1Database; env: WorkerEnv; auth: { userId: string; tenantId: string } }
interface WorkerEnv { CHAT_RELAY_DO: DurableObjectNamespace; [key: string]: unknown }

// File manager (overhaul P2): thin pass-through to the host files_root surface. No FS logic here —
// the host owns behavior; the worker only relays, gated by the files:write scope. `dataBase64` is
// never logged.
export default async function filesUploadChunk(
  _parent: unknown,
  args: { path: string; chunkIndex: number; totalChunks: number; dataBase64: string; done: boolean },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:write');
  return desktopRpc<unknown>(ctx, 'files_upload_chunk', {
    path: args.path,
    chunkIndex: args.chunkIndex,
    totalChunks: args.totalChunks,
    dataBase64: args.dataBase64,
    done: args.done,
  });
}
