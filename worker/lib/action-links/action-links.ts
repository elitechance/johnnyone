/**
 * action-links.ts — mint / preview / redeem for Discord action links.
 *
 * Flow (see docs in the resolvers for the GraphQL surface):
 *   desktop host `mintActionLink` (127.0.0.1:7788)
 *     → worker `createActionLink` (authenticated: the desktop's relay credential)
 *     → stores the link's args + label under a fresh jti in D1 `action_link_mints`
 *     → signs a 120 s HS256 JWT {iss, aud, iat, exp, jti, tenant, sub, act} with
 *       ACTION_LINK_SECRET (the secret never leaves the worker)
 *     → returns https://<web>/act?t=<jwt> (≤ 512 chars: Discord's link-button limit)
 *   phone taps the link → /act page (GET: display only)
 *     → worker `previewActionLink(token)`: verifies, consumes NOTHING
 *   phone taps Confirm → POST worker `runActionLink(token)`:
 *     → verify signature / aud / iss / exp (±5 s) / tenant+user, and that the jti's mint row
 *       matches the token (tenant, user, act, exp) — args and label come from that row
 *     → INSERT the jti into action_link_redemptions (PRIMARY KEY) — the insert IS the claim:
 *       `ON CONFLICT DO NOTHING` + `changes === 1` means exactly one request ever wins, even
 *       when two arrive at once; the loser is told the link was already used
 *       In the same D1 batch (one transaction) every sibling of the link's `group` (a Yes/No
 *       pair) is marked `superseded`, so redeeming one answer uses up the others.
 *     → only then relay `perform_action_link` to the minter's desktop, which re-checks the
 *       allowlist and that it minted this jti, and runs the existing path for the action.
 *
 * Audit: a row per mint / preview / redeem / rejection of a token whose SIGNATURE verified.
 * Unsigned garbage (bad header/signature, malformed) and non-POST redeems write no row — they
 * cost an unauthenticated caller nothing, so they are only counted in a log line (H-1-16).
 *
 * The token is a bearer credential. It is never logged, never stored, never returned except
 * by the mint itself; audit rows carry the jti only.
 *
 * Every function takes its dependencies (db, secret, clock, relay) explicitly so the security
 * properties are testable against a real D1 without a desktop (action-links.test.ts).
 */
import {
  actionScope,
  describeTarget,
  INVISIBLE_RE,
  validateActionArgs,
  validateLabel,
  type ActionArgs,
  type ActionKind,
} from './actions';
import {
  ACTION_LINK_AUD,
  ACTION_LINK_ISS,
  ACTION_LINK_TTL_S,
  jtiForLog,
  signActionToken,
  verifyActionToken,
  type ActionClaims,
  type VerifyCode,
} from './token';

export const DEFAULT_ACTION_LINK_BASE_URL = 'https://johnnyone.pages.dev';
/** Discord rejects a link button whose URL is longer than this. */
export const MAX_ACTION_LINK_URL = 512;
/** An expired link previews its label/target only this long past exp; after that, nothing. */
export const EXPIRED_DETAIL_WINDOW_S = 600;
/** Mint and redemption rows are useless after exp; they are pruned a day later. */
export const ROW_RETENTION_S = 86_400;
/** Audit rows are kept for 30 days. */
export const EVENT_RETENTION_DAYS = 30;

/** Relay one RPC to the desktop node of (tenantId, userId). Throws when it cannot. */
export type ActionRelay = (
  identity: { tenantId: string; userId: string },
  method: string,
  params: Record<string, unknown>,
) => Promise<unknown>;

export interface ActionLinkDeps {
  db: D1Database;
  secret: string | undefined;
  /** Seconds since the epoch. */
  now: () => number;
  relay: ActionRelay;
  /** Base URL of the web app that serves /act. */
  baseUrl?: string;
  /** Schedule best-effort work past the response (ctx.waitUntil); awaited inline when absent. */
  defer?: (p: Promise<unknown>) => void;
  /** Log sink for unaudited rejections (defaults to console.warn). Never given the token. */
  log?: (line: string) => void;
}

export type RejectCode =
  | 'expired'
  | 'used'
  | 'superseded'
  | 'invalid'
  | 'unavailable'
  | 'uncertain'
  | 'restarted'
  | 'answered'
  | 'ended'
  | 'failed';

