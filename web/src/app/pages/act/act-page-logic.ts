/**
 * DOM-free logic for the /act page (Discord action links).
 *
 * A Discord button opens `/act?t=<jwt>`. That GET only ever DISPLAYS: the page asks the worker to
 * `previewActionLink` (verifies, consumes nothing) and shows the label, the action, the target and
 * a countdown, plus Confirm / Cancel. Only Confirm sends `runActionLink`, as a POST — the one
 * request that uses the link up. Discord's unfurler and any browser prefetch therefore cannot act.
 *
 * Neither request carries the user's session: no Authorization header, `credentials: 'omit'`.
 * The link's token in the POST body is the only credential, which is also why the Confirm needs no
 * CSRF token — a cross-site page cannot ride an ambient cookie or header here; it would need the
 * token itself, and holding the token is exactly the authority the link grants.
 *
 * No Angular, no fetch, no Date.now(): every function takes what it needs, so the spec can drive it.
 */

export type ActionLinkAct = 'answer' | 'approve_plan' | 'run_phase' | 'ack';

/** The worker's `ActionLinkPreview`. */
export interface ActionLinkPreview {
  state: 'valid' | 'expired' | 'used' | 'invalid';
  act: string | null;
  label: string | null;
  target: string | null;
  /** exp, seconds since the epoch */
  expiresAt: number | null;
  message: string | null;
  /** For `used`: done | failed | uncertain | superseded | answered | ended | pending. */
  outcome?: string | null;
  /** The worker's clock (s). The countdown runs against it, not the phone's. */
  serverNow?: number | null;
}

/** The worker's `ActionLinkResult`. */
export interface ActionLinkResult {
  ok: boolean;
  code: string | null;
  summary: string;
}

export type ActView =
  | { kind: 'loading' }
  | { kind: 'missing'; message: string }
  | { kind: 'ready'; label: string; action: string; target: string; secondsLeft: number; countdown: string }
  | { kind: 'confirming'; label: string; action: string; target: string }
  | { kind: 'expired'; label: string | null; target: string | null; message: string }
  | { kind: 'used'; label: string | null; message: string; outcome: string | null }
  | { kind: 'invalid'; message: string }
  | { kind: 'done'; summary: string }
  | { kind: 'failed'; message: string }
  | { kind: 'cancelled'; message: string };

/** Copy. Mirrors the worker's ACTION_LINK_COPY so the page reads the same when offline. */
export const ACT_COPY = {
  missing: 'This link is missing its code. Open the button in Discord again.',
  expired: 'This link expired, ask J1 for a new one.',
  used: 'This link was already used, ask J1 for a new one.',
  invalid: 'This link is not valid, ask J1 for a new one.',
  unavailable: 'J1 could not reach your desktop host. This link is now used up; open J1 to do it by hand.',
  failed: 'J1 could not do this. This link is now used up; open J1 to check what happened.',
  network: 'Could not reach J1. Check your connection; the link was not used.',
  /** A relay timeout, or the connection dropped mid-Confirm (H-1-1). */
  uncertain: 'It may have gone through, so check J1 before trying again.',
  restarted: 'J1 restarted since this link was made, so ask for a new link.',
  usedDone: 'Already done ✓',
  usedFailed: 'Tried, but J1 could not do it, so check J1.',
  superseded: 'Another answer was already chosen.',
  answered: 'The agent already got an answer, so nothing was sent.',
  /** The agent's session ended (archived, or its plan job finished) since the link was made. */
  ended: "That agent's session has ended, so nothing was sent.",
  usedEnded: "That agent's session had ended, so nothing was sent.",
  cancelled: 'Cancelled. Nothing happened, and the link was not used.',
} as const;

const ACTION_NAMES: Record<ActionLinkAct, string> = {
  answer: 'Answer the agent',
  approve_plan: 'Approve the plan',
  run_phase: 'Run a phase',
  ack: 'Acknowledge',
};

export function actionName(act: string | null): string {
  return act && act in ACTION_NAMES ? ACTION_NAMES[act as ActionLinkAct] : 'Unknown action';
}

/** A token is base64url JWT-shaped and bounded; anything else is not worth a request. */
const TOKEN_RE = /^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/;

export function readToken(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const t = raw.trim();
  return t.length > 0 && t.length <= 2048 && TOKEN_RE.test(t) ? t : null;
}

/**
 * Server clock minus phone clock, in ms, measured when the preview arrived (H-1-10). Add it to
 * `Date.now()` for every countdown tick, so a phone whose clock runs fast does not hide Confirm
 * early (or a slow one keep it past the worker's deadline). 0 when the server sent no clock.
 */
export function clockOffsetMs(serverNowS: number | null | undefined, clientNowMs: number): number {
  if (typeof serverNowS !== 'number' || !Number.isFinite(serverNowS)) return 0;
  return serverNowS * 1000 - clientNowMs;
}

/** The copy for a used link, by how it ended (H-1-5). */
export function usedCopy(outcome: string | null | undefined): string {
  switch (outcome) {
    case 'done':
      return ACT_COPY.usedDone;
    case 'failed':
      return ACT_COPY.usedFailed;
    case 'uncertain':
      return ACT_COPY.uncertain;
    case 'superseded':
      return ACT_COPY.superseded;
    case 'answered':
      return ACT_COPY.answered;
    case 'ended':
      return ACT_COPY.usedEnded;
    default:
      return ACT_COPY.used;
  }
}

