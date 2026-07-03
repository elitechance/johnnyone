// Pure, Angular/Ionic-free briefing logic (overhaul P4, decision D8) so it can be unit-tested under
// the plugin-less web vitest — the real `BriefingPage` pulls in Ionic + the chat-window, which that
// config cannot import. The page/composer delegate their decisions to exactly these functions.
//
// `AiMessage` is imported type-only (erased at build time), so no Angular/runtime dependency leaks in.
import type { AiMessage } from '../../../../../ui/src/models/ai-message.model';

/** Minimal raw-row shape (from `listAiMessages`) that `foldMessages` maps to the chat-window shape. */
export interface RawMessage {
  id: string;
  sessionId?: string;
  role: string;
  content: string;
  createdAt?: string;
}

/**
 * The first-turn briefing preamble (overhaul P4, D2). Sent client-side as message #1 to seed the
 * briefing behavior on the existing relay chat path; later turns are plain replies (the preamble is
 * already in the resumed context).
 */
export const BRIEFING_PREAMBLE =
  "You are J1's briefing assistant. Before any planning begins, clarify this initiative: ask concise " +
  'clarifying questions, then draft a short, concrete brief the planner can act on. Pin down scope, ' +
  'constraints, and acceptance up front. When the brief is solid, summarize it so it can be accepted ' +
  'into planning.';

/**
 * Compose the first-turn briefing message: the preamble, the raw ask, then optional `Attached files:`
 * / `Referenced host paths:` blocks (each omitted when its list is empty). Pure — unit-testable.
 */
export function composeBriefingSeed(
  rawAsk: string,
  attachmentPaths: string[] = [],
  referencePaths: string[] = [],
): string {
  const parts: string[] = [BRIEFING_PREAMBLE, '', `Initial ask:\n${(rawAsk ?? '').trim()}`];
  const attachments = attachmentPaths.map((p) => p.trim()).filter(Boolean);
  if (attachments.length > 0) {
    parts.push('', 'Attached files:', ...attachments.map((p) => `- ${p}`));
  }
  const refs = referencePaths.map((p) => p.trim()).filter(Boolean);
  if (refs.length > 0) {
    parts.push('', 'Referenced host paths:', ...refs.map((p) => `- ${p}`));
  }
  return parts.join('\n');
}

const ALLOWED_ROLES: readonly string[] = ['user', 'assistant', 'system', 'tool'];

function normalizeRole(role: string): AiMessage['role'] {
  return ALLOWED_ROLES.includes(role) ? (role as AiMessage['role']) : 'assistant';
}

/** Order-preserving map from raw message rows to the `AiMessage` shape the chat-window consumes. */
export function foldMessages(raw: RawMessage[]): AiMessage[] {
  return raw.map((m) => ({
    id: m.id,
    sessionId: m.sessionId ?? '',
    role: normalizeRole(m.role),
    content: m.content ?? '',
    inputTokens: 0,
    outputTokens: 0,
    costCents: 0,
    createdAt: m.createdAt ?? '',
  }));
}

/** Accept is valid only while the Initiative is still in briefing (overhaul P4, D1). */
export function canAccept(initiativeStatus: string | null | undefined): boolean {
  return initiativeStatus === 'briefing';
}

/** Seed the first turn only when the conversation is still empty. */
export function shouldSeed(messages: { length: number } | null | undefined): boolean {
  return !!messages && messages.length === 0;
}

// ── Chip reducers (composer state: pending attachments + reference paths) ──────────────────────────

export type ChipAction =
  | { type: 'add'; value: string }
  | { type: 'remove'; value: string }
  | { type: 'reset' };

function chipReducer(state: string[], action: ChipAction): string[] {
  switch (action.type) {
    case 'add': {
      const value = action.value.trim();
      if (!value || state.includes(value)) return state; // dedupe + ignore blanks
      return [...state, value];
    }
    case 'remove':
      return state.filter((entry) => entry !== action.value);
    case 'reset':
      return [];
    default:
      return state;
  }
}

export function pendingAttachmentsReducer(state: string[], action: ChipAction): string[] {
  return chipReducer(state, action);
}

export function referencePathsReducer(state: string[], action: ChipAction): string[] {
  return chipReducer(state, action);
}