/** The user-facing copy for each outcome. Deliberately generic: it reveals nothing about why. */
export const ACTION_LINK_COPY: Record<RejectCode, string> = {
  expired: 'This link expired, ask J1 for a new one.',
  used: 'This link was already used, ask J1 for a new one.',
  invalid: 'This link is not valid, ask J1 for a new one.',
  unavailable: 'J1 could not reach your desktop host. This link is now used up; open J1 to do it by hand.',
  uncertain: 'It may have gone through, so check J1 before trying again.',
  restarted: 'J1 restarted since this link was made, so ask for a new link.',
  superseded: 'Another answer was already chosen.',
  answered: 'The agent already got an answer, so nothing was sent.',
  ended: "That agent's session has ended, so nothing was sent.",
  failed: 'J1 could not do this. This link is now used up; open J1 to check what happened.',
};

/** How a used link ended, as the preview reports it (H-1-5). */
export type UsedOutcome = 'done' | 'failed' | 'uncertain' | 'superseded' | 'answered' | 'ended' | 'pending';

export const USED_COPY: Record<UsedOutcome, string> = {
  done: 'Already done ✓',
  failed: 'Tried, but J1 could not do it, so check J1.',
  uncertain: ACTION_LINK_COPY.uncertain,
  superseded: 'Another answer was already chosen.',
  answered: 'The agent already got an answer, so nothing was sent.',
  ended: "That agent's session had ended, so nothing was sent.",
  pending: 'This link is being used right now; check J1 in a moment.',
};

export interface MintInput {
  act: string;
  args: string;
  label: string;
  /** Sibling links (a Yes/No pair) share a group; redeeming one uses up the rest. */
  group?: string | null;
  /** Human description of the target (plan / agent titles), from the desktop. */
  targetTitle?: string | null;
}

export interface MintResult {
  url: string;
  jti: string;
  expiresAt: number;
}

export interface PreviewResult {
  state: 'valid' | 'expired' | 'used' | 'invalid';
  act: ActionKind | null;
  label: string | null;
  target: string | null;
  expiresAt: number | null;
  message: string | null;
  /** For `used`: how it ended. */
  outcome: UsedOutcome | null;
  /** The worker's clock (s), so the page counts down against it, not the phone's (H-1-10). */
  serverNow: number;
}

export interface RunResult {
  ok: boolean;
  code: RejectCode | null;
  summary: string;
}

export class MintError extends Error {}

const GROUP_RE = /^[A-Za-z0-9_-]{1,64}$/;
const TITLE_FORMAT_RE = /\p{Cf}/gu;
const TITLE_INVISIBLE_RE = new RegExp(INVISIBLE_RE.source, 'gu');
// eslint-disable-next-line no-control-regex
const TITLE_CONTROL_RE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g;

