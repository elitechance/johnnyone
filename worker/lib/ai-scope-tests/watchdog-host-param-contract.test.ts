import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import * as desktopRpcMod from '../../lib/runtime/desktop-rpc';
import * as apiKeyMod from '../../lib/auth/api-key';
import { authedCtx } from '../auth/test-authed-ctx';

// Contract pin for the watchdog "Missing 'id' parameter" bug.
//
// Saving a watchdog prompt from the console ALWAYS failed: the Worker resolver sent
// `{ sessionId, prompt }` while the host handler `rpc_update_session_watchdog_prompt` reads
// `params.get("id")` and answers `Missing 'id' parameter`. Every OTHER watchdog handler reads
// `sessionId` — the host is the odd one out — but fixing the host needs an engine rebuild, so the
// Worker maps `sessionId -> id` for that one method.
//
// A payload-shape test can only pin the one method it knows about. This test pins the CLASS of bug:
// it reads the host dispatch table + handler bodies straight out of the Rust source (no running
// host required), collects every key each handler reads with `.get("<key>")`, and asserts the
// Worker resolver sends a params object carrying every one of those keys. Any future rename on
// either side — host starts reading `session_id`, Worker stops sending `prompt` — fails here.

const HOST_SOURCE = 'desktop/src-tauri/src/agent/mod.rs';

const HOST_OK = { ok: true };

const mockCtx = authedCtx({ tenantId: 't1', userId: 'u1' });

function hostSource(): string {
  // Resolved from the repo root via `process.cwd()` (vitest runs there), so the test is cwd-stable.
  return readFileSync(resolve(process.cwd(), HOST_SOURCE), 'utf8');
}

/**
 * The host source is read lazily, INSIDE the specs, and through this wrapper. Two reasons:
 *   1. `process.cwd()` is the repo root under vitest, but the worker runs in a workerd pool whose
 *      fs layer can refuse a top-of-module read of a file outside `worker/`;
 *   2. a missing/unreadable engine source must fail with a clear message naming the path rather
 *      than an opaque `no such file or directory, readAll ...` at import time (which would take
 *      the whole suite down with zero tests collected).
 */
function hostSourceOrThrow(): string {
  const path = resolve(process.cwd(), HOST_SOURCE);
  try {
    return hostSourceAt(path);
  } catch (err) {
    // Fallback for a suite launched from a nested directory: walk up from this file.
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let i = 0; i < 8; i += 1) {
      dir = dirname(dir);
      try {
        return hostSourceAt(resolve(dir, HOST_SOURCE));
      } catch {
        /* keep walking up */
      }
    }
    throw new Error(
      `Cannot read the host RPC source at ${path} (${String(err)}). ` +
        'This contract test needs the desktop engine source next to the worker source.',
    );
  }
}

function hostSourceAt(path: string): string {
  return readFileSync(path, 'utf8');
}

/**
 * The workerd pool's fs layer only exposes files that the module graph pulled in, so a plain
 * `readFileSync` of the engine source (outside `worker/`) can throw ENOENT even though the file is
 * on disk. Ask vite to inline it as raw text and fall back to that when the direct read fails —
 * same bytes, same assertions, no running host required.
 */
async function hostSourceText(): Promise<string> {
  try {
    return hostSourceOrThrow();
  } catch (fsErr) {
    // Static (analyzable) specifier so vite bundles the engine source into the worker bundle.
    // `?raw` is only honoured when the importer sits inside the vite root, hence the sibling
    // copy of this spec under `worker/lib/host-source/`.
    const attempts: Array<Promise<{ default?: unknown }>> = [
      import('./host-source/agent-mod.rs?raw'),
      import('../../host-source/agent-mod.rs?raw'),
      import('../host-source/agent-mod.rs?raw'),
      import('../../../desktop/src-tauri/src/agent/mod.rs?raw'),
    ];
    for (const attempt of attempts) {
      try {
        const mod = await attempt;
        const text = String(mod.default ?? mod);
        if (text.includes('rpc_update_session_watchdog_prompt')) return text;
      } catch {
        /* try the next candidate path */
      }
    }
    throw fsErr;
  }
}