export function secondsLeft(expiresAtS: number | null, nowMs: number): number {
  if (expiresAtS == null) return 0;
  return Math.max(0, Math.floor(expiresAtS - nowMs / 1000));
}

export function formatCountdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** The view for a preview at time `nowMs`. A valid link whose clock ran out reads as expired. */
export function viewFromPreview(p: ActionLinkPreview, nowMs: number): ActView {
  switch (p.state) {
    case 'valid': {
      const left = secondsLeft(p.expiresAt, nowMs);
      if (left <= 0) {
        return { kind: 'expired', label: p.label, target: p.target, message: ACT_COPY.expired };
      }
      return {
        kind: 'ready',
        label: p.label ?? '',
        action: actionName(p.act),
        target: p.target ?? '',
        secondsLeft: left,
        countdown: formatCountdown(left),
      };
    }
    case 'expired':
      return { kind: 'expired', label: p.label, target: p.target, message: p.message || ACT_COPY.expired };
    case 'used':
      return { kind: 'used', label: p.label, message: usedCopy(p.outcome), outcome: p.outcome ?? null };
    default:
      return { kind: 'invalid', message: p.message || ACT_COPY.invalid };
  }
}

/** Advance a `ready` view one tick; anything else is unchanged. */
export function tick(view: ActView, expiresAtS: number | null, nowMs: number): ActView {
  if (view.kind !== 'ready') return view;
  const left = secondsLeft(expiresAtS, nowMs);
  if (left <= 0) return { kind: 'expired', label: view.label, target: view.target, message: ACT_COPY.expired };
  return { ...view, secondsLeft: left, countdown: formatCountdown(left) };
}

/** Confirm is offered only on a ready, unexpired link. */
export function canConfirm(view: ActView): boolean {
  return view.kind === 'ready' && view.secondsLeft > 0;
}

/** The result of the Confirm POST, as a view. */
export function viewFromResult(r: ActionLinkResult): ActView {
  if (r.ok) return { kind: 'done', summary: r.summary || 'Done.' };
  switch (r.code) {
    case 'expired':
      return { kind: 'expired', label: null, target: null, message: ACT_COPY.expired };
    case 'used':
      return { kind: 'used', label: null, message: ACT_COPY.used, outcome: null };
    case 'superseded':
      return { kind: 'used', label: null, message: ACT_COPY.superseded, outcome: 'superseded' };
    case 'answered':
      return { kind: 'used', label: null, message: ACT_COPY.answered, outcome: 'answered' };
    case 'ended':
      return { kind: 'used', label: null, message: ACT_COPY.ended, outcome: 'ended' };
    case 'uncertain':
      return { kind: 'failed', message: ACT_COPY.uncertain };
    case 'restarted':
      return { kind: 'failed', message: ACT_COPY.restarted };
    case 'unavailable':
      return { kind: 'failed', message: ACT_COPY.unavailable };
    case 'failed':
      return { kind: 'failed', message: ACT_COPY.failed };
    default:
      return { kind: 'invalid', message: ACT_COPY.invalid };
  }
}

/** "Open in J1" is offered whenever the link can no longer do it for you (not after "Already done"). */
export function offersOpenInJ1(view: ActView): boolean {
  return (
    view.kind === 'expired' ||
    (view.kind === 'used' && view.outcome !== 'done') ||
    view.kind === 'failed' ||
    view.kind === 'invalid' ||
    view.kind === 'missing'
  );
}

/** The normal, logged-in console. Fixed path: never derived from the link. */
export const OPEN_IN_J1_PATH = '/initiatives';

export interface GraphqlPost {
  method: 'POST';
  headers: Record<string, string>;
  credentials: 'omit';
  referrerPolicy: 'no-referrer';
  cache: 'no-store';
  body: string;
}

function post(query: string, token: string): GraphqlPost {
  return {
    method: 'POST',
    // Deliberately NO Authorization / x-tenant-id: the token is the whole credential.
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    credentials: 'omit',
    referrerPolicy: 'no-referrer',
    cache: 'no-store',
    body: JSON.stringify({ query, variables: { t: token } }),
  };
}

export const PREVIEW_QUERY =
  'query($t: String!) { previewActionLink(token: $t) { state act label target expiresAt message outcome serverNow } }';
export const RUN_MUTATION = 'mutation($t: String!) { runActionLink(token: $t) { ok code summary } }';

/**
 * The preview request. A POST as well (never a GET with the token in the URL), so the token
 * stays out of worker and proxy access logs. It never consumes the link.
 */
export function previewRequest(token: string): GraphqlPost {
  return post(PREVIEW_QUERY, token);
}

/**
 * The ONLY request that uses the link up. Built only from a ready view, so there is no code path
 * that redeems on load, on a tick or on Cancel.
 */
export function confirmRequest(view: ActView, token: string): GraphqlPost | null {
  return canConfirm(view) ? post(RUN_MUTATION, token) : null;
}

/** Pull `data.<field>` out of a GraphQL response, or null on errors / a bad shape. */
export function graphqlField<T>(json: unknown, field: string): T | null {
  if (!json || typeof json !== 'object') return null;
  const data = (json as { data?: Record<string, unknown> }).data;
  const v = data?.[field];
  return v && typeof v === 'object' ? (v as T) : null;
}