function parseArgs(raw: string): unknown {
  if (typeof raw !== 'string' || raw.length > 2048) return undefined;
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/**
 * Audit: one row per mint / preview / redeem / rejection, jti only. Best-effort: an audit write
 * failing must never turn a refusal into an acceptance or vice versa.
 */
export async function auditActionLink(
  db: D1Database,
  row: { kind: 'mint' | 'preview' | 'redeem' | 'reject'; jti: string | null; tenantId: string | null; act: string | null; reason: string | null },
): Promise<void> {
  try {
    await db
      .prepare(
        `INSERT INTO action_link_events (id, jti, tenant_id, kind, act, reason, created_at)
         VALUES (?, ?, ?, ?, ?, ?, datetime('now'))`,
      )
      .bind(crypto.randomUUID(), row.jti, row.tenantId, row.kind, row.act, row.reason)
      .run();
  } catch {
    // swallowed on purpose (see above)
  }
}

/** Count an unaudited rejection in a log line: the stage and the code, never the token. */
function logOnly(deps: ActionLinkDeps, stage: string, code: string): void {
  (deps.log ?? ((l: string) => console.warn(l)))(`action-link ${stage} rejected (unaudited): ${code}`);
}

/** Opportunistic pruning on each mint (H-1-17). Best-effort. */
async function prune(db: D1Database, nowS: number): Promise<void> {
  const cutoff = nowS - ROW_RETENTION_S;
  try {
    await db.batch([
      db.prepare('DELETE FROM action_link_mints WHERE exp < ?').bind(cutoff),
      db.prepare('DELETE FROM action_link_redemptions WHERE exp < ?').bind(cutoff),
      db.prepare(`DELETE FROM action_link_events WHERE created_at < datetime('now', ?)`).bind(`-${EVENT_RETENTION_DAYS} days`),
    ]);
  } catch {
    // best-effort
  }
}

/** Mirror an audit row onto the plan timeline on the desktop (best-effort, never awaited by the caller's outcome). */
function timeline(deps: ActionLinkDeps, link: Link, kind: string, reason: string | null): void {
  const { claims } = link;
  const p = deps
    .relay({ tenantId: claims.tenant, userId: claims.sub }, 'record_action_link_event', {
      jti: claims.jti,
      kind,
      reason,
      act: claims.act,
      args: link.args,
    })
    .catch(() => undefined);
  if (deps.defer) deps.defer(p);
}

function cleanTitle(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  const t = raw.replace(TITLE_CONTROL_RE, ' ').replace(TITLE_FORMAT_RE, '').replace(TITLE_INVISIBLE_RE, '').trim();
  return t ? [...t].slice(0, 120).join('') : null;
}

/**
 * Mint: called by createActionLink with an already-authenticated identity. The identity, not
 * the caller's input, decides tenant and sub.
 */
export async function mintActionLink(
  deps: ActionLinkDeps,
  identity: { tenantId: string; userId: string },
  input: MintInput,
): Promise<MintResult> {
  const parsed = parseArgs(input.args);
  const check = validateActionArgs(input.act, parsed);
  if (!check.ok) {
    await auditActionLink(deps.db, { kind: 'reject', jti: null, tenantId: identity.tenantId, act: String(input.act).slice(0, 32), reason: `mint: ${check.reason}` });
    throw new MintError(`createActionLink: ${check.reason}`);
  }
  const labelErr = validateLabel(input.label);
  if (labelErr) throw new MintError(`createActionLink: ${labelErr}`);
  const group = input.group == null || input.group === '' ? null : input.group;
  if (group !== null && !GROUP_RE.test(group)) {
    throw new MintError('createActionLink: group must be 1-64 characters of [A-Za-z0-9_-]');
  }
  if (group !== null) {
    // A group closes once one of its links is redeemed: a link minted into it afterwards would be
    // a second answer to a question that already has one (H-2-29).
    const closed = await deps.db
      .prepare(
        `SELECT 1 AS closed FROM action_link_redemptions r JOIN action_link_mints m ON m.jti = r.jti
         WHERE m.group_id = ? AND m.tenant_id = ? AND m.user_id = ? AND r.status <> 'superseded' LIMIT 1`,
      )
      .bind(group, identity.tenantId, identity.userId)
      .first();
    if (closed) throw new MintError(`createActionLink: group ${group} was already answered; use a new group`);
  }

  const iat = deps.now();
  await prune(deps.db, iat);
  const claims: ActionClaims = {
    iss: ACTION_LINK_ISS,
    aud: ACTION_LINK_AUD,
    iat,
    exp: iat + ACTION_LINK_TTL_S,
    jti: crypto.randomUUID(),
    tenant: identity.tenantId,
    sub: identity.userId,
    act: input.act as ActionKind,
  };
  // Sign first (fails closed without a secret), check the URL fits a Discord button, and only
  // then write the mint row — so no row exists for a link that was never handed out.
  const token = await signActionToken(claims, deps.secret);
  const base = (deps.baseUrl || DEFAULT_ACTION_LINK_BASE_URL).replace(/\/+$/, '');
  const url = `${base}/act?t=${token}`;
  if (url.length > MAX_ACTION_LINK_URL) {
    throw new MintError(`createActionLink: link is ${url.length} characters, over Discord's ${MAX_ACTION_LINK_URL}`);
  }
  await deps.db
    .prepare(
      `INSERT INTO action_link_mints (jti, tenant_id, user_id, act, args_json, label, exp, group_id, target_title, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))`,
    )
    .bind(
      claims.jti,
      claims.tenant,
      claims.sub,
      claims.act,
      JSON.stringify(check.args),
      input.label.trim(),
      claims.exp,
      group,
      cleanTitle(input.targetTitle),
    )
    .run();
  await auditActionLink(deps.db, { kind: 'mint', jti: claims.jti, tenantId: claims.tenant, act: claims.act, reason: null });
  return { url, jti: claims.jti, expiresAt: claims.exp };
}

/** A verified token joined with its mint row. */
interface Link {
  claims: ActionClaims;
  args: ActionArgs;
  label: string;
  group: string | null;
  targetTitle: string | null;
}

/**
 * The mint row for a signature-verified token, or null when there is none or it disagrees with
 * the token. Args are re-validated against the allowlist on the way out.
 */
async function loadMint(db: D1Database, c: ActionClaims): Promise<Link | null> {
  const row = await db
    .prepare('SELECT tenant_id, user_id, act, args_json, label, exp, group_id, target_title FROM action_link_mints WHERE jti = ?')
    .bind(c.jti)
    .first<{ tenant_id: string; user_id: string; act: string; args_json: string; label: string; exp: number; group_id: string | null; target_title: string | null }>();
  if (!row) return null;
  if (row.tenant_id !== c.tenant || row.user_id !== c.sub || row.act !== c.act || Number(row.exp) !== c.exp) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(row.args_json);
  } catch {
    return null;
  }
  const check = validateActionArgs(c.act, parsed);
  if (!check.ok || validateLabel(row.label)) return null;
  return { claims: c, args: check.args, label: row.label, group: row.group_id ?? null, targetTitle: row.target_title ?? null };
}

