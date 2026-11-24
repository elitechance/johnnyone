/**
 * Live UI bug 3: a plan-chat message showed `08:14 AM` live and `01:14 PM` after a reload. The engine
 * stores `datetime('now')` (UTC, no zone: `2026-11-24 13:14:38`, confirmed on the live engine) and
 * `new Date()` read that as LOCAL time. Pinned in a UTC-5 zone so the shift is visible.
 */
process.env.TZ = 'America/New_York';

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AiMessage } from '../../../../../ui/src/models/ai-message.model';
import { formatMessageTime, parseServerTime, toIsoUtc } from '../../../../../ui/src/lib/server-time';
import { normalizeStoredMessages } from '../../../../../ui/src/services/ai-chat-logic';

const STORED = '2026-11-24 13:14:38'; // what listAiMessages returns
const LIVE = '2026-11-24T13:14:38.000Z'; // what the optimistic/streamed bubble carries

describe('engine timestamps are UTC', () => {
  it('a zone-less engine time becomes ISO UTC; ISO input passes through', () => {
    expect(toIsoUtc(STORED)).toBe('2026-11-24T13:14:38Z');
    expect(toIsoUtc('2026-11-24T13:14:38')).toBe('2026-11-24T13:14:38Z');
    expect(toIsoUtc('2026-11-24 13:14:38.250')).toBe('2026-11-24T13:14:38.250Z');
    expect(toIsoUtc(LIVE)).toBe(LIVE);
    expect(toIsoUtc('2026-11-24T13:14:38+02:00')).toBe('2026-11-24T13:14:38+02:00');
    expect(toIsoUtc(null)).toBe('');
  });

  it('the stored and the live form are the same instant', () => {
    expect(parseServerTime(STORED)?.getTime()).toBe(Date.UTC(2026, 10, 24, 13, 14, 38));
    expect(parseServerTime(STORED)?.getTime()).toBe(parseServerTime(LIVE)?.getTime());
    expect(parseServerTime('nope')).toBeNull();
    expect(parseServerTime('')).toBeNull();
  });

  it('the bubble shows the same clock time live and after a reload (no 5 h shift)', () => {
    const live = formatMessageTime(LIVE, 'en-US');
    const reloaded = formatMessageTime(STORED, 'en-US');
    expect(reloaded).toBe(live);
    expect(reloaded).toMatch(/08:14/);
    expect(formatMessageTime(undefined)).toBe('');
  });

  it('stored messages are normalized on fetch', () => {
    const m: AiMessage = { id: 'a', sessionId: 's', role: 'user', content: 'x', inputTokens: 0, outputTokens: 0, costCents: 0, createdAt: STORED };
    expect(normalizeStoredMessages([m])[0].createdAt).toBe('2026-11-24T13:14:38Z');
    expect(normalizeStoredMessages(null)).toEqual([]);
  });

  it('the bubble and the chat service use the UTC rules (wiring)', () => {
    const bubble = readFileSync(resolve(__dirname, '../../../../../ui/src/components/message-bubble/message-bubble.component.ts'), 'utf8');
    expect(bubble).toContain('return formatMessageTime(this.message.createdAt)');
    const svc = readFileSync(resolve(__dirname, '../../../../../ui/src/services/ai-chat.service.ts'), 'utf8');
    expect(svc).toContain('this.messages.set(normalizeStoredMessages(messages))');
  });
});
