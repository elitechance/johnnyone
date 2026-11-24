/**
 * G-1-3 / G-2-23, through the REAL AiChatService (a root singleton): a refused draft never leaks
 * into another plan's composer, is cleared by a later send, and is restored ONLY for refusals.
 */
import '@angular/compiler';
import { Injector, runInInjectionContext } from '@angular/core';
import { of, Subject, NEVER } from 'rxjs';
import { describe, expect, it } from 'vitest';
import { AiChatService } from '../../../../../ui/src/services/ai-chat.service';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';

const BUSY = 'The plan agent is still answering the last message. Try again when it finishes.';

function service() {
  const deltas = new Subject<any>();
  let relay = 0;
  const api = {
    getSession: (id: string) => of({ id, title: id }),
    listMessages: () => of([]),
    onRelayChatMessage: () => NEVER,
    sendRelayMessage: () => of({ relayId: `r${++relay}` }),
    onRelayChatDelta: () => deltas,
  };
  const injector = Injector.create({ providers: [{ provide: JohnnyApiService, useValue: api }] });
  const chat = runInInjectionContext(injector, () => new AiChatService());
  const finalError = (text: string, sessionId = 'A') =>
    deltas.next({ relayId: `r${relay}`, sessionId, chunkType: 'error', delta: text, isFinal: true });
  return { chat, finalError };
}

describe('restoredDraft lifecycle', () => {
  it('a BUSY refusal restores the text; opening plan B clears it (B composer stays empty)', () => {
    const { chat, finalError } = service();
    chat.loadSession('A');
    chat.sendMessage('add a rollback section');
    finalError(BUSY);
    expect(chat.restoredDraft()?.text).toBe('add a rollback section');
    chat.loadSession('B');
    expect(chat.restoredDraft()).toBeNull();
  });

  it('a later successful send clears it (no restore later)', () => {
    const { chat, finalError } = service();
    chat.loadSession('A');
    chat.sendMessage('first');
    finalError(BUSY);
    expect(chat.restoredDraft()).not.toBeNull();
    chat.sendMessage('first');
    expect(chat.restoredDraft()).toBeNull();
  });

  it('a crash or timeout (the engine may have stored the message) restores nothing', () => {
    for (const text of [
      'Plan agent turn failed (exit 1): boom',
      'Response timed out waiting for model output. Please retry.',
    ]) {
      const { chat, finalError } = service();
      chat.loadSession('A');
      chat.sendMessage('hello');
      finalError(text);
      expect(chat.restoredDraft()).toBeNull();
      expect(chat.turnError()).toBe(text);
    }
  });

  it('other refusals restore too (retired plan agent, ended session)', () => {
    for (const text of [
      'This plan is in planning — the plan agent is retired.',
      "This agent's job has ended; it does not take new messages. Transcript only.",
      'This session is archived; it does not take new messages.',
    ]) {
      const { chat, finalError } = service();
      chat.loadSession('A');
      chat.sendMessage('x');
      finalError(text);
      expect(chat.restoredDraft()?.text).toBe('x');
    }
  });
});