const CASES = [
  {
    method: 'update_session_watchdog',
    file: '../../resolvers/ai/update-session-watchdog',
    args: { sessionId: 's1', enabled: true, intervalSecs: 300 },
  },
  {
    method: 'update_session_watchdog_prompt',
    file: '../../resolvers/ai/update-session-watchdog-prompt',
    args: { sessionId: 's1', prompt: 'Still there? Nudge prompt.' },
  },
  {
    method: 'update_session_watchdog_triggers',
    file: '../../resolvers/ai/update-session-watchdog-triggers',
    args: { sessionId: 's1', onBlocked: true, onStaleReport: false },
  },
  {
    method: 'nudge_session_watchdog',
    file: '../../resolvers/ai/send-session-watchdog-nudge',
    args: { sessionId: 's1' },
  },
] as const;

/**
 * `"update_session_watchdog" => Self::rpc_update_session_watchdog(` → the fn name.
 * The dispatch arms are not all one-liners — rustfmt wraps the two longest into a block arm
 * (`"nudge_session_watchdog" => {` / `Self::rpc_nudge_session_watchdog(&req.params, state).await`),
 * so allow an optional `{` and any whitespace/newline between the arrow and `Self::`.
 */
function hostHandlerName(source: string, method: string): string {
  const dispatch = new RegExp(
    `"${method}"\\s*=>\\s*\\{?\\s*Self::([A-Za-z0-9_]+)\\s*\\(`,
  );
  const found = dispatch.exec(source);
  if (!found) {
    throw new Error(`No dispatch line \`"${method}" => Self::<fn>(\` in ${HOST_SOURCE}`);
  }
  return found[1];
}

/** The body of `fn <name>(` / `async fn <name>(` up to the next `\n    fn ` / `\n    async fn `. */
function hostHandlerBody(source: string, fnName: string): string {
  const start = new RegExp(`\\n    (?:async )?fn ${fnName}\\s*\\(`).exec(source);
  if (!start) {
    throw new Error(`No \`fn ${fnName}(\` definition in ${HOST_SOURCE}`);
  }
  const rest = source.slice(start.index);
  const next = rest.slice(1).search(/\n    (?:async )?fn /);
  return next === -1 ? rest : rest.slice(0, next + 1);
}

/** Every key the handler reads with `.get("<key>")`. */
function hostReadKeys(body: string): string[] {
  const keys: string[] = [];
  const re = /\.get\(\s*"([^"]+)"\s*\)/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(body)) !== null) {
    if (!keys.includes(match[1])) keys.push(match[1]);
  }
  return keys;
}

async function sentParams(file: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
  const rpcSpy = vi
    .spyOn(desktopRpcMod, 'desktopRpc')
    .mockResolvedValueOnce(HOST_OK as any);
  const { default: resolver } = await import(file);
  await resolver(null, { ...args }, mockCtx as any);
  expect(rpcSpy).toHaveBeenCalledTimes(1);
  return rpcSpy.mock.calls[0][2] as Record<string, unknown>;
}

function stubAuth() {
  return vi.spyOn(apiKeyMod, 'authorizeForAltToken').mockResolvedValue(undefined as any);
}

describe('Watchdog Worker↔host param-name contract (no running host)', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  let source = '';

  beforeAll(async () => {
    source = await hostSourceText();
  });

  for (const { method, file, args } of CASES) {
    it(`${method}: every key the host reads is present in what the Worker sends`, async () => {
      stubAuth();
      const fnName = hostHandlerName(source, method);
      const readKeys = hostReadKeys(hostHandlerBody(source, fnName));
      expect(readKeys.length, `host handler ${fnName} reads no .get("<key>") keys`).toBeGreaterThan(0);

      const params = await sentParams(file, args);
      for (const key of readKeys) {
        expect(
          params,
          `${method}: host handler ${fnName} reads params.get("${key}"), but the Worker resolver ` +
            `${file} sends [${Object.keys(params).join(', ')}] — missing "${key}"`,
        ).toHaveProperty(key);
      }
    });
  }

  it('the prompt handler is the one host watchdog method reading `id` (rebuild to drop the mapping)', () => {
    const readingId = CASES.filter(({ method }) =>
      hostReadKeys(hostHandlerBody(source, hostHandlerName(source, method))).includes('id'),
    ).map(({ method }) => method);
    // If the engine is ever rebuilt to standardise on `sessionId`, this list empties and the
    // `id` mapping in update-session-watchdog-prompt.ts should be removed.
    expect(readingId).toEqual(['update_session_watchdog_prompt']);
  });
});