function targetOf(l: Link): string {
  return describeTarget(l.claims.act, l.args, l.targetTitle);
}

type Checked =
  | { ok: true; link: Link }
  /**
   * `link` is set only when the signature verified AND the mint row matched. `signed` is false
   * when the signature never verified: such a rejection is not audited (H-1-16).
   */
  | { ok: false; code: RejectCode; jti: string | null; link?: Link; reason: string; signed: boolean };

const UNSIGNED: ReadonlySet<VerifyCode> = new Set<VerifyCode>(['malformed', 'bad_header', 'bad_signature']);

/**
 * Everything preview and run share: signature, aud, iss, exp, allowlist, then tenant. The
 * tenant check: if the request names a tenant (x-tenant-id) it must be the token's, and the
 * token's user must still be an active user of an active tenant.
 */
async function checkToken(deps: ActionLinkDeps, token: unknown, requestTenant: string | null): Promise<Checked> {
  const now = deps.now();
  const v = await verifyActionToken(token, deps.secret, now);
  if (!v.ok) {
    const code: RejectCode = v.code === 'expired' ? 'expired' : 'invalid';
    // An expired-but-genuine link still names its mint row, so the page can show what it was
    // for — but only for a short while past exp (H-1-17).
    const recent = v.code === 'expired' && v.claims && now - v.claims.exp <= EXPIRED_DETAIL_WINDOW_S;
    const link = recent && v.claims ? (await loadMint(deps.db, v.claims)) ?? undefined : undefined;
    return { ok: false, code, jti: jtiForLog(v.claims), link, reason: v.code, signed: !UNSIGNED.has(v.code) };
  }
  const c = v.claims;
  if (requestTenant && requestTenant !== c.tenant) {
    return { ok: false, code: 'invalid', jti: c.jti, reason: 'wrong_tenant', signed: true };
  }
  const member = await deps.db
    .prepare(
      `SELECT 1 AS ok FROM users u JOIN tenants t ON t.tenant_id = u.tenant_id
       WHERE u.id = ? AND u.tenant_id = ? AND u.status = 'active' AND t.is_active = 1 LIMIT 1`,
    )
    .bind(c.sub, c.tenant)
    .first<{ ok: number }>();
  if (!member) return { ok: false, code: 'invalid', jti: c.jti, reason: 'wrong_tenant', signed: true };
  const link = await loadMint(deps.db, c);
  if (!link) return { ok: false, code: 'invalid', jti: c.jti, reason: 'no_matching_mint', signed: true };
  return { ok: true, link };
}

