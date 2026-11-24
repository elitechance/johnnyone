/**
 * Pure (DOM-free) rules for the relay chat stream, shared by `AiChatService` and its spec.
 *
 * An `error`-kind FINAL delta is how the engine refuses or fails a turn (plan agent BUSY, "plan
 * left briefing", a crash, an idle timeout — `send_relay_chat_refusal` and the relay loop). It is
 * shown as an inline notice; it is never appended to the assistant's text.
 */
import type { AiMessage } from '../models/ai-message.model';

export interface ChatDeltaLike {
  chunkType?: string | null;
  delta?: string | null;
  isFinal?: boolean | null;
}

/** `text` accumulates; `control` (system/result, non-final errors) is skipped; `error` is a final error. */
export type DeltaKind = 'text' | 'control' | 'error';

export function classifyDelta(d: ChatDeltaLike): DeltaKind {
  const kind = (d.chunkType || '').toLowerCase();
  if (kind === 'error') return d.isFinal ? 'error' : 'control';
  if (kind === 'system' || kind === 'result') return 'control';
  return 'text';
}

/** The inline notice for a final error delta (never blank). */
export function errorNoticeText(d: ChatDeltaLike): string {
  const text = (d.delta ?? '').trim();
  return text || 'The agent could not answer this message.';
}

/**
 * Messages after a final error delta. A turn that streamed no assistant text was refused or failed
 * before it produced anything: its optimistic user bubble (`optimisticId`) is removed — a refused
 * send stores nothing on the engine, and a failed one comes back with the next refetch. A turn that
 * did stream text keeps everything (the stored reply carries the error line).
 */
/**
 * Is this final error a REFUSAL — the engine turned the send away before storing it (BUSY, a
 * retired plan agent, a non-claude provider, an ended/archived session)? A crash or timeout is not:
 * the engine may already have stored the message (G-2-23).
 */
export function isRefusalText(text: string | null | undefined): boolean {
  return /still answering the last message|the plan agent is retired|plan agents run on claude_code|does not take new messages/i.test(
    text ?? '',
  );
}

/**
 * The text to put back into the composer after a REFUSED send's bubble was removed (G-1-3: a BUSY
 * refusal must not lose what was typed), else `null` — also for a crash/timeout (G-2-23).
 */
export function draftAfterError(
  msgs: readonly AiMessage[],
  optimisticId: string | null,
  streamedText: boolean,
  errorText?: string | null,
): string | null {
  if (streamedText || !optimisticId) return null;
  if (errorText !== undefined && !isRefusalText(errorText)) return null;
  const bubble = msgs.find((m) => m.id === optimisticId && m.role === 'user');
  return bubble ? bubble.content : null;
}

/** The composer text once a restored draft arrives: never overwrite what the user typed since. */
export function applyRestoredDraft(current: string, draft: string | null | undefined): string {
  if (!draft) return current;
  return current.trim() ? current : draft;
}

export function messagesAfterError(
  msgs: readonly AiMessage[],
  optimisticId: string | null,
  streamedText: boolean,
): AiMessage[] {
  if (streamedText || !optimisticId) return [...msgs];
  return msgs.filter((m) => m.id !== optimisticId);
}
