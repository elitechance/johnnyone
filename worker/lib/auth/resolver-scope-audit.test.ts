import { describe, it, expect } from 'vitest';
import { API_SCOPES } from './scopes';

/**
 * resolver-scope-audit.test.ts — the SCOPE half of the resolver auth audit.
 *
 * Why this file exists separately from resolver-auth-audit.test.ts:
 * that audit only proves a protected resolver obtains an IDENTITY, and its
 * classifier treats a bare `desktopRpc(`/`relayRpc(` call as sufficient
 * (both helpers call requireIdentity internally). That is correct for
 * identity and WRONG for authority: a `jk_` API key with an EMPTY scope array
 * passes requireIdentity, so an identity-only resolver is reachable by any
 * key regardless of what the operator granted it. That hole is exactly how
 * updateSetting / getSetting / readHostFile shipped ungated.
 *
 * This test is additive and deliberately narrow: it does not re-classify the
 * tree. It is an explicit allow-list — every resolver named here relays a
 * host-surface RPC and MUST carry the exact scope string listed. Adding a row
 * is the cheap way to pin a new host surface to the authority it needs.
 *
 * The check is deliberately NOT "the literal string appears somewhere in the
 * file". A first draft of this test was exactly that, and QA proved it green
 * against four mutants (gate only in a comment / gate in a dead branch / RPC
 * called before the gate / gate's throw swallowed by try-catch). Those four
 * are now fixture cases below, each asserted to be REJECTED. The live rule is:
 * comments are stripped first, and the gate must be the FIRST STATEMENT of the
 * exported default function's body — which is also what every compliant
 * resolver already looks like, so nothing had to be loosened to fit.
 */
const SCOPE_GATED: Record<string, string> = {
  // ── Host files_root surface (already compliant; seeded as regression pins) ──
  'files-read.ts': 'files:read',
  'files-list-dir.ts': 'files:read',
  'files-write.ts': 'files:write',
  'files-delete.ts': 'files:write',
  'files-mkdir.ts': 'files:write',
  'files-rename.ts': 'files:write',
  'files-upload-chunk.ts': 'files:write',

  // ── Host settings surface ──
  // settings carry files_root itself — the root the host measures its own path
  // confinement against — so settings:write is strictly more privileged than
  // files:write and must never be satisfiable by it.
  'update-setting.ts': 'settings:write',
  // get_setting can return webhook URLs and provider keys, so the read side is
  // gated too rather than left on identity alone.
  'get-setting.ts': 'settings:read',

  // ── Host plan-workspace file read ──
  // The host path-confines read_host_file to the plan's workspace_path, so the
  // residual exposure is purely "which identity may ask" — a file read.
  'read-host-file.ts': 'files:read',
};

/** Live, non-vacuous: tolerates the `<T>` type argument every real call uses. */
const RPC_CALL = /\b(?:desktopRpc|relayRpc)\s*(?:<[^>]*>)?\s*\(/;

/**
 * Blank out `//` and block comments, preserving length and newlines so every
 * index computed afterwards still lines up with the original source. String
 * and template literals are tracked so a `//` inside one is never treated as
 * a comment. (Regex literals are not tracked — no resolver in SCOPE_GATED
 * contains one, and a new one would show up as a hard failure, not a silent
 * pass.)
 */
export function stripComments(src: string): string {
  const out = src.split('');
  let i = 0;
  const blank = (from: number, to: number) => {
    for (let k = from; k < to && k < out.length; k++) {
      if (out[k] !== '\n') out[k] = ' ';
    }
  };
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (c === '/' && next === '/') {
      const end = src.indexOf('\n', i);
      blank(i, end === -1 ? src.length : end);
      i = end === -1 ? src.length : end;
      continue;
    }
    if (c === '/' && next === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end === -1 ? src.length : end + 2;
      blank(i, stop);
      i = stop;
      continue;
    }
    if (c === "'" || c === '"' || c === '`') {
      const quote = c;
      i++;
      while (i < src.length) {
        if (src[i] === '\\') { i += 2; continue; }
        if (src[i] === quote) { i++; break; }
        i++;
      }
      continue;
    }
    i++;
  }
  return out.join('');
}

