import { desktopRpc } from '../../lib/runtime/desktop-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

interface ResolverContext {
  db: D1Database;
  env: WorkerEnv;
  auth: { userId: string; tenantId: string };
  request?: Request;
  [key: string]: unknown;
}

interface WorkerEnv {
  CHAT_RELAY_DO: DurableObjectNamespace;
  [key: string]: unknown;
}

// Briefing loop (overhaul P4, D5): thin pass-through to the host `initiative_upload_chunk`, which
// re-roots the P2 upload engine at <initiatives_dir>/<id>/attachments/. No FS logic here — the host
// owns the path-guard + caps; the worker only relays, gated by the files:write scope. `dataBase64`
// is never logged (D10).
export default async function initiativeUploadChunk(
  _parent: unknown,
  args: {
    initiativeId: string;
    path: string;
    chunkIndex: number;
    totalChunks: number;
    dataBase64: string;
    done: boolean;
  },
  ctx: ResolverContext,
) {
  await authorizeForAltToken(ctx, 'files:write');
  return desktopRpc<unknown>(ctx, 'initiative_upload_chunk', {
    initiativeId: args.initiativeId,
    path: args.path,
    chunkIndex: args.chunkIndex,
    totalChunks: args.totalChunks,
    dataBase64: args.dataBase64,
    done: args.done,
  });
}