async function usedOutcome(db: D1Database, jti: string): Promise<UsedOutcome | null> {
  const row = await db.prepare('SELECT status FROM action_link_redemptions WHERE jti = ?').bind(jti).first<{ status: string }>();
  if (!row) return null;
  switch (row.status) {
    case 'failed': {
      // An ended session is stored as 'failed' (the status CHECK predates it); the audit row's
      // reason tells the two apart (C-1-1).
      const ended = await db
        .prepare(`SELECT 1 AS ok FROM action_link_events WHERE jti = ? AND kind = 'reject' AND reason = ?`)
        .bind(jti, `redeem: ${ENDED_REASON}`)
        .first<{ ok: number }>();
      return ended ? 'ended' : 'failed';
    }
    case 'done':
    case 'uncertain':
    case 'superseded':
    case 'answered':
      return row.status;
    default:
      return 'pending';
  }
}

async function reject(deps: ActionLinkDeps, checked: Extract<Checked, { ok: false }>, stage: string): Promise<void> {
  if (!checked.signed) {
    logOnly(deps, stage, checked.reason);
    return;
  }
  // Only a verified token with a matching mint row has a trustworthy tenant; anything else is
  // recorded without one, and never reaches a desktop timeline.
  const trusted = checked.link;
  await auditActionLink(deps.db, {
    kind: 'reject',
    jti: checked.jti,
    tenantId: trusted?.claims.tenant ?? null,
    act: trusted?.claims.act ?? null,
    reason: `${stage}: ${checked.reason}`,
  });
  if (trusted) timeline(deps, trusted, 'rejected', `${stage}: ${checked.reason}`);
}

/** Preview: verify and describe. NEVER writes action_link_redemptions. */
export async function previewActionLink(
  deps: ActionLinkDeps,
  token: unknown,
  requestTenant: string | null,
): Promise<PreviewResult> {
  const serverNow = deps.now();
  const checked = await checkToken(deps, token, requestTenant);
  if (!checked.ok) {
    await reject(deps, checked, 'preview');
    // An expired-but-genuine link may still show what it was for, so the page can offer
    // "Open in J1" for that exact thing. Nothing is shown for an invalid one.
    const l = checked.code === 'expired' ? checked.link : undefined;
    return {
      state: checked.code === 'expired' ? 'expired' : 'invalid',
      act: l?.claims.act ?? null,
      label: l?.label ?? null,
      target: l ? targetOf(l) : null,
      expiresAt: l?.claims.exp ?? null,
      message: ACTION_LINK_COPY[checked.code],
      outcome: null,
      serverNow,
    };
  }
  const { link } = checked;
  const c = link.claims;
  const outcome = await usedOutcome(deps.db, c.jti);
  await auditActionLink(deps.db, { kind: 'preview', jti: c.jti, tenantId: c.tenant, act: c.act, reason: outcome ? `already_used:${outcome}` : null });
  timeline(deps, link, 'previewed', outcome ? `already_used:${outcome}` : null);
  return {
    state: outcome ? 'used' : 'valid',
    act: c.act,
    label: link.label,
    target: targetOf(link),
    expiresAt: c.exp,
    message: outcome ? USED_COPY[outcome] : null,
    outcome,
    serverNow,
  };
}

/** The audit reason of a redeem refused because the agent's session ended (C-1-1). */
const ENDED_REASON = 'desktop_ended';

/** Classify a relay failure. A timeout may still have acted, so it is `uncertain` (H-1-1). */
export function relayFailureCode(message: string): RejectCode {
  if (/timed out|timeout/i.test(message)) return 'uncertain';
  if (/unknown_link/.test(message)) return 'restarted';
  if (/answered:/.test(message)) return 'answered';
  // The host's `ended:` prefix (action_links.rs `ENDED`): the session ended since the mint.
  if (/(^|\s)ended:/.test(message)) return 'ended';
  if (/No online backend app found|Desktop not connected/i.test(message)) return 'unavailable';
  return 'failed';
}

/**
 * Run (redeem): verify, claim the jti atomically, then act. `method` is the HTTP method of the
 * GraphQL request; anything but POST is refused before the token is even looked at.
 */