/**
 * The body of `export default [async] function name(...) { … }`, brace-matched.
 * Returns null when there is no such declaration (e.g. an object-literal
 * resolver), which the caller reports as a violation rather than a pass.
 */
export function defaultFunctionBody(strippedSrc: string): string | null {
  const sig = /export\s+default\s+(?:async\s+)?function\s*\*?\s*[A-Za-z0-9_$]*\s*\(/.exec(strippedSrc);
  if (!sig) return null;
  let i = sig.index + sig[0].length;
  let depth = 1;
  while (i < strippedSrc.length && depth > 0) {
    if (strippedSrc[i] === '(') depth++;
    else if (strippedSrc[i] === ')') depth--;
    i++;
  }
  if (depth !== 0) return null;
  const open = strippedSrc.indexOf('{', i);
  if (open === -1) return null;
  depth = 1;
  let j = open + 1;
  while (j < strippedSrc.length && depth > 0) {
    if (strippedSrc[j] === '{') depth++;
    else if (strippedSrc[j] === '}') depth--;
    j++;
  }
  if (depth !== 0) return null;
  return strippedSrc.slice(open + 1, j - 1);
}

/**
 * The single rule, shared by the real resolvers and the mutant fixtures.
 * Returns [] when the source enforces `scope` properly.
 */
export function scopeGateViolations(src: string, scope: string): string[] {
  const v: string[] = [];
  const stripped = stripComments(src);
  const gate = `await authorizeForAltToken(ctx, '${scope}')`;

  if (!stripped.includes(`authorizeForAltToken(ctx, '${scope}')`)) {
    v.push(`no live (non-comment) authorizeForAltToken(ctx, '${scope}') call`);
  }

  const body = defaultFunctionBody(stripped);
  if (body === null) {
    v.push('no `export default function` declaration to inspect');
  } else if (!body.trimStart().startsWith(gate)) {
    const firstStatement = body.trimStart().split('\n')[0].trim();
    v.push(`scope gate is not the first statement of the default export (first statement: \`${firstStatement}\`)`);
  }

  // Redundant with "first statement" today, kept live so the ordering property
  // is still pinned if that rule is ever relaxed for a resolver that needs a
  // prologue. Indices are valid because stripComments preserves length.
  const gateAt = stripped.indexOf(gate);
  const rpcAt = stripped.search(RPC_CALL);
  if (gateAt > -1 && rpcAt > -1 && gateAt > rpcAt) {
    v.push('relay/desktop RPC call precedes the scope gate');
  }

  return v;
}

const resolverSources = import.meta.glob('../../resolvers/**/*.ts', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function sourceForBasename(basename: string): string {
  const hit = Object.entries(resolverSources).find(([k]) =>
    k.replace(/\\/g, '/').endsWith(`/${basename}`),
  );
  if (!hit) throw new Error(`SCOPE_GATED names a resolver that does not exist: ${basename}`);
  return hit[1];
}

/**
 * QA's four mutants. Each is a plausible regression that the naive
 * "literal string is present" check shipped green. All four must be REJECTED.
 */
const MUTANTS: Record<string, string> = {
  'A: the gate literal lives only inside a comment': `
import { relayRpc } from '../../lib/runtime/relay-rpc';
// Previously this was gated with await authorizeForAltToken(ctx, 'settings:write') but
// we removed it for the CI smoke path.
export default async function updateSetting(_p, args, ctx) {
  return relayRpc(ctx, 'set_setting', { key: args.key, value: args.value });
}
`,
  'B: the gate sits in a dead branch, the RPC is unconditional': `
import { relayRpc } from '../../lib/runtime/relay-rpc';
export default async function updateSetting(_p, args, ctx) {
  if (false) { await authorizeForAltToken(ctx, 'settings:write'); }
  return relayRpc(ctx, 'set_setting', { key: args.key, value: args.value });
}
`,
  'C: the RPC is awaited first, the gate after (real ordering bug)': `
import { relayRpc } from '../../lib/runtime/relay-rpc';
export default async function updateSetting(_p, args, ctx) {
  const out = await relayRpc<boolean>(ctx, 'set_setting', { key: args.key, value: args.value });
  await authorizeForAltToken(ctx, 'settings:write');
  return out;
}
`,
  'D: the gate is present but its denial is swallowed by try/catch': `
import { relayRpc } from '../../lib/runtime/relay-rpc';
export default async function updateSetting(_p, args, ctx) {
  try { await authorizeForAltToken(ctx, 'settings:write'); } catch {}
  return relayRpc<boolean>(ctx, 'set_setting', { key: args.key, value: args.value });
}
`,
};

/** The shape every compliant resolver has — proves the rule is satisfiable. */
const COMPLIANT_FIXTURE = `
import { relayRpc, type RelayRpcContext } from '../../lib/runtime/relay-rpc';
import { authorizeForAltToken } from '../../lib/auth/api-key';

// A comment mentioning authorizeForAltToken(ctx, 'settings:write') must not be
// what satisfies the test — the real call below is.
export default async function updateSetting(
  _parent: unknown,
  args: { key: string; value: string },
  ctx: RelayRpcContext,
) {
  await authorizeForAltToken(ctx, 'settings:write');
  return relayRpc<boolean>(ctx, 'set_setting', { key: args.key, value: args.value });
}
`;

describe('resolver scope audit', () => {
  it('every scope SCOPE_GATED requires is a real API scope', () => {
    const unknown = [...new Set(Object.values(SCOPE_GATED))].filter(
      (s) => !(API_SCOPES as readonly string[]).includes(s),
    );
    expect(unknown, `scopes not in API_SCOPES: ${unknown.join(', ')}`).toEqual([]);
  });

  it('every scope-gated resolver enforces its exact scope as its first statement', () => {
    const violations: string[] = [];
    for (const [basename, scope] of Object.entries(SCOPE_GATED)) {
      for (const v of scopeGateViolations(sourceForBasename(basename), scope)) {
        violations.push(`${basename} (${scope}): ${v}`);
      }
    }
    expect(violations, `ungated host-surface resolvers:\n  ${violations.join('\n  ')}`).toEqual([]);
  });

  it('the RPC-call regex actually matches the `<T>(ctx, …)` form every resolver uses', () => {
    // Guards against the original bug: `\s*[<(]\s*ctx` never matched
    // `relayRpc<boolean>(ctx, …)`, so the ordering assertion was vacuous.
    expect(RPC_CALL.test('return relayRpc<boolean>(ctx, "set_setting", {})')).toBe(true);
    expect(RPC_CALL.test('return desktopRpc<unknown>(ctx, "read_host_file", {})')).toBe(true);
    expect(RPC_CALL.test('return desktopRpc(ctx, "list_prompt_library", {})')).toBe(true);
    for (const [basename] of Object.entries(SCOPE_GATED)) {
      const stripped = stripComments(sourceForBasename(basename));
      expect(stripped.search(RPC_CALL), `${basename}: no RPC call matched`).toBeGreaterThan(-1);
    }
  });

  it('accepts the compliant resolver shape', () => {
    expect(scopeGateViolations(COMPLIANT_FIXTURE, 'settings:write')).toEqual([]);
  });

  it.each(Object.entries(MUTANTS))('rejects mutant %s', (_name, src) => {
    const violations = scopeGateViolations(src, 'settings:write');
    expect(violations.length, 'mutant was accepted').toBeGreaterThan(0);
  });

  it('comment stripping does not eat a // inside a string literal', () => {
    const stripped = stripComments(`const u = 'https://x/y'; // gone\nconst v = 1;`);
    expect(stripped).toContain("'https://x/y'");
    expect(stripped).not.toContain('gone');
    expect(stripped).toHaveLength(`const u = 'https://x/y'; // gone\nconst v = 1;`.length);
  });
});
