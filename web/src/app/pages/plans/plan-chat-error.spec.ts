/**
 * Item B: refusals are visible in the Plans chat. The relay sends a refused/failed turn as ONE
 * `error`-kind final delta (`send_relay_chat_refusal`: BUSY, "plan left briefing"; the relay loop:
 * a crash, an idle timeout). `AiChatService` used to drop it; these pure rules decide what it shows.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AiMessage } from '../../../../../ui/src/models/ai-message.model';
import { classifyDelta, errorNoticeText, messagesAfterError } from '../../../../../ui/src/services/ai-chat-logic';

const BUSY = 'The plan agent is still answering the last message. Try again when it finishes.';

function msg(id: string, role: AiMessage['role'], content = 'x'): AiMessage {
  return { id, sessionId: 's', role, content, inputTokens: 0, outputTokens: 0, costCents: 0, createdAt: '' };
}

describe('classifyDelta', () => {
  it('a final error delta is an error (shown), never assistant text', () => {
    expect(classifyDelta({ chunkType: 'error', delta: BUSY, isFinal: true })).toBe('error');
    expect(classifyDelta({ chunkType: 'Error', delta: 'x', isFinal: true })).toBe('error');
  });
  it('a non-final error, system and result are control chunks', () => {
    expect(classifyDelta({ chunkType: 'error', delta: 'x', isFinal: false })).toBe('control');
    expect(classifyDelta({ chunkType: 'system', isFinal: false })).toBe('control');
    expect(classifyDelta({ chunkType: 'result', isFinal: true })).toBe('control');
  });
  it('text (and an unknown kind) accumulates', () => {
    expect(classifyDelta({ chunkType: 'text', delta: 'hi' })).toBe('text');
    expect(classifyDelta({ delta: 'hi' })).toBe('text');
  });
});

describe('errorNoticeText', () => {
  it('is the engine reason, trimmed', () => {
    expect(errorNoticeText({ delta: `  ${BUSY}\n` })).toBe(BUSY);
    expect(errorNoticeText({ delta: 'This plan is in planning — the plan agent is retired.' })).toContain('retired');
  });
  it('is never blank', () => {
    expect(errorNoticeText({ delta: '' })).toBe('The agent could not answer this message.');
    expect(errorNoticeText({ delta: null })).toBe('The agent could not answer this message.');
  });
});

describe('messagesAfterError', () => {
  const before = [msg('m1', 'user'), msg('m2', 'assistant'), msg('temp-1', 'user', 'second')];
  it('a refused send (no text streamed) loses its optimistic bubble — the engine stored nothing', () => {
    expect(messagesAfterError(before, 'temp-1', false).map((m) => m.id)).toEqual(['m1', 'm2']);
  });
  it('a turn that streamed text keeps everything (the stored reply carries the error line)', () => {
    expect(messagesAfterError(before, 'temp-1', true)).toEqual(before);
  });
  it('without a pending bubble nothing is removed, and the input is not mutated', () => {
    expect(messagesAfterError(before, null, false)).toEqual(before);
    const copy = [...before];
    messagesAfterError(before, 'temp-1', false);
    expect(before).toEqual(copy);
  });
});

describe('wiring (read as text: the service/page cannot be TestBed-compiled here)', () => {
  const SERVICE = readFileSync(resolve(__dirname, '../../../../../ui/src/services/ai-chat.service.ts'), 'utf8');
  const PAGE = readFileSync(resolve(__dirname, 'plan-detail.page.html'), 'utf8');
  it('the chat service routes a final error into turnError and drops the refused bubble', () => {
    expect(SERVICE).toContain("if (kind === 'error') {");
    expect(SERVICE).toContain('this.turnError.set(errorNoticeText(delta));');
    expect(SERVICE).toContain('messagesAfterError(msgs, this.pendingUserMessageId, streamedText)');
    expect(SERVICE).not.toContain("kind === 'system' || kind === 'result' || kind === 'error'");
  });
  it('the Plans chat shows it inline as an alert', () => {
    expect(PAGE).toContain('@if (chat.turnError(); as err) {');
    expect(PAGE).toContain('class="turn-error" role="alert"');
  });
});