export async function redeemActionLink(
  deps: ActionLinkDeps,
  token: unknown,
  requestTenant: string | null,
  method: string | null,
): Promise<RunResult> {
  if (method !== 'POST') {
    logOnly(deps, 'redeem', `method ${String(method).slice(0, 8)}`);
    return { ok: false, code: 'invalid', summary: ACTION_LINK_COPY.invalid };
  }
  const checked = await checkToken(deps, token, requestTenant);
  if (!checked.ok) {
    await reject(deps, checked, 'redeem');
    return { ok: false, code: checked.code, summary: ACTION_LINK_COPY[checked.code] };
  }
  const { link } = checked;
  const c = link.claims;

  // Insert-before-act. The PRIMARY KEY on jti makes this the single-use gate: of any number of
  // concurrent redeems, exactly one insert changes a row. The sibling statement runs in the SAME
  // batch (one D1 transaction): it marks every other unredeemed link of this link's group as
  // superseded, so of a Yes/No pair only the first tap acts. It is gated on THIS request's claim
  // nonce: a replay of an already-used link changes nothing in the first statement, finds no row
  // with its nonce, and so supersedes nothing either (H-2-29).
  const claimId = crypto.randomUUID();
  const statements = [
    deps.db
      .prepare(
        `INSERT INTO action_link_redemptions (jti, tenant_id, user_id, act, exp, status, claim_id, redeemed_at)
         VALUES (?, ?, ?, ?, ?, 'claimed', ?, datetime('now'))
         ON CONFLICT(jti) DO NOTHING`,
      )
      .bind(c.jti, c.tenant, c.sub, c.act, c.exp, claimId),
  ];
  if (link.group) {
    statements.push(
      deps.db
        .prepare(
          `INSERT INTO action_link_redemptions (jti, tenant_id, user_id, act, exp, status, redeemed_at)
           SELECT m.jti, m.tenant_id, m.user_id, m.act, m.exp, 'superseded', datetime('now')
           FROM action_link_mints m
           WHERE m.group_id = ? AND m.tenant_id = ? AND m.user_id = ? AND m.jti <> ?
             AND EXISTS (SELECT 1 FROM action_link_redemptions r WHERE r.jti = ? AND r.claim_id = ?)
           ON CONFLICT(jti) DO NOTHING`,
        )
        .bind(link.group, c.tenant, c.sub, c.jti, c.jti, claimId),
    );
  }
  const [claim] = await deps.db.batch(statements);
  if ((claim.meta?.changes ?? 0) !== 1) {
    // A sibling already answered: say so, rather than the generic "already used" (H-2-25).
    const code: RejectCode = (await usedOutcome(deps.db, c.jti)) === 'superseded' ? 'superseded' : 'used';
    await reject(deps, { ok: false, code, jti: c.jti, link, reason: code === 'superseded' ? 'superseded' : 'reused_jti', signed: true }, 'redeem');
    return { ok: false, code, summary: ACTION_LINK_COPY[code] };
  }

  let outcome: RunResult;
  let status: 'done' | 'failed' | 'uncertain' | 'answered' = 'failed';
  let reason: string | null = null;
  try {
    const data = (await deps.relay({ tenantId: c.tenant, userId: c.sub }, 'perform_action_link', {
      jti: c.jti,
      act: c.act,
      args: link.args,
      exp: c.exp,
    })) as { summary?: unknown } | null;
    status = 'done';
    const summary = typeof data?.summary === 'string' && data.summary.length <= 300 ? data.summary : 'Done.';
    outcome = { ok: true, code: null, summary };
  } catch (e) {
    const code = relayFailureCode(String((e as Error)?.message ?? e));
    status = code === 'uncertain' ? 'uncertain' : code === 'answered' ? 'answered' : 'failed';
    reason = `desktop_${code}`;
    outcome = { ok: false, code, summary: ACTION_LINK_COPY[code] };
  }

  // The link stays consumed either way (fail closed): a relay that timed out may still have acted,
  // so re-opening the jti could act twice.
  try {
    await deps.db
      .prepare(`UPDATE action_link_redemptions SET status = ?, finished_at = datetime('now') WHERE jti = ?`)
      .bind(status, c.jti)
      .run();
  } catch {
    // best-effort
  }
  await auditActionLink(deps.db, {
    kind: outcome.ok ? 'redeem' : 'reject',
    jti: c.jti,
    tenantId: c.tenant,
    act: c.act,
    reason: outcome.ok ? null : `redeem: ${reason}`,
  });
  return outcome;
}

/** Re-exported for the resolvers' per-action scope gate at mint time. */
export { actionScope };
export type { ActionArgs };
