/**
 * Pure (DOM-free) rules for a terminal pane the engine REFUSES to start (G-1-1): a finished
 * worker/reviewer's pane is never re-created (`terminal::AGENT_JOB_ENDED`, also returned for the
 * rows the boot backfill archived). The refusal reaches the page two ways — a rejected
 * `terminal_command_ack` on the relay socket, or a failed `captureTerminal` — and both surfaces
 * (`/shells/:id` Raw and the initiative console's Raw tab) render the same "Transcript only"
 * notice instead of a blank terminal.
 */
import { jobEndedNotice } from '../shells/shells-page-logic';
import { shellRoutePath } from '../shells/shells-route';
import { rawAttachNeeded } from './console-tabs-logic';

/** A rejected terminal command, as the relay service emits it. `kind` is what was asked. */
export interface CommandRejection {
  sessionId: string;
  error: string;
  kind?: string;
}

/** The notice for an archived (ended) session opened read-only, when no job-ended refusal named it. */
export const ARCHIVED_SHELL_NOTE = 'This session has ended. Transcript only.';

/**
 * Which rejected commands are worth the terminal-error line (G-2-17): opening the pane (visual
 * subscribe/refresh/history) and typing into it. A refused resize or kill is noise there.
 */
export function surfacesAsTerminalError(kind: string | null | undefined): boolean {
  return kind === 'input' || kind === 'terminal_visual_subscribe' || (kind ?? '').startsWith('visual_');
}

/** Is this shell opened read-only? A plan agent (view-only pane) or any non-active row. */
export function isReadOnlyShell(s: { planAgent?: boolean | null; status?: string | null } | null | undefined): boolean {
  if (!s) return false;
  return s.planAgent === true || (!!s.status && s.status !== 'active');
}

/** The view-only banner text for a read-only shell. */
export function readOnlyNote(
  s: { planAgent?: boolean | null; status?: string | null } | null | undefined,
  planAgentNote: string,
  jobEnded: string | null,
): string {
  if (s?.planAgent === true) return planAgentNote;
  return jobEnded ?? ARCHIVED_SHELL_NOTE;
}

/** Keep polling the console's primary pane? Not once the engine refused it (G-2-22). */
export function shouldPollPane(
  sessionId: string,
  primaryId: string | null | undefined,
  notices: Readonly<Record<string, string>>,
): boolean {
  return sessionId === primaryId && !notices[sessionId];
}

/** Drop a session's notice (a LIVE screen arrived: the pane exists again). Never mutates. */
export function withoutPaneNotice(notices: Readonly<Record<string, string>>, sessionId: string): Record<string, string> {
  if (!(sessionId in notices)) return { ...notices };
  const next = { ...notices };
  delete next[sessionId];
  return next;
}

/**
 * Transcript rows for a read-only (archived) session that has no live stream events: its stored
 * chat messages, so an ended session's Transcript is not blank.
 */
export function transcriptRowsFromMessages(
  messages: ReadonlyArray<{ role: string; content: string; created_at?: string | null }>,
): Array<{ type: 'you' | 'agent'; text: string; at?: string }> {
  return messages
    .filter((m) => (m.role === 'user' || m.role === 'assistant') && (m.content ?? '').trim())
    .map((m) => ({ type: m.role === 'user' ? ('you' as const) : ('agent' as const), text: m.content, at: m.created_at ?? undefined }));
}

/** The text of an error of any shape (`Error`, GraphQL error, string). */
export function errorText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (err && typeof err === 'object' && 'message' in err) return String((err as { message: unknown }).message ?? '');
  return String(err ?? '');
}

/** The "Transcript only" notice when `err` is the engine's job-ended refusal, else `null`. */
export function paneRefusalNotice(err: unknown): string | null {
  return jobEndedNotice(errorText(err));
}

/**
 * A rejected ack that concerns a session this page is SHOWING (the /shells session or the
 * console's primary session), else `null` (an ack for a background pane stays in the console log).
 */
export function rejectionForView(
  ack: { sessionId?: string | null; accepted?: boolean | null; error?: string | null } | null | undefined,
  viewed: ReadonlyArray<string | null | undefined>,
): CommandRejection | null {
  if (!ack || ack.accepted || !ack.sessionId) return null;
  if (!viewed.some((id) => !!id && id === ack.sessionId)) return null;
  return { sessionId: ack.sessionId, error: (ack.error ?? '').trim() || 'The terminal command was refused.' };
}

/** Record (or keep) a session's refusal notice. Never mutates; `null` leaves the map as is. */
export function withPaneNotice(
  notices: Readonly<Record<string, string>>,
  sessionId: string | null | undefined,
  notice: string | null,
): Record<string, string> {
  if (!sessionId || !notice || notices[sessionId] === notice) return { ...notices };
  return { ...notices, [sessionId]: notice };
}

export type RawPaneView =
  | { kind: 'ended'; note: string; transcriptLink: string }
  | { kind: 'attach' }
  | { kind: 'screen' };

/**
 * What a Raw surface renders for `sessionId`: the "Transcript only" notice when there is no screen
 * and the engine refused the pane (a recorded notice, or the current terminal error); the attach
 * affordance when no pane is attached; otherwise the screen. A live screen always wins.
 */
export function rawPaneView(
  sessionId: string | null | undefined,
  hasScreen: boolean,
  notices: Readonly<Record<string, string>>,
  terminalError?: string | null,
  ended?: boolean,
): RawPaneView {
  const sid = (sessionId ?? '').trim();
  if (sid) {
    // A recorded refusal, or an ended (archived) session, wins over ANY screen: a screen here can
    // only be a cached, non-live frame (a live one clears the notice) (G-2-24).
    const recorded = notices[sid] ?? (ended ? ARCHIVED_SHELL_NOTE : null);
    const note = recorded ?? (hasScreen ? null : jobEndedNotice(terminalError));
    if (note) return { kind: 'ended', note, transcriptLink: shellRoutePath(sid, 'transcript') };
  }
  return rawAttachNeeded(sessionId, hasScreen) ? { kind: 'attach' } : { kind: 'screen' };
}
