/**
 * Grep-style classifier for the resolver auth audit (acceptance 7).
 * Pure string checks — no fs. Used by resolver-auth-audit.test.ts.
 */

const RPC_CALL = /\b(desktopRpc|relayRpc)\s*[<(]/;
const DESTRUCTURE = /\b(?:const|let|var)\s*\{[^}]*(?:\bdb\b|\bauth\b|\bpubsub\b)[^}]*\}\s*=\s*ctx/;

export function isProtectedSource(src: string): boolean {
  if (src.includes('ctx.db')) return true;
  if (src.includes('CHAT_RELAY_DO')) return true;
  if (RPC_CALL.test(src)) return true;
  if (src.includes('ctx.auth')) return true;
  if (src.includes('ctx.pubsub')) return true;
  if (src.includes('pubsub.subscribe(')) return true;
  if (/subscribe\s*:/.test(src)) return true;
  if (DESTRUCTURE.test(src)) return true;
  return false;
}

export function obtainsIdentity(
  src: string,
  helpers: { desktopRpcHasRequire: boolean; relayRpcHasRequire: boolean },
): boolean {
  if (src.includes('requireIdentity') || src.includes('authorizeForAltToken')) {
    return true;
  }
  if (/\bdesktopRpc\s*[<(]/.test(src) && helpers.desktopRpcHasRequire) return true;
  if (/\brelayRpc\s*[<(]/.test(src) && helpers.relayRpcHasRequire) return true;
  return false;
}

export function classifyResolverSource(
  src: string,
  helpers: { desktopRpcHasRequire: boolean; relayRpcHasRequire: boolean },
): { protected: boolean; safe: boolean } {
  const prot = isProtectedSource(src);
  if (!prot) return { protected: false, safe: true };
  return { protected: true, safe: obtainsIdentity(src, helpers) };
}
