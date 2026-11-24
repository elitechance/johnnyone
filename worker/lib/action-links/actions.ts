/**
 * actions.ts — the action-link allowlist (pure, no I/O).
 *
 * An action link is a bearer credential for exactly ONE of these actions with exactly these
 * arguments. Everything here is validated at MINT time (createActionLink) and again at
 * verify time (preview / run), so a token whose payload was produced by anything other than
 * this code — a leaked secret, a future bug — still cannot name an action or an argument shape
 * that is not listed here. The desktop host repeats the same checks before it acts
 * (desktop/src-tauri/src/services/action_links.rs), so the allowlist is enforced on both sides
 * of the relay.
 */
import type { ApiScope } from '../auth/scopes';

export const ACTION_KINDS = ['answer', 'approve_plan', 'run_phase', 'ack'] as const;
export type ActionKind = (typeof ACTION_KINDS)[number];

export type ActionArgs = Record<string, string>;

export type ArgsCheck = { ok: true; args: ActionArgs } | { ok: false; reason: string };

/** Max characters of an `answer` reply: it is typed into an agent's composer, one line. */
export const REPLY_MAX = 200;
/** Max characters of the label the /act page shows. */
export const LABEL_MAX = 120;

export const REPLY_FORBIDDEN_FIRST = ['!', '/', '#'];

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const PHASE_RE = /^[A-Za-z0-9._-]{1,64}$/;
// Any C0/C1 control char, DEL, or a Unicode line/paragraph separator. A reply is typed into a
// TUI: a newline would submit early, an ESC would start a key sequence.
const CONTROL_RE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/;

export function isActionKind(v: unknown): v is ActionKind {
  return typeof v === 'string' && (ACTION_KINDS as readonly string[]).includes(v);
}

/**
 * The API scope a minter must hold for each action. A link can never do more than the identity
 * that minted it: answering types into an agent terminal (terminal:write); approving a plan and
 * running a phase start agents (plans:write); `ack` has no side effect (plans:read).
 */
export function actionScope(act: ActionKind): ApiScope {
  switch (act) {
    case 'answer':
      return 'terminal:write';
    case 'approve_plan':
    case 'run_phase':
      return 'plans:write';
    case 'ack':
      return 'plans:read';
  }
}

function onlyKeys(args: Record<string, unknown>, allowed: string[], required: string[]): string | null {
  for (const k of Object.keys(args)) {
    if (!allowed.includes(k)) return `unexpected argument "${k}"`;
  }
  for (const k of required) {
    if (!(k in args)) return `missing argument "${k}"`;
  }
  for (const k of Object.keys(args)) {
    if (typeof args[k] !== 'string') return `argument "${k}" must be a string`;
  }
  return null;
}

/**
 * Unicode general category Cf (format characters: ZWSP, joiners, bidi controls, BOM, word joiner,
 * tag characters, ...). Invisible, so text carrying one does not say what it looks like it says,
 * and a ZWSP in front of `!` would hide it from the first-character check (H-2-31). The host
 * rejects the same set (`is_format_char` in action_links.rs).
 */
export const FORMAT_RE = /\p{Cf}/u;

export function validateReply(reply: string): string | null {
  if (reply.trim().length === 0) return 'reply must not be empty';
  if (FORMAT_RE.test(reply)) return 'reply must not contain invisible format characters';
  if ([...reply].length > REPLY_MAX) return `reply must be at most ${REPLY_MAX} characters`;
  if (CONTROL_RE.test(reply)) return 'reply must be one line with no control characters';
  // A reply is typed into an agent CLI, where a leading `!` runs a shell command (bash mode),
  // `/` is a slash command (and `//` the console's key directive) and `#` writes to memory. A
  // link must only ever type an ANSWER. Same set as the host's REPLY_FORBIDDEN_FIRST.
  const first = reply.trimStart()[0];
  if (first && REPLY_FORBIDDEN_FIRST.includes(first)) return `reply must not start with ${first}`;
  return null;
}

export function validateLabel(label: unknown): string | null {
  if (typeof label !== 'string' || label.trim().length === 0) return 'label is required';
  if ([...label].length > LABEL_MAX) return `label must be at most ${LABEL_MAX} characters`;
  if (CONTROL_RE.test(label)) return 'label must be one line with no control characters';
  if (FORMAT_RE.test(label)) return 'label must not contain invisible format characters';
  return null;
}

/** Validate `args` for `act`. Unknown keys are refused, so a token carries nothing unvetted. */
export function validateActionArgs(act: unknown, rawArgs: unknown): ArgsCheck {
  if (!isActionKind(act)) return { ok: false, reason: 'action is not allowlisted' };
  if (rawArgs === null || typeof rawArgs !== 'object' || Array.isArray(rawArgs)) {
    return { ok: false, reason: 'args must be an object' };
  }
  const args = rawArgs as Record<string, unknown>;
  let err: string | null;
  switch (act) {
    case 'answer':
      err = onlyKeys(args, ['sessionId', 'reply'], ['sessionId', 'reply']);
      if (!err && !ID_RE.test(args.sessionId as string)) err = 'sessionId is not a valid id';
      if (!err) err = validateReply(args.reply as string);
      break;
    case 'approve_plan':
      err = onlyKeys(args, ['planId'], ['planId']);
      if (!err && !ID_RE.test(args.planId as string)) err = 'planId is not a valid id';
      break;
    case 'run_phase':
      err = onlyKeys(args, ['planId', 'phaseId'], ['planId', 'phaseId']);
      if (!err && !ID_RE.test(args.planId as string)) err = 'planId is not a valid id';
      if (!err && (!PHASE_RE.test(args.phaseId as string) || (args.phaseId as string).includes('..'))) {
        err = 'phaseId is not a valid phase id';
      }
      break;
    case 'ack':
      err = onlyKeys(args, ['alertId'], []);
      if (!err && args.alertId !== undefined && !ID_RE.test(args.alertId as string)) err = 'alertId is not a valid id';
      break;
  }
  if (err) return { ok: false, reason: err };
  return { ok: true, args: args as ActionArgs };
}

function short(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

/**
 * The one-line "target" the /act page shows next to the label: what will happen, to what. The
 * human `title` (plan / agent names, stored with the mint by the desktop) is preferred; shortened
 * ids are the fallback. Never the plan's contents.
 */
export function describeTarget(act: ActionKind, args: ActionArgs, title?: string | null): string {
  const t = title && title.trim() ? title.trim() : null;
  switch (act) {
    case 'answer':
      return `Reply "${args.reply}" to ${t ?? `agent session ${short(args.sessionId)}`}`;
    case 'approve_plan':
      return `Accept ${t ? `"${t}"` : `plan ${short(args.planId)}`} and start planning`;
    case 'run_phase':
      return t ? `Run phase ${args.phaseId}: ${t}` : `Run phase ${args.phaseId} of plan ${short(args.planId)}`;
    case 'ack':
      return 'Mark the alert as seen (no other effect)';
  }
}
