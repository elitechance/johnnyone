/** G-1-3: a refused (BUSY) send puts the typed text back into the composer. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AiMessage } from '../../../../../ui/src/models/ai-message.model';
import { applyRestoredDraft, draftAfterError } from '../../../../../ui/src/services/ai-chat-logic';

function msg(id: string, role: AiMessage['role'], content: string): AiMessage {
  return { id, sessionId: 's', role, content, inputTokens: 0, outputTokens: 0, costCents: 0, createdAt: '' };
}

describe('draftAfterError', () => {
  const msgs = [msg('m1', 'user', 'first'), msg('temp-9', 'user', 'add a rollback section')];
  it('returns the refused bubble text when nothing streamed', () => {
    expect(draftAfterError(msgs, 'temp-9', false)).toBe('add a rollback section');
  });
  it('nothing to restore once the turn streamed text, or without a pending bubble', () => {
    expect(draftAfterError(msgs, 'temp-9', true)).toBeNull();
    expect(draftAfterError(msgs, null, false)).toBeNull();
    expect(draftAfterError(msgs, 'gone', false)).toBeNull();
  });
});

describe('applyRestoredDraft', () => {
  it('fills an empty composer and never overwrites newer typing', () => {
    expect(applyRestoredDraft('', 'again')).toBe('again');
    expect(applyRestoredDraft('  ', 'again')).toBe('again');
    expect(applyRestoredDraft('new text', 'again')).toBe('new text');
    expect(applyRestoredDraft('', null)).toBe('');
  });
});

describe('wiring', () => {
  const ROOT = resolve(__dirname, '../../../../../ui/src');
  it('service → chat window → composer → Plans page', () => {
    const svc = readFileSync(resolve(ROOT, 'services/ai-chat.service.ts'), 'utf8');
    expect(svc).toContain("const draft = draftAfterError(this.messages(), this.pendingUserMessageId, streamedText, delta.delta ?? '');");
    expect(svc).toContain('if (draft) this.restoredDraft.set({ text: draft, seq: ++this.draftSeq });');
    expect(readFileSync(resolve(ROOT, 'components/chat-window/chat-window.component.html'), 'utf8')).toContain('[restoredDraft]="restoredDraft"');
    const composer = readFileSync(resolve(ROOT, 'components/message-composer/message-composer.component.ts'), 'utf8');
    expect(composer).toContain('this.messageText = applyRestoredDraft(this.messageText, draft.text);');
    expect(readFileSync(resolve(__dirname, 'plan-detail.page.html'), 'utf8')).toContain('[restoredDraft]="chat.restoredDraft()"');
  });
});